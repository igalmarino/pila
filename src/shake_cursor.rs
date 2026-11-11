//! Shake to find the pointer: shaking it makes the cursor bigger for a
//! moment, so it's easy to spot on a large screen or among several
//! (`[cursor] shake-to-find`, `shake-magnification`).
//!
//! `ShakeDetector` looks at the pointer's last second of motion: it's a shake
//! when the way travelled is many times the diagonal of the box it stayed in
//! (back and forth, not along a line or round a circle) and that box isn't
//! the hand's tremor. While the shaking goes on the cursor grows, up to the
//! configured magnification; `HOLD_MS` after the last shake a timer starts it
//! shrinking back. The timer only starts the change: frames come as for any
//! other, from the pointer's motion while it grows (each move redraws it) and
//! from `step_animations` while it shrinks.
//!
//! Drawing it is the backends' (`render::pointer_elements`): the hotspot
//! stays under the pointer, so hit testing doesn't change. Named shapes are
//! drawn from the theme's larger images (`CursorTheme::get_magnified`);
//! nested and headless Pila draws them itself while the cursor is magnified,
//! with the host's cursor hidden (`Backend::draws_cursor`).

use std::{collections::VecDeque, time::Duration};

use smithay::{
    input::pointer::CursorImageStatus,
    output::Output,
    reexports::calloop::{
        timer::{TimeoutAction, Timer},
        RegistrationToken,
    },
    utils::{Logical, Point},
};
use tracing::debug;

use crate::{
    animation::{Animation, Curve},
    Pila,
};

/// How far back the motion a shake is told by goes.
const WINDOW: Duration = Duration::from_secs(1);
/// The most moves kept (a fast mouse reports a thousand a second).
const MAX_HISTORY: usize = 64;
/// A shake travels more than this many times its box's diagonal.
const SENSITIVITY: f64 = 4.0;
/// The smallest diagonal of a shake's box, in logical pixels: less is the
/// hand's tremor or a small correction.
const MIN_SPAN: f64 = 60.0;

/// How long shaking takes to grow the cursor all the way (`[animations]
/// speed` applies; none with animations off).
const GROW_MS: u32 = 500;
/// How long it takes to shrink back.
const SHRINK_MS: u32 = 250;
/// How long the cursor stays big after the last shake (at `[animations]
/// speed`, even with animations off).
const HOLD_MS: u32 = 400;

/// The pointer's recent motion, by the turns it took.
#[derive(Debug, Default)]
pub struct ShakeDetector {
    /// Times (on `Pila::clock`) and places: where the pointer turned, and
    /// where it is now.
    history: VecDeque<(Duration, Point<f64, Logical>)>,
}

impl ShakeDetector {
    /// The pointer is at `at` at `now`: whether it's being shaken.
    pub fn push(&mut self, now: Duration, at: Point<f64, Logical>) -> bool {
        while let Some(&(time, _)) = self.history.front() {
            if now.saturating_sub(time) <= WINDOW {
                break;
            }
            self.history.pop_front();
        }
        // Going on the same way (less than a right angle off the last
        // stretch, so a hand's drift across it doesn't count as a turn): the
        // last point moves, rather than another being added.
        let len = self.history.len();
        let same_way = len >= 2 && {
            let (_, before) = self.history[len - 2];
            let (_, last) = self.history[len - 1];
            let (stretch, now_moving) = (last - before, at - last);
            stretch.x * now_moving.x + stretch.y * now_moving.y >= 0.0
        };
        if same_way {
            self.history.pop_back();
        }
        self.history.push_back((now, at));
        if self.history.len() > MAX_HISTORY {
            self.history.pop_front();
        }
        self.shaking()
    }

    /// Whether the motion kept is a shake.
    fn shaking(&self) -> bool {
        let mut points = self.history.iter().map(|&(_, at)| at);
        let Some(first) = points.next() else {
            return false;
        };
        let (mut min, mut max, mut last) = (first, first, first);
        let mut travelled = 0.0;
        for at in points {
            let step = at - last;
            travelled += step.x.hypot(step.y);
            min = (min.x.min(at.x), min.y.min(at.y)).into();
            max = (max.x.max(at.x), max.y.max(at.y)).into();
            last = at;
        }
        let span = max - min;
        let diagonal = span.x.hypot(span.y);
        diagonal >= MIN_SPAN && travelled > SENSITIVITY * diagonal
    }

    pub fn clear(&mut self) {
        self.history.clear();
    }
}

/// Where the cursor's size is.
#[derive(Debug, Default)]
enum Phase {
    /// Its own size: nothing to do.
    #[default]
    Rest,
    /// Being shaken since `start`, last at `last`: growing from `from`
    /// (1, or what it was when it was shrinking) until it's the
    /// configured magnification. It doesn't grow past `last`.
    Growing {
        start: Duration,
        last: Duration,
        from: f64,
    },
    /// Back to its own size.
    Shrinking(Animation),
}

/// The cursor's magnification (`Pila::shake_cursor`).
#[derive(Debug, Default)]
pub struct ShakeCursor {
    detector: ShakeDetector,
    phase: Phase,
    /// The timer that starts it shrinking once the shaking stops.
    hold: Option<RegistrationToken>,
}

impl ShakeCursor {
    /// Whether it's drawn magnified (perhaps still at its own size, about
    /// to grow or done shrinking).
    pub fn magnified(&self) -> bool {
        !matches!(self.phase, Phase::Rest)
    }

    /// Whether it's changing on its own, with no input: shrinking.
    pub fn changing(&self, now: Duration) -> bool {
        matches!(&self.phase, Phase::Shrinking(shrink) if !shrink.done(now))
    }
}

/// How big shaking from `from` since `start` until `last` made the cursor,
/// at `now`: `grow` takes it from 1 to `max`.
fn grown(
    from: f64,
    start: Duration,
    last: Duration,
    now: Duration,
    grow: Duration,
    max: f64,
) -> f64 {
    if grow.is_zero() {
        return max;
    }
    let shaken = now.min(last).saturating_sub(start).as_secs_f64();
    (from + (max - 1.0) * shaken / grow.as_secs_f64()).min(max)
}

impl Pila {
    /// How many times bigger the cursor is drawn now, `None` at its own size
    /// (and not drawn magnified).
    pub fn cursor_magnification(&self) -> Option<f64> {
        let now = self.animation_time();
        let max = self.config.cursor.shake_magnification;
        match &self.shake_cursor.phase {
            Phase::Rest => None,
            Phase::Growing { start, last, from } => {
                let grow = self.config.animations.scaled(GROW_MS);
                Some(grown(*from, *start, *last, now, grow, max))
            }
            Phase::Shrinking(shrink) => Some(shrink.value(now)),
        }
    }

    /// The pointer moved to `location` (not a warp): grow the cursor if it's
    /// being shaken.
    pub(crate) fn shake_cursor_motion(&mut self, location: Point<f64, Logical>) {
        if !self.config.cursor.shake_to_find
            || matches!(self.cursor_status, CursorImageStatus::Hidden)
        {
            return;
        }
        let now = self.animation_time();
        if !self.shake_cursor.detector.push(now, location) {
            return;
        }
        let current = self.cursor_magnification();
        let shake = &mut self.shake_cursor;
        match &mut shake.phase {
            Phase::Growing { last, .. } => *last = now,
            phase => {
                debug!(from = current, "cursor shaken: growing");
                *phase = Phase::Growing {
                    start: now,
                    last: now,
                    from: current.unwrap_or(1.0),
                };
            }
        }
        if shake.hold.is_none() {
            let hold = self.config.animations.at_speed(HOLD_MS);
            shake.hold = self
                .loop_handle
                .insert_source(Timer::from_duration(hold), |_, _, state| {
                    state.shake_cursor_hold_ended()
                })
                .ok();
        }
        if current.is_none() {
            self.cursor_magnified_changed();
        }
        self.queue_redraw_pointer(location, location, true);
    }

    /// The hold timer fired: shrink the cursor back if the shaking stopped,
    /// else wait until `HOLD_MS` after the last shake.
    fn shake_cursor_hold_ended(&mut self) -> TimeoutAction {
        let now = self.animation_time();
        let Phase::Growing { last, .. } = self.shake_cursor.phase else {
            self.shake_cursor.hold = None;
            return TimeoutAction::Drop;
        };
        let ends = last + self.config.animations.at_speed(HOLD_MS);
        if now < ends {
            return TimeoutAction::ToDuration(ends - now);
        }
        let from = self.cursor_magnification().unwrap_or(1.0);
        debug!(from, "cursor shaking stopped: shrinking");
        let shrink = self.config.animations.scaled(SHRINK_MS);
        self.shake_cursor.phase =
            Phase::Shrinking(Animation::new(now, shrink, Curve::EaseOut, from, 1.0));
        self.shake_cursor.hold = None;
        self.shake_cursor.detector.clear();
        let at = self.pointer_location();
        self.queue_redraw_pointer(at, at, true);
        TimeoutAction::Drop
    }

    /// After a frame (`step_animations`): the output whose cursor changes
    /// on its own and needs another frame, if one does. A cursor done
    /// shrinking is at rest after this frame; it needs one more there (nested,
    /// the host takes its cursor back).
    pub(crate) fn step_shake_cursor(&mut self) -> Option<Output> {
        let now = self.animation_time();
        let Phase::Shrinking(shrink) = &self.shake_cursor.phase else {
            return None;
        };
        if shrink.done(now) {
            self.shake_cursor.phase = Phase::Rest;
            self.cursor_magnified_changed();
        }
        self.output_at(self.pointer_location())
    }

    /// Put the cursor back at its own size at once (shake to find turned off).
    pub(crate) fn reset_shake_cursor(&mut self) {
        let shake = &mut self.shake_cursor;
        shake.detector.clear();
        if let Some(hold) = shake.hold.take() {
            self.loop_handle.remove(hold);
        }
        shake.phase = Phase::Rest;
        self.cursor_magnified_changed();
        let at = self.pointer_location();
        self.queue_redraw_pointer(at, at, true);
    }

    /// The cursor started or stopped being drawn magnified: nested, whether
    /// the host or Pila draws a named shape changes.
    fn cursor_magnified_changed(&mut self) {
        let magnified = self.shake_cursor.magnified();
        if let Some(backend) = &mut self.backend {
            backend.cursor_changed(&self.cursor_status, magnified);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds `points` to a detector a move every `step`; whether the last
    /// one was a shake.
    fn shaken(points: impl IntoIterator<Item = (f64, f64)>, step: Duration) -> bool {
        let mut detector = ShakeDetector::default();
        let mut now = Duration::from_secs(10);
        let mut shaking = false;
        for point in points {
            shaking = detector.push(now, point.into());
            now += step;
        }
        shaking
    }

    /// Back and forth along x, `legs` times, `width` wide, a move every 8 px.
    fn zigzag(legs: usize, width: f64) -> Vec<(f64, f64)> {
        let steps = (width / 8.0) as usize;
        let mut points = Vec::new();
        for leg in 0..legs {
            for i in 0..steps {
                let x = 8.0 * i as f64;
                let x = if leg % 2 == 0 { x } else { width - x };
                // A little drift up and down, as a hand does.
                points.push((500.0 + x, 300.0 + (i % 3) as f64));
            }
        }
        points
    }

    const MOVE: Duration = Duration::from_millis(4);

    #[test]
    fn shaking_back_and_forth_is_a_shake() {
        // 8 legs of 200 px, 25 moves each at 4 ms: 800 ms.
        assert!(shaken(zigzag(8, 200.0), MOVE));
    }

    #[test]
    fn a_line_a_circle_and_a_tremor_arent() {
        let line = (0..200).map(|i| (4.0 * i as f64, 2.0 * i as f64));
        assert!(!shaken(line, MOVE));
        let circle = (0..200).map(|i| {
            let angle = i as f64 / 200.0 * std::f64::consts::TAU;
            (500.0 + 150.0 * angle.cos(), 400.0 + 150.0 * angle.sin())
        });
        assert!(!shaken(circle, MOVE));
        // Back and forth, but only 16 px wide.
        assert!(!shaken(zigzag(30, 16.0), MOVE));
        // A couple of corrections, not a shake.
        assert!(!shaken(zigzag(3, 200.0), MOVE));
    }

    #[test]
    fn a_shake_spread_over_more_than_the_window_isnt() {
        // The same 8 legs, over 8 s: never more than one leg in a second.
        assert!(!shaken(zigzag(8, 200.0), Duration::from_millis(40)));
    }

    #[test]
    fn the_cursor_grows_while_shaken_and_no_further_than_the_last_shake() {
        let s = Duration::from_secs;
        let ms = Duration::from_millis;
        let grow = ms(500);
        assert_eq!(grown(1.0, s(10), s(11), s(10), grow, 3.0), 1.0);
        assert_eq!(grown(1.0, s(10), s(11), s(10) + ms(250), grow, 3.0), 2.0);
        // Shaking stopped at 10.1 s: 10.3 s finds it as it was then.
        let then = grown(1.0, s(10), s(10) + ms(100), s(10) + ms(300), grow, 3.0);
        assert!((then - 1.4).abs() < 1e-9, "{then}");
        assert_eq!(grown(1.0, s(10), s(20), s(20), grow, 3.0), 3.0);
        // From where it was shrinking.
        assert_eq!(grown(2.0, s(10), s(20), s(10) + ms(125), grow, 3.0), 2.5);
        // Animations off: all the way at once.
        assert_eq!(grown(1.0, s(10), s(10), s(10), Duration::ZERO, 3.0), 3.0);
    }
}
