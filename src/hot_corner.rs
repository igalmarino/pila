//! The overview's hot corner (`[overview] hot-corner`): the pointer pushed
//! into a corner of the screen (`hot-corner-position`; an output's corner
//! pixel with no output beyond it either way) opens or closes the overview,
//! once until it leaves the corner.
//!
//! Two settings make it harder to hit by accident: `hot-corner-pressure`,
//! how far relative motion must push the pointer past the corner (the part
//! clamping to the outputs drops, `Pila::hot_corner`'s `pushed`; absolute
//! input never pushes), and `hot-corner-delay`, how long the pointer must
//! stay there, counted from reaching the corner (`Visit::since`), not from
//! the push: a push slower than the delay fires as soon as it's far enough.
//! Once pushed far enough, a delay still to run is a timer
//! (`HotCorner::timer`), removed when the pointer leaves. Nothing happens
//! with a button held or a grab running, over a fullscreen window, while
//! locked, or while picking a color or a region.

use std::time::{Duration, Instant};

use smithay::{
    output::Output,
    reexports::calloop::{
        timer::{TimeoutAction, Timer},
        RegistrationToken,
    },
    utils::{Logical, Point},
};

use crate::{config::OverviewAction, Pila};

/// The pointer's visit to the hot corner.
#[derive(Default)]
pub struct HotCorner {
    visit: Option<Visit>,
}

struct Visit {
    output: Output,
    /// When the pointer got there.
    since: Instant,
    /// How far it has been pushed past the corner since.
    pushed: f64,
    /// It acted (or was refused): nothing more until it leaves.
    done: bool,
    /// The delay still to run once pushed far enough.
    timer: Option<RegistrationToken>,
}

impl Pila {
    /// The pointer moved to `location`, `pushed` past it by relative
    /// motion clamped to the outputs: in the hot corner, pushed far enough
    /// and there long enough, it opens or closes the overview.
    pub(crate) fn hot_corner(
        &mut self,
        location: Point<f64, Logical>,
        pushed: Point<f64, Logical>,
        held: bool,
    ) {
        let config = self.config.overview;
        let corner = config
            .hot_corner
            .then(|| self.hot_corner_output(location))
            .flatten();
        let Some(output) = corner else {
            self.leave_hot_corner();
            return;
        };
        if self
            .overview
            .hot_corner
            .visit
            .as_ref()
            .is_some_and(|visit| visit.output != output)
        {
            self.leave_hot_corner();
        }
        let visit = self.overview.hot_corner.visit.get_or_insert_with(|| Visit {
            output,
            since: Instant::now(),
            pushed: 0.0,
            done: false,
            timer: None,
        });
        if visit.done {
            return;
        }
        let (out_x, out_y) = config.hot_corner_position.outward();
        visit.pushed += (pushed.x * out_x).max(0.0) + (pushed.y * out_y).max(0.0);
        if visit.pushed < f64::from(config.hot_corner_pressure) {
            return;
        }
        let delay = Duration::from_millis(config.hot_corner_delay.into());
        let left = delay.saturating_sub(visit.since.elapsed());
        if left.is_zero() {
            self.hot_corner_act(held);
            return;
        }
        if visit.timer.is_some() {
            return;
        }
        visit.timer = self
            .loop_handle
            .insert_source(Timer::from_duration(left), |_, _, state| {
                if let Some(visit) = state.overview.hot_corner.visit.as_mut() {
                    visit.timer = None;
                }
                let held = state
                    .seat
                    .get_pointer()
                    .is_some_and(|pointer| pointer.is_grabbed());
                state.hot_corner_act(held);
                TimeoutAction::Drop
            })
            .ok();
    }

    /// The pointer isn't in the hot corner (any more).
    fn leave_hot_corner(&mut self) {
        let Some(visit) = self.overview.hot_corner.visit.take() else {
            return;
        };
        if let Some(token) = visit.timer {
            self.loop_handle.remove(token);
        }
    }

    /// The output whose corner `location` is in, if it's a corner of the
    /// whole layout.
    fn hot_corner_output(&self, location: Point<f64, Logical>) -> Option<Output> {
        let (out_x, out_y) = self.config.overview.hot_corner_position.outward();
        let beyond = |x: f64, y: f64| {
            self.space
                .outputs()
                .filter_map(|o| self.space.output_geometry(o))
                .any(|g| g.to_f64().contains(Point::from((x, y))))
        };
        self.space.outputs().find_map(|output| {
            let geo = self.space.output_geometry(output)?.to_f64();
            // The corner pixel, and just past each edge that meets there.
            let x = if out_x < 0.0 {
                geo.loc.x
            } else {
                geo.loc.x + geo.size.w - 1.0
            };
            let y = if out_y < 0.0 {
                geo.loc.y
            } else {
                geo.loc.y + geo.size.h - 1.0
            };
            let inside =
                location.x >= x && location.y >= y && location.x < x + 1.0 && location.y < y + 1.0;
            let edge_x = if out_x < 0.0 {
                geo.loc.x - 0.5
            } else {
                geo.loc.x + geo.size.w + 0.5
            };
            let edge_y = if out_y < 0.0 {
                geo.loc.y - 0.5
            } else {
                geo.loc.y + geo.size.h + 0.5
            };
            let edge = !beyond(edge_x, location.y) && !beyond(location.x, edge_y);
            (inside && edge).then(|| output.clone())
        })
    }

    /// Pushed far enough and long enough: open or close the overview,
    /// unless something refuses it now. Either way, once per visit.
    fn hot_corner_act(&mut self, held: bool) {
        let Some(visit) = self.overview.hot_corner.visit.as_mut() else {
            return;
        };
        if std::mem::replace(&mut visit.done, true) {
            return;
        }
        let output = visit.output.clone();
        if held
            || !self.config.overview.hot_corner
            || self.is_locked()
            || self.color_pick_open()
            || self.region_selecting()
            || (!self.overview_open() && self.fullscreen_on(&output))
        {
            return;
        }
        tracing::debug!(output = output.name(), "hot corner");
        self.overview_action(OverviewAction::Toggle);
    }

    /// Whether the window on top on `output` is fullscreen.
    fn fullscreen_on(&self, output: &Output) -> bool {
        self.space
            .elements()
            .rev()
            .filter(|w| !crate::workspaces::is_override_redirect(w))
            .find(|w| self.window_output(w).as_ref() == Some(output))
            .is_some_and(|w| crate::state::fill_state(w).1)
    }
}
