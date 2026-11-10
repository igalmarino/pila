//! What the keyboard can be focused on: a Wayland surface (toplevel, popup,
//! layer surface, lock surface) or an X11 window. X11 windows need more than
//! a `wl_keyboard.enter` on their surface: Smithay's `X11Surface` also sets
//! the X input focus (`XSetInputFocus` / `WM_TAKE_FOCUS`), so the keyboard
//! focus is this enum rather than a bare `WlSurface`. The pointer focus is a
//! surface too (Xwayland routes pointer events from it), in a type of Pila's
//! (`PointerFocus`).
//!
//! Both are where input reaches clients, whatever path sent it (Pila's or a
//! Smithay grab's), so that's where the session lock is enforced: while
//! locked, nothing but a lock surface (`Pila::is_lock_surface`) gets an enter,
//! a key, modifiers, motion, a button, a scroll, a gesture or a drag
//! (`Pila::refused_while_locked`, which logs the attempt and, in debug
//! builds, panics to show the path that tried: every such path is a bug).
//! Leaving is always allowed.

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

use smithay::input::pointer::{
    AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
    GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
    GestureSwipeUpdateEvent, MotionEvent, PointerTarget, RelativeMotionEvent,
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
        if data.refused_while_locked(self.surface().as_ref(), "keyboard focus") {
            return;
        }
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
        if data.refused_while_locked(self.surface().as_ref(), "keyboard focus") {
            return;
        }
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
        if data.refused_while_locked(self.surface().as_ref(), "keyboard focus") {
            return;
        }
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

/// What the pointer can be focused on: a surface, or nothing at all (an X11
/// window Xwayland hasn't given a surface yet, when a popup grab hands it
/// the pointer: there's nothing to point at, and events to it go nowhere).
#[derive(Debug, Clone, PartialEq)]
pub enum PointerFocus {
    Surface(WlSurface),
    Nothing,
}

impl PointerFocus {
    pub fn surface(&self) -> Option<&WlSurface> {
        match self {
            PointerFocus::Surface(surface) => Some(surface),
            PointerFocus::Nothing => None,
        }
    }

    /// The surface to send an event to: none for `Nothing`, nor while the
    /// session is locked for one that isn't a lock surface (refused).
    fn target(&self, data: &mut Pila) -> Option<WlSurface> {
        let surface = self.surface()?;
        (!data.refused_while_locked(Some(surface), "pointer focus")).then(|| surface.clone())
    }
}

impl From<WlSurface> for PointerFocus {
    fn from(surface: WlSurface) -> Self {
        PointerFocus::Surface(surface)
    }
}

/// Popup grabs give the pointer to what has the keyboard.
impl From<KeyboardFocus> for PointerFocus {
    fn from(focus: KeyboardFocus) -> Self {
        focus
            .surface()
            .map_or(PointerFocus::Nothing, PointerFocus::Surface)
    }
}

impl IsAlive for PointerFocus {
    fn alive(&self) -> bool {
        self.surface().is_some_and(IsAlive::alive)
    }
}

impl WaylandFocus for PointerFocus {
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        self.surface().map(Cow::Borrowed)
    }

    fn same_client_as(&self, object_id: &ObjectId) -> bool {
        self.surface()
            .is_some_and(|surface| surface.same_client_as(object_id))
    }
}

/// Every event but `leave` goes through `target`.
macro_rules! pointer_events {
    ($($method:ident($event:ty)),* $(,)?) => {
        $(
            fn $method(&self, seat: &Seat<Pila>, data: &mut Pila, event: $event) {
                if let Some(surface) = self.target(data) {
                    PointerTarget::<Pila>::$method(&surface, seat, data, event);
                }
            }
        )*
    };
}

impl PointerTarget<Pila> for PointerFocus {
    pointer_events! {
        enter(&MotionEvent),
        motion(&MotionEvent),
        relative_motion(&RelativeMotionEvent),
        button(&ButtonEvent),
        axis(AxisFrame),
        gesture_swipe_begin(&GestureSwipeBeginEvent),
        gesture_swipe_update(&GestureSwipeUpdateEvent),
        gesture_swipe_end(&GestureSwipeEndEvent),
        gesture_pinch_begin(&GesturePinchBeginEvent),
        gesture_pinch_update(&GesturePinchUpdateEvent),
        gesture_pinch_end(&GesturePinchEndEvent),
        gesture_hold_begin(&GestureHoldBeginEvent),
        gesture_hold_end(&GestureHoldEndEvent),
    }

    fn frame(&self, seat: &Seat<Pila>, data: &mut Pila) {
        if let Some(surface) = self.target(data) {
            PointerTarget::<Pila>::frame(&surface, seat, data);
        }
    }

    fn leave(&self, seat: &Seat<Pila>, data: &mut Pila, serial: Serial, time: InputTime) {
        if let Some(surface) = self.surface() {
            PointerTarget::<Pila>::leave(surface, seat, data, serial, time);
        }
    }
}

/// A drag over a surface: refused like any pointer event while locked.
impl smithay::input::dnd::DndFocus<Pila> for PointerFocus {
    type OfferData<S: smithay::input::dnd::Source> =
        <WlSurface as smithay::input::dnd::DndFocus<Pila>>::OfferData<S>;

    fn enter<S: smithay::input::dnd::Source>(
        &self,
        data: &mut Pila,
        dh: &smithay::reexports::wayland_server::DisplayHandle,
        source: std::sync::Arc<S>,
        seat: &Seat<Pila>,
        location: smithay::utils::Point<f64, smithay::utils::Logical>,
        serial: &Serial,
    ) -> Option<Self::OfferData<S>> {
        let surface = self.target(data)?;
        smithay::input::dnd::DndFocus::enter(&surface, data, dh, source, seat, location, serial)
    }

    fn motion<S: smithay::input::dnd::Source>(
        &self,
        data: &mut Pila,
        offer: Option<&mut Self::OfferData<S>>,
        seat: &Seat<Pila>,
        location: smithay::utils::Point<f64, smithay::utils::Logical>,
        time: InputTime,
    ) {
        if let Some(surface) = self.target(data) {
            smithay::input::dnd::DndFocus::motion(&surface, data, offer, seat, location, time);
        }
    }

    fn leave<S: smithay::input::dnd::Source>(
        &self,
        data: &mut Pila,
        offer: Option<&mut Self::OfferData<S>>,
        seat: &Seat<Pila>,
    ) {
        if let Some(surface) = self.surface() {
            smithay::input::dnd::DndFocus::leave(surface, data, offer, seat);
        }
    }

    fn drop<S: smithay::input::dnd::Source>(
        &self,
        data: &mut Pila,
        offer: Option<&mut Self::OfferData<S>>,
        seat: &Seat<Pila>,
    ) {
        if let Some(surface) = self.target(data) {
            smithay::input::dnd::DndFocus::drop(&surface, data, offer, seat);
        }
    }
}
