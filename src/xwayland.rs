//! X11 apps through Xwayland. pila starts Xwayland at launch and runs Smithay's
//! X11 window manager (`X11Wm`) on it; spawned clients get its `DISPLAY`.
//!
//! X11 windows live in the same `Space` as Wayland ones (`Window::new_x11_window`).
//! Managed windows are placed and focused like Wayland toplevels; override-
//! redirect windows (menus, tooltips) sit where they ask, above everything,
//! without focus. pila tells X where a managed window is (`X11Surface::configure`)
//! whenever it moves it (`Pila::x11_window_moved`).
//!
//! The startup command waits until X is ready (or failed), so `pila -c xterm`
//! works. Xwayland dying before it's ready counts as failed (`XWaylandSource`).

use std::{
    os::fd::{AsRawFd, BorrowedFd, OwnedFd},
    process::Stdio,
    time::{Duration, Instant},
};

use smithay::{
    desktop::Window,
    input::pointer::GrabStartData as PointerGrabStartData,
    reexports::calloop::{
        EventSource, Poll, PostAction, Readiness, RegistrationToken, Token, TokenFactory,
    },
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
        xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    },
    xwayland::{
        xwm::{Reorder, ResizeEdge as X11ResizeEdge, WmWindowProperty, XwmId},
        X11Surface, X11Wm, XWayland, XWaylandEvent, XwmHandler,
    },
};
use tracing::{debug, info, warn};

use crate::{focus::KeyboardFocus, state::fill_state, Pila};

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

/// How many times Xwayland is started again within `RESTART_WINDOW`: one that
/// can't stay up mustn't be started forever.
const MAX_RESTARTS: usize = 3;
const RESTART_WINDOW: Duration = Duration::from_secs(60);

/// Whether Xwayland may be started again `now`, given when it was lately.
fn may_restart(restarts: &mut Vec<Instant>, now: Instant) -> bool {
    restarts.retain(|at| now.duration_since(*at) < RESTART_WINDOW);
    if restarts.len() >= MAX_RESTARTS {
        return false;
    }
    restarts.push(now);
    true
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

/// Xwayland while pila runs it (`Pila::xwayland`).
pub struct XwaylandState {
    /// Its window manager and display number, once it's ready.
    pub wm: Option<X11Wm>,
    pub display: Option<u32>,
    pub shell_state: XWaylandShellState,
    /// Its event source, removed on exit (`stop_xwayland`).
    pub token: Option<RegistrationToken>,
    /// When it was started again lately (`may_restart`).
    pub restarts: Vec<Instant>,
}

/// Smithay's Xwayland event source, plus the one failure it misses: Xwayland
/// dying before it's ready (a crash, a broken install) closes the pipe it
/// writes its display number to, and Smithay takes that end of file for "not
/// yet". Watched level-triggered, the pipe then wakes the event loop forever
/// and the startup commands, waiting for X, never run.
struct XWaylandSource(XWayland);

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
        if readiness.readable && closed_unwritten(self.0.poll_fd()) {
            callback(XWaylandEvent::Error, &mut ());
            return Ok(PostAction::Disable);
        }
        self.0.process_events(readiness, token, callback)
    }

    fn register(
        &mut self,
        poll: &mut Poll,
        factory: &mut TokenFactory,
    ) -> smithay::reexports::calloop::Result<()> {
        self.0.register(poll, factory)
    }

    fn reregister(
        &mut self,
        poll: &mut Poll,
        factory: &mut TokenFactory,
    ) -> smithay::reexports::calloop::Result<()> {
        self.0.reregister(poll, factory)
    }

    fn unregister(&mut self, poll: &mut Poll) -> smithay::reexports::calloop::Result<()> {
        self.0.unregister(poll)
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
    /// Start Xwayland; the `startup` commands run once X is ready (or failed
    /// to start).
    pub fn start_xwayland(&mut self, startup: Vec<String>) {
        ensure_x11_socket_dir();
        let spawned = with_signals_unblocked(|| {
            XWayland::spawn(
                &self.display_handle,
                None,
                std::iter::empty::<(String, String)>(),
                std::iter::empty::<String>(),
                true,
                Stdio::null(),
                Stdio::null(),
                |_| (),
            )
        });
        let (xwayland, client) = match spawned {
            Ok(spawned) => spawned,
            Err(err) => {
                warn!(?err, "failed to start Xwayland; X11 apps won't run");
                self.start_session(startup);
                return;
            }
        };
        let inserted =
            self.loop_handle
                .insert_source(XWaylandSource(xwayland), move |event, _, state| {
                    match event {
                        XWaylandEvent::Ready {
                            x11_socket,
                            display_number,
                        } => {
                            // Nested and on a TTY alike, Xwayland runs at scale 1.
                            state.client_compositor_state(&client).set_client_scale(1.0);
                            match X11Wm::start_wm(
                                state.loop_handle.clone(),
                                &state.display_handle,
                                x11_socket,
                                client.clone(),
                            ) {
                                Ok(wm) => {
                                    info!(display = display_number, "Xwayland ready");
                                    state.xwayland.wm = Some(wm);
                                    state.xwayland.display = Some(display_number);
                                }
                                Err(err) => warn!(?err, "failed to start the X11 window manager"),
                            }
                        }
                        XWaylandEvent::Error => {
                            warn!("Xwayland failed to start; X11 apps won't run")
                        }
                    }
                    let startup = std::mem::take(&mut state.pending_startup);
                    state.start_session(startup);
                });
        match inserted {
            Ok(token) => {
                self.xwayland.token = Some(token);
                self.pending_startup = startup;
            }
            Err(err) => {
                warn!(?err, "failed to watch Xwayland");
                self.start_session(startup);
            }
        }
    }

    /// Stop Xwayland on exit: dropping its event source kills the X server
    /// and removes its lock and socket files. (Left to the event loop's own
    /// drop, it doesn't happen.)
    pub fn stop_xwayland(&mut self) {
        self.xwayland.wm = None;
        if let Some(token) = self.xwayland.token.take() {
            self.loop_handle.remove(token);
        }
    }

    /// The window manager's connection to X closed: Xwayland died. Its windows
    /// are gone, and what's started from now on mustn't get its `DISPLAY`. It
    /// starts again, unless it keeps dying.
    fn xwayland_gone(&mut self) {
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
        self.stop_xwayland();
        self.xwayland.display = None;
        if was_focused {
            self.focus_recent_window();
        }
        self.refresh_pointer_focus();
        self.queue_redraw();

        if may_restart(&mut self.xwayland.restarts, Instant::now()) {
            warn!("Xwayland died; starting it again");
            self.loop_handle
                .insert_idle(|state| state.start_xwayland(Vec::new()));
            self.wake.ping();
        } else {
            warn!("Xwayland keeps dying; X11 apps won't run until pila restarts");
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

impl XWaylandShellHandler for Pila {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland.shell_state
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
        self.space
            .map_element(Window::new_x11_window(surface), location, false);
        self.queue_redraw();
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

    fn allow_selection_access(&mut self, xwm: XwmId, _selection: SelectionTarget) -> bool {
        self.x11_selection_access(xwm)
    }

    fn new_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        self.x11_new_selection(selection, mime_types);
    }

    fn send_selection(
        &mut self,
        _xwm: XwmId,
        selection: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
    ) {
        self.x11_send_selection(selection, mime_type, fd);
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

// Selections (clipboard, primary) the other way: X11 apps set and read them.
// Only the X11 app with keyboard focus may read, like Wayland's data device.
impl Pila {
    fn x11_selection_access(&self, xwm: XwmId) -> bool {
        let focus = self.seat.get_keyboard().and_then(|k| k.current_focus());
        let allowed = matches!(
            &focus,
            Some(KeyboardFocus::X11(window)) if window.xwm_id() == Some(xwm)
        );
        debug!(allowed, ?focus, "X11 app asks to read a selection");
        allowed
    }

    fn x11_new_selection(&mut self, selection: SelectionTarget, mime_types: Vec<String>) {
        match selection {
            SelectionTarget::Clipboard => {
                set_data_device_selection(&self.display_handle, &self.seat, mime_types, ())
            }
            SelectionTarget::Primary => {
                set_primary_selection(&self.display_handle, &self.seat, mime_types, ())
            }
        }
    }

    fn x11_send_selection(&mut self, selection: SelectionTarget, mime_type: String, fd: OwnedFd) {
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
        let mut restarts = Vec::new();
        for _ in 0..MAX_RESTARTS {
            assert!(may_restart(&mut restarts, start));
        }
        assert!(!may_restart(&mut restarts, start + Duration::from_secs(1)));
        assert!(may_restart(&mut restarts, start + RESTART_WINDOW));
    }

    #[test]
    fn x11_sockets_are_only_trusted_in_a_directory_of_root_or_the_user() {
        assert!(trusted_owner(0, 1000));
        assert!(trusted_owner(1000, 1000));
        assert!(!trusted_owner(1001, 1000));
    }
}
