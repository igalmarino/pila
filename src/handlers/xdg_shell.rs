use smithay::{
    backend::renderer::utils::with_renderer_surface_state,
    delegate_xdg_decoration, delegate_xdg_shell,
    desktop::{
        find_popup_root_surface, get_popup_toplevel_coords, PopupKeyboardGrab, PopupKind,
        PopupPointerGrab, PopupUngrabStrategy, Window,
    },
    input::{
        pointer::{Focus, GrabStartData as PointerGrabStartData},
        Seat,
    },
    reexports::{
        wayland_protocols::xdg::{
            decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
            shell::server::xdg_toplevel,
        },
        wayland_server::{
            protocol::{wl_output, wl_seat, wl_surface::WlSurface},
            Resource,
        },
    },
    utils::{Serial, SERIAL_COUNTER},
    wayland::shell::xdg::{
        decoration::XdgDecorationHandler, PopupSurface, PositionerState, ToplevelSurface,
        XdgShellHandler, XdgShellState,
    },
};
use tracing::{debug, warn};

use crate::{state::fill_state, Pila};

impl XdgShellHandler for Pila {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // Tell the client how much room it has before it picks a size.
        let pointer_location = self
            .seat
            .get_pointer()
            .map(|p| p.current_location())
            .unwrap_or_default();
        if let Some(output_geo) = self.output_geometry_at(pointer_location) {
            surface.with_pending_state(|state| {
                state.bounds = Some(output_geo.size);
            });
        }
        // Mapped on its first buffer commit, once we know its size.
        self.unmapped.push(Window::new_wayland_window(surface));
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let wl_surface = surface.wl_surface();
        self.unmapped
            .retain(|w| w.toplevel().is_some_and(|t| t.wl_surface() != wl_surface));

        let was_focused = self
            .seat
            .get_keyboard()
            .and_then(|k| k.current_focus())
            .is_some_and(|focus| &focus == wl_surface);

        if let Some(window) = self.window_for_surface(wl_surface) {
            self.space.unmap_elem(&window);
        }
        if was_focused {
            let next = self.space.elements().next_back().cloned();
            self.focus_window(next, SERIAL_COUNTER.next_serial());
        }
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        if let Err(err) = self.popups.track_popup(PopupKind::Xdg(surface)) {
            warn!(?err, "failed to track popup");
        }
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    fn move_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let Some(seat) = Seat::from_resource(&seat) else {
            return;
        };
        let Some(start_data) = check_grab(&seat, surface.wl_surface(), serial) else {
            return;
        };
        let Some(window) = self.window_for_surface(surface.wl_surface()) else {
            return;
        };
        self.begin_move(window, start_data, serial);
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        seat: wl_seat::WlSeat,
        serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        let Some(seat) = Seat::from_resource(&seat) else {
            return;
        };
        let Some(start_data) = check_grab(&seat, surface.wl_surface(), serial) else {
            return;
        };
        let Some(window) = self.window_for_surface(surface.wl_surface()) else {
            return;
        };
        self.begin_resize(window, edges.into(), start_data, serial);
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        self.update_fill(&surface, |_, fullscreen| (true, fullscreen));
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        self.update_fill(&surface, |_, fullscreen| (false, fullscreen));
    }

    fn fullscreen_request(
        &mut self,
        surface: ToplevelSurface,
        _output: Option<wl_output::WlOutput>,
    ) {
        // Single output for now; the requested output is ignored.
        self.update_fill(&surface, |maximized, _| (maximized, true));
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        self.update_fill(&surface, |maximized, _| (maximized, false));
    }

    /// Menus: keyboard input goes to the popup, and a click outside the
    /// client dismisses the whole popup chain. Based on anvil.
    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let Some(seat) = Seat::<Pila>::from_resource(&seat) else {
            return;
        };
        let kind = PopupKind::Xdg(surface);
        let Some(root) = find_popup_root_surface(&kind)
            .ok()
            .filter(|root| self.window_for_surface(root).is_some())
        else {
            return;
        };
        let mut grab = match self.popups.grab_popup(root, kind, &seat, serial) {
            Ok(grab) => grab,
            Err(err) => {
                debug!(?err, "popup grab denied");
                return;
            }
        };

        // Don't override an unrelated grab (e.g. a window move in progress);
        // a nested menu replacing its parent menu's grab is fine.
        if let Some(keyboard) = seat.get_keyboard() {
            if keyboard.is_grabbed()
                && !(keyboard.has_grab(serial)
                    || keyboard.has_grab(grab.previous_serial().unwrap_or(serial)))
            {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            keyboard.set_focus(self, grab.current_grab(), serial);
            keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        }
        if let Some(pointer) = seat.get_pointer() {
            if pointer.is_grabbed()
                && !(pointer.has_grab(serial)
                    || pointer.has_grab(grab.previous_serial().unwrap_or_else(|| grab.serial())))
            {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
        }
    }
}
delegate_xdg_shell!(Pila);

// zxdg_decoration_manager_v1
//
// pila doesn't draw title bars yet, so every toplevel is told to decorate
// itself, even if it asks for server-side. Answering ServerSide without
// drawing anything would leave the window with no title bar at all.

impl XdgDecorationHandler for Pila {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        // Goes out with the initial configure (or the next one if that's already sent).
        set_client_side(&toplevel);
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: DecorationMode) {
        // The protocol requires a configure in response, even if the mode is unchanged.
        set_client_side(&toplevel);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        set_client_side(&toplevel);
    }
}
delegate_xdg_decoration!(Pila);

fn set_client_side(toplevel: &ToplevelSurface) {
    toplevel.with_pending_state(|state| {
        state.decoration_mode = Some(DecorationMode::ClientSide);
    });
    if toplevel.is_initial_configure_sent() {
        toplevel.send_configure();
    }
}

/// Only start a client-requested move/resize if the client really holds a
/// pointer button press on that surface.
fn check_grab(
    seat: &Seat<Pila>,
    surface: &WlSurface,
    serial: Serial,
) -> Option<PointerGrabStartData<Pila>> {
    let pointer = seat.get_pointer()?;
    if !pointer.has_grab(serial) {
        return None;
    }
    let start_data = pointer.grab_start_data()?;
    let (focus, _) = start_data.focus.as_ref()?;
    if !focus.id().same_client_as(&surface.id()) {
        return None;
    }
    Some(start_data)
}

impl Pila {
    /// Called on every `wl_surface.commit`.
    pub fn handle_xdg_commit(&mut self, surface: &WlSurface) {
        if let Some(index) = self
            .unmapped
            .iter()
            .position(|w| w.toplevel().is_some_and(|t| t.wl_surface() == surface))
        {
            let window = self.unmapped[index].clone();
            let toplevel = window.toplevel().unwrap();
            if !toplevel.is_initial_configure_sent() {
                toplevel.send_configure();
            } else if has_buffer(surface) {
                self.unmapped.remove(index);
                self.place_new_window(window);
            }
        }

        self.popups.commit(surface);
        if let Some(PopupKind::Xdg(ref xdg)) = self.popups.find_popup(surface) {
            if !xdg.is_initial_configure_sent() {
                // The initial configure is always allowed.
                xdg.send_configure().expect("initial configure failed");
            }
        }
    }

    fn update_fill(
        &mut self,
        surface: &ToplevelSurface,
        f: impl FnOnce(bool, bool) -> (bool, bool),
    ) {
        match self.window_for_surface(surface.wl_surface()) {
            Some(window) => {
                let (maximized, fullscreen) = fill_state(&window);
                let (maximized, fullscreen) = f(maximized, fullscreen);
                self.set_fill_state(&window, maximized, fullscreen);
            }
            None if surface.is_initial_configure_sent() => {
                surface.send_configure();
            }
            None => {}
        }
    }

    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let Some(window) = self.window_for_surface(&root) else {
            return;
        };
        let Some(window_geo) = self.space.element_geometry(&window) else {
            return;
        };
        let Some(output_geo) = self.output_geometry_at(window_geo.loc.to_f64()) else {
            return;
        };

        // The positioner's target rectangle is relative to the parent's geometry.
        let mut target = output_geo;
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= window_geo.loc;

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}

fn has_buffer(surface: &WlSurface) -> bool {
    with_renderer_surface_state(surface, |state| state.buffer().is_some()).unwrap_or(false)
}
