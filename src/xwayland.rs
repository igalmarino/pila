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
//! works.

use std::{os::fd::OwnedFd, process::Stdio};

use smithay::{
    delegate_xwayland_shell,
    desktop::Window,
    input::pointer::GrabStartData as PointerGrabStartData,
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

impl Pila {
    /// Start Xwayland; the `startup` commands run once X is ready (or failed
    /// to start).
    pub fn start_xwayland(&mut self, startup: Vec<String>) {
        let spawned = XWayland::spawn(
            &self.display_handle,
            None,
            std::iter::empty::<(String, String)>(),
            true,
            Stdio::null(),
            Stdio::null(),
            |_| (),
        );
        let (xwayland, client) = match spawned {
            Ok(spawned) => spawned,
            Err(err) => {
                warn!(?err, "failed to start Xwayland; X11 apps won't run");
                self.spawn_all(&startup);
                return;
            }
        };
        let inserted = self
            .loop_handle
            .insert_source(xwayland, move |event, _, state| {
                match event {
                    XWaylandEvent::Ready {
                        x11_socket,
                        display_number,
                    } => {
                        // Nested and on a TTY alike, Xwayland runs at scale 1.
                        state.client_compositor_state(&client).set_client_scale(1.0);
                        match X11Wm::start_wm(state.loop_handle.clone(), x11_socket, client.clone())
                        {
                            Ok(wm) => {
                                info!(display = display_number, "Xwayland ready");
                                state.xwm = Some(wm);
                                state.xdisplay = Some(display_number);
                            }
                            Err(err) => warn!(?err, "failed to start the X11 window manager"),
                        }
                    }
                    XWaylandEvent::Error => warn!("Xwayland failed to start; X11 apps won't run"),
                }
                let startup = std::mem::take(&mut state.pending_startup);
                state.spawn_all(&startup);
            });
        match inserted {
            Ok(token) => {
                self.xwayland_token = Some(token);
                self.pending_startup = startup;
            }
            Err(err) => {
                warn!(?err, "failed to watch Xwayland");
                self.spawn_all(&startup);
            }
        }
    }

    /// Stop Xwayland on exit: dropping its event source kills the X server
    /// and removes its lock and socket files. (Left to the event loop's own
    /// drop, it doesn't happen.)
    pub fn stop_xwayland(&mut self) {
        self.xwm = None;
        if let Some(token) = self.xwayland_token.take() {
            self.loop_handle.remove(token);
        }
    }

    /// The mapped window for an X11 surface.
    pub fn window_for_x11(&self, surface: &X11Surface) -> Option<Window> {
        self.space
            .elements()
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
            let _ = x11.configure(Rectangle::new(loc, x11.geometry().size));
        }
    }

    /// Raise an X11 window in X's own stacking order too, so X keeps
    /// delivering input to the window pila shows on top.
    pub fn x11_window_raised(&mut self, window: &Window) {
        if let (Some(x11), Some(xwm)) = (window.x11_surface(), self.xwm.as_mut()) {
            let _ = xwm.raise_window(x11);
        }
    }
}

impl XWaylandShellHandler for Pila {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell_state
    }
}
delegate_xwayland_shell!(Pila);

impl XwmHandler for Pila {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.xwm
            .as_mut()
            .expect("X11 event without a window manager")
    }

    fn new_window(&mut self, _xwm: XwmId, _window: X11Surface) {}
    fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn map_window_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Err(err) = surface.set_mapped(true) {
            warn!(?err, "failed to map X11 window");
            return;
        }
        let window = Window::new_x11_window(surface);
        // Placed like a Wayland window (centered on the pointer's output,
        // focused), then X is told where it ended up. A window that asked to
        // be maximized or fullscreen before mapping gets that size now.
        self.place_new_window(window.clone());
        let (maximized, fullscreen) = fill_state(&window);
        if maximized || fullscreen {
            self.set_fill_state(&window, maximized, fullscreen);
        } else {
            self.x11_window_moved(&window);
        }
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        // Menus and tooltips: exactly where X put them, on top, unfocused.
        let location = surface.geometry().loc;
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
            self.remove_foreign_toplevel(&window);
            self.space.unmap_elem(&window);
        }
        if !surface.is_override_redirect() {
            let _ = surface.set_mapped(false);
        }
        if was_focused {
            self.focus_top_window(SERIAL_COUNTER.next_serial());
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
        let mut geo = surface.geometry();
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
            self.space.map_element(window, geometry.loc, false);
            self.queue_redraw();
        }
    }

    fn property_notify(&mut self, _xwm: XwmId, surface: X11Surface, property: WmWindowProperty) {
        // The title bar and taskbars show the title; decorations may change.
        if matches!(
            property,
            WmWindowProperty::Title | WmWindowProperty::Class | WmWindowProperty::MotifHints
        ) {
            if let Some(window) = self.window_for_x11(&surface) {
                self.update_foreign_toplevel_x11(&window);
            }
            self.queue_redraw();
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
    (Some(focus) == surface.wl_surface().as_ref() && !state.is_locked()).then_some(start)
}
