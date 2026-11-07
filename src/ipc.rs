//! The IPC socket, what `pila msg` talks to (`ipc_client.rs`):
//! `$XDG_RUNTIME_DIR/pila-<WAYLAND_DISPLAY>.sock`, in `PILA_SOCKET` for the
//! programs pila starts. One JSON request per line, one JSON response per line.
//! The format is public: `docs/IPC.md` says what it guarantees.
//!
//! It's on the event loop, so nothing here may block or take long whatever a
//! client does: reads and writes don't block, what a client hasn't read waits in
//! a queue, and a client past a limit is disconnected. Answers are only built
//! when asked for.

use std::{
    cell::{Cell, RefCell},
    io::{Read, Write},
    net::Shutdown,
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    rc::Rc,
};

use serde_json::{json, Value};
use smithay::{
    desktop::Window,
    reexports::calloop::{generic::Generic, Interest, Mode, PostAction},
    utils::{Transform, SERIAL_COUNTER},
};
use tracing::{debug, info, warn};

use crate::{
    config::Action,
    foreign_toplevel::{identifier, title_and_app_id},
    snap,
    stacking::{is_always_on_top, is_on_all_workspaces},
    state::{close_window, fill_state},
    workspaces::is_override_redirect,
    Pila,
};

/// Raised when a field is removed or changes meaning; adding one doesn't.
const PROTOCOL: u32 = 1;
/// The longest request. Longer means a client that's wrong.
const MAX_REQUEST: usize = 64 * 1024;
/// How much a client may leave unread before it's disconnected.
const MAX_UNREAD: usize = 1024 * 1024;
const MAX_CLIENTS: usize = 32;
/// Reads per wake, so that a client sending nonstop can't hold the loop (the
/// source stays ready, and the rest comes next time round).
const READS_PER_WAKE: usize = 16;

/// One client's state, shared by its read source and, while it's behind, its
/// write source. Dropped with the last of them, which frees its place.
struct Client {
    stream: UnixStream,
    /// Read, waiting for the end of a line.
    pending: Vec<u8>,
    /// Answered, waiting to be written.
    unread: Vec<u8>,
    writing: bool,
    clients: Rc<Cell<usize>>,
}

impl Drop for Client {
    fn drop(&mut self) {
        self.clients.set(self.clients.get() - 1);
    }
}

impl Client {
    /// Write what fits. An error means the client is gone.
    fn flush(&mut self) -> std::io::Result<()> {
        let mut written = 0;
        let result = loop {
            if written == self.unread.len() {
                break Ok(());
            }
            match (&self.stream).write(&self.unread[written..]) {
                Ok(0) => break Err(std::io::ErrorKind::WriteZero.into()),
                Ok(n) => written += n,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break Ok(()),
                Err(err) => break Err(err),
            }
        };
        self.unread.drain(..written);
        result
    }
}

/// Open the socket. Without it pila works all the same, so a failure is logged.
pub fn init(state: &mut Pila) {
    if let Err(err) = open(state) {
        warn!(%err, "no IPC socket: pila msg won't work");
    }
}

fn open(state: &mut Pila) -> std::io::Result<()> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| std::io::Error::other("XDG_RUNTIME_DIR is not set"))?;
    let mut name = std::ffi::OsString::from("pila-");
    name.push(&state.socket_name);
    name.push(".sock");
    let path = dir.join(name);
    // Left by a pila that crashed: the Wayland socket's name is ours now.
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let clients = Rc::new(Cell::new(0));
    state
        .loop_handle
        .insert_source(
            Generic::new(listener, Interest::READ, Mode::Level),
            move |_, listener, state| {
                loop {
                    match listener.accept() {
                        Ok((stream, _)) if clients.get() >= MAX_CLIENTS => {
                            debug!("IPC: too many clients");
                            drop(stream);
                        }
                        Ok((stream, _)) => add_client(state, stream, &clients),
                        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(err) => {
                            warn!(?err, "IPC: accept failed");
                            break;
                        }
                    }
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    info!(path = %path.display(), "IPC socket");
    state.ipc_socket = Some(path);
    Ok(())
}

/// Remove the socket when pila quits.
pub fn close(state: &Pila) {
    if let Some(path) = &state.ipc_socket {
        let _ = std::fs::remove_file(path);
    }
}

fn add_client(state: &mut Pila, stream: UnixStream, clients: &Rc<Cell<usize>>) {
    if let Err(err) = stream.set_nonblocking(true) {
        warn!(?err, "IPC: client setup failed");
        return;
    }
    let Ok(source) = stream.try_clone() else {
        return;
    };
    clients.set(clients.get() + 1);
    let client = Rc::new(RefCell::new(Client {
        stream,
        pending: Vec::new(),
        unread: Vec::new(),
        writing: false,
        clients: clients.clone(),
    }));
    let result = state.loop_handle.insert_source(
        Generic::new(source, Interest::READ, Mode::Level),
        move |_, _, state| Ok(client_readable(state, &client)),
    );
    if let Err(err) = result {
        warn!(?err, "IPC: client setup failed");
    }
}

fn client_readable(state: &mut Pila, client: &Rc<RefCell<Client>>) -> PostAction {
    let mut chunk = [0u8; 4096];
    let mut at_end = false;
    for _ in 0..READS_PER_WAKE {
        // Bound first: reading doesn't borrow the client while it's filled.
        let read = (&client.borrow().stream).read(&mut chunk);
        match read {
            Ok(0) => {
                at_end = true;
                break;
            }
            Ok(n) => client.borrow_mut().pending.extend_from_slice(&chunk[..n]),
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(_) => {
                at_end = true;
                break;
            }
        }
    }
    loop {
        let line = {
            let mut client = client.borrow_mut();
            let Some(end) = client.pending.iter().position(|b| *b == b'\n') else {
                break;
            };
            client.pending.drain(..=end).collect::<Vec<u8>>()
        };
        if line.len() > MAX_REQUEST {
            return disconnect(client);
        }
        let mut text = respond(state, &line).to_string();
        text.push('\n');
        client
            .borrow_mut()
            .unread
            .extend_from_slice(text.as_bytes());
        if client.borrow().unread.len() > MAX_UNREAD {
            // Written as it goes: only what it really left unread counts.
            let flushed = client.borrow_mut().flush();
            if flushed.is_err() || client.borrow().unread.len() > MAX_UNREAD {
                debug!("IPC: a client isn't reading");
                return disconnect(client);
            }
        }
    }
    if client.borrow().pending.len() > MAX_REQUEST {
        debug!("IPC: a request is too long");
        return disconnect(client);
    }
    if client.borrow_mut().flush().is_err() {
        return disconnect(client);
    }
    write_the_rest(state, client);
    if at_end {
        // The write source, if any, keeps the client until it's all written.
        PostAction::Remove
    } else {
        PostAction::Continue
    }
}

/// End a client: its other source, if any, finds out through the socket.
fn disconnect(client: &Rc<RefCell<Client>>) -> PostAction {
    let _ = client.borrow().stream.shutdown(Shutdown::Both);
    PostAction::Remove
}

/// What didn't fit in the socket goes when the client reads some.
fn write_the_rest(state: &mut Pila, client: &Rc<RefCell<Client>>) {
    {
        let client = client.borrow();
        if client.writing || client.unread.is_empty() {
            return;
        }
    }
    let Ok(source) = client.borrow().stream.try_clone() else {
        return;
    };
    client.borrow_mut().writing = true;
    let shared = client.clone();
    let result = state.loop_handle.insert_source(
        Generic::new(source, Interest::WRITE, Mode::Level),
        move |_, _, _| {
            let mut client = shared.borrow_mut();
            if client.flush().is_err() {
                let _ = client.stream.shutdown(Shutdown::Both);
                client.writing = false;
                return Ok(PostAction::Remove);
            }
            if client.unread.is_empty() {
                client.writing = false;
                return Ok(PostAction::Remove);
            }
            Ok(PostAction::Continue)
        },
    );
    if let Err(err) = result {
        warn!(?err, "IPC: can't wait to write");
        client.borrow_mut().writing = false;
        let _ = client.borrow().stream.shutdown(Shutdown::Both);
    }
}

fn respond(state: &mut Pila, line: &[u8]) -> Value {
    let answer = serde_json::from_slice::<Value>(line)
        .map_err(|err| format!("bad request: {err}"))
        .and_then(|request| handle(state, &request));
    match answer {
        Ok(Value::Object(mut map)) => {
            map.insert("ok".into(), true.into());
            Value::Object(map)
        }
        Ok(_) => json!({ "ok": true }),
        Err(error) => json!({ "ok": false, "error": error }),
    }
}

fn handle(state: &mut Pila, request: &Value) -> Result<Value, String> {
    let name = request
        .get("request")
        .and_then(Value::as_str)
        .ok_or("a request needs a \"request\" string")?;
    match name {
        "version" => Ok(json!({ "protocol": PROTOCOL, "version": crate::version() })),
        "windows" => Ok(json!({ "protocol": PROTOCOL, "windows": state.ipc_windows() })),
        "outputs" => Ok(json!({ "protocol": PROTOCOL, "outputs": state.ipc_outputs() })),
        "workspaces" => Ok(json!({ "protocol": PROTOCOL, "workspaces": state.ipc_workspaces() })),
        "focused" => {
            let window = state.focused_window().and_then(|w| state.ipc_window(&w));
            Ok(json!({ "protocol": PROTOCOL, "window": window }))
        }
        "action" | "focus" | "close" if state.is_locked() => {
            // Not even `quit`: a program mustn't get past the lock screen this way.
            Err("the session is locked".to_string())
        }
        "action" => {
            let text = string_field(request, "action")?;
            if let Some(action) = Action::parse(text)? {
                state.ipc_action(action);
            }
            Ok(json!({}))
        }
        "focus" => {
            let window = named_window(state, request)?;
            state.activate_toplevel(&window);
            Ok(json!({}))
        }
        "close" => {
            close_window(&named_window(state, request)?);
            Ok(json!({}))
        }
        other => Err(format!("unknown request {other:?}")),
    }
}

fn string_field<'a>(request: &'a Value, name: &str) -> Result<&'a str, String> {
    request
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("a request needs a {name:?} string"))
}

fn named_window(state: &Pila, request: &Value) -> Result<Window, String> {
    let id = string_field(request, "id")?;
    state
        .window_for_identifier(id)
        .ok_or_else(|| format!("no window {id:?}"))
}

/// As in the config's `[[output]]`.
fn transform_name(transform: Transform) -> &'static str {
    match transform {
        Transform::Normal => "normal",
        Transform::_90 => "90",
        Transform::_180 => "180",
        Transform::_270 => "270",
        Transform::Flipped => "flipped",
        Transform::Flipped90 => "flipped-90",
        Transform::Flipped180 => "flipped-180",
        Transform::Flipped270 => "flipped-270",
    }
}

impl Pila {
    /// A binding's action, but with no key held: a window switch ends at once,
    /// where one from the keyboard ends as its modifiers are released.
    fn ipc_action(&mut self, action: Action) {
        self.run_action(action, SERIAL_COUNTER.next_serial());
        if self.windows.switch.is_some() {
            self.end_switch();
        }
        self.queue_redraw();
    }

    /// Managed windows, mapped ones back to front, then those off the `Space`.
    fn ipc_windows(&self) -> Vec<Value> {
        self.space
            .elements()
            .chain(self.off_space_windows())
            .filter(|w| !is_override_redirect(w))
            .filter_map(|w| self.ipc_window(w))
            .collect()
    }

    fn ipc_window(&self, window: &Window) -> Option<Value> {
        let id = identifier(window)?;
        let (title, app_id) = title_and_app_id(window);
        let location = self.window_location(window)?;
        let size = window.geometry().size;
        let (maximized, fullscreen) = fill_state(window);
        Some(json!({
            "id": id,
            "app_id": app_id,
            "title": title,
            "workspace": self.workspace_of(window) + 1,
            "output": self.window_output(window).map(|o| o.name()),
            "focused": self.focused_window().as_ref() == Some(window),
            "x": location.x,
            "y": location.y,
            "width": size.w,
            "height": size.h,
            "minimized": self.is_minimized(window),
            "maximized": maximized,
            "fullscreen": fullscreen,
            "snapped": snap::snapped_side(window).map(snap::Side::name),
            "always_on_top": is_always_on_top(window),
            "all_workspaces": is_on_all_workspaces(window),
            "x11": window.x11_surface().is_some(),
        }))
    }

    fn ipc_outputs(&self) -> Vec<Value> {
        let pointer = self.pointer_location();
        self.space
            .outputs()
            .filter_map(|output| {
                let geo = self.space.output_geometry(output)?;
                let properties = output.physical_properties();
                Some(json!({
                    "name": output.name(),
                    "make": properties.make,
                    "model": properties.model,
                    "x": geo.loc.x,
                    "y": geo.loc.y,
                    "width": geo.size.w,
                    "height": geo.size.h,
                    "refresh": output.current_mode().map(|mode| mode.refresh),
                    "scale": output.current_scale().fractional_scale(),
                    "transform": transform_name(output.current_transform()),
                    "focused": geo.to_f64().contains(pointer),
                }))
            })
            .collect()
    }

    fn ipc_workspaces(&self) -> Vec<Value> {
        let managed: Vec<usize> = self
            .space
            .elements()
            .chain(self.off_space_windows())
            .filter(|w| !is_override_redirect(w))
            .map(|w| self.workspace_of(w))
            .collect();
        (0..self.config.workspaces)
            .map(|index| {
                json!({
                    "index": index + 1,
                    "active": index == self.workspaces.active,
                    "windows": managed.iter().filter(|w| **w == index).count(),
                })
            })
            .collect()
    }
}
