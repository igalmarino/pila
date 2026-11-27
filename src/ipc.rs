//! The IPC socket, what `pila msg` talks to (`ipc_client.rs`):
//! `$XDG_RUNTIME_DIR/pila-<WAYLAND_DISPLAY>.sock`, in `PILA_SOCKET` for the
//! programs pila starts. One JSON request per line, one JSON response per line.
//! The format is public: `docs/IPC.md` says what it guarantees.
//!
//! It's on the event loop, so nothing here may block or take long whatever a
//! client does: reads and writes don't block, what a client hasn't read waits in
//! a queue, and a client past a limit is disconnected. Answers are only built
//! when asked for.
//!
//! A client may `subscribe` to events: whole snapshots of the windows,
//! workspaces, outputs, keyboard layouts, colors or the window switch, sent
//! first as they are and then whenever one changes (`Pila::send_ipc_events`,
//! from `after_frames`, where the taskbars hear about windows too; the layouts
//! every loop iteration, `send_keyboard_events`; the switch in the iteration it
//! changed, `send_switch_events`; the colors when the config is applied,
//! `send_colors_event`). A change that may come with no frame at all (the
//! monitors turned off) sends them anyway (`ipc_events_due`). Each subscriber
//! keeps the last snapshot it got of each kind; with none, nothing is built.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    io::{Read, Write},
    net::Shutdown,
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    rc::Rc,
    rc::Weak,
};

use serde_json::{json, Value};
use smithay::{
    desktop::Window,
    reexports::calloop::{generic::Generic, Interest, Mode, PostAction},
    utils::{IsAlive, Transform, SERIAL_COUNTER},
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
    /// Written right after the answer being built: a new subscriber's first
    /// events.
    after_answer: Vec<u8>,
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
        after_answer: Vec::new(),
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
        let mut text = respond(state, client, &line).to_string();
        text.push('\n');
        {
            let mut client = client.borrow_mut();
            client.unread.extend_from_slice(text.as_bytes());
            let events = std::mem::take(&mut client.after_answer);
            client.unread.extend_from_slice(&events);
        }
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

fn respond(state: &mut Pila, client: &Rc<RefCell<Client>>, line: &[u8]) -> Value {
    let answer = parse_request(line).and_then(|request| handle(state, client, request));
    match answer {
        Ok(Value::Object(mut map)) => {
            map.insert("ok".into(), true.into());
            Value::Object(map)
        }
        Ok(_) => json!({ "ok": true }),
        Err(error) => json!({ "ok": false, "error": error }),
    }
}

/// A request of `pila msg`'s protocol (`docs/IPC.md`), as `parse_request`
/// reads it.
#[derive(Debug, PartialEq)]
pub enum Request {
    Version,
    Windows,
    Outputs,
    Workspaces,
    Keyboard,
    /// `[colors]`, for clients that look like Pila (a bar's or launcher's theme).
    Colors,
    /// The Mod+Tab in progress, for a program that shows it.
    Switch,
    /// Events of these kinds, from now on.
    Subscribe(Vec<Kind>),
    Focused,
    /// `None` for `"none"`.
    Action(Option<Action>),
    /// By the window's id.
    Focus(String),
    Close(String),
    /// By the window's id; `Focus` brings it back.
    Minimize(String),
    /// The screen shares remembered for apps (`screen_grants.rs`).
    ScreenShares,
    /// Forget one, by its id, or all of them (`None`).
    ForgetScreenShare(Option<String>),
    /// The direct Wayland screen capture permissions (`permissions.rs`).
    CapturePermissions,
    /// Forget one capture permission, by its id, or all of them (`None`).
    ForgetCapturePermission(Option<String>),
    /// What keeps the screen on now (`idle.rs`).
    IdleInhibitors,
}

/// A line of the protocol as a request, or why it isn't one. Reads it and
/// nothing else, so it can be fuzzed (`fuzz/`): what a request does, and
/// whether it may while locked, is `handle`'s.
pub fn parse_request(line: &[u8]) -> Result<Request, String> {
    let request =
        serde_json::from_slice::<Value>(line).map_err(|err| format!("bad request: {err}"))?;
    let name = request
        .get("request")
        .and_then(Value::as_str)
        .ok_or("a request needs a \"request\" string")?;
    Ok(match name {
        "version" => Request::Version,
        "windows" => Request::Windows,
        "outputs" => Request::Outputs,
        "workspaces" => Request::Workspaces,
        "keyboard" => Request::Keyboard,
        "colors" => Request::Colors,
        "switch" => Request::Switch,
        "subscribe" => Request::Subscribe(match request.get("events") {
            None => Kind::ALL.to_vec(),
            Some(Value::Array(names)) if !names.is_empty() => names
                .iter()
                .map(|name| {
                    name.as_str()
                        .and_then(Kind::named)
                        .ok_or_else(|| format!("no such events {name}"))
                })
                .collect::<Result<_, _>>()?,
            Some(_) => return Err("\"events\" is a list of event names".to_string()),
        }),
        "focused" => Request::Focused,
        "action" => Request::Action(Action::parse(string_field(&request, "action")?)?),
        "focus" => Request::Focus(string_field(&request, "id")?.to_string()),
        "close" => Request::Close(string_field(&request, "id")?.to_string()),
        "minimize" => Request::Minimize(string_field(&request, "id")?.to_string()),
        "screen-shares" => Request::ScreenShares,
        "forget-screen-share" => Request::ForgetScreenShare(match request.get("all") {
            Some(Value::Bool(true)) => None,
            _ => Some(string_field(&request, "id")?.to_string()),
        }),
        "capture-permissions" => Request::CapturePermissions,
        "idle-inhibitors" => Request::IdleInhibitors,
        "forget-capture-permission" => Request::ForgetCapturePermission(match request.get("all") {
            Some(Value::Bool(true)) => None,
            _ => Some(string_field(&request, "id")?.to_string()),
        }),
        other => return Err(format!("unknown request {other:?}")),
    })
}

fn handle(
    state: &mut Pila,
    client: &Rc<RefCell<Client>>,
    request: Request,
) -> Result<Value, String> {
    match request {
        Request::Version => Ok(json!({ "protocol": PROTOCOL, "version": crate::version() })),
        Request::Windows => Ok(json!({ "protocol": PROTOCOL, "windows": state.ipc_windows() })),
        Request::Outputs => Ok(json!({ "protocol": PROTOCOL, "outputs": state.ipc_outputs() })),
        Request::Workspaces => {
            Ok(json!({ "protocol": PROTOCOL, "workspaces": state.ipc_workspaces() }))
        }
        Request::Keyboard => Ok(json!({ "protocol": PROTOCOL, "layouts": state.ipc_layouts() })),
        Request::Colors => Ok(json!({ "protocol": PROTOCOL, "colors": state.ipc_colors() })),
        Request::Switch => Ok(json!({ "protocol": PROTOCOL, "switch": state.ipc_switch() })),
        Request::Subscribe(kinds) => {
            state.subscribe(client, kinds);
            Ok(json!({ "protocol": PROTOCOL }))
        }
        Request::Focused => {
            let window = state.focused_window().and_then(|w| state.ipc_window(&w));
            Ok(json!({ "protocol": PROTOCOL, "window": window }))
        }
        Request::Action(action) => {
            // Not even `quit`: a program mustn't get past the lock screen this
            // way. A few actions get past nothing (`runs_while_locked`).
            if state.is_locked() && !action.as_ref().is_some_and(Action::runs_while_locked) {
                return Err("the session is locked".to_string());
            }
            if let Some(action) = action {
                state.ipc_action(action);
            }
            Ok(json!({}))
        }
        Request::Focus(_) | Request::Close(_) | Request::Minimize(_) if state.is_locked() => {
            Err("the session is locked".to_string())
        }
        Request::Focus(id) => {
            state.activate_toplevel(&named_window(state, &id)?);
            Ok(json!({}))
        }
        Request::Close(id) => {
            close_window(&named_window(state, &id)?);
            Ok(json!({}))
        }
        Request::Minimize(id) => {
            let window = named_window(state, &id)?;
            state.minimize_window(&window);
            Ok(json!({}))
        }
        Request::ScreenShares => Ok(json!({
            "protocol": PROTOCOL,
            "screen_shares": state.ipc_screen_shares(),
        })),
        // Forgetting what isn't read yet would forget nothing, and it would
        // be back once read.
        Request::ForgetScreenShare(_) if !state.screen_grants_read() => Err(
            "the remembered screen shares are not read yet: Pila reads them once it serves \
             the desktop portal"
                .to_string(),
        ),
        Request::ForgetCapturePermission(_) if !state.capture_permissions_read() => {
            Err("the saved capture permissions are not read yet: try again".to_string())
        }
        // Forgetting takes access away: allowed while locked too.
        Request::ForgetScreenShare(Some(id)) => match state.forget_screen_grant(&id) {
            true => Ok(json!({})),
            false => Err(format!("no screen share remembered as {id:?}")),
        },
        Request::ForgetScreenShare(None) => {
            Ok(json!({ "forgotten": state.forget_screen_grants() }))
        }
        Request::CapturePermissions => Ok(json!({
            "protocol": PROTOCOL,
            "capture_permissions": state.ipc_capture_permissions(),
        })),
        // Forgetting takes access away: allowed while locked too.
        Request::ForgetCapturePermission(Some(id)) => match state.forget_capture_permission(&id) {
            true => Ok(json!({})),
            false => Err(format!("no capture permission remembered as {id:?}")),
        },
        Request::ForgetCapturePermission(None) => {
            Ok(json!({ "forgotten": state.forget_capture_permissions() }))
        }
        Request::IdleInhibitors => Ok(json!({
            "protocol": PROTOCOL,
            "idle_inhibitors": state.ipc_idle_inhibitors(),
        })),
    }
}

fn string_field<'a>(request: &'a Value, name: &str) -> Result<&'a str, String> {
    request
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("a request needs a {name:?} string"))
}

fn named_window(state: &Pila, id: &str) -> Result<Window, String> {
    state
        .window_for_identifier(id)
        .ok_or_else(|| format!("no window {id:?}"))
}

/// What a client can subscribe to: one snapshot each.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Windows,
    Workspaces,
    Outputs,
    Keyboard,
    Colors,
    Switch,
}

impl Kind {
    const ALL: [Kind; 6] = [
        Kind::Windows,
        Kind::Workspaces,
        Kind::Outputs,
        Kind::Keyboard,
        Kind::Colors,
        Kind::Switch,
    ];
    /// Built after frames are drawn; the rest every loop iteration.
    const AFTER_FRAMES: [Kind; 3] = [Kind::Windows, Kind::Workspaces, Kind::Outputs];

    fn name(self) -> &'static str {
        match self {
            Kind::Windows => "windows",
            Kind::Workspaces => "workspaces",
            Kind::Outputs => "outputs",
            Kind::Keyboard => "keyboard",
            Kind::Colors => "colors",
            Kind::Switch => "switch",
        }
    }

    /// The event's list (or object, for colors and the switch): named as the answer to the request of the same
    /// name has it.
    fn field(self) -> &'static str {
        match self {
            Kind::Keyboard => "layouts",
            other => other.name(),
        }
    }

    fn named(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

struct Subscriber {
    client: Weak<RefCell<Client>>,
    kinds: Vec<Kind>,
    /// The last event of each kind it was sent, by `Kind::ALL`'s order.
    last: [Option<String>; Kind::ALL.len()],
}

/// The clients subscribed to events (`Pila::ipc_events`).
#[derive(Default)]
pub struct Subscribers {
    list: Vec<Subscriber>,
    /// Something changed that no frame may follow (the monitors went off):
    /// `after_frames` sends the events anyway (`send_ipc_events_due`).
    due: bool,
    /// The layouts, and the one in use, the last keyboard event was made
    /// from (`send_keyboard_events`).
    keyboard: Option<(Vec<String>, usize)>,
    /// The window switch changed this iteration (`Pila::switch_changed`).
    switch: bool,
    /// Where each window was in the last windows event, by id: the window
    /// being moved or resized with the pointer stays there until it's let
    /// go, so that a drag sends one event rather than one a frame.
    settled: HashMap<String, serde_json::Map<String, Value>>,
}

/// A premultiplied color as `#rrggbb`, or `#rrggbbaa` when not opaque.
fn hex([r, g, b, a]: [f32; 4]) -> String {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let straight = |v: f32| if a > 0.0 { byte(v / a) } else { 0 };
    let rgb = format!("#{:02x}{:02x}{:02x}", straight(r), straight(g), straight(b));
    match byte(a) {
        255 => rgb,
        alpha => format!("{rgb}{alpha:02x}"),
    }
}

/// A window's geometry in IPC answers.
const GEOMETRY: [&str; 4] = ["x", "y", "width", "height"];

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
    /// Subscribe `client` to `kinds` (again, if it already was: the new list
    /// replaces the old). Its first events are what's there now.
    fn subscribe(&mut self, client: &Rc<RefCell<Client>>, kinds: Vec<Kind>) {
        let weak = Rc::downgrade(client);
        self.ipc_events.list.retain(|s| !s.client.ptr_eq(&weak));
        let mut subscriber = Subscriber {
            client: weak,
            kinds,
            last: Default::default(),
        };
        let mut first = String::new();
        for &kind in &subscriber.kinds {
            let event = self.ipc_event(kind);
            first.push_str(&event);
            first.push('\n');
            subscriber.last[kind as usize] = Some(event);
        }
        client
            .borrow_mut()
            .after_answer
            .extend_from_slice(first.as_bytes());
        self.ipc_events.list.push(subscriber);
    }

    /// Send each subscriber the windows, workspaces and outputs that changed
    /// since it got its last ones. Called after frames are drawn, as taskbars
    /// are updated.
    pub fn send_ipc_events(&mut self) {
        self.ipc_events.due = false;
        self.send_events_of(&Kind::AFTER_FRAMES);
    }

    /// Send the events after a change no frame may follow (`ipc_events_due`),
    /// from `after_frames` when nothing was drawn.
    pub fn send_ipc_events_due(&mut self) {
        if std::mem::take(&mut self.ipc_events.due) {
            self.send_ipc_events();
        }
    }

    /// Something changed that may come with no frame (every monitor turned
    /// off draws nothing): send the events at the end of this iteration.
    pub fn ipc_events_due(&mut self) {
        self.ipc_events.due = true;
    }

    /// The colors, to their subscribers, if they changed since their last
    /// event: when the config is applied (`apply_config`).
    pub fn send_colors_event(&mut self) {
        self.send_events_of(&[Kind::Colors]);
    }

    /// The window switch started, stepped, lost a window or ended: its
    /// subscribers hear of it at the end of this loop iteration
    /// (`send_switch_events`), drawn or not, since a switch can end with
    /// nothing new to draw.
    pub fn switch_changed(&mut self) {
        self.ipc_events.switch = true;
        self.switcher_view_changed();
    }

    /// The switch, to its subscribers, if it changed this iteration.
    pub fn send_switch_events(&mut self) {
        if std::mem::take(&mut self.ipc_events.switch) {
            self.send_events_of(&[Kind::Switch]);
        }
    }

    /// The same for the keyboard layouts, every loop iteration: an xkb
    /// option's keys switch them with nothing to draw. The event is made
    /// only when they changed since the last one (`keyboard_layouts_are`).
    pub fn send_keyboard_events(&mut self) {
        let subscribed = self
            .ipc_events
            .list
            .iter()
            .any(|s| s.kinds.contains(&Kind::Keyboard));
        if !subscribed {
            return;
        }
        if let Some((names, active)) = self.ipc_events.keyboard.take() {
            let same = self.keyboard_layouts_are(&names, active);
            self.ipc_events.keyboard = Some((names, active));
            if same {
                return;
            }
        }
        self.ipc_events.keyboard = Some(self.keyboard_layouts());
        self.send_events_of(&[Kind::Keyboard]);
    }

    fn send_events_of(&mut self, kinds: &[Kind]) {
        self.ipc_events.list.retain(|s| s.client.strong_count() > 0);
        if self.ipc_events.list.is_empty() {
            return;
        }
        let mut events: [Option<String>; Kind::ALL.len()] = Default::default();
        for &kind in kinds {
            if self.ipc_events.list.iter().any(|s| s.kinds.contains(&kind)) {
                events[kind as usize] = Some(self.ipc_event(kind));
            }
        }
        // A client is subscribed once (`subscribe` replaces its entry).
        let mut written = Vec::new();
        for subscriber in &mut self.ipc_events.list {
            let Some(client) = subscriber.client.upgrade() else {
                continue;
            };
            let mut out = String::new();
            for &kind in &subscriber.kinds {
                let Some(event) = &events[kind as usize] else {
                    continue;
                };
                if subscriber.last[kind as usize].as_ref() != Some(event) {
                    subscriber.last[kind as usize] = Some(event.clone());
                    out.push_str(event);
                    out.push('\n');
                }
            }
            if !out.is_empty() {
                client.borrow_mut().unread.extend_from_slice(out.as_bytes());
                written.push(client);
            }
        }
        for client in written {
            let behind = {
                let mut c = client.borrow_mut();
                c.flush().is_err() || c.unread.len() > MAX_UNREAD
            };
            if behind {
                debug!("IPC: a subscriber isn't reading");
                let _ = client.borrow().stream.shutdown(Shutdown::Both);
                continue;
            }
            write_the_rest(self, &client);
        }
    }

    /// An event line: `{"event": kind, field: [...]}` (`{...}` for colors).
    fn ipc_event(&mut self, kind: Kind) -> String {
        let value = match kind {
            Kind::Windows => Value::Array(self.ipc_windows_settled()),
            Kind::Workspaces => Value::Array(self.ipc_workspaces()),
            Kind::Outputs => Value::Array(self.ipc_outputs()),
            Kind::Keyboard => Value::Array(self.ipc_layouts()),
            Kind::Colors => self.ipc_colors(),
            Kind::Switch => self.ipc_switch(),
        };
        let mut event = serde_json::Map::new();
        event.insert("event".into(), kind.name().into());
        event.insert(kind.field().into(), value);
        Value::Object(event).to_string()
    }

    /// The windows, the one being moved or resized with the pointer where it
    /// was before (`Subscribers::settled`; as it is if it wasn't listed yet).
    fn ipc_windows_settled(&mut self) -> Vec<Value> {
        let grabbed = self.grabbed_window().and_then(|w| identifier(&w));
        let mut windows = self.ipc_windows();
        let mut settled = HashMap::with_capacity(windows.len());
        for window in &mut windows {
            let Value::Object(fields) = window else {
                continue;
            };
            let Some(id) = fields.get("id").and_then(Value::as_str).map(str::to_string) else {
                continue;
            };
            let kept = (grabbed.as_ref() == Some(&id))
                .then(|| self.ipc_events.settled.remove(&id))
                .flatten();
            let geometry = match kept {
                Some(kept) => {
                    fields.extend(kept.clone());
                    kept
                }
                None => GEOMETRY
                    .iter()
                    .filter_map(|&f| Some((f.to_string(), fields.get(f)?.clone())))
                    .collect(),
            };
            settled.insert(id, geometry);
        }
        self.ipc_events.settled = settled;
        windows
    }

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
            "urgent": crate::urgency::is_urgent(window),
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
                    "power": self
                        .backend
                        .as_ref()
                        .and_then(|b| b.output_power(output))
                        .unwrap_or(true),
                }))
            })
            .collect()
    }

    /// The Mod+Tab in progress: its windows' ids in its order (most recent
    /// first), the selected one's and the output it shows on; null if none.
    fn ipc_switch(&self) -> Value {
        let Some(switch) = &self.windows.switch else {
            return Value::Null;
        };
        let (order, index) = switch.order();
        let selected = order.get(index).filter(|w| w.alive()).and_then(identifier);
        json!({
            "windows": order.iter().filter(|w| w.alive()).filter_map(identifier).collect::<Vec<_>>(),
            "selected": selected,
            "output": switch.output(),
        })
    }

    /// The physical keyboard's layouts, in order.
    /// `[colors]` as the config writes them (`#rrggbb`, or `#rrggbbaa` when
    /// translucent), named with `_` for `-`.
    fn ipc_colors(&self) -> Value {
        let c = &self.config.colors;
        let opaque = |[r, g, b]: [f32; 3]| [r, g, b, 1.0];
        let colors = [
            ("bar", c.bar),
            ("bar_inactive", c.bar_inactive),
            ("bar_urgent", c.bar_urgent),
            ("icon", opaque(c.icon)),
            ("icon_inactive", opaque(c.icon_inactive)),
            ("hover", opaque(c.hover)),
            ("hover_icon", opaque(c.hover_icon)),
            ("close_hover", opaque(c.close_hover)),
            ("close_hover_icon", opaque(c.close_hover_icon)),
            ("background", c.background),
            ("snap_preview", c.snap_preview),
        ];
        Value::Object(
            colors
                .into_iter()
                .map(|(name, color)| (name.to_string(), hex(color).into()))
                .collect(),
        )
    }

    fn ipc_layouts(&mut self) -> Vec<Value> {
        let (names, active) = self.keyboard_layouts();
        names
            .into_iter()
            .enumerate()
            .map(|(index, name)| {
                json!({ "index": index + 1, "name": name, "active": index == active })
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

#[cfg(test)]
mod tests {
    use smithay::utils::Transform;

    /// `outputs` names a transform as `[[output]]` does: each name reads
    /// back as the transform it came from.
    #[test]
    fn transform_names_are_the_configs() {
        for transform in [
            Transform::Normal,
            Transform::_90,
            Transform::_180,
            Transform::_270,
            Transform::Flipped,
            Transform::Flipped90,
            Transform::Flipped180,
            Transform::Flipped270,
        ] {
            let name = super::transform_name(transform);
            assert_eq!(
                crate::config::parse_transform(name),
                Ok(transform),
                "{name}"
            );
        }
    }

    /// `colors` writes a color as `[colors]` does, translucent ones
    /// included (premultiplied inside Pila).
    #[test]
    fn colors_read_back_as_written() {
        for text in [
            "#2f3136",
            "#000000",
            "#ffffff",
            "#4477cc4d",
            "#c42b1c80",
            "#12345601",
        ] {
            let color = crate::config::parse_color("c", text, true).unwrap();
            assert_eq!(super::hex(color), text);
        }
        // Fully transparent: no color left to tell.
        let clear = crate::config::parse_color("c", "#ff000000", true).unwrap();
        assert_eq!(super::hex(clear), "#00000000");
    }
}
