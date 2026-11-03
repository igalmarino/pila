//! Touchpad gestures (swipe, pinch, hold), forwarded to the client under
//! the pointer through zwp_pointer_gestures_v1 (its global is created in
//! `handlers/protocols.rs`). Firefox and GTK apps zoom on a pinch, for
//! example. (Two-finger scrolling isn't a gesture: libinput sends it as
//! axis events.)
//!
//! Only libinput produces gestures (on a TTY); nested, the host keeps them.
//! Smithay's pointer handle routes them to the pointer focus, or to the
//! active grab, so they respect the session lock like other pointer input.

use smithay::{
    backend::input::{
        Event, GestureBeginEvent, GestureEndEvent, GesturePinchUpdateEvent as _,
        GestureSwipeUpdateEvent as _, InputBackend,
    },
    input::pointer::{
        GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent,
        GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
        GestureSwipeUpdateEvent,
    },
    utils::SERIAL_COUNTER,
};

use crate::Pila;

impl Pila {
    pub(crate) fn on_gesture_swipe_begin<I: InputBackend>(
        &mut self,
        event: I::GestureSwipeBeginEvent,
    ) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        pointer.gesture_swipe_begin(
            self,
            &GestureSwipeBeginEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time_msec(),
                fingers: event.fingers(),
            },
        );
    }

    pub(crate) fn on_gesture_swipe_update<I: InputBackend>(
        &mut self,
        event: I::GestureSwipeUpdateEvent,
    ) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        pointer.gesture_swipe_update(
            self,
            &GestureSwipeUpdateEvent {
                time: event.time_msec(),
                delta: event.delta(),
            },
        );
    }

    pub(crate) fn on_gesture_swipe_end<I: InputBackend>(&mut self, event: I::GestureSwipeEndEvent) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        pointer.gesture_swipe_end(
            self,
            &GestureSwipeEndEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time_msec(),
                cancelled: event.cancelled(),
            },
        );
    }

    pub(crate) fn on_gesture_pinch_begin<I: InputBackend>(
        &mut self,
        event: I::GesturePinchBeginEvent,
    ) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        pointer.gesture_pinch_begin(
            self,
            &GesturePinchBeginEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time_msec(),
                fingers: event.fingers(),
            },
        );
    }

    pub(crate) fn on_gesture_pinch_update<I: InputBackend>(
        &mut self,
        event: I::GesturePinchUpdateEvent,
    ) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        pointer.gesture_pinch_update(
            self,
            &GesturePinchUpdateEvent {
                time: event.time_msec(),
                delta: event.delta(),
                scale: event.scale(),
                rotation: event.rotation(),
            },
        );
    }

    pub(crate) fn on_gesture_pinch_end<I: InputBackend>(&mut self, event: I::GesturePinchEndEvent) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        pointer.gesture_pinch_end(
            self,
            &GesturePinchEndEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time_msec(),
                cancelled: event.cancelled(),
            },
        );
    }

    pub(crate) fn on_gesture_hold_begin<I: InputBackend>(
        &mut self,
        event: I::GestureHoldBeginEvent,
    ) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        pointer.gesture_hold_begin(
            self,
            &GestureHoldBeginEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time_msec(),
                fingers: event.fingers(),
            },
        );
    }

    pub(crate) fn on_gesture_hold_end<I: InputBackend>(&mut self, event: I::GestureHoldEndEvent) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        pointer.gesture_hold_end(
            self,
            &GestureHoldEndEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time_msec(),
                cancelled: event.cancelled(),
            },
        );
    }
}
