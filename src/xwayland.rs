//! X11 apps through Xwayland. pila reserves an X display at launch (its
//! number, lock file and sockets: `XWaylandDisplay`), which spawned clients get
//! as `DISPLAY`, and starts Xwayland when an X11 app first connects to it
//! (`listen_for_x11`), running Smithay's X11 window manager (`X11Wm`) on it.
//! With `[xwayland] idle-timeout` it stops again once no X11 client is left
//! (`check_xwayland_idle`), and the next one starts it. `[xwayland] enabled =
//! false` reserves nothing; turned off while Xwayland runs, it stays for its
//! apps and the display goes when it stops (`xwayland_config_changed`).
//!
//! X11 windows live in the same `Space` as Wayland ones (`Window::new_x11_window`).
//! Managed windows are placed and focused like Wayland toplevels; override-
//! redirect windows (menus, tooltips) sit where they ask, above everything,
//! without focus. pila tells X where a managed window is (`X11Surface::configure`)
//! whenever it moves it (`Pila::x11_window_moved`).
//!
//! The app waits in the socket's queue while Xwayland starts, so `pila -c
//! xterm` works. Xwayland dying before it's ready counts as failed
//! (`XWaylandSource`).

use std::{
    os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
    process::Stdio,
    time::{Duration, Instant},
};

use smithay::{
    desktop::Window,
    input::{pointer::GrabStartData as PointerGrabStartData, Seat},
    reexports::calloop::{
        generic::Generic,
        timer::{TimeoutAction, Timer},
        EventSource, Interest, Mode, Poll, PostAction, Readiness, RegistrationToken, Token,
        TokenFactory,
    },
    reexports::wayland_server::{protocol::wl_surface::WlSurface, Resource},
    utils::{Logical, Rectangle, SERIAL_COUNTER},
    wayland::{
        compositor::CompositorHandler,
        selection::{
            data_device::{
                clear_data_device_selection, current_data_device_selection_userdata,
                request_data_device_client_selection, set_data_device_selection,
            },
            primary_selection::{
                clear_primary_selection, current_primary_selection_userdata,
                request_primary_client_selection, set_primary_selection,
            },
            SelectionTarget,
        },
        xwayland_keyboard_grab::{XWaylandKeyboardGrab, XWaylandKeyboardGrabHandler},
        xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    },
    xwayland::{
        xwm::{
            Reorder, ResizeEdge as X11ResizeEdge, WmInputModel, WmWindowProperty, WmWindowType,
            XwmId,
        },
        X11Surface, X11Wm, XWayland, XWaylandDisplay, XWaylandEvent, XwmHandler,
    },
};
use tracing::{debug, info, warn};

use crate::{focus::KeyboardFocus, state::fill_state, sync::Locked, Pila};

/// Where X servers put their sockets. Usually created at boot (tmpfiles)
/// or by the first X server; a minimal system or a container may lack it,
/// and then Xwayland can't start. Created like X does: world-writable with
/// the sticky bit.
const X11_SOCKET_DIR: &str = "/tmp/.X11-unix";

/// Whether a directory of `owner` is safe for this user's X sockets: it should
/// be root's, or this user's own. Anyone else's could listen in.
fn trusted_owner(owner: u32, me: u32) -> bool {
    owner == 0 || owner == me
}

fn ensure_x11_socket_dir() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let dir = std::path::Path::new(X11_SOCKET_DIR);
    if let Ok(metadata) = std::fs::symlink_metadata(dir) {
        // SAFETY: getuid has no failure modes or preconditions.
        let me = unsafe { libc::getuid() };
        if !trusted_owner(metadata.uid(), me) {
            warn!(
                dir = X11_SOCKET_DIR,
                owner = metadata.uid(),
                "the X11 socket directory belongs to another user, who can reach X11 apps"
            );
        }
        return;
    }
    let created = std::fs::create_dir(dir)
        .and_then(|()| std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o1777)));
    match created {
        Ok(()) => info!(dir = X11_SOCKET_DIR, "created the X11 socket directory"),
        // Another X server may have just made it.
        Err(_) if dir.exists() => {}
        Err(err) => warn!(
            ?err,
            dir = X11_SOCKET_DIR,
            "can't create the X11 socket directory"
        ),
    }
}

/// How many times Xwayland may fail (die, or not start) within
/// `RESTART_WINDOW` and still be started again: an app waiting in the socket's
/// queue would start a broken one forever.
const MAX_RESTARTS: usize = 3;
const RESTART_WINDOW: Duration = Duration::from_secs(60);

/// Whether Xwayland may be started again after failing `now`, given when it
/// failed lately.
fn may_restart(failures: &mut Vec<Instant>, now: Instant) -> bool {
    failures.retain(|at| now.duration_since(*at) < RESTART_WINDOW);
    if failures.len() >= MAX_RESTARTS {
        return false;
    }
    failures.push(now);
    true
}

/// One of the reserved display's listening sockets, watched while Xwayland
/// doesn't run (`Pila::listen_for_x11`). Holding the reservation keeps the
/// socket open while it's watched.
struct ListenSocket(XWaylandDisplay, usize);

impl AsFd for ListenSocket {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.listen_sockets()[self.1].as_fd()
    }
}

/// Whether the X clients on `display` are all pila's own (its window manager
/// and the connection asking) or Xwayland's (`xwayland`: it has one of its
/// own). X knows a local client's process from its socket (XRes
/// `LocalClientPID`); one X can't name counts as someone else's. Blocks: run
/// off the event loop.
fn only_pilas_x11_clients(display: u32, cookie: &[u8], xwayland: u32) -> Result<bool, String> {
    use x11rb::{
        connection::RequestConnection,
        protocol::res::{ClientIdMask, ClientIdSpec, ConnectionExt as _},
    };
    let error = |err: &dyn std::fmt::Display| err.to_string();
    let (conn, _) = crate::xauth::connect(display, cookie)?;
    conn.extension_information(x11rb::protocol::res::X11_EXTENSION_NAME)
        .map_err(|e| error(&e))?
        .ok_or("Xwayland has no X-Resource extension")?;
    conn.res_query_version(1, 2)
        .map_err(|e| error(&e))?
        .reply()
        .map_err(|e| error(&e))?;
    let clients = conn
        .res_query_clients()
        .map_err(|e| error(&e))?
        .reply()
        .map_err(|e| error(&e))?
        .clients;
    let all = ClientIdSpec {
        client: 0,
        mask: ClientIdMask::LOCAL_CLIENT_PID,
    };
    let ids = conn
        .res_query_client_ids(&[all])
        .map_err(|e| error(&e))?
        .reply()
        .map_err(|e| error(&e))?
        .ids;
    let pids: Vec<u32> = ids
        .iter()
        .filter_map(|id| id.value.first().copied())
        .collect();
    debug!(clients = clients.len(), ?pids, "X11 clients");
    Ok(all_ours(
        clients.len(),
        &pids,
        &[std::process::id(), xwayland],
    ))
}

/// Whether each of `clients` X clients is of one of the processes `ours`,
/// given the process IDs X knows (`only_pilas_x11_clients`).
fn all_ours(clients: usize, pids: &[u32], ours: &[u32]) -> bool {
    pids.len() == clients && pids.iter().all(|pid| ours.contains(pid))
}

/// `f` with the signals of pila's signal source unblocked, for what starts a
/// program pila can't reach into (`Pila::session_env` does it for the others):
/// a blocked mask survives exec. One arriving meanwhile acts as if pila had no
/// source for it, so this is only for what takes as little as starting a program.
fn with_signals_unblocked<T>(f: impl FnOnce() -> T) -> T {
    // SAFETY: the sets are initialized by sigemptyset and sigaddset, or written
    // by pthread_sigmask, before they're read.
    unsafe {
        let mut signals = std::mem::zeroed::<libc::sigset_t>();
        let mut previous = std::mem::zeroed::<libc::sigset_t>();
        libc::sigemptyset(&mut signals);
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGCHLD] {
            libc::sigaddset(&mut signals, signal);
        }
        libc::pthread_sigmask(libc::SIG_UNBLOCK, &signals, &mut previous);
        let result = f();
        libc::pthread_sigmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut());
        result
    }
}

/// The X display reserved (`XwaylandState::reservation`).
pub struct Reservation {
    /// Its number, lock file and listening sockets.
    pub display: XWaylandDisplay,
    /// Its cookie (`xauth.rs`): Xwayland's `-auth` file, and `XAUTHORITY`
    /// for what Pila starts.
    pub authority: crate::xauth::Xauthority,
}

/// Xwayland and the display it serves (`Pila::xwayland`).
pub struct XwaylandState {
    /// The X display pila holds from startup, whether Xwayland runs or not.
    /// `None` when X11 apps can't run (no Xwayland installed, it keeps
    /// failing, or it's turned off).
    pub reservation: Option<Reservation>,
    /// Watching the reservation's sockets while Xwayland doesn't run
    /// (`listen_for_x11`).
    pub listening: Vec<RegistrationToken>,
    /// Its window manager, once it's ready.
    pub wm: Option<X11Wm>,
    pub shell_state: XWaylandShellState,
    /// Its event source while it runs, removed to stop it (`kill_xwayland`),
    /// and its process.
    pub token: Option<RegistrationToken>,
    pub pid: Option<u32>,
    /// When it failed lately (`may_restart`).
    pub failures: Vec<Instant>,
    /// The Wayland clipboard's and primary selection's MIME types, offered to
    /// a window manager that starts after they were set.
    pub clipboard: Option<Vec<String>>,
    pub primary: Option<Vec<String>>,
    /// The idle check's timer (`[xwayland] idle-timeout`), while one's due.
    pub idle_check: Option<RegistrationToken>,
    /// Xwayland was stopped for being idle: its window manager's connection
    /// closing isn't a crash (`xwayland_gone`).
    pub stopped_idle: bool,
}

impl XwaylandState {
    pub fn new(shell_state: XWaylandShellState) -> Self {
        Self {
            reservation: None,
            listening: Vec::new(),
            wm: None,
            shell_state,
            token: None,
            pid: None,
            failures: Vec::new(),
            clipboard: None,
            primary: None,
            idle_check: None,
            stopped_idle: false,
        }
    }

    /// The display reserved for X11 apps, from startup on; what new programs
    /// get as `DISPLAY` is `Pila::x11_display`.
    pub fn display(&self) -> Option<u32> {
        self.reservation
            .as_ref()
            .map(|reservation| reservation.display.display_number())
    }

    /// Whether Xwayland is up, with its window manager.
    pub fn running(&self) -> bool {
        self.wm.is_some()
    }
}

/// Smithay's Xwayland event source, plus the one failure it misses: Xwayland
/// dying before it's ready (a crash, a broken install) closes the pipe it
/// writes its display number to, and Smithay takes that end of file for "not
/// yet". Watched level-triggered, the pipe then wakes the event loop forever
/// and the app waiting for X never hears back.
///
/// It also unregisters once: Smithay's disables itself once Xwayland is ready
/// (calloop unregisters it then), and removing it later must not try again.
struct XWaylandSource {
    xwayland: XWayland,
    registered: bool,
}

impl EventSource for XWaylandSource {
    type Event = XWaylandEvent;
    type Metadata = ();
    type Ret = ();
    type Error = std::io::Error;

    fn process_events<F>(
        &mut self,
        readiness: Readiness,
        token: Token,
        mut callback: F,
    ) -> std::io::Result<PostAction>
    where
        F: FnMut(XWaylandEvent, &mut ()),
    {
        if readiness.readable && closed_unwritten(self.xwayland.poll_fd()) {
            callback(XWaylandEvent::Error, &mut ());
            return Ok(PostAction::Disable);
        }
        self.xwayland.process_events(readiness, token, callback)
    }

    fn register(
        &mut self,
        poll: &mut Poll,
        factory: &mut TokenFactory,
    ) -> smithay::reexports::calloop::Result<()> {
        self.xwayland.register(poll, factory)?;
        self.registered = true;
        Ok(())
    }

    fn reregister(
        &mut self,
        poll: &mut Poll,
        factory: &mut TokenFactory,
    ) -> smithay::reexports::calloop::Result<()> {
        self.xwayland.reregister(poll, factory)
    }

    fn unregister(&mut self, poll: &mut Poll) -> smithay::reexports::calloop::Result<()> {
        if !std::mem::replace(&mut self.registered, false) {
            return Ok(());
        }
        self.xwayland.unregister(poll)
    }
}

/// Whether the pipe's writer is gone with nothing left in it to read: what
/// Xwayland wrote before dying is still read first.
fn closed_unwritten(fd: BorrowedFd<'_>) -> bool {
    let mut poll = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd, no wait.
    let hung_up = unsafe { libc::poll(&mut poll, 1, 0) } == 1 && poll.revents & libc::POLLHUP != 0;
    let mut unread: libc::c_int = 0;
    // SAFETY: FIONREAD writes one c_int.
    hung_up
        && unsafe { libc::ioctl(fd.as_raw_fd(), libc::FIONREAD, &mut unread) } == 0
        && unread == 0
}

impl Pila {
    /// Hold an X display for X11 apps from startup: they get its `DISPLAY` at
    /// once, and the first to connect starts Xwayland (`listen_for_x11`).
    pub fn reserve_xwayland(&mut self) {
        if !self.config.xwayland.enabled {
            info!("Xwayland is turned off ([xwayland] enabled = false): no X11 display");
            return;
        }
        if self.xwayland.reservation.is_some() {
            return;
        }
        if !crate::config::in_path("Xwayland") {
            info!("Xwayland isn't installed; X11 apps won't run");
            return;
        }
        ensure_x11_socket_dir();
        // Only the socket file, no abstract socket: an abstract socket has no
        // permissions, so any process in the network namespace (a Flatpak
        // with network access but no X11 permission, another user) could
        // reach X and, with the cookie leaked or `-auth` gone, get the
        // clipboard and XTEST into X11 apps.
        let reservation = match XWaylandDisplay::new(None, false) {
            Ok(reservation) => reservation,
            Err(err) => {
                warn!(?err, "can't reserve an X11 display; X11 apps won't run");
                return;
            }
        };
        let number = reservation.display_number();
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from);
        let authority = runtime
            .ok_or_else(|| std::io::Error::other("XDG_RUNTIME_DIR is not set"))
            .and_then(|runtime| crate::xauth::Xauthority::create(&runtime, number));
        let authority = match authority {
            Ok(authority) => authority,
            Err(err) => {
                warn!(?err, "can't write the X11 cookie; X11 apps won't run");
                return;
            }
        };
        info!(
            display = number,
            "X11 display reserved; Xwayland starts when an X11 app connects"
        );
        self.xwayland.reservation = Some(Reservation {
            display: reservation,
            authority,
        });
        self.listen_for_x11();
    }

    /// Watch the reserved display's sockets, while Xwayland doesn't run: an
    /// app connecting makes one readable. The connection waits in the
    /// socket's queue for Xwayland to take it (`x11_app_connecting`).
    fn listen_for_x11(&mut self) {
        // Turned off while it ran: the display goes with it.
        if !self.config.xwayland.enabled {
            info!("Xwayland is turned off; X11 display released");
            self.stop_xwayland();
            return;
        }
        let Some(reservation) = self.xwayland.reservation.as_ref() else {
            return;
        };
        let reservation = reservation.display.clone();
        if !self.xwayland.listening.is_empty() {
            return;
        }
        for index in 0..reservation.listen_sockets().len() {
            let socket = ListenSocket(reservation.clone(), index);
            let source = Generic::new(socket, Interest::READ, Mode::Level);
            let inserted = self.loop_handle.insert_source(source, |_, _, state| {
                // Not from here: a source removed from its own callback
                // stays in the poll. The idle callback runs before the next
                // poll, and inside a source it needs no wake.
                state
                    .loop_handle
                    .insert_idle(|state| state.x11_app_connecting());
                Ok(PostAction::Continue)
            });
            match inserted {
                Ok(token) => self.xwayland.listening.push(token),
                Err(err) => warn!(error = ?err.error, "can't watch the X11 display's socket"),
            }
        }
    }

    fn stop_listening(&mut self) {
        for token in self.xwayland.listening.drain(..) {
            self.loop_handle.remove(token);
        }
    }

    /// An X11 app is connecting to the reserved display: start Xwayland,
    /// which takes the connection waiting in the socket.
    fn x11_app_connecting(&mut self) {
        self.stop_listening();
        if self.xwayland.token.is_none() {
            self.spawn_xwayland();
        }
    }

    /// Start Xwayland on the reserved display. Once it's ready its window
    /// manager starts, with the selections Wayland clients set meanwhile.
    fn spawn_xwayland(&mut self) {
        let Some(Reservation { display, authority }) = &self.xwayland.reservation else {
            return;
        };
        let reservation = display.clone();
        let number = reservation.display_number();
        // Only clients with the cookie get in; the window manager's
        // connection (`-wm`) needs none.
        let auth = [
            "-auth".to_string(),
            authority.path().to_string_lossy().into_owned(),
        ];
        let spawned = with_signals_unblocked(|| {
            XWayland::spawn_with_inherited_env_on_display(
                &self.display_handle,
                reservation,
                std::iter::empty::<(String, String)>(),
                auth,
                Stdio::null(),
                Stdio::null(),
                |_| (),
            )
        });
        let (xwayland, client) = match spawned {
            Ok(spawned) => spawned,
            Err(err) => {
                warn!(?err, "failed to start Xwayland");
                self.xwayland_failed();
                return;
            }
        };
        info!(display = number, "starting Xwayland for an X11 app");
        self.xwayland.pid = xwayland.pid();
        let inserted = self.loop_handle.insert_source(
            XWaylandSource {
                xwayland,
                registered: false,
            },
            move |event, _, state| {
                match event {
                    XWaylandEvent::Ready { x11_socket, .. } => {
                        // Nested and on a TTY alike, Xwayland runs at scale 1.
                        state.client_compositor_state(&client).set_client_scale(1.0);
                        match X11Wm::start_wm(
                            state.loop_handle.clone(),
                            &state.display_handle,
                            x11_socket,
                            client.clone(),
                        ) {
                            Ok(wm) => {
                                info!(display = number, "Xwayland ready");
                                state.xwayland.wm = Some(wm);
                                state.offer_selections_to_x11();
                                state.arm_xwayland_idle_check();
                            }
                            Err(err) => {
                                warn!(?err, "failed to start the X11 window manager");
                                // Not from its own source's callback.
                                state.loop_handle.insert_idle(Pila::xwayland_failed);
                            }
                        }
                    }
                    XWaylandEvent::Error => {
                        warn!("Xwayland failed to start");
                        state.loop_handle.insert_idle(Pila::xwayland_failed);
                    }
                }
            },
        );
        match inserted {
            Ok(token) => self.xwayland.token = Some(token),
            Err(err) => {
                warn!(error = ?err.error, "failed to watch Xwayland");
                self.xwayland_failed();
            }
        }
    }

    /// Xwayland didn't get as far as its window manager: stop what's left
    /// of it, and wait for the next app unless it keeps failing.
    fn xwayland_failed(&mut self) {
        self.kill_xwayland();
        self.listen_again_unless_failing();
    }

    /// Wait for the next X11 app after Xwayland failed, unless it failed too
    /// often lately (`may_restart`): then let the display go, so apps get
    /// refused rather than start it forever.
    fn listen_again_unless_failing(&mut self) {
        if may_restart(&mut self.xwayland.failures, Instant::now()) {
            self.listen_for_x11();
        } else {
            warn!("Xwayland keeps failing; X11 apps won't run until pila restarts");
            self.stop_xwayland();
        }
    }

    /// Stop the X server: dropping its event source kills it. Its window
    /// manager goes first, so the connection closing isn't taken for a crash
    /// (`XwmHandler::disconnected` compares ids).
    fn kill_xwayland(&mut self) {
        self.xwayland.wm = None;
        self.xwayland.pid = None;
        self.xwayland.stopped_idle = false;
        if let Some(token) = self.xwayland.token.take() {
            self.loop_handle.remove(token);
        }
        if let Some(token) = self.xwayland.idle_check.take() {
            self.loop_handle.remove(token);
        }
    }

    /// Stop Xwayland on exit, and give the display back: the reservation's
    /// last holder removes its lock and socket files. (Left to the event
    /// loop's own drop, it doesn't happen.)
    pub fn stop_xwayland(&mut self) {
        self.kill_xwayland();
        self.stop_listening();
        self.xwayland.reservation = None;
    }

    /// The window manager's connection to X closed: Xwayland stopped for
    /// being idle, or died. The next X11 app starts it again, on the same
    /// display, unless it keeps dying.
    fn xwayland_gone(&mut self) {
        if self.xwayland.stopped_idle {
            info!("Xwayland stopped; it starts again when an X11 app connects");
            self.forget_xwayland();
            self.listen_for_x11();
        } else {
            warn!("Xwayland died; it starts again when an X11 app connects");
            self.forget_xwayland();
            self.listen_again_unless_failing();
        }
    }

    /// Stop Xwayland and forget what went with it: its windows, its
    /// selections, the focus on one of them.
    fn forget_xwayland(&mut self) {
        let was_focused = matches!(
            self.seat.get_keyboard().and_then(|k| k.current_focus()),
            Some(KeyboardFocus::X11(_))
        );
        let windows: Vec<Window> = self
            .space
            .elements()
            .chain(self.off_space_windows())
            .filter(|w| w.x11_surface().is_some())
            .cloned()
            .collect();
        for window in &windows {
            self.forget_window(window);
            self.forget_animations(window);
            self.unmap_window(window);
        }
        self.x11_cleared_selection(SelectionTarget::Clipboard);
        self.x11_cleared_selection(SelectionTarget::Primary);
        self.kill_xwayland();
        if was_focused {
            self.focus_recent_window();
        }
        self.refresh_pointer_focus();
        self.queue_redraw();
    }

    /// Whether any X11 window is managed or shown, minimized ones and
    /// menus included.
    fn has_x11_windows(&self) -> bool {
        self.space
            .elements()
            .chain(self.off_space_windows())
            .any(|w| w.x11_surface().is_some())
    }

    /// With `[xwayland] idle-timeout`, while Xwayland runs with no X11
    /// window: check whether any X11 client is left once the timeout is over
    /// (`check_xwayland_idle`). Read when the last X11 window goes (and on a
    /// config change).
    pub fn arm_xwayland_idle_check(&mut self) {
        let Some(seconds) = self.config.xwayland.idle_timeout else {
            return;
        };
        if !self.xwayland.running() || self.xwayland.idle_check.is_some() || self.has_x11_windows()
        {
            return;
        }
        let timer = Timer::from_duration(Duration::from_secs(seconds.into()));
        let inserted = self.loop_handle.insert_source(timer, |_, _, state| {
            state.xwayland.idle_check = None;
            state.check_xwayland_idle();
            TimeoutAction::Drop
        });
        match inserted {
            Ok(token) => self.xwayland.idle_check = Some(token),
            Err(err) => warn!(error = ?err.error, "can't time Xwayland's idle check"),
        }
    }

    /// `[xwayland]` changed. Turned on: reserve the display (programs Pila
    /// starts from then on get it). Turned off: give it back, or, while
    /// Xwayland runs, leave it to its apps until it stops (`listen_for_x11`).
    /// Then time the idle check again.
    pub fn xwayland_config_changed(&mut self) {
        if !self.config.xwayland.enabled
            && !self.xwayland.running()
            && self.xwayland.token.is_none()
        {
            if self.xwayland.reservation.is_some() {
                info!("Xwayland is turned off; X11 display released");
                self.stop_xwayland();
            }
        } else if self.config.xwayland.enabled && self.xwayland.reservation.is_none() {
            self.reserve_xwayland();
        }
        if let Some(token) = self.xwayland.idle_check.take() {
            self.loop_handle.remove(token);
        }
        self.arm_xwayland_idle_check();
    }

    /// The display new programs get as `DISPLAY`: none while X11 apps are
    /// turned off, even if Xwayland still runs for those it has.
    pub fn x11_display(&self) -> Option<u32> {
        self.xwayland
            .display()
            .filter(|_| self.config.xwayland.enabled)
    }

    /// The Xauthority file new programs get as `XAUTHORITY`, with
    /// `x11_display`.
    pub fn x11_authority(&self) -> Option<&std::path::Path> {
        self.x11_display()?;
        self.xwayland
            .reservation
            .as_ref()
            .map(|reservation| reservation.authority.path())
    }

    /// The idle timeout is over with no X11 window: ask X, on a thread (it
    /// blocks), whether only pila's own clients are left.
    fn check_xwayland_idle(&mut self) {
        let (Some(wm), Some(reservation), Some(pid)) = (
            &self.xwayland.wm,
            &self.xwayland.reservation,
            self.xwayland.pid,
        ) else {
            return;
        };
        let display = reservation.display.display_number();
        let cookie = reservation.authority.cookie();
        if self.config.xwayland.idle_timeout.is_none() || self.has_x11_windows() {
            return;
        }
        let id = wm.id();
        self.off_loop(
            "pila-x11-idle",
            move || only_pilas_x11_clients(display, &cookie, pid),
            move |state, result| state.xwayland_idle_checked(id, result),
        );
    }

    /// What X said about its clients (`check_xwayland_idle`): with only
    /// pila's own left, stop Xwayland until an X11 app connects again; with
    /// others (a settings daemon, an app with no window yet), check again
    /// after another timeout.
    fn xwayland_idle_checked(&mut self, id: XwmId, result: Result<bool, String>) {
        // Xwayland may have gone, or a window come, meanwhile.
        let same = self.xwayland.wm.as_ref().is_some_and(|wm| wm.id() == id);
        if !same || self.config.xwayland.idle_timeout.is_none() || self.has_x11_windows() {
            return;
        }
        match result {
            Ok(true) => {
                info!("no X11 clients left; stopping Xwayland");
                // Only the server: its window manager, which X may still send
                // events to, goes when the connection closes (`xwayland_gone`).
                self.xwayland.stopped_idle = true;
                if let Some(token) = self.xwayland.token.take() {
                    self.loop_handle.remove(token);
                }
                // Dropping the source only marks its Wayland client dead,
                // until a Wayland client's next request: end it at once. Not
                // reaped before that client's cleanup, the process is still
                // Xwayland.
                if let Some(pid) = self.xwayland.pid.and_then(|pid| i32::try_from(pid).ok()) {
                    // SAFETY: a signal, to pila's own child.
                    unsafe { libc::kill(pid, libc::SIGTERM) };
                }
            }
            Ok(false) => {
                debug!("X11 clients without windows are left; Xwayland keeps running");
                self.arm_xwayland_idle_check();
            }
            Err(err) => warn!(%err, "can't ask Xwayland for its clients; it keeps running"),
        }
    }

    /// Offer X the Wayland selections set before its window manager started.
    fn offer_selections_to_x11(&mut self) {
        let Some(wm) = self.xwayland.wm.as_mut() else {
            return;
        };
        let selections = [
            (SelectionTarget::Clipboard, self.xwayland.clipboard.clone()),
            (SelectionTarget::Primary, self.xwayland.primary.clone()),
        ];
        for (target, mime_types) in selections {
            if let Some(mime_types) = mime_types {
                if let Err(err) = wm.new_selection(target, Some(mime_types)) {
                    warn!(?err, ?target, "failed to offer the selection to X11 apps");
                }
            }
        }
    }

    /// A Wayland client set (or cleared) a selection: kept for an X that
    /// starts later, offered to a running one.
    pub fn wayland_selection_changed(
        &mut self,
        target: SelectionTarget,
        mime_types: Option<Vec<String>>,
    ) {
        match target {
            SelectionTarget::Clipboard => self.xwayland.clipboard = mime_types.clone(),
            SelectionTarget::Primary => self.xwayland.primary = mime_types.clone(),
        }
        if let Some(xwm) = self.xwayland.wm.as_mut() {
            if let Err(err) = xwm.new_selection(target, mime_types) {
                warn!(?err, ?target, "failed to offer the selection to X11 apps");
            }
        }
    }

    /// The managed window (mapped, minimized or on another workspace) for
    /// an X11 surface.
    pub fn window_for_x11(&self, surface: &X11Surface) -> Option<Window> {
        self.space
            .elements()
            .chain(self.off_space_windows())
            .find(|w| w.x11_surface() == Some(surface))
            .cloned()
    }

    /// Tell X where pila put a managed X11 window (after placing or moving it),
    /// so its menus and coordinates line up. No-op for Wayland windows.
    pub fn x11_window_moved(&self, window: &Window) {
        let Some(x11) = window.x11_surface() else {
            return;
        };
        if x11.is_override_redirect() {
            return;
        }
        if let Some(loc) = self.space.element_location(window) {
            let _ = x11.configure(Rectangle::new(loc, x11.last_configure().size));
        }
    }

    /// Raise an X11 window in X's own stacking order too, so X keeps
    /// delivering input to the window pila shows on top.
    pub fn x11_window_raised(&mut self, window: &Window) {
        if let (Some(x11), Some(xwm)) = (window.x11_surface(), self.xwayland.wm.as_mut()) {
            let _ = xwm.raise_window(x11);
        }
    }
}

/// The X11 surface whose app grabbed the keyboard (XGrabKeyboard, through
/// xwayland-keyboard-grab), on the seat's user data, until Xwayland lets go.
#[derive(Default)]
struct XWaylandGrab(std::sync::Mutex<Option<(WlSurface, XWaylandKeyboardGrab<Pila>)>>);

impl XWaylandKeyboardGrabHandler for Pila {
    /// Not Smithay's keyboard grab: it would hold the focus where it is
    /// (Mod+Tab, a click elsewhere, the lock screen couldn't take it) and a
    /// device has one grab, the input method's. The grabbing surface's app
    /// gets the keys of Pila's bindings while it has the focus instead, as
    /// with keyboard-shortcuts-inhibit (`Pila::shortcuts_inhibited`): a
    /// click elsewhere gets out, and Quit is never inhibited.
    fn grab(&mut self, surface: WlSurface, seat: Seat<Self>, grab: XWaylandKeyboardGrab<Self>) {
        debug!(surface = ?surface.id(), "X11 keyboard grab");
        let slot = seat.user_data().get_or_insert(XWaylandGrab::default);
        *slot.0.locked() = Some((surface, grab));
    }

    fn keyboard_focus_for_xsurface(&self, surface: &WlSurface) -> Option<KeyboardFocus> {
        self.window_for_surface(surface)
            .and_then(|window| crate::state::focus_target(&window))
    }
}

/// Whether an override-redirect window gets the keyboard when it's mapped:
/// one with no menu-like window type (a launcher like dmenu, which grabs the
/// keyboard in X) that takes input. Menus, tooltips and the like don't, as
/// in wlroots (`wlr_xwayland_surface_override_redirect_wants_focus`).
fn override_redirect_takes_keyboard(surface: &X11Surface) -> bool {
    let menu_like = matches!(
        surface.window_type(),
        Some(
            WmWindowType::Combo
                | WmWindowType::Dnd
                | WmWindowType::DropdownMenu
                | WmWindowType::Menu
                | WmWindowType::Notification
                | WmWindowType::PopupMenu
                | WmWindowType::Splash
                | WmWindowType::Tooltip
                | WmWindowType::Utility
        )
    );
    !menu_like && surface.input_model() != WmInputModel::None
}

/// Whether `surface`'s app holds an X11 keyboard grab on `seat`.
pub fn has_keyboard_grab(seat: &Seat<Pila>, surface: &WlSurface) -> bool {
    seat.user_data().get::<XWaylandGrab>().is_some_and(|slot| {
        slot.0
            .locked()
            .as_ref()
            .is_some_and(|(grabbed, grab)| grab.grab().is_alive() && grabbed == surface)
    })
}

impl XWaylandShellHandler for Pila {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland.shell_state
    }

    /// An X11 window got its surface. When X names it after the surface's
    /// commit, what it shows appears now, with no commit to redraw it; and
    /// with a surface it counts as focused, so the window that had the
    /// focus before draws its title bar unfocused. Once per X11 window.
    fn surface_associated(&mut self, _xwm: XwmId, _wl_surface: WlSurface, _surface: X11Surface) {
        self.queue_redraw();
    }
}

impl XwmHandler for Pila {
    // Smithay asks only from the window manager's own event sources, which
    // `stop_xwayland`/`xwayland_gone` end with it; and it takes a reference,
    // so it can't be told there's none. (A clipboard transfer's source that
    // fired after the window manager went away would still get here.)
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.xwayland
            .wm
            .as_mut()
            .expect("X11 event without a window manager")
    }

    fn disconnected(&mut self, xwm: XwmId) {
        if self.xwayland.wm.as_ref().is_some_and(|wm| wm.id() == xwm) {
            self.xwayland_gone();
        }
    }

    fn new_window(&mut self, _xwm: XwmId, _window: X11Surface) {}
    fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn map_window_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Err(err) = surface.set_mapped(true) {
            warn!(?err, "failed to map X11 window");
            return;
        }
        let window = Window::new_x11_window(surface.clone());
        // A size from window rules first: placing it uses its size.
        let props = self.resolve_window_rules(&window);
        if let Some((w, h)) = props.size {
            let loc = surface.last_configure().loc;
            debug!(target: "pila::rules", app_id = surface.class(), w, h, "initial size");
            let _ = surface.configure(Rectangle::new(loc, (w, h).into()));
        }
        // Placed like a Wayland window (centered on the pointer's output,
        // focused), then X is told where it ended up. A window that asked to
        // be maximized or fullscreen before mapping (or whose rules say so)
        // gets that size now.
        self.place_new_window_with(window.clone(), props.clone());
        let (maximized, fullscreen) = fill_state(&window);
        if maximized || fullscreen {
            self.set_fill_state(&window, maximized, fullscreen);
        } else if props.open_maximized == Some(true) || props.open_fullscreen == Some(true) {
            self.apply_open_fill(&window, &props);
        } else {
            self.x11_window_moved(&window);
        }
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        // Menus and tooltips: exactly where X put them, on top, unfocused.
        let location = surface.last_configure().loc;
        let takes_keyboard = override_redirect_takes_keyboard(&surface);
        self.map_window(Window::new_x11_window(surface.clone()), location, false);
        self.queue_redraw();
        // A launcher (dmenu) grabs the keyboard in X, which only gets keys
        // while one of Xwayland's surfaces has it: it gets the keyboard, out
        // of the focus history, and `unmapped_window` gives it back. Not
        // over the lock screen or a layer surface's exclusive keyboard.
        if takes_keyboard && !self.is_locked() && self.exclusive_layer().is_none() {
            debug!(
                class = surface.class(),
                "X11 override-redirect window takes the keyboard"
            );
            self.dismiss_popup_grab();
            self.set_keyboard_focus(Some(surface.into()), SERIAL_COUNTER.next_serial());
        }
    }

    fn unmapped_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        let was_focused = self
            .seat
            .get_keyboard()
            .and_then(|k| k.current_focus())
            .is_some_and(|focus| focus == surface.clone().into());
        if let Some(window) = self.window_for_x11(&surface) {
            self.forget_window(&window);
            self.animate_close(&window);
            self.unmap_window(&window);
        }
        if !surface.is_override_redirect() {
            let _ = surface.set_mapped(false);
        }
        if was_focused {
            self.focus_recent_window();
        }
        self.queue_redraw();
        self.arm_xwayland_idle_check();
    }

    fn destroyed_window(&mut self, _xwm: XwmId, _surface: X11Surface) {}

    fn configure_request(
        &mut self,
        _xwm: XwmId,
        surface: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        // Sizes are honored. Positions only before pila manages the window:
        // once mapped, pila decides where it goes (like for Wayland windows).
        let mut geo = surface.last_configure();
        if let Some(w) = w {
            geo.size.w = w as i32;
        }
        if let Some(h) = h {
            geo.size.h = h as i32;
        }
        if self.window_for_x11(&surface).is_none() {
            geo.loc.x = x.unwrap_or(geo.loc.x);
            geo.loc.y = y.unwrap_or(geo.loc.y);
        }
        let _ = surface.configure(geo);
    }

    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        surface: X11Surface,
        geometry: Rectangle<i32, Logical>,
        _above: Option<u32>,
    ) {
        // Override-redirect windows move themselves; follow them.
        if !surface.is_override_redirect() {
            return;
        }
        if let Some(window) = self.window_for_x11(&surface) {
            self.map_window(window, geometry.loc, false);
            self.queue_redraw();
        }
    }

    fn property_notify(&mut self, _xwm: XwmId, surface: X11Surface, property: WmWindowProperty) {
        // The urgency hint (`urgency.rs`).
        if property == WmWindowProperty::Hints {
            if let Some(window) = self.window_for_x11(&surface) {
                let urgent = surface.hints().is_some_and(|hints| hints.urgent);
                self.set_urgent(&window, urgent);
            }
        }
        if property == WmWindowProperty::TransientFor {
            if let Some(window) = self.window_for_x11(&surface) {
                self.transient_parent_changed(&window);
            }
        }
        // The title bar and taskbars show the title; decorations may change.
        if matches!(
            property,
            WmWindowProperty::Title | WmWindowProperty::Class | WmWindowProperty::MotifHints
        ) {
            if let Some(window) = self.window_for_x11(&surface) {
                self.update_foreign_toplevel_x11(&window);
                if property != WmWindowProperty::MotifHints {
                    self.rules_maybe_changed(&window);
                }
            }
            self.queue_redraw();
        }
    }

    fn minimize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(window) = self.window_for_x11(&surface) {
            self.minimize_window(&window);
        }
    }

    fn unminimize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(window) = self
            .window_for_x11(&surface)
            .filter(|w| self.is_minimized(w))
        {
            self.restore_window(&window);
        }
    }

    fn modal_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        self.x11_modal_request(&surface, true);
    }

    fn unmodal_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        self.x11_modal_request(&surface, false);
    }

    fn maximize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        self.x11_fill_request(&surface, |_, fullscreen| (true, fullscreen));
    }

    fn unmaximize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        self.x11_fill_request(&surface, |_, fullscreen| (false, fullscreen));
    }

    fn fullscreen_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        self.x11_fill_request(&surface, |maximized, _| (maximized, true));
    }

    fn unfullscreen_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        self.x11_fill_request(&surface, |maximized, _| (maximized, false));
    }

    fn resize_request(
        &mut self,
        _xwm: XwmId,
        surface: X11Surface,
        _button: u32,
        edges: X11ResizeEdge,
    ) {
        // `_NET_WM_MOVERESIZE` from the client's own frame or handles.
        let (Some(window), Some(start)) = (self.window_for_x11(&surface), x11_grab(self, &surface))
        else {
            return;
        };
        self.begin_resize(window, edges.into(), start, SERIAL_COUNTER.next_serial());
    }

    // Selections (clipboard, primary) the other way: X11 apps set and read
    // them. Only the X11 app with keyboard focus may read, like Wayland's
    // data device.
    fn allow_selection_access(&mut self, xwm: XwmId, _selection: SelectionTarget) -> bool {
        let focus = self.seat.get_keyboard().and_then(|k| k.current_focus());
        let allowed = matches!(
            &focus,
            Some(KeyboardFocus::X11(window)) if window.xwm_id() == Some(xwm)
        );
        debug!(allowed, ?focus, "X11 app asks to read a selection");
        allowed
    }

    fn new_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        // X's own now: nothing of Wayland's to offer a later X.
        match selection {
            SelectionTarget::Clipboard => self.xwayland.clipboard = None,
            SelectionTarget::Primary => self.xwayland.primary = None,
        }
        match selection {
            SelectionTarget::Clipboard => {
                set_data_device_selection(&self.display_handle, &self.seat, mime_types, ())
            }
            SelectionTarget::Primary => {
                set_primary_selection(&self.display_handle, &self.seat, mime_types, ())
            }
        }
    }

    fn send_selection(
        &mut self,
        _xwm: XwmId,
        selection: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
    ) {
        let result = match selection {
            SelectionTarget::Clipboard => {
                request_data_device_client_selection(&self.seat, mime_type, fd)
                    .map_err(|e| e.to_string())
            }
            SelectionTarget::Primary => request_primary_client_selection(&self.seat, mime_type, fd)
                .map_err(|e| e.to_string()),
        };
        if let Err(err) = result {
            warn!(
                err,
                ?selection,
                "failed to send a Wayland selection to an X11 app"
            );
        }
    }

    fn cleared_selection(&mut self, _xwm: XwmId, selection: SelectionTarget) {
        self.x11_cleared_selection(selection);
    }

    fn move_request(&mut self, _xwm: XwmId, surface: X11Surface, _button: u32) {
        let (Some(window), Some(start)) = (self.window_for_x11(&surface), x11_grab(self, &surface))
        else {
            return;
        };
        self.begin_move(window, start, SERIAL_COUNTER.next_serial());
    }
}

impl Pila {
    /// An X11 app gave up a selection it set (or Xwayland went away).
    fn x11_cleared_selection(&mut self, selection: SelectionTarget) {
        // Only clear what X11 set (pila's selection user data); a Wayland
        // client's selection stays.
        match selection {
            SelectionTarget::Clipboard => {
                if current_data_device_selection_userdata(&self.seat).is_some() {
                    clear_data_device_selection(&self.display_handle, &self.seat)
                }
            }
            SelectionTarget::Primary => {
                if current_primary_selection_userdata(&self.seat).is_some() {
                    clear_primary_selection(&self.display_handle, &self.seat)
                }
            }
        }
    }
}

impl Pila {
    /// Maximize/fullscreen requests from X11 clients. Before mapping, only the
    /// flags are set; `map_window_request` applies them.
    fn x11_fill_request(
        &mut self,
        surface: &X11Surface,
        f: impl FnOnce(bool, bool) -> (bool, bool),
    ) {
        let (maximized, fullscreen) = f(surface.is_maximized(), surface.is_fullscreen());
        match self.window_for_x11(surface) {
            Some(window) => self.set_fill_state(&window, maximized, fullscreen),
            None => {
                let _ = surface.set_maximized(maximized);
                let _ = surface.set_fullscreen(fullscreen);
            }
        }
    }

    /// `_NET_WM_STATE_MODAL` asked for or taken back: X keeps the state
    /// (`X11Surface::is_modal` reads it, as `transients.rs` does), and a
    /// mapped dialog turning modal takes the focus from its parent.
    fn x11_modal_request(&mut self, surface: &X11Surface, modal: bool) {
        if let Err(err) = surface.set_modal(modal) {
            warn!(?err, "failed to set an X11 window's modal state");
            return;
        }
        if let Some(window) = self.window_for_x11(surface) {
            self.modal_changed(&window, modal);
        }
    }
}

/// The pointer grab a client-initiated X11 move/resize starts from: the
/// button must still be held (an implicit grab) on that window's surface,
/// like `check_grab` for xdg windows.
fn x11_grab(state: &Pila, surface: &X11Surface) -> Option<PointerGrabStartData<Pila>> {
    let pointer = state.seat.get_pointer()?;
    let start = pointer.grab_start_data()?;
    let (focus, _) = start.focus.as_ref()?;
    let same = surface
        .wl_surface()
        .is_some_and(|wl| focus.surface() == Some(&wl));
    (same && !state.is_locked()).then_some(start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xwayland_stops_restarting_if_it_keeps_dying() {
        let start = Instant::now();
        let mut failures = Vec::new();
        for _ in 0..MAX_RESTARTS {
            assert!(may_restart(&mut failures, start));
        }
        assert!(!may_restart(&mut failures, start + Duration::from_secs(1)));
        assert!(may_restart(&mut failures, start + RESTART_WINDOW));
    }

    #[test]
    fn xwayland_is_idle_with_only_pilas_own_clients_left() {
        assert!(all_ours(3, &[8, 7, 7], &[7, 8]));
        assert!(!all_ours(4, &[8, 7, 7, 9], &[7, 8]));
        // A client X can't name the process of isn't taken for pila's.
        assert!(!all_ours(4, &[8, 7, 7], &[7, 8]));
    }

    #[test]
    fn x11_sockets_are_only_trusted_in_a_directory_of_root_or_the_user() {
        assert!(trusted_owner(0, 1000));
        assert!(trusted_owner(1000, 1000));
        assert!(!trusted_owner(1001, 1000));
    }
}
