mod compositor;
mod layer_shell;
pub mod protocols;
mod xdg_activation;
mod xdg_shell;

use smithay::{
    delegate_cursor_shape, delegate_data_device, delegate_output, delegate_primary_selection,
    delegate_seat,
    input::{keyboard::LedState, pointer::CursorImageStatus, Seat, SeatHandler, SeatState},
    output::Output,
    reexports::wayland_server::{
        protocol::{wl_output::WlOutput, wl_surface::WlSurface},
        Resource,
    },
    wayland::{
        output::OutputHandler,
        selection::{
            data_device::{
                set_data_device_focus, ClientDndGrabHandler, DataDeviceHandler, DataDeviceState,
                ServerDndGrabHandler,
            },
            primary_selection::{
                set_primary_focus, PrimarySelectionHandler, PrimarySelectionState,
            },
            SelectionHandler, SelectionSource, SelectionTarget,
        },
        tablet_manager::TabletSeatHandler,
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
        &mut self.seat_state
    }

    // Stored for the renderer; the backend decides who draws what (nested,
    // the host draws named shapes; see `Backend::cursor_changed`).
    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        if let Some(backend) = &mut self.backend {
            backend.cursor_changed(&image);
        }
        self.cursor_status = image;
        self.queue_redraw();
    }

    fn led_state_changed(&mut self, _seat: &Seat<Self>, leds: LedState) {
        if let Some(backend) = &mut self.backend {
            backend.update_leds(leds);
        }
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&KeyboardFocus>) {
        let focused = focused.and_then(KeyboardFocus::surface);
        // A pointer constraint only holds while its window has keyboard focus.
        self.release_constraint_for_focus(focused.as_ref());
        // Clipboard and primary selection offers follow keyboard focus.
        let dh = &self.display_handle;
        let client = focused.and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, client.clone());
        set_primary_focus(dh, seat, client);
    }
}
delegate_seat!(Pila);

// wp_cursor_shape_v1 (server-side cursors): shape requests arrive through
// `cursor_image` as `CursorImageStatus::Named`. Smithay requires
// `TabletSeatHandler` for this even without tablet support; the default
// (ignore tablet tool cursors) is fine.

impl TabletSeatHandler for Pila {}
delegate_cursor_shape!(Pila);

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
        if let Some(xwm) = self.xwm.as_mut() {
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
        if let Some(xwm) = self.xwm.as_mut() {
            if let Err(err) = xwm.send_selection(ty, mime_type, fd, self.loop_handle.clone()) {
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
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for Pila {}
impl ServerDndGrabHandler for Pila {}
delegate_data_device!(Pila);

// zwp_primary_selection (middle-click paste)

impl PrimarySelectionHandler for Pila {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}
delegate_primary_selection!(Pila);

// wl_output, xdg_output

impl OutputHandler for Pila {
    // ext-workspace groups list the client's wl_outputs (ext_workspace.rs).
    fn output_bound(&mut self, _output: Output, _wl_output: WlOutput) {
        self.refresh_workspaces();
    }
}
delegate_output!(Pila);
