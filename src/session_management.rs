//! xdg-session-management-v1: a client asks pila to keep its toplevels' state
//! and restore it later, even after pila or the client restarts.
//!
//! A session (a random id the client keeps) holds named toplevels. While a
//! toplevel is in a session (`add_toplevel`, `restore_toplevel`), its state
//! follows the window (`refresh_sessions`, each loop iteration, only over those
//! windows): position and size when floating (for a snapped or filled one,
//! those it returns to), maximized, fullscreen. That's all it keeps: not the
//! workspace, minimized nor the stacking order; the protocol leaves that to
//! the compositor. Nothing is kept at a window's closing, then: the last
//! state is already here, and a client's null buffer would have lost its size.
//!
//! `restore_toplevel` of a known name applies the state in the initial
//! configure (`begin_session_restore`, before the window rules, which win) and
//! when placing the window (`take_session_restore`; a window goes where it was
//! on its output, inside the usable area; with the output gone it opens as
//! usual). A window a client unmaps and maps again without that is a new one.
//!
//! Sessions are saved in `$XDG_STATE_HOME/pila/sessions.json`, by a thread, a
//! second after the last change and at exit; never on the event loop. Nested,
//! they stay in memory (`load_and_persist`), so that a pila on the side leaves
//! the login session's alone, and so they do in a second pila on the same state
//! (`take_sessions_lock`; `scripts/tty`). At most `MAX_SESSIONS`: the least
//! recently used not in use go, empty ones first. What a client can leave there
//! is bounded (`MAX_TOPLEVELS`, `MAX_NAME_BYTES`, `MAX_CLIENT_SESSIONS`).
//!
//! It only touches the client's own windows, and a session id is secret and
//! unguessable, so sandboxed apps get it too (no `is_privileged`). X11 windows
//! have no such protocol.

use std::{
    collections::HashMap,
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::mpsc::{self, Sender},
    thread::JoinHandle,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use smithay::{
    desktop::Window,
    output::Output,
    reexports::{
        calloop::timer::{TimeoutAction, Timer},
        wayland_server::{
            backend::ClientId, protocol::wl_surface::WlSurface, Client, DataInit, Dispatch,
            DisplayHandle, GlobalDispatch, New, Resource, WEnum,
        },
    },
    utils::{Logical, Point, Rectangle, Size},
    wayland::shell::xdg::ToplevelSurface,
};
use tracing::{debug, info, warn};

use crate::{
    session_protocol::{
        xdg_session_manager_v1::{self, XdgSessionManagerV1},
        xdg_session_v1::{self, XdgSessionV1},
        xdg_toplevel_session_v1::{self, XdgToplevelSessionV1},
    },
    snap::snapped_from,
    state::{fill_state, restore_geometry},
    Pila,
};

const VERSION: u32 = 1;
const MAX_SESSIONS: usize = 256;
/// The saved state a client can leave behind is bounded, since it's on disk
/// and a sandboxed app may ask for it: toplevels per session, the bytes of
/// their names, and sessions a client can have in use at once.
const MAX_TOPLEVELS: usize = 128;
const MAX_NAME_BYTES: usize = 256;
const MAX_CLIENT_SESSIONS: usize = 32;
/// How long after a change sessions are saved.
const SAVE_DELAY: Duration = Duration::from_secs(1);

/// The protocol's state (`Pila::session_management`).
#[derive(Default)]
pub struct SessionManagement {
    sessions: HashMap<String, Session>,
    /// Toplevels in a session that's in use.
    tracked: Vec<Tracked>,
    /// `None`: only in memory.
    writer: Option<Writer>,
    save_scheduled: bool,
}

#[derive(Serialize, Deserialize)]
struct Session {
    toplevels: HashMap<String, Saved>,
    /// Seconds since the epoch.
    last_used: u64,
    /// The object using it. Another object for this session is inert.
    #[serde(skip)]
    owner: Option<XdgSessionV1>,
}

/// A toplevel's state. All default: nothing known yet.
#[derive(Serialize, Deserialize, Clone, Copy, Default, PartialEq, Debug)]
struct Saved {
    /// x, y, width, height when floating.
    rect: Option<(i32, i32, i32, i32)>,
    maximized: bool,
    fullscreen: bool,
}

impl Saved {
    fn rect(&self) -> Option<Rectangle<i32, Logical>> {
        self.rect
            .map(|(x, y, w, h)| Rectangle::new((x, y).into(), (w, h).into()))
    }
}

struct Tracked {
    object: XdgToplevelSessionV1,
    toplevel: ToplevelSurface,
    session: String,
    name: String,
    /// `restore_toplevel` found this, and the window isn't placed yet.
    restoring: Option<Saved>,
}

/// The saving thread.
struct Writer {
    texts: Sender<String>,
    thread: JoinHandle<()>,
    /// Held while this pila saves (`take_sessions_lock`).
    _lock: Option<std::fs::File>,
}

/// Create the global. Called once from `Pila::new`.
pub fn init_global(dh: &DisplayHandle) {
    dh.create_global::<Pila, XdgSessionManagerV1, ()>(VERSION, ());
}

/// `$XDG_STATE_HOME/pila/sessions.json`.
fn sessions_path() -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        })?;
    Some(state.join("pila/sessions.json"))
}

/// Write `text` to `path` whole or not at all. Sessions' ids are secrets: only
/// for the user.
fn write_atomically(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temporary = path.with_extension("json.tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&temporary, path)
}

/// Only one pila at a time saves sessions (`scripts/tty` runs one next to the
/// login session's): the lock on a file next to them, held until it exits, or
/// `None` if another pila has it.
fn take_sessions_lock(path: &Path) -> std::io::Result<Option<std::fs::File>> {
    use smithay::reexports::rustix::{
        fs::{flock, FlockOperation},
        io::Errno,
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path.with_extension("lock"))?;
    match flock(&file, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(Some(file)),
        Err(Errno::WOULDBLOCK) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

/// 128 random bits, in hex.
fn new_session_id() -> Option<String> {
    let mut bytes = [0u8; 16];
    let read = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes));
    if let Err(err) = read {
        warn!(?err, "no random session id");
        return None;
    }
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `loc` moved as little as possible for a window of `size`, with a title bar
/// `bar` high above it, to be inside `area`; its top left stays inside if it's
/// too big to fit.
pub fn fit_in_area(
    loc: Point<i32, Logical>,
    size: Size<i32, Logical>,
    bar: i32,
    area: Rectangle<i32, Logical>,
) -> Point<i32, Logical> {
    let end = area.loc + area.size.to_point();
    Point::from((
        loc.x.min(end.x - size.w).max(area.loc.x),
        loc.y.min(end.y - size.h).max(area.loc.y + bar),
    ))
}

impl SessionManagement {
    /// Load the saved sessions (a broken file: none), and save them from now on.
    pub fn load_and_persist(&mut self) {
        let Some(path) = sessions_path() else {
            warn!("no state directory: sessions won't outlive pila");
            return;
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(sessions) => self.sessions = sessions,
                Err(err) => warn!(?path, %err, "ignoring a broken sessions file"),
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => warn!(?path, %err, "can't read the sessions file"),
        }
        debug!(sessions = self.sessions.len(), "sessions loaded");
        let lock = match take_sessions_lock(&path) {
            Ok(Some(lock)) => Some(lock),
            Ok(None) => {
                info!("another pila saves the sessions: this one's sessions won't be saved");
                return;
            }
            // A filesystem that can't lock: saving matters more.
            Err(err) => {
                warn!(?path, %err, "can't lock the sessions file");
                None
            }
        };
        let (texts, received) = mpsc::channel::<String>();
        let thread = std::thread::Builder::new()
            .name("pila-sessions".into())
            .spawn(move || {
                while let Ok(mut text) = received.recv() {
                    while let Ok(newer) = received.try_recv() {
                        text = newer;
                    }
                    if let Err(err) = write_atomically(&path, &text) {
                        warn!(?path, %err, "can't save the sessions");
                    }
                }
            });
        match thread {
            Ok(thread) => {
                self.writer = Some(Writer {
                    texts,
                    thread,
                    _lock: lock,
                })
            }
            Err(err) => warn!(%err, "no thread to save the sessions"),
        }
    }

    fn save(&mut self) {
        self.save_scheduled = false;
        let Some(writer) = &self.writer else {
            return;
        };
        match serde_json::to_string(&self.sessions) {
            Ok(text) => {
                let _ = writer.texts.send(text);
            }
            Err(err) => warn!(%err, "can't serialize the sessions"),
        }
    }

    /// Make room for a new session: the least recently used not in use, those
    /// with no windows to restore first.
    fn evict(&mut self) {
        while self.sessions.len() >= MAX_SESSIONS {
            let oldest = self
                .sessions
                .iter()
                .filter(|(_, session)| session.owner.is_none())
                .min_by_key(|(_, session)| (!session.toplevels.is_empty(), session.last_used))
                .map(|(id, _)| id.clone());
            let Some(oldest) = oldest else {
                return;
            };
            debug!(session = oldest, "forgetting the oldest session");
            self.sessions.remove(&oldest);
        }
    }

    /// Make room for toplevel `name` in session `id`: one no window is using
    /// goes if it's full. Whether it can go in.
    fn room_for_toplevel(&mut self, id: &str, name: &str) -> bool {
        let tracked = &self.tracked;
        let Some(session) = self.sessions.get_mut(id) else {
            return false;
        };
        if session.toplevels.len() < MAX_TOPLEVELS || session.toplevels.contains_key(name) {
            return true;
        }
        let unused = session
            .toplevels
            .keys()
            .filter(|known| !tracked.iter().any(|t| t.session == id && t.name == **known))
            .min()
            .cloned();
        match unused {
            Some(unused) => {
                debug!(session = id, toplevel = unused, "forgetting a toplevel");
                session.toplevels.remove(&unused);
                true
            }
            None => {
                debug!(session = id, "a session with too many toplevels");
                false
            }
        }
    }

    /// The sessions `client` has in use.
    fn client_sessions(&self, client: &Client) -> usize {
        self.sessions
            .values()
            .filter(|s| s.owner.as_ref().and_then(|o| o.client()).as_ref() == Some(client))
            .count()
    }

    /// The session `object` uses; `None` if it's inert.
    fn owned_by(&mut self, object: &XdgSessionV1, id: &str) -> Option<&mut Session> {
        self.sessions
            .get_mut(id)
            .filter(|session| session.owner.as_ref() == Some(object))
    }

    fn tracked_for(&self, surface: &WlSurface) -> Option<&Tracked> {
        self.tracked
            .iter()
            .find(|t| t.toplevel.wl_surface() == surface)
    }
}

impl Pila {
    /// A change to save, soon.
    fn schedule_session_save(&mut self) {
        let sessions = &mut self.session_management;
        if sessions.writer.is_none() || sessions.save_scheduled {
            return;
        }
        sessions.save_scheduled = true;
        let timer = Timer::from_duration(SAVE_DELAY);
        let inserted = self.loop_handle.insert_source(timer, |_, _, state| {
            state.session_management.save();
            TimeoutAction::Drop
        });
        if let Err(err) = inserted {
            warn!(%err, "can't schedule saving the sessions");
            self.session_management.save_scheduled = false;
        }
    }

    /// pila is quitting: save what changed, and wait for it.
    pub fn finish_sessions(&mut self) {
        self.refresh_sessions();
        if self.session_management.save_scheduled {
            self.session_management.save();
        }
        if let Some(Writer { texts, thread, .. }) = self.session_management.writer.take() {
            drop(texts);
            let _ = thread.join();
        }
    }

    /// Bring the saved state up to date with the windows in sessions.
    pub fn refresh_sessions(&mut self) {
        let mut changed = false;
        for index in 0..self.session_management.tracked.len() {
            let tracked = &self.session_management.tracked[index];
            let Some(window) = self.window_for_surface(tracked.toplevel.wl_surface()) else {
                continue;
            };
            let previous = self
                .session_management
                .sessions
                .get(&tracked.session)
                .and_then(|session| session.toplevels.get(&tracked.name))
                .copied()
                .unwrap_or_default();
            let Some(current) = self.window_saved_state(&window, previous) else {
                continue;
            };
            if current == previous {
                continue;
            }
            let sessions = &mut self.session_management;
            let tracked = &sessions.tracked[index];
            if let Some(session) = sessions.sessions.get_mut(&tracked.session) {
                session.toplevels.insert(tracked.name.clone(), current);
                changed = true;
            }
        }
        if changed {
            self.schedule_session_save();
        }
    }

    /// A mapped window's state to save; `None` if it isn't mapped. A filled
    /// one that never floated keeps the `previous` rect.
    fn window_saved_state(&self, window: &Window, previous: Saved) -> Option<Saved> {
        let location = self.window_location(window)?;
        let (maximized, fullscreen) = fill_state(window);
        let rect = if let Some(before) = snapped_from(window) {
            before
        } else if maximized || fullscreen {
            match restore_geometry(window).or(previous.rect()) {
                Some(rect) => rect,
                None => {
                    return Some(Saved {
                        rect: None,
                        maximized,
                        fullscreen,
                    })
                }
            }
        } else {
            Rectangle::new(location, window.geometry().size)
        };
        if rect.is_empty() {
            return None;
        }
        Some(Saved {
            rect: Some((rect.loc.x, rect.loc.y, rect.size.w, rect.size.h)),
            maximized,
            fullscreen,
        })
    }

    /// What `restore_toplevel` found for this window, not yet placed.
    fn session_restoring(&self, window: &Window) -> Option<Saved> {
        if self.session_management.tracked.is_empty() {
            return None;
        }
        self.session_management
            .tracked_for(window.toplevel()?.wl_surface())?
            .restoring
    }

    /// The output a window being restored goes on, if it's still there.
    pub fn session_output(&self, window: &Window) -> Option<Output> {
        self.output_overlapping(self.session_restoring(window)?.rect()?)
    }

    /// A window's initial configure is about to go: tell the client it's being
    /// restored, and set its size and fill state.
    pub fn begin_session_restore(&mut self, window: &Window) {
        let Some(saved) = self.session_restoring(window) else {
            return;
        };
        let Some(toplevel) = window.toplevel() else {
            return;
        };
        debug!(?saved, "restoring a window from its session");
        if let Some(tracked) = self.session_management.tracked_for(toplevel.wl_surface()) {
            tracked.object.restored();
        }
        if let Some(rect) = saved.rect() {
            toplevel.with_pending_state(|state| state.size = Some(rect.size));
        }
        if saved.maximized || saved.fullscreen {
            // On the output `output_geometry_for_window` gives it.
            self.set_fill_state(window, saved.maximized, saved.fullscreen);
        }
    }

    /// A window is being placed: the rect its session had and that rect's
    /// output, if it's being restored and the output is still there.
    pub fn take_session_restore(
        &mut self,
        window: &Window,
    ) -> Option<(Rectangle<i32, Logical>, Output)> {
        let output = self.session_output(window);
        let surface = window.toplevel()?.wl_surface();
        let saved = self
            .session_management
            .tracked
            .iter_mut()
            .find(|t| t.toplevel.wl_surface() == surface)?
            .restoring
            .take()?;
        Some((saved.rect()?, output?))
    }

    /// A toplevel was destroyed: its state stays, for restoring later.
    pub fn forget_session_toplevel(&mut self, surface: &WlSurface) {
        self.session_management
            .tracked
            .retain(|t| t.toplevel.wl_surface() != surface);
    }

    /// `add_toplevel` and `restore_toplevel`: the checks both share. Whether
    /// the toplevel can go in.
    fn check_new_toplevel(
        &self,
        session: &XdgSessionV1,
        id: &str,
        toplevel: &ToplevelSurface,
        name: &str,
    ) -> bool {
        let sessions = &self.session_management;
        if name.is_empty() {
            session.post_error(xdg_session_v1::Error::InvalidName, "an empty toplevel name");
            return false;
        }
        if name.len() > MAX_NAME_BYTES {
            session.post_error(
                xdg_session_v1::Error::InvalidName,
                "a too long toplevel name",
            );
            return false;
        }
        if sessions.tracked_for(toplevel.wl_surface()).is_some() {
            session.post_error(
                xdg_session_v1::Error::AlreadyAdded,
                "the toplevel is already in a session",
            );
            return false;
        }
        if sessions
            .tracked
            .iter()
            .any(|t| t.session == id && t.name == name)
        {
            session.post_error(
                xdg_session_v1::Error::NameInUse,
                format!("toplevel name {name:?} is in use"),
            );
            return false;
        }
        true
    }

    /// Stop updating the toplevels of a session whose object went or was
    /// replaced.
    fn untrack_session(&mut self, id: &str) {
        self.session_management.tracked.retain(|t| t.session != id);
    }
}

impl GlobalDispatch<XdgSessionManagerV1, ()> for Pila {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<XdgSessionManagerV1>,
        _: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<XdgSessionManagerV1, ()> for Pila {
    fn request(
        state: &mut Self,
        client: &Client,
        manager: &XdgSessionManagerV1,
        request: xdg_session_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let xdg_session_manager_v1::Request::GetSession {
            id,
            reason,
            session_id,
        } = request
        else {
            return;
        };
        let empty_id = session_id.as_deref() == Some("");
        let known = session_id.filter(|id| state.session_management.sessions.contains_key(id));
        let is_known = known.is_some();
        // A failure leaves the object inert: the session id `""` is no session.
        let session_id = known.or_else(new_session_id).unwrap_or_default();
        let session = data_init.init(id, session_id.clone());
        if !matches!(reason, WEnum::Value(_)) {
            manager.post_error(
                xdg_session_manager_v1::Error::InvalidReason,
                "an invalid reason",
            );
            return;
        }
        if empty_id {
            manager.post_error(
                xdg_session_manager_v1::Error::InvalidSessionId,
                "an empty session id",
            );
            return;
        }
        if session_id.is_empty() {
            return;
        }
        if state.session_management.client_sessions(client) >= MAX_CLIENT_SESSIONS {
            debug!("a client with too many sessions: this one is inert");
            return;
        }
        if is_known {
            let owner = state.session_management.sessions[&session_id].owner.clone();
            if let Some(owner) = owner {
                if owner.client().as_ref() == Some(client) {
                    manager.post_error(
                        xdg_session_manager_v1::Error::InUse,
                        "the session is already in use",
                    );
                    return;
                }
                owner.replaced();
                state.untrack_session(&session_id);
            }
            let saved = state
                .session_management
                .sessions
                .get_mut(&session_id)
                .expect("known");
            saved.owner = Some(session.clone());
            saved.last_used = now();
            debug!(session = session_id, "session restored");
            session.restored();
        } else {
            state.session_management.evict();
            state.session_management.sessions.insert(
                session_id.clone(),
                Session {
                    toplevels: HashMap::new(),
                    last_used: now(),
                    owner: Some(session.clone()),
                },
            );
            debug!(session = session_id, "session created");
            session.created(session_id);
        }
        state.schedule_session_save();
    }
}

impl Dispatch<XdgSessionV1, String> for Pila {
    fn request(
        state: &mut Self,
        _: &Client,
        session: &XdgSessionV1,
        request: xdg_session_v1::Request,
        id: &String,
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        use xdg_session_v1::Request;
        let in_use = state.session_management.owned_by(session, id).is_some();
        match request {
            Request::AddToplevel {
                id: object,
                toplevel,
                name,
            } => {
                let object = data_init.init(object, ());
                let toplevel = state.protocols.xdg_shell.get_toplevel(&toplevel);
                let (true, Some(toplevel)) = (in_use, toplevel) else {
                    return;
                };
                if !state.check_new_toplevel(session, id, &toplevel, &name) {
                    return;
                }
                let sessions = &mut state.session_management;
                if sessions.sessions[id].toplevels.contains_key(&name) {
                    session.post_error(
                        xdg_session_v1::Error::NameInUse,
                        format!("toplevel name {name:?} is known; restore it"),
                    );
                    return;
                }
                if !sessions.room_for_toplevel(id, &name) {
                    return;
                }
                if let Some(owned) = sessions.owned_by(session, id) {
                    owned.toplevels.insert(name.clone(), Saved::default());
                }
                sessions.tracked.push(Tracked {
                    object,
                    toplevel,
                    session: id.clone(),
                    name,
                    restoring: None,
                });
                state.schedule_session_save();
            }
            Request::RestoreToplevel {
                id: object,
                toplevel,
                name,
            } => {
                let object = data_init.init(object, ());
                let toplevel = state.protocols.xdg_shell.get_toplevel(&toplevel);
                let (true, Some(toplevel)) = (in_use, toplevel) else {
                    return;
                };
                if !state.check_new_toplevel(session, id, &toplevel, &name) {
                    return;
                }
                if toplevel.is_initial_configure_sent() {
                    session.post_error(
                        xdg_session_v1::Error::AlreadyMapped,
                        "restoring a toplevel after its first commit",
                    );
                    return;
                }
                let sessions = &mut state.session_management;
                if !sessions.room_for_toplevel(id, &name) {
                    return;
                }
                let Some(owned) = sessions.owned_by(session, id) else {
                    return;
                };
                let saved = owned.toplevels.entry(name.clone()).or_default();
                let restoring = Some(*saved).filter(|saved| *saved != Saved::default());
                sessions.tracked.push(Tracked {
                    object,
                    toplevel,
                    session: id.clone(),
                    name,
                    restoring,
                });
                state.schedule_session_save();
            }
            Request::RemoveToplevel { name } => {
                let sessions = &mut state.session_management;
                let Some(owned) = sessions.owned_by(session, id) else {
                    return;
                };
                owned.toplevels.remove(&name);
                sessions
                    .tracked
                    .retain(|t| t.session != *id || t.name != name);
                state.schedule_session_save();
            }
            Request::Remove => {
                if in_use {
                    state.session_management.sessions.remove(id);
                    state.untrack_session(id);
                    state.schedule_session_save();
                }
            }
            // `destroyed` does it.
            Request::Destroy => {}
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, session: &XdgSessionV1, id: &String) {
        if let Some(owned) = state.session_management.owned_by(session, id) {
            owned.owner = None;
            state.untrack_session(id);
        }
    }
}

impl Dispatch<XdgToplevelSessionV1, ()> for Pila {
    fn request(
        state: &mut Self,
        _: &Client,
        object: &XdgToplevelSessionV1,
        request: xdg_toplevel_session_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let xdg_toplevel_session_v1::Request::Rename { name } = request else {
            return;
        };
        let sessions = &mut state.session_management;
        let Some(tracked) = sessions.tracked.iter_mut().find(|t| t.object == *object) else {
            return;
        };
        let Some(session) = sessions.sessions.get_mut(&tracked.session) else {
            return;
        };
        if name == tracked.name {
            return;
        }
        if name.len() > MAX_NAME_BYTES {
            if let Some(owner) = &session.owner {
                owner.post_error(
                    xdg_session_v1::Error::InvalidName,
                    "a too long toplevel name",
                );
            }
            return;
        }
        if session.toplevels.contains_key(&name) {
            if let Some(owner) = &session.owner {
                owner.post_error(
                    xdg_session_v1::Error::NameInUse,
                    format!("toplevel name {name:?} is in use"),
                );
            }
            return;
        }
        let saved = session.toplevels.remove(&tracked.name).unwrap_or_default();
        session.toplevels.insert(name.clone(), saved);
        tracked.name = name;
        state.schedule_session_save();
    }

    fn destroyed(state: &mut Self, _: ClientId, object: &XdgToplevelSessionV1, _: &()) {
        state
            .session_management
            .tracked
            .retain(|t| t.object != *object);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: ((i32, i32), (i32, i32)) = ((100, 50), (1000, 800));

    fn fit(loc: (i32, i32), size: (i32, i32), bar: i32) -> (i32, i32) {
        let area = Rectangle::new(AREA.0.into(), AREA.1.into());
        let fitted = fit_in_area(loc.into(), size.into(), bar, area);
        (fitted.x, fitted.y)
    }

    #[test]
    fn a_window_inside_the_area_stays_where_it_is() {
        assert_eq!(fit((300, 200), (400, 300), 28), (300, 200));
        // Touching the edges, its title bar included.
        assert_eq!(fit((100, 78), (1000, 772), 28), (100, 78));
    }

    #[test]
    fn a_window_partly_outside_moves_in() {
        assert_eq!(fit((-50, 200), (400, 300), 28), (100, 200));
        assert_eq!(fit((900, 700), (400, 300), 28), (700, 550));
        // Its title bar counts at the top.
        assert_eq!(fit((300, 60), (400, 300), 28), (300, 78));
    }

    #[test]
    fn a_window_too_big_keeps_its_top_left_inside() {
        assert_eq!(fit((300, 200), (2000, 2000), 28), (100, 78));
    }

    #[test]
    fn saved_sessions_read_back_as_written() {
        let saved = Saved {
            rect: Some((1, 2, 3, 4)),
            maximized: true,
            fullscreen: false,
        };
        let sessions = HashMap::from([(
            "id".to_string(),
            Session {
                toplevels: HashMap::from([("main".to_string(), saved)]),
                last_used: 5,
                owner: None,
            },
        )]);
        let text = serde_json::to_string(&sessions).unwrap();
        let read: HashMap<String, Session> = serde_json::from_str(&text).unwrap();
        assert_eq!(read["id"].toplevels["main"], saved);
        assert_eq!(read["id"].last_used, 5);
    }
}
