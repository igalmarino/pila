//! The desktop portal's ScreenCast backend, served by Pila itself
//! (`org.freedesktop.impl.portal.desktop.pila` on the session bus):
//! xdg-desktop-portal hands apps' screen-sharing requests to it, and the
//! streams are Pila's own PipeWire nodes (`screencast.rs`), fed from the
//! frames it draws. No program of its own: the service lives in the
//! compositor.
//!
//! The bus is served from zbus's thread, never the event loop: a request
//! that needs Pila (`Start`) goes to the event loop as a `Request` and its
//! method waits there, off the loop, for the `Reply`.

use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, Waker},
};

use tracing::{debug, warn};
use zbus::{
    fdo, object_server::SignalEmitter, ObjectServer, OwnedObjectPath, OwnedVariant, Variant,
};

use crate::sync::Locked;

/// The name xdg-desktop-portal finds this backend by (`pila.portal`).
pub const BUS_NAME: &str = "org.freedesktop.impl.portal.desktop.pila";
/// Where backends serve their interfaces.
const PATH: &str = "/org/freedesktop/portal/desktop";

/// Source types (`AvailableSourceTypes`): a monitor.
const SOURCE_MONITOR: u32 = 1;
/// Cursor modes (`AvailableCursorModes`): hidden, drawn into the frames, or
/// sent beside them for the app to draw (metadata: what browsers ask for,
/// and they ask for nothing if it isn't offered).
const CURSOR_HIDDEN: u32 = 1;
const CURSOR_EMBEDDED: u32 = 2;
const CURSOR_METADATA: u32 = 4;

/// How a stream shows the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorMode {
    Hidden,
    /// Drawn into the frames.
    Embedded,
    /// Its position and image in each buffer's cursor metadata, for the
    /// app to draw.
    Metadata,
}

impl CursorMode {
    /// The mode an app asked for (`cursor_mode`); hidden if it asked for
    /// none, as the portal's documentation has it.
    fn from_request(mode: Option<u32>) -> Self {
        match mode {
            Some(CURSOR_EMBEDDED) => CursorMode::Embedded,
            Some(CURSOR_METADATA) => CursorMode::Metadata,
            _ => CursorMode::Hidden,
        }
    }
}

/// A portal response: done, cancelled by the user, or failed.
const RESPONSE_SUCCESS: u32 = 0;
const RESPONSE_CANCELLED: u32 = 1;
const RESPONSE_OTHER: u32 = 2;

/// What the bus asks of the event loop.
pub enum Request {
    /// Pila owns the bus name; keep the connection (dropping it stops the
    /// service).
    Serving(zbus::Connection),
    /// The bus couldn't be served.
    Failed(String),
    /// Start the streams of `session` and reply with them: what's shared
    /// is picked now, by the user (`share_picker.rs`).
    Start {
        session: String,
        cursor: CursorMode,
        /// Who asks, as the picker names it: its desktop entry's name, else
        /// its app id; `None` if the portal didn't say (`app_name`).
        app: Option<String>,
        reply: Reply<Result<Vec<Stream>, Refused>>,
    },
    /// The app (or xdg-desktop-portal) closed `session`: stop its streams.
    Close { session: String },
}

/// A stream started for a session, as `Start` answers it.
#[derive(Debug, Clone)]
pub struct Stream {
    pub node_id: u32,
    /// Where the source is and its size, in logical coordinates.
    pub position: (i32, i32),
    pub size: (i32, i32),
}

/// Why a session didn't start.
#[derive(Debug)]
pub enum Refused {
    /// Nothing was picked.
    Cancelled,
    /// Something failed (logged).
    Failed,
}

/// Start serving the portal on the bus at `address`, on a thread: `send`
/// gets `Request::Serving` (or `Failed`), then the requests.
pub fn serve(address: String, send: calloop::channel::Sender<Request>) {
    let thread_send = send.clone();
    let spawned = std::thread::Builder::new()
        .name("pila-portal".into())
        .spawn(move || {
            let send = thread_send;
            let screencast = ScreenCast {
                send: send.clone(),
                selections: Mutex::default(),
            };
            let built = zbus::block_on(
                zbus::connection::Builder::address(address.as_str())
                    .method_timeout(std::time::Duration::from_secs(5))
                    .name(BUS_NAME)
                    .serve_at(PATH, screencast)
                    .build(),
            );
            let _ = send.send(match built {
                Ok(connection) => Request::Serving(connection),
                Err(err) => Request::Failed(err.to_string()),
            });
        });
    if let Err(err) = spawned {
        let _ = send.send(Request::Failed(err.to_string()));
    }
}

/// Tell the app that Pila ended `session` (its source went away) and stop
/// serving it, on a thread: the bus may be slow.
pub fn end_session(connection: &zbus::Connection, session: String) {
    let connection = connection.clone();
    let spawned = std::thread::Builder::new()
        .name("pila-portal-end".into())
        .spawn(move || {
            let ended = zbus::block_on(async {
                let server = connection.object_server();
                let iface = server.interface::<_, Session>(session.as_str()).await?;
                Session::closed(iface.signal_emitter()).await?;
                server.remove::<Session, _>(session.as_str()).await?;
                zbus::Result::Ok(())
            });
            if let Err(err) = ended {
                debug!(%err, session, "portal: can't end the session on the bus");
            }
        });
    if let Err(err) = spawned {
        warn!(?err, "portal: can't end a session");
    }
}

/// What `SelectSources` chose for a session, used by `Start`.
#[derive(Debug, Clone, Copy)]
struct Selection {
    cursor: CursorMode,
}

/// `org.freedesktop.impl.portal.ScreenCast`.
struct ScreenCast {
    send: calloop::channel::Sender<Request>,
    selections: Mutex<HashMap<String, Selection>>,
}

type Results = HashMap<String, OwnedVariant>;

#[zbus::interface(name = "org.freedesktop.impl.portal.ScreenCast")]
impl ScreenCast {
    async fn create_session(
        &self,
        _handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        app_id: String,
        _options: HashMap<String, OwnedVariant>,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> (u32, Results) {
        let path = session_handle.to_string();
        debug!(app_id, session = path, "portal: new screencast session");
        let session = Session {
            send: self.send.clone(),
            path: path.clone(),
        };
        if let Err(err) = server.at(session_handle.as_ref(), session).await {
            warn!(%err, "portal: can't serve a session");
            return (RESPONSE_OTHER, Results::new());
        }
        self.selections.locked().insert(
            path,
            Selection {
                cursor: CursorMode::Hidden,
            },
        );
        (RESPONSE_SUCCESS, Results::new())
    }

    async fn select_sources(
        &self,
        _handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        _app_id: String,
        options: HashMap<String, OwnedVariant>,
    ) -> (u32, Results) {
        let wanted = |key: &str| options.get(key).and_then(|v| u32::try_from(v).ok());
        let types = wanted("types").unwrap_or(SOURCE_MONITOR);
        if types & SOURCE_MONITOR == 0 {
            debug!(types, "portal: only monitors can be shared");
            return (RESPONSE_OTHER, Results::new());
        }
        let cursor = CursorMode::from_request(wanted("cursor_mode"));
        debug!(types, ?cursor, "portal: sources selected");
        let mut selections = self.selections.locked();
        let Some(selection) = selections.get_mut(session_handle.as_str()) else {
            return (RESPONSE_OTHER, Results::new());
        };
        selection.cursor = cursor;
        (RESPONSE_SUCCESS, Results::new())
    }

    async fn start(
        &self,
        _handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        app_id: String,
        _parent_window: String,
        _options: HashMap<String, OwnedVariant>,
    ) -> (u32, Results) {
        let session = session_handle.to_string();
        let Some(selection) = self.selections.locked().get(&session).copied() else {
            return (RESPONSE_OTHER, Results::new());
        };
        let (reply, answer) = reply_channel();
        let request = Request::Start {
            session,
            cursor: selection.cursor,
            app: app_name(&app_id, &data_dirs()),
            reply,
        };
        if self.send.send(request).is_err() {
            return (RESPONSE_OTHER, Results::new());
        }
        let streams = match answer.await {
            Some(Ok(streams)) => streams,
            Some(Err(Refused::Cancelled)) => return (RESPONSE_CANCELLED, Results::new()),
            Some(Err(Refused::Failed)) | None => return (RESPONSE_OTHER, Results::new()),
        };
        match streams_result(&streams) {
            Ok(results) => (RESPONSE_SUCCESS, results),
            Err(err) => {
                warn!(%err, "portal: can't describe the streams");
                (RESPONSE_OTHER, Results::new())
            }
        }
    }

    #[zbus(property)]
    fn available_source_types(&self) -> u32 {
        SOURCE_MONITOR
    }

    #[zbus(property)]
    fn available_cursor_modes(&self) -> u32 {
        CURSOR_HIDDEN | CURSOR_EMBEDDED | CURSOR_METADATA
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        5
    }
}

/// Where desktop entries are looked for: `$XDG_DATA_HOME` (or
/// `~/.local/share`), then `$XDG_DATA_DIRS` (or `/usr/local/share` and
/// `/usr/share`), each's `applications`.
fn data_dirs() -> Vec<std::path::PathBuf> {
    let env = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
    let home = env("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| env("HOME").map(|home| std::path::Path::new(&home).join(".local/share")));
    let system = env("XDG_DATA_DIRS")
        .map(|dirs| std::env::split_paths(&dirs).collect::<Vec<_>>())
        .unwrap_or_else(|| vec!["/usr/local/share".into(), "/usr/share".into()]);
    home.into_iter()
        .chain(system)
        .map(|dir| dir.join("applications"))
        .collect()
}

/// The name an app goes by: its desktop entry's `Name` (the first
/// `<app_id>.desktop` in `dirs`), else the app id itself; `None` for no app
/// id (apps outside a sandbox often have none). Reads files: on the bus's
/// thread, never the event loop.
fn app_name(app_id: &str, dirs: &[std::path::PathBuf]) -> Option<String> {
    let app_id = app_id.trim();
    if app_id.is_empty() {
        return None;
    }
    let file = format!("{app_id}.desktop");
    let name = dirs.iter().find_map(|dir| {
        let text = std::fs::read_to_string(dir.join(&file)).ok()?;
        let mut in_entry = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_entry = line == "[Desktop Entry]";
            } else if in_entry {
                if let Some(name) = line.strip_prefix("Name=") {
                    return Some(name.trim().to_string()).filter(|n| !n.is_empty());
                }
            }
        }
        None
    });
    Some(name.unwrap_or_else(|| app_id.to_string()))
}

/// `Start`'s results: `streams`, `a(ua{sv})`.
fn streams_result(streams: &[Stream]) -> zbus::Result<Results> {
    let streams: Vec<(u32, HashMap<String, Variant<'static>>)> = streams
        .iter()
        .map(|stream| {
            let properties = HashMap::from([
                ("position".to_string(), Variant::from(stream.position)),
                ("size".to_string(), Variant::from(stream.size)),
                ("source_type".to_string(), Variant::from(SOURCE_MONITOR)),
            ]);
            (stream.node_id, properties)
        })
        .collect();
    let streams = OwnedVariant::try_from(Variant::from(streams))?;
    Ok(Results::from([("streams".to_string(), streams)]))
}

/// `org.freedesktop.impl.portal.Session`, one per screencast session.
struct Session {
    send: calloop::channel::Sender<Request>,
    path: String,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Session")]
impl Session {
    async fn close(&self, #[zbus(object_server)] server: &ObjectServer) -> fdo::Result<()> {
        let _ = self.send.send(Request::Close {
            session: self.path.clone(),
        });
        server.remove::<Session, _>(self.path.as_str()).await?;
        Ok(())
    }

    #[zbus(signal)]
    async fn closed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }
}

/// The event loop's side of a request's answer: `send` it once. Dropped
/// unsent, the waiting method gets `None`.
pub struct Reply<T>(Arc<Mutex<Slot<T>>>);

/// The bus's side: a future of the answer.
struct Answer<T>(Arc<Mutex<Slot<T>>>);

struct Slot<T> {
    value: Option<T>,
    closed: bool,
    waker: Option<Waker>,
}

fn reply_channel<T>() -> (Reply<T>, Answer<T>) {
    let slot = Arc::new(Mutex::new(Slot {
        value: None,
        closed: false,
        waker: None,
    }));
    (Reply(slot.clone()), Answer(slot))
}

impl<T> Reply<T> {
    pub fn send(self, value: T) {
        self.0.locked().value = Some(value);
        // Drop wakes the method.
    }
}

impl<T> Drop for Reply<T> {
    fn drop(&mut self) {
        let mut slot = self.0.locked();
        slot.closed = true;
        if let Some(waker) = slot.waker.take() {
            waker.wake();
        }
    }
}

impl<T> Future for Answer<T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        let mut slot = self.0.locked();
        if let Some(value) = slot.value.take() {
            return Poll::Ready(Some(value));
        }
        if slot.closed {
            return Poll::Ready(None);
        }
        slot.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_is_what_was_sent_or_none_if_dropped() {
        let (reply, answer) = reply_channel::<u32>();
        reply.send(7);
        assert_eq!(zbus::block_on(answer), Some(7));
        let (reply, answer) = reply_channel::<u32>();
        drop(reply);
        assert_eq!(zbus::block_on(answer), None);
    }

    #[test]
    fn an_app_is_named_by_its_desktop_entry_else_its_id() {
        let dir = std::env::temp_dir().join(format!("pila-app-name-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("org.example.Browser.desktop"),
            "[Desktop Action new]\nName=New Window\n\n[Desktop Entry]\nName[es]=Navegador\nName=Web Browser\n",
        )
        .unwrap();
        let dirs = [dir.join("missing"), dir.clone()];
        assert_eq!(
            app_name("org.example.Browser", &dirs).as_deref(),
            Some("Web Browser")
        );
        assert_eq!(
            app_name("org.example.Other", &dirs).as_deref(),
            Some("org.example.Other")
        );
        assert_eq!(app_name("", &dirs), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn streams_are_described_as_the_portal_wants() {
        let results = streams_result(&[Stream {
            node_id: 42,
            position: (0, 0),
            size: (1920, 1080),
        }])
        .unwrap();
        let streams = &results["streams"];
        assert_eq!(streams.value_signature().to_string(), "a(ua{sv})");
    }
}
