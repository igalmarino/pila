//! xdg_activation_v1: lets a client bring a window (its own or another app's)
//! to the front, e.g. a browser raising itself when a link is opened.
//!
//! Focus-stealing policy: a token is only issued if the client asked for it
//! in response to user input it received while it had keyboard focus. Tokens
//! are single-use and expire after `TOKEN_TIMEOUT`.

use std::time::Duration;

use smithay::{
    delegate_xdg_activation,
    input::Seat,
    reexports::wayland_server::{protocol::wl_surface::WlSurface, Resource},
    utils::SERIAL_COUNTER,
    wayland::xdg_activation::{
        XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
    },
};
use tracing::debug;

use crate::Pila;

const TOKEN_TIMEOUT: Duration = Duration::from_secs(10);

impl XdgActivationHandler for Pila {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.xdg_activation_state
    }

    /// Returning `false` makes Smithay drop the token, so `request_activation`
    /// only ever sees tokens that passed this check.
    fn token_created(&mut self, _token: XdgActivationToken, data: XdgActivationTokenData) -> bool {
        // Unused tokens would otherwise pile up.
        self.xdg_activation_state
            .retain_tokens(|_, d| d.timestamp.elapsed() < TOKEN_TIMEOUT);

        let valid = self.token_is_valid(&data);
        debug!(valid, app_id = ?data.app_id, "activation token requested");
        valid
    }

    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        token_data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        self.xdg_activation_state.remove_token(&token);
        if token_data.timestamp.elapsed() >= TOKEN_TIMEOUT {
            debug!("ignoring expired activation token");
            return;
        }
        let Some(window) = self.window_for_surface(&surface) else {
            return;
        };
        // Unmapped windows are focused when they map anyway.
        if self.space.elements().any(|w| w == &window) {
            debug!("activating window");
            self.focus_window(Some(window), SERIAL_COUNTER.next_serial());
        }
    }
}
delegate_xdg_activation!(Pila);

impl Pila {
    fn token_is_valid(&self, data: &XdgActivationTokenData) -> bool {
        let Some((serial, seat)) = &data.serial else {
            return false;
        };
        let Some(keyboard) = self.seat.get_keyboard() else {
            return false;
        };
        // The serial must come from an event sent after the current focus
        // was entered, and the requesting client must own that focus.
        let fresh = keyboard
            .last_enter()
            .is_some_and(|enter| serial.is_no_older_than(&enter));
        let focused_client = keyboard
            .current_focus()
            .and_then(|focus| focus.surface())
            .and_then(|focus| self.display_handle.get_client(focus.id()).ok())
            .map(|client| client.id());
        Seat::from_resource(seat).as_ref() == Some(&self.seat)
            && fresh
            && focused_client.is_some()
            && focused_client == data.client_id
    }
}
