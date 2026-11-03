//! Touchpad gestures (swipe, pinch, hold), forwarded to the client under
//! the pointer through zwp_pointer_gestures_v1 (its global is created in
//! `handlers/protocols.rs`). Firefox and GTK apps zoom on a pinch, for
//! example. (Two-finger scrolling isn't a gesture: libinput sends it as
//! axis events.)
//!
//! Only libinput produces gestures (on a TTY); nested, the host keeps them.
//! Smithay's pointer handle routes them to the pointer focus, or to the
//! active grab, so they respect the session lock like other pointer input.
//!
//! pila's own gestures (`[gestures]` in the config, e.g. a three-finger
//! swipe to switch windows) are swipes with a finger count that has an
//! action: such a swipe is kept whole from the clients, from begin to end
//! (`Pila::swipe`). Its direction is fixed once the fingers have moved
//! `LOCK` along one axis; after `TRIGGER` the action runs, and window
//! switching steps once more for every further `STEP`, ending the switch
//! when the fingers lift. Not while the session is locked or a client
//! inhibits shortcuts (a VM gets the swipe then).

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

use crate::{
    config::{Action, Gesture, SwipeDirection},
    Pila,
};

/// Movement (libinput's pointer units, about pixels) before a swipe's
/// direction is fixed.
const LOCK: f64 = 20.0;
/// Movement along the direction before the action runs.
const TRIGGER: f64 = 60.0;
/// Further movement per extra step when switching windows.
const STEP: f64 = 140.0;

/// A swipe pila is handling.
#[derive(Debug)]
pub struct Swipe {
    fingers: u32,
    total: (f64, f64),
    direction: Option<SwipeDirection>,
    /// Steps already taken (1 once the action ran).
    steps: u32,
}

impl Swipe {
    pub fn new(fingers: u32) -> Self {
        Self {
            fingers,
            total: (0.0, 0.0),
            direction: None,
            steps: 0,
        }
    }

    /// Add the fingers' movement. Returns the direction and how many new
    /// steps it completed (0 most of the time).
    pub fn update(&mut self, dx: f64, dy: f64) -> Option<(SwipeDirection, u32)> {
        self.total.0 += dx;
        self.total.1 += dy;
        let (x, y) = self.total;
        let direction = match self.direction {
            Some(direction) => direction,
            None if x.abs().max(y.abs()) < LOCK => return None,
            None => {
                let direction = if x.abs() >= y.abs() {
                    if x < 0.0 {
                        SwipeDirection::Left
                    } else {
                        SwipeDirection::Right
                    }
                } else if y < 0.0 {
                    SwipeDirection::Up
                } else {
                    SwipeDirection::Down
                };
                *self.direction.insert(direction)
            }
        };
        let along = match direction {
            SwipeDirection::Left => -x,
            SwipeDirection::Right => x,
            SwipeDirection::Up => -y,
            SwipeDirection::Down => y,
        };
        let steps = if along < TRIGGER {
            0
        } else {
            1 + ((along - TRIGGER) / STEP) as u32
        };
        let new = steps.saturating_sub(self.steps);
        self.steps = self.steps.max(steps);
        Some((direction, new))
    }
}

impl Pila {
    pub(crate) fn on_gesture_swipe_begin<I: InputBackend>(
        &mut self,
        event: I::GestureSwipeBeginEvent,
    ) {
        let fingers = event.fingers();
        if !self.is_locked() && !self.shortcuts_inhibited() && self.config.handles_swipes(fingers) {
            self.swipe = Some(Swipe::new(fingers));
            return;
        }
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        pointer.gesture_swipe_begin(
            self,
            &GestureSwipeBeginEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time_msec(),
                fingers,
            },
        );
    }

    pub(crate) fn on_gesture_swipe_update<I: InputBackend>(
        &mut self,
        event: I::GestureSwipeUpdateEvent,
    ) {
        if let Some(swipe) = &mut self.swipe {
            let delta = event.delta();
            let fingers = swipe.fingers;
            if let Some((direction, new)) = swipe.update(delta.x, delta.y) {
                // The first step(s) of this swipe: one-shot actions run now.
                let first = new > 0 && swipe.steps == new;
                self.run_swipe(Gesture { fingers, direction }, new, first);
            }
            return;
        }
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
        if self.swipe.take().is_some() {
            // A window switch made by the swipe ends when the fingers lift.
            self.end_switch();
            return;
        }
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

    /// Run a swipe's action for `steps` new steps: window switching steps
    /// each time, anything else runs once, when the swipe takes its `first`.
    fn run_swipe(&mut self, gesture: Gesture, steps: u32, first: bool) {
        let Some(action) = self.config.gesture_action(gesture) else {
            return;
        };
        let times = match action {
            Action::CycleFocus | Action::CycleFocusBack => steps,
            _ => u32::from(first),
        };
        for _ in 0..times {
            self.run_action(action.clone(), SERIAL_COUNTER.next_serial());
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_moves_fix_no_direction() {
        let mut swipe = Swipe::new(3);
        assert_eq!(swipe.update(LOCK / 2.0, LOCK / 3.0), None);
        assert_eq!(swipe.update(-LOCK, LOCK / 4.0), None);
    }

    #[test]
    fn direction_follows_the_dominant_axis_and_stays() {
        let mut swipe = Swipe::new(3);
        let left = swipe.update(-(LOCK + 1.0), 1.0);
        assert_eq!(left.map(|(d, _)| d), Some(SwipeDirection::Left));
        // Moving down afterwards doesn't change it.
        let still = swipe.update(0.0, 10.0 * STEP);
        assert_eq!(still.map(|(d, _)| d), Some(SwipeDirection::Left));
        let mut swipe = Swipe::new(3);
        let up = swipe.update(1.0, -(LOCK + 1.0));
        assert_eq!(up.map(|(d, _)| d), Some(SwipeDirection::Up));
    }

    #[test]
    fn steps_come_at_the_trigger_and_every_step_after() {
        let left = |swipe: &mut Swipe, dx: f64| swipe.update(-dx, 0.0).map(|(_, n)| n);
        let mut swipe = Swipe::new(3);
        assert_eq!(left(&mut swipe, TRIGGER - 1.0), Some(0));
        assert_eq!(left(&mut swipe, 2.0), Some(1));
        assert_eq!(left(&mut swipe, STEP - 2.0), Some(0));
        assert_eq!(left(&mut swipe, 2.0), Some(1));
        // A big jump completes several at once; going back takes none away.
        assert_eq!(left(&mut swipe, 2.0 * STEP), Some(2));
        assert_eq!(left(&mut swipe, -STEP), Some(0));
        assert_eq!(left(&mut swipe, STEP), Some(0));
    }
}
