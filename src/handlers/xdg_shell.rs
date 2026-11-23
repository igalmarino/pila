use crate::sync::Locked;
use smithay::wayland::input_method::InputMethodSeat;
use smithay::{
    desktop::{
        find_popup_root_surface, get_popup_toplevel_coords, layer_map_for_output,
        PopupKeyboardGrab, PopupKind, PopupPointerGrab, PopupUngrabStrategy, Window,
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
    utils::{Logical, Point, Serial},
    wayland::shell::xdg::{
        decoration::XdgDecorationHandler, Configure, PopupSurface, PositionerState,
        ToplevelSurface, XdgShellHandler, XdgShellState,
    },
};
use tracing::{debug, warn};

use smithay::output::Output;

use crate::{
    state::{fill_state, has_buffer},
    Pila,
};

impl XdgShellHandler for Pila {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.protocols.xdg_shell
    }

    fn ack_configure(&mut self, surface: WlSurface, configure: Configure) {
        if let Configure::Toplevel(_) = configure {
            self.resize_acked(&surface);
        }
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // Tell the client how much room it has before it picks a size.
        let pointer_location = self.pointer_location();
        if let Some(output_geo) = self.usable_geometry_at(pointer_location) {
            surface.with_pending_state(|state| {
                state.bounds = Some(output_geo.size);
            });
        }
        crate::animation::watch_unmap(surface.wl_surface());
        // Mapped on its first buffer commit, once we know its size.
        self.windows
            .unmapped
            .push(Window::new_wayland_window(surface));
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let wl_surface = surface.wl_surface();
        self.forget_session_toplevel(wl_surface);
        self.windows
            .unmapped
            .retain(|w| w.toplevel().is_some_and(|t| t.wl_surface() != wl_surface));

        let was_focused = self.keyboard_focus().as_ref() == Some(wl_surface);

        if let Some(window) = self.window_for_surface(wl_surface) {
            self.forget_window(&window);
            self.animate_close(&window);
            self.unmap_window(&window);
        }
        if was_focused {
            self.focus_recent_window();
        }
    }

    // A rule may match the new title or app id (`rules.rs`); the title bar
    // shows the title.
    // A title or app id applies as it's set, not at a commit: a window off
    // the screen may not commit for a while.
    fn title_changed(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            self.queue_redraw_window(&window);
            self.rules_maybe_changed(&window);
        }
        self.update_foreign_toplevel(surface.wl_surface());
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            self.rules_maybe_changed(&window);
        }
        self.update_foreign_toplevel(surface.wl_surface());
    }

    fn parent_changed(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            self.transient_parent_changed(&window);
        }
    }

    // A popup's surface may live on, but it shows no more. Rare, and gone
    // from its window's bounds already: every output.
    fn popup_destroyed(&mut self, surface: PopupSurface) {
        crate::state::forget_mapped(surface.wl_surface());
        self.queue_redraw();
        // Inside the client's request: see to the menu's grab after it.
        self.loop_handle
            .insert_idle(|state| state.end_finished_popup_grab());
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        if let Err(err) = crate::surface_limits::check_positioner(&positioner) {
            crate::surface_limits::refuse(surface.wl_surface(), err);
            return;
        }
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
        if let Err(err) = crate::surface_limits::check_positioner(&positioner) {
            crate::surface_limits::refuse(surface.wl_surface(), err);
            return;
        }
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    /// An app that draws its own title bar asks for the window menu (a right
    /// click on it), at `location` relative to its window geometry.
    fn show_window_menu(
        &mut self,
        surface: ToplevelSurface,
        seat: wl_seat::WlSeat,
        serial: Serial,
        location: Point<i32, Logical>,
    ) {
        let Some(seat) = Seat::from_resource(&seat) else {
            return;
        };
        // While the press holds, or just after its release (`recent_press`).
        let Some(start_data) = check_grab(&seat, surface.wl_surface(), serial)
            .or_else(|| self.recent_press(surface.wl_surface(), serial))
        else {
            return;
        };
        let Some(window) = self.window_for_surface(surface.wl_surface()) else {
            return;
        };
        let Some(origin) = self.space.element_location(&window) else {
            return;
        };
        // `location` is surface-local: its window geometry may start inside
        // it (client-side shadows).
        let at = origin - window.geometry().loc + location;
        self.open_window_menu(&window, at, start_data, serial);
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

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            self.minimize_window(&window);
        }
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
        output: Option<wl_output::WlOutput>,
    ) {
        // The output the client asked for, if any; else the window's own.
        // Not honored before the window is mapped: it then goes fullscreen
        // on the pointer's output (`place_new_window`).
        let output = output.as_ref().and_then(Output::from_resource);
        match self.window_for_surface(surface.wl_surface()) {
            Some(window) => {
                let (maximized, _) = fill_state(&window);
                self.set_fill_state_on(&window, maximized, true, output.as_ref());
            }
            None => self.update_fill(&surface, |maximized, _| (maximized, true)),
        }
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
        // Nothing but the lock surface gets input while locked.
        if self.is_locked() {
            return;
        }
        let kind = PopupKind::Xdg(surface);
        let Some(root) = find_popup_root_surface(&kind).ok().filter(|root| {
            self.window_for_surface(root).is_some() || self.layer_for_surface(root).is_some()
        }) else {
            return;
        };
        let mut grab = match self.popups.grab_popup(root.into(), kind, &seat, serial) {
            Ok(grab) => grab,
            Err(err) => {
                debug!(?err, "popup grab denied");
                return;
            }
        };

        // An input method holding the keyboard keeps it: a device has one
        // grab, and Smithay can't give the input method's back. The menu
        // takes the pointer only and the keyboard stays on the window; the
        // input method passes on the keys it doesn't use (`input_method.rs`).
        let keyboard = seat
            .get_keyboard()
            .filter(|_| !seat.input_method().keyboard_grabbed());
        let pointer = seat.get_pointer();
        // Don't override an unrelated grab (e.g. a window move in progress);
        // a nested menu replacing its parent menu's grab is fine.
        let keyboard_taken = keyboard.as_ref().is_some_and(|keyboard| {
            keyboard.is_grabbed()
                && !(keyboard.has_grab(serial)
                    || keyboard.has_grab(grab.previous_serial().unwrap_or(serial)))
        });
        let pointer_taken = pointer.as_ref().is_some_and(|pointer| {
            pointer.is_grabbed()
                && !(pointer.has_grab(serial)
                    || pointer.has_grab(grab.previous_serial().unwrap_or_else(|| grab.serial())))
        });
        if keyboard_taken || pointer_taken {
            grab.ungrab(PopupUngrabStrategy::All);
            return;
        }
        match keyboard {
            Some(keyboard) => {
                keyboard.set_focus(self, grab.current_grab(), serial);
                keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
                self.input.pointer_only_menu = None;
            }
            None => self.input.pointer_only_menu = Some(grab.clone()),
        }
        if let Some(pointer) = pointer {
            let grab = crate::grabs::MenuPointerGrab::new(PopupPointerGrab::new(&grab));
            pointer.set_grab(self, grab, serial, Focus::Keep);
        }
    }
}

// zxdg_decoration_manager_v1
//
// pila draws title bars (see `decoration.rs`), so it prefers server-side
// decorations but honors a client that asks to draw its own. Clients that
// never bind xdg-decoration (e.g. GTK) always draw their own.

impl XdgDecorationHandler for Pila {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        // Goes out with the initial configure (or the next one if that's already sent).
        set_decoration_mode(&toplevel, DecorationMode::ServerSide);
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: DecorationMode) {
        set_decoration_mode(&toplevel, mode);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        set_decoration_mode(&toplevel, DecorationMode::ServerSide);
    }

    /// The client destroyed its decoration object: it draws its own from this
    /// commit (Smithay has dropped the mode), and when it maps again too.
    fn decoration_destroyed(&mut self, toplevel: ToplevelSurface) {
        smithay::wayland::compositor::with_states(toplevel.wl_surface(), |states| {
            if let Some(chosen) = states.data_map.get::<ChosenDecoration>() {
                *chosen.0.locked() = None;
            }
        });
    }
}

/// The decoration mode a toplevel settled on with pila, kept in its surface
/// data: unmapping discards the toplevel's state, decoration mode included,
/// but the decoration object lives on and its client won't ask again, so the
/// mode goes out again with the initial configure of the new mapping. None
/// once the object is destroyed.
struct ChosenDecoration(std::sync::Mutex<Option<DecorationMode>>);

fn set_decoration_mode(toplevel: &ToplevelSurface, mode: DecorationMode) {
    toplevel.with_pending_state(|state| {
        state.decoration_mode = Some(mode);
    });
    smithay::wayland::compositor::with_states(toplevel.wl_surface(), |states| {
        *states
            .data_map
            .get_or_insert_threadsafe(|| ChosenDecoration(std::sync::Mutex::new(Some(mode))))
            .0
            .locked() = Some(mode);
    });
    // The protocol requires a configure in response, even if the mode is unchanged.
    if toplevel.is_initial_configure_sent() {
        toplevel.send_configure();
    }
}

/// Before an initial configure: a toplevel mapping again gets the decoration
/// mode it had (`ChosenDecoration`); a new one has none yet.
fn restore_decoration_mode(toplevel: &ToplevelSurface) {
    let chosen = smithay::wayland::compositor::with_states(toplevel.wl_surface(), |states| {
        states
            .data_map
            .get::<ChosenDecoration>()
            .and_then(|chosen| *chosen.0.locked())
    });
    if let Some(mode) = chosen {
        toplevel.with_pending_state(|state| {
            state.decoration_mode.get_or_insert(mode);
        });
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
    if !focus.surface()?.id().same_client_as(&surface.id()) {
        return None;
    }
    Some(start_data)
}

impl Pila {
    /// Called on every `wl_surface.commit`.
    pub fn handle_xdg_commit(&mut self, surface: &WlSurface) {
        // The unmapping commit isn't the new initial commit (Smithay has reset
        // the initial configure by now): the client's next commit is.
        if self.unmap_if_buffer_removed(surface) {
            return;
        }
        let unmapped = self
            .windows
            .unmapped
            .iter()
            .enumerate()
            .find_map(|(index, w)| {
                let toplevel = w.toplevel().filter(|t| t.wl_surface() == surface)?;
                Some((index, w.clone(), toplevel.clone()))
            });
        if let Some((index, window, toplevel)) = unmapped {
            let toplevel = &toplevel;
            if !toplevel.is_initial_configure_sent() {
                restore_decoration_mode(toplevel);
                self.begin_session_restore(&window);
                self.apply_initial_rules(&window);
                // After the rules: one may name the output it opens on.
                self.send_initial_scale(surface, self.opening_output(&window));
                toplevel.send_configure();
            } else if has_buffer(surface) {
                self.windows.unmapped.remove(index);
                self.place_new_window(window);
            }
        }

        self.popups.commit(surface);
        match self.popups.find_popup(surface) {
            Some(PopupKind::Xdg(ref xdg)) => {
                // A popup hidden by this commit lost its parent (Smithay
                // resets the role): nothing to configure.
                let hidden =
                    crate::state::commit_map_change(surface) == crate::state::MapChange::Unmapped;
                if !xdg.is_initial_configure_sent() && !hidden {
                    let output = self.output_for_surface(surface);
                    self.send_initial_scale(surface, output);
                    // The initial configure is always allowed: an error (only
                    // for a configure after it) would be Smithay's.
                    if let Err(err) = xdg.send_configure() {
                        warn!(?err, "initial popup configure refused");
                    }
                }
            }
            // An input method's candidate window: its size changes with
            // what's typed (`input_method.rs`).
            Some(PopupKind::InputMethod(ref popup)) => self.place_input_method_popup(popup),
            None => {}
        }
    }

    /// A client unmaps a toplevel by committing a null buffer: it leaves
    /// the screen (and Mod+Tab, taskbars) and maps again like a new window,
    /// after a new initial commit (Smithay resets the initial configure).
    /// A new `Window` waits in `unmapped`, so nothing of the old mapping
    /// (taskbar handle, rules, restore geometry) carries over; the old one's
    /// close animation, started by `watch_unmap`, fades out on its own.
    fn unmap_if_buffer_removed(&mut self, surface: &WlSurface) -> bool {
        let waiting = self
            .windows
            .unmapped
            .iter()
            .any(|w| w.toplevel().is_some_and(|t| t.wl_surface() == surface));
        if has_buffer(surface) || waiting {
            return false;
        }
        let Some(window) = self.window_for_surface(surface) else {
            return false;
        };
        let Some(toplevel) = window
            .toplevel()
            .filter(|t| t.wl_surface() == surface)
            .cloned()
        else {
            return false;
        };
        let app_id = crate::foreign_toplevel::app_id(&window);
        debug!(app_id, "window unmapped by its client");
        let was_focused = self.focused_window().as_ref() == Some(&window);
        self.forget_window(&window);
        self.unmap_window(&window);
        self.windows
            .unmapped
            .push(Window::new_wayland_window(toplevel));
        if was_focused {
            self.focus_recent_window();
        }
        self.refresh_pointer_focus();
        self.queue_redraw();
        true
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
                // Asked for the state it's in: nothing changes, but the
                // protocol still answers with a configure.
                if !self.request_fill_state(&window, maximized, fullscreen)
                    && surface.is_initial_configure_sent()
                {
                    surface.send_configure();
                }
            }
            None if surface.is_initial_configure_sent() => {
                surface.send_configure();
            }
            None => {}
        }
    }

    /// Keep a popup inside its parent's output. The parent is a window or a
    /// layer surface.
    pub fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        // Where the root's geometry origin is, and the output it's on.
        let (parent_loc, output_geo) = if let Some(window) = self.window_for_surface(&root) {
            let Some(window_geo) = self.space.element_geometry(&window) else {
                return;
            };
            let Some(output_geo) = self.output_geometry_at(window_geo.loc.to_f64()) else {
                return;
            };
            (window_geo.loc, output_geo)
        } else if let Some((output, layer)) = self.layer_for_surface(&root) {
            let Some(output_geo) = self.space.output_geometry(&output) else {
                return;
            };
            let Some(layer_geo) = layer_map_for_output(&output).layer_geometry(&layer) else {
                return;
            };
            (output_geo.loc + layer_geo.loc, output_geo)
        } else {
            return;
        };

        // The positioner's target rectangle is relative to the parent's geometry.
        let mut target = output_geo;
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= parent_loc;

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}
