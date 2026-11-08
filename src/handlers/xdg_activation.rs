//! xdg_activation_v1: lets a client bring a window (its own or another app's)
//! to the front, e.g. a browser raising itself when a link is opened.
//!
//! Focus-stealing policy: a token is only issued for user input the client
//! received while it had the keyboard focus, or the pointer (a click on a
//! notification or panel, which take no keyboard). Tokens are single-use and
//! expire after `TOKEN_TIMEOUT`.

use std::time::Duration;

use smithay::{
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
        &mut self.protocols.xdg_activation
    }

    /// Returning `false` makes Smithay drop the token, so `request_activation`
    /// only ever sees tokens that passed this check.
    fn token_created(&mut self, _token: XdgActivationToken, data: XdgActivationTokenData) -> bool {
        // Unused tokens would otherwise pile up.
        self.protocols
            .xdg_activation
            .retain_tokens(|_, d| d.timestamp.elapsed() < TOKEN_TIMEOUT);

        // No serial: no user input asked for it, so it can only make its
        // window urgent (`urgency.rs`; not specified, but what clients mean).
        if data.serial.is_none() {
            data.user_data
                .insert_if_missing(|| crate::urgency::UrgentOnlyToken);
            debug!(app_id = ?data.app_id, "urgency token requested");
            return true;
        }
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
        self.protocols.xdg_activation.remove_token(&token);
        if token_data.timestamp.elapsed() >= TOKEN_TIMEOUT {
            debug!("ignoring expired activation token");
            return;
        }
        let Some(window) = self.window_for_surface(&surface) else {
            return;
        };
        if token_data
            .user_data
            .get::<crate::urgency::UrgentOnlyToken>()
            .is_some()
        {
            self.set_urgent(&window, true);
            return;
        }
        // On another workspace: go there; minimized: bring it back (as a
        // taskbar's activate does).
        self.reveal_workspace_of(&window);
        if self.is_minimized(&window) {
            self.show_window(&window);
        }
        // Unmapped windows are focused when they map anyway.
        if self.space.elements().any(|w| w == &window) {
            debug!("activating window");
            self.focus_window(Some(window), SERIAL_COUNTER.next_serial());
        }
    }
}

impl Pila {
    fn token_is_valid(&self, data: &XdgActivationTokenData) -> bool {
        let Some((serial, seat)) = &data.serial else {
            return false;
        };
        if Seat::from_resource(seat).as_ref() != Some(&self.seat) || data.client_id.is_none() {
            return false;
        }
        let client_of = |surface: &WlSurface| {
            self.display_handle
                .get_client(surface.id())
                .ok()
                .map(|client| client.id())
        };
        // A key (or anything after the keyboard focus was entered) of the
        // client holding the keyboard focus.
        let keyboard = self.seat.get_keyboard().is_some_and(|keyboard| {
            keyboard
                .last_enter()
                .is_some_and(|enter| serial.is_no_older_than(&enter))
                && keyboard
                    .current_focus()
                    .and_then(|focus| focus.surface().and_then(|s| client_of(&s)))
                    == data.client_id
        });
        // A click (or motion) on a surface of the client under the pointer:
        // a notification or a panel, which take no keyboard (as niri, which
        // doesn't check the client).
        let pointer = self.seat.get_pointer().is_some_and(|pointer| {
            pointer
                .last_enter()
                .is_some_and(|enter| serial.is_no_older_than(&enter))
                && pointer.current_focus().and_then(|s| client_of(&s)) == data.client_id
        });
        keyboard || pointer
    }
}
