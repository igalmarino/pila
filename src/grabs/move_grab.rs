// Based on Smithay's smallvil example (MIT).
use crate::{
    snap::{edge_target, SnapTarget},
    Pila,
};
use smithay::{
    backend::input::ButtonState,
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, CursorImageStatus, GestureHoldBeginEvent, GestureHoldEndEvent,
        GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
        GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
        GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab, PointerInnerHandle,
        RelativeMotionEvent,
    },
    utils::{IsAlive, Logical, Point},
};

pub struct MoveSurfaceGrab {
    pub start_data: PointerGrabStartData<Pila>,
    pub window: Window,
    pub initial_window_location: Point<i32, Logical>,
    /// Where the window snaps if released now (pointer at an output edge).
    pub snap: Option<SnapTarget>,
    /// The pointer moved since the grab started. The press that started a
    /// modifier or title bar drag reaches the grab after it's installed; only
    /// a press after some motion means a release was lost.
    pub moved: bool,
    /// The window is maximized, fullscreen or snapped: it's restored (and
    /// starts following the pointer) only once the pointer has moved
    /// `RESTORE_DISTANCE` from where the grab started.
    pub restore_on_drag: bool,
}

/// How far the pointer moves before a drag restores a filled or snapped
/// window, in logical pixels: less is a click.
const RESTORE_DISTANCE: f64 = 8.0;

impl PointerGrab<Pila> for MoveSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut Pila,
        handle: &mut PointerInnerHandle<'_, Pila>,
        _focus: Option<(crate::focus::PointerFocus, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        // While the grab is active, no client has pointer focus
        handle.motion(data, None, event);
        // Minimized, moved to another workspace or closed meanwhile: the
        // move is over (and must not map the window again).
        if !self.window.alive() || data.space.element_location(&self.window).is_none() {
            handle.unset_grab(self, data, event.serial, event.time, true);
            return;
        }
        self.moved = true;

        if self.restore_on_drag {
            let delta = event.location - self.start_data.location;
            if delta.x.hypot(delta.y) < RESTORE_DISTANCE {
                return;
            }
            self.restore_on_drag = false;
            data.restore_for_drag(&self.window, event.location);
            let Some(location) = data.space.element_location(&self.window) else {
                handle.unset_grab(self, data, event.serial, event.time, true);
                return;
            };
            // The drag goes on from here.
            self.initial_window_location = location;
            self.start_data.location = event.location;
        }

        let delta = event.location - self.start_data.location;
        let new_location = self.initial_window_location.to_f64() + delta;
        data.map_window(self.window.clone(), new_location.to_i32_round(), true);
        data.keep_transients_above(&self.window);
        data.x11_window_moved(&self.window);

        // At an output edge: preview where the window would snap.
        let target = data
            .output_geometry_at(event.location)
            .and_then(|output| edge_target(output, event.location));
        if target != self.snap {
            tracing::debug!(?target, location = ?event.location, "move grab: snap target changed");
            self.snap = target;
            data.snap_preview = target.and_then(|t| data.snap_target_rect(&self.window, t));
            if let Some(rect) = data.snap_preview {
                data.snap_preview_buffer
                    .update(rect.size, data.config.colors.snap_preview);
            }
            data.queue_redraw();
        }
    }

    fn relative_motion(
        &mut self,
        data: &mut Pila,
        handle: &mut PointerInnerHandle<'_, Pila>,
        focus: Option<(crate::focus::PointerFocus, Point<f64, Logical>)>,
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
        tracing::debug!(button = event.button, state = ?event.state, snap = ?self.snap, "move grab: button");

        // End the grab when the button that started it is released
        // (left for client moves, left or right for modifier drags). A new
        // press of that button means its release got lost (nested: released
        // over the host window's own title bar), so that ends it too.
        let pressed_again = self.moved
            && event.state == ButtonState::Pressed
            && event.button == self.start_data.button;
        if pressed_again || !handle.current_pressed().contains(&self.start_data.button) {
            // No more buttons are pressed, release the grab (snapping the
            // window if it was dropped at an output edge).
            if let Some(target) = self.snap.take() {
                data.apply_snap_target(&self.window, target);
            }
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
        if data.snap_preview.take().is_some() {
            data.queue_redraw();
        }
    }
}
