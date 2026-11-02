mod compositor;
mod layer_shell;
pub mod protocols;
mod xdg_activation;
mod xdg_shell;

use smithay::{
    delegate_cursor_shape, delegate_data_device, delegate_output, delegate_primary_selection,
    delegate_seat,
    input::{pointer::CursorImageStatus, Seat, SeatHandler, SeatState},
    reexports::wayland_server::{protocol::wl_surface::WlSurface, Resource},
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
            SelectionHandler,
        },
        tablet_manager::TabletSeatHandler,
    },
};

use crate::Pila;

// wl_seat

impl SeatHandler for Pila {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Pila> {
        &mut self.seat_state
    }

    // Nested: named shapes are passed on to the winit window for the host to
    // draw; client cursor surfaces are rendered by pila (`cursor.rs`). A
    // real-hardware backend must also draw named shapes itself.
    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        if let Some(backend) = &self.backend {
            crate::winit::apply_cursor(backend.window(), &image);
        }
        self.cursor_status = image;
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
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

impl OutputHandler for Pila {}
delegate_output!(Pila);
