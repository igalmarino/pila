mod compositor;
mod layer_shell;
pub mod protocols;
mod xdg_activation;
mod xdg_shell;

use smithay::{
    input::{
        dnd::{DnDGrab, DndGrabHandler, DndTarget, GrabType, Source},
        keyboard::LedState,
        pointer::{CursorImageStatus, Focus},
        tablet::TabletSeatHandler,
        Seat, SeatHandler, SeatState,
    },
    output::Output,
    reexports::wayland_server::{
        protocol::{wl_output::WlOutput, wl_surface::WlSurface},
        Resource,
    },
    utils::{Logical, Point, Serial},
    wayland::{
        output::OutputHandler,
        selection::{
            data_device::{
                set_data_device_focus, DataDeviceHandler, DataDeviceState, WaylandDndGrabHandler,
            },
            primary_selection::{
                set_primary_focus, PrimarySelectionHandler, PrimarySelectionState,
            },
            SelectionHandler, SelectionSource, SelectionTarget,
        },
    },
};

use std::os::fd::OwnedFd;

use tracing::warn;

use crate::{focus::KeyboardFocus, Pila};

// wl_seat

impl SeatHandler for Pila {
    type KeyboardFocus = KeyboardFocus;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Pila> {
        &mut self.protocols.seat
    }

    // Stored for the renderer; the backend decides who draws what (nested,
    // the host draws named shapes; see `Backend::cursor_changed`).
    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        if let Some(backend) = &mut self.backend {
            backend.cursor_changed(&image);
        }
        self.cursor_status = image;
        // Where the pointer is (Pila may draw it there). Smithay calls this
        // with the pointer locked, so its location is asked for after.
        self.loop_handle.insert_idle(|state| {
            let at = state.pointer_location();
            state.queue_redraw_pointer(at, at, true);
        });
        self.wake.ping();
    }

    fn led_state_changed(&mut self, _seat: &Seat<Self>, leds: LedState) {
        if let Some(backend) = &mut self.backend {
            backend.update_leds(leds);
        }
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&KeyboardFocus>) {
        // Title bars and shadows show which window has focus.
        self.queue_redraw();
        let focused = focused.and_then(KeyboardFocus::surface);
        // A pointer constraint only holds while its window has keyboard focus.
        self.release_constraint_for_focus(focused.as_ref());
        // Clipboard and primary selection offers follow keyboard focus.
        let dh = &self.display_handle;
        let client = focused.as_ref().and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, client.clone());
        set_primary_focus(dh, seat, client);
        // The user is looking at it now (`urgency.rs`). From `focused`: Smithay
        // calls this with the keyboard locked, so asking it would deadlock.
        let window = focused
            .map(|surface| self.popup_root(&surface).unwrap_or(surface))
            .and_then(|surface| self.window_for_surface(&surface));
        if let Some(window) = window {
            self.set_urgent(&window, false);
        }
    }
}

impl Pila {
    /// Give the keyboard to `focus`, or to nothing. Smithay calls
    /// `focus_changed` when focus moves to a surface but not when it's
    /// cleared; then this does it, so a pointer constraint is released and
    /// selection offers stop going to the client that had focus.
    pub fn set_keyboard_focus(
        &mut self,
        focus: Option<KeyboardFocus>,
        serial: smithay::utils::Serial,
    ) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let cleared = focus.is_none();
        keyboard.set_focus(self, focus, serial);
        if cleared {
            let seat = self.seat.clone();
            self.focus_changed(&seat, None);
        }
    }
}

// wp_cursor_shape_v1 (server-side cursors): shape requests arrive through
// `cursor_image` as `CursorImageStatus::Named`. Smithay requires
// `TabletSeatHandler` for this even without tablet support; the default
// (ignore tablet tool cursors) is fine.

impl TabletSeatHandler for Pila {
    type ToolFocus = WlSurface;
}

// wl_data_device (clipboard, drag and drop)

impl SelectionHandler for Pila {
    type SelectionUserData = ();

    // A Wayland client set (or cleared) a selection: offer it to X11 apps.
    fn new_selection(
        &mut self,
        ty: SelectionTarget,
        source: Option<SelectionSource>,
        _seat: Seat<Self>,
    ) {
        if let Some(xwm) = self.xwayland.wm.as_mut() {
            if let Err(err) = xwm.new_selection(ty, source.map(|s| s.mime_types())) {
                warn!(?err, ?ty, "failed to offer the selection to X11 apps");
            }
        }
    }

    // A Wayland client pastes a selection owned by an X11 app.
    fn send_selection(
        &mut self,
        ty: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
        _seat: Seat<Self>,
        _user_data: &(),
    ) {
        if let Some(xwm) = self.xwayland.wm.as_mut() {
            if let Err(err) = xwm.send_selection(ty, mime_type, fd) {
                warn!(
                    ?err,
                    ?ty,
                    "failed to send an X11 selection to a Wayland client"
                );
            }
        }
    }
}

impl DataDeviceHandler for Pila {
    fn data_device_state(&mut self) -> &mut DataDeviceState {
        &mut self.protocols.data_device
    }
}

// A client starts a drag (wl_data_device.start_drag) with the button or
// touch point it holds: pila runs Smithay's DnD grab for it and draws its icon
// at the pointer (`Pila::dnd_icon`) until the drop or cancel. Not implementing
// this cancels every drag (Smithay's default).
impl WaylandDndGrabHandler for Pila {
    fn dnd_requested<S: Source>(
        &mut self,
        source: S,
        icon: Option<WlSurface>,
        seat: Seat<Self>,
        serial: Serial,
        type_: GrabType,
    ) {
        self.dnd_icon = icon.map(|surface| crate::render::DndIcon {
            surface,
            offset: (0, 0).into(),
        });
        self.queue_redraw();
        let dh = self.display_handle.clone();
        match type_ {
            GrabType::Pointer => {
                let Some(pointer) = seat.get_pointer() else {
                    return source.cancel();
                };
                let Some(start) = pointer.grab_start_data() else {
                    return source.cancel();
                };
                let grab = DnDGrab::new_pointer(&dh, start, source, seat);
                pointer.set_grab(self, grab, serial, Focus::Keep);
            }
            GrabType::Touch => {
                let Some(touch) = seat.get_touch() else {
                    return source.cancel();
                };
                let Some(start) = touch.grab_start_data() else {
                    return source.cancel();
                };
                let grab = DnDGrab::new_touch(&dh, start, source, seat);
                touch.set_grab(self, grab, serial);
            }
        }
    }
}

impl DndGrabHandler for Pila {
    fn dropped(
        &mut self,
        _target: Option<DndTarget<'_, Self>>,
        _validated: bool,
        _seat: Seat<Self>,
        _location: Point<f64, Logical>,
    ) {
        self.end_dnd();
    }

    fn cancelled(&mut self, _seat: Seat<Self>, _location: Point<f64, Logical>) {
        self.end_dnd();
    }
}

impl Pila {
    /// A drag ended: its icon goes.
    fn end_dnd(&mut self) {
        if self.dnd_icon.take().is_some() {
            self.queue_redraw();
        }
    }
}

// zwp_primary_selection (middle-click paste)

impl PrimarySelectionHandler for Pila {
    fn primary_selection_state(&mut self) -> &mut PrimarySelectionState {
        &mut self.protocols.primary_selection
    }
}

// wl_output, xdg_output

impl OutputHandler for Pila {
    // ext-workspace groups list the client's wl_outputs (ext_workspace.rs).
    fn output_bound(&mut self, _output: Output, _wl_output: WlOutput) {
        self.refresh_workspaces();
    }
}
