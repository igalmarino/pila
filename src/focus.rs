//! What the keyboard can be focused on: a Wayland surface (toplevel, popup,
//! layer surface, lock surface) or an X11 window. X11 windows need more than
//! a `wl_keyboard.enter` on their surface: Smithay's `X11Surface` also sets
//! the X input focus (`XSetInputFocus` / `WM_TAKE_FOCUS`), so the keyboard
//! focus is this enum rather than a bare `WlSurface`. The pointer focus stays
//! a `WlSurface`; Xwayland routes pointer events from it.

use std::borrow::Cow;

use smithay::{
    backend::input::InputTime,
    backend::input::KeyState,
    desktop::PopupKind,
    input::{
        keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
        Seat,
    },
    reexports::wayland_server::{backend::ObjectId, protocol::wl_surface::WlSurface},
    utils::{IsAlive, Serial},
    wayland::seat::WaylandFocus,
    xwayland::X11Surface,
};

use crate::Pila;

// `X11Surface` is large (over 400 bytes), but focus targets are cloned on
// input paths, where a `Box` would add an allocation to every clone.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum KeyboardFocus {
    Surface(WlSurface),
    X11(X11Surface),
}

impl KeyboardFocus {
    /// The Wayland surface behind the focus. An X11 window only has one once
    /// Xwayland has associated it (always the case for mapped windows).
    pub fn surface(&self) -> Option<WlSurface> {
        match self {
            KeyboardFocus::Surface(surface) => Some(surface.clone()),
            KeyboardFocus::X11(window) => window.wl_surface(),
        }
    }
}

impl From<WlSurface> for KeyboardFocus {
    fn from(surface: WlSurface) -> Self {
        KeyboardFocus::Surface(surface)
    }
}

impl From<X11Surface> for KeyboardFocus {
    fn from(window: X11Surface) -> Self {
        KeyboardFocus::X11(window)
    }
}

impl From<PopupKind> for KeyboardFocus {
    fn from(popup: PopupKind) -> Self {
        KeyboardFocus::Surface(popup.wl_surface().clone())
    }
}

/// Pointer focus is a plain surface; popup grabs need this conversion.
impl From<KeyboardFocus> for WlSurface {
    fn from(focus: KeyboardFocus) -> Self {
        match focus {
            KeyboardFocus::Surface(surface) => surface,
            // Grabs only start from Wayland surfaces; an X11 window without a
            // surface yet has nothing to point at.
            KeyboardFocus::X11(window) => window
                .wl_surface()
                .expect("X11 window without a wl_surface as pointer focus"),
        }
    }
}

impl IsAlive for KeyboardFocus {
    fn alive(&self) -> bool {
        match self {
            KeyboardFocus::Surface(surface) => surface.alive(),
            KeyboardFocus::X11(window) => window.alive(),
        }
    }
}

impl WaylandFocus for KeyboardFocus {
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        match self {
            KeyboardFocus::Surface(surface) => Some(Cow::Borrowed(surface)),
            KeyboardFocus::X11(window) => window.wl_surface().map(Cow::Owned),
        }
    }

    fn same_client_as(&self, object_id: &ObjectId) -> bool {
        match self {
            KeyboardFocus::Surface(surface) => surface.same_client_as(object_id),
            KeyboardFocus::X11(window) => window.same_client_as(object_id),
        }
    }
}

impl KeyboardTarget<Pila> for KeyboardFocus {
    fn enter(
        &self,
        seat: &Seat<Pila>,
        data: &mut Pila,
        keys: Vec<KeysymHandle<'_>>,
        serial: Serial,
    ) {
        match self {
            KeyboardFocus::Surface(surface) => {
                KeyboardTarget::enter(surface, seat, data, keys, serial)
            }
            KeyboardFocus::X11(window) => KeyboardTarget::enter(window, seat, data, keys, serial),
        }
    }

    fn leave(&self, seat: &Seat<Pila>, data: &mut Pila, serial: Serial) {
        match self {
            KeyboardFocus::Surface(surface) => KeyboardTarget::leave(surface, seat, data, serial),
            KeyboardFocus::X11(window) => KeyboardTarget::leave(window, seat, data, serial),
        }
    }

    fn key(
        &self,
        seat: &Seat<Pila>,
        data: &mut Pila,
        key: KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: InputTime,
    ) {
        match self {
            KeyboardFocus::Surface(surface) => {
                KeyboardTarget::key(surface, seat, data, key, state, serial, time)
            }
            KeyboardFocus::X11(window) => {
                KeyboardTarget::key(window, seat, data, key, state, serial, time)
            }
        }
    }

    fn modifiers(
        &self,
        seat: &Seat<Pila>,
        data: &mut Pila,
        modifiers: ModifiersState,
        serial: Serial,
    ) {
        match self {
            KeyboardFocus::Surface(surface) => {
                KeyboardTarget::modifiers(surface, seat, data, modifiers, serial)
            }
            KeyboardFocus::X11(window) => {
                KeyboardTarget::modifiers(window, seat, data, modifiers, serial)
            }
        }
    }
}
