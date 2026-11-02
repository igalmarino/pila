// Based on Smithay's smallvil example (MIT).
use crate::Pila;
use smithay::{
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, CursorImageStatus, GestureHoldBeginEvent, GestureHoldEndEvent,
        GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
        GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
        GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab, PointerInnerHandle,
        RelativeMotionEvent,
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point},
};

pub struct MoveSurfaceGrab {
    pub start_data: PointerGrabStartData<Pila>,
    pub window: Window,
    pub initial_window_location: Point<i32, Logical>,
}

impl PointerGrab<Pila> for MoveSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut Pila,
        handle: &mut PointerInnerHandle<'_, Pila>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        // While the grab is active, no client has pointer focus
        handle.motion(data, None, event);

        let delta = event.location - self.start_data.location;
        let new_location = self.initial_window_location.to_f64() + delta;
        data.space
            .map_element(self.window.clone(), new_location.to_i32_round(), true);
        data.x11_window_moved(&self.window);
    }

    fn relative_motion(
        &mut self,
        data: &mut Pila,
        handle: &mut PointerInnerHandle<'_, Pila>,
        focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(
        &mut self,
        data: &mut Pila,
        handle: &mut PointerInnerHandle<'_, Pila>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);

        // End the grab when the button that started it is released
        // (left for client moves, left or right for modifier drags).
        if !handle.current_pressed().contains(&self.start_data.button) {
            // No more buttons are pressed, release the grab.
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(
        &mut self,
        data: &mut Pila,
        handle: &mut PointerInnerHandle<'_, Pila>,
        details: AxisFrame,
    ) {
        handle.axis(data, details)
    }

    fn frame(&mut self, data: &mut Pila, handle: &mut PointerInnerHandle<'_, Pila>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut Pila,
        handle: &mut PointerInnerHandle<'_, Pila>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event)
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut Pila,
        handle: &mut PointerInnerHandle<'_, Pila>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event)
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut Pila,
        handle: &mut PointerInnerHandle<'_, Pila>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event)
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut Pila,
        handle: &mut PointerInnerHandle<'_, Pila>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event)
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut Pila,
        handle: &mut PointerInnerHandle<'_, Pila>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event)
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut Pila,
        handle: &mut PointerInnerHandle<'_, Pila>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event)
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut Pila,
        handle: &mut PointerInnerHandle<'_, Pila>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event)
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut Pila,
        handle: &mut PointerInnerHandle<'_, Pila>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event)
    }

    fn start_data(&self) -> &PointerGrabStartData<Pila> {
        &self.start_data
    }

    fn unset(&mut self, data: &mut Pila) {
        data.set_cursor(CursorImageStatus::default_named());
    }
}
