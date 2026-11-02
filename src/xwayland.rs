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

use std::process::Stdio;

use smithay::{
    delegate_xwayland_shell,
    desktop::Window,
    utils::{Logical, Rectangle, SERIAL_COUNTER},
    wayland::{
        compositor::CompositorHandler,
        xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    },
    xwayland::{
        xwm::{Reorder, ResizeEdge as X11ResizeEdge, XwmId},
        X11Surface, X11Wm, XWayland, XWaylandEvent, XwmHandler,
    },
};
use tracing::{info, warn};

use crate::Pila;

impl Pila {
    /// Start Xwayland; `then_spawn` runs once X is ready (or failed to start).
    pub fn start_xwayland(&mut self, then_spawn: String) {
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
                self.spawn(&then_spawn);
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
                if let Some(command) = state.pending_startup.take() {
                    state.spawn(&command);
                }
            });
        match inserted {
            Ok(_) => self.pending_startup = Some(then_spawn),
            Err(err) => {
                warn!(?err, "failed to watch Xwayland");
                self.spawn(&then_spawn);
            }
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
        // focused), then X is told where it ended up.
        self.place_new_window(window.clone());
        self.x11_window_moved(&window);
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

    fn resize_request(
        &mut self,
        _xwm: XwmId,
        _surface: X11Surface,
        _button: u32,
        _edges: X11ResizeEdge,
    ) {
        // Client-side resize handles: not supported yet. Mod+Right drag and
        // pila's resize border work.
    }

    fn move_request(&mut self, _xwm: XwmId, _surface: X11Surface, _button: u32) {
        // Client-side move (e.g. a custom title bar): not supported yet.
        // Mod+Left drag and pila's title bar work.
    }
}
