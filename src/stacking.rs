//! Windows that stay put as others come and go: above the others
//! (`always-on-top`) or on every workspace (`all-workspaces`), and moving
//! windows and focus between outputs.
//!
//! Both marks are in the window's user data, set by a window rule when it opens
//! or by a binding. A window's transient family shares them: a dialog opened from
//! a marked window is marked too.
//!
//! `Pila::keep_on_top` puts always-on-top windows back above the others, keeping
//! their order. Whatever raises a window has to end in it: `raise_family` and
//! `keep_transients_above` do. A window on all workspaces is never parked:
//! `workspaces.rs` leaves it in the `Space` and draws it fixed during a slide.

use std::cell::Cell;

use smithay::{
    desktop::Window,
    output::Output,
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER},
};
use tracing::debug;

use crate::{
    config::OutputTarget,
    decoration,
    session_management::fit_in_area,
    state::{fill_state, restore_geometry, set_restore_geometry},
    workspaces::is_override_redirect,
    Pila,
};

#[derive(Default)]
struct Marks {
    on_top: Cell<bool>,
    all_workspaces: Cell<bool>,
}

fn marks(window: &Window) -> &Marks {
    window.user_data().insert_if_missing(Marks::default);
    window.user_data().get::<Marks>().unwrap()
}

pub fn is_always_on_top(window: &Window) -> bool {
    marks(window).on_top.get()
}

pub fn is_on_all_workspaces(window: &Window) -> bool {
    marks(window).all_workspaces.get()
}

/// `window` stops being on all workspaces, with its family.
pub fn leave_all_workspaces(family: &[Window]) {
    for member in family {
        marks(member).all_workspaces.set(false);
    }
}

/// `next` or `prev` of `outputs` from `from`, around the end; `None` with no other.
fn neighbor(outputs: &[Output], from: &Output, target: OutputTarget) -> Option<Output> {
    let index = outputs.iter().position(|o| o == from)?;
    let count = outputs.len();
    if count < 2 {
        return None;
    }
    let next = match target {
        OutputTarget::Next => (index + 1) % count,
        OutputTarget::Prev => (index + count - 1) % count,
    };
    Some(outputs[next].clone())
}

impl Pila {
    /// Mark `window` as `rule` says when it opens, or as its parent is.
    pub(crate) fn mark_new_window(&mut self, window: &Window, on_top: bool, all_workspaces: bool) {
        let parent = self.transient_parent(window);
        let inherit = |mark: fn(&Window) -> bool| parent.as_ref().is_some_and(mark);
        marks(window)
            .on_top
            .set(on_top || inherit(is_always_on_top));
        marks(window)
            .all_workspaces
            .set(all_workspaces || inherit(is_on_all_workspaces));
    }

    /// `window` was set on another window: it takes that one's marks.
    pub(crate) fn inherit_marks(&mut self, window: &Window) {
        self.mark_new_window(
            window,
            is_always_on_top(window),
            is_on_all_workspaces(window),
        );
    }

    pub fn toggle_always_on_top(&mut self, window: &Window) {
        let on = !is_always_on_top(window);
        debug!(on, "always on top");
        let family = self.transient_family(window);
        for member in &family {
            marks(member).on_top.set(on);
        }
        // The last one marked comes first among those marked.
        self.raise_in_order(&family);
        self.keep_on_top();
        self.refresh_pointer_focus();
        self.queue_redraw();
    }

    pub fn toggle_all_workspaces(&mut self, window: &Window) {
        let on = !is_on_all_workspaces(window);
        debug!(on, "on all workspaces");
        for member in self.transient_family(window) {
            marks(&member).all_workspaces.set(on);
        }
    }

    /// Put the always-on-top windows above the others, in their order, and X11
    /// menus above those. Nothing if they already are: a change of order
    /// redraws what it touches, and this runs on each step of a drag.
    pub(crate) fn keep_on_top(&mut self) {
        let managed: Vec<Window> = self
            .space
            .elements()
            .filter(|w| !is_override_redirect(w))
            .cloned()
            .collect();
        let marked: Vec<Window> = managed
            .iter()
            .filter(|w| is_always_on_top(w))
            .cloned()
            .collect();
        if managed.ends_with(&marked) {
            return;
        }
        let menus: Vec<Window> = self
            .space
            .elements()
            .filter(|w| is_override_redirect(w))
            .cloned()
            .collect();
        self.raise_in_order(&marked);
        self.raise_in_order(&menus);
    }

    /// The outputs, by position.
    fn outputs_in_order(&self) -> Vec<Output> {
        let mut outputs: Vec<(Output, (i32, i32))> = self
            .space
            .outputs()
            .filter_map(|output| {
                let geo = self.space.output_geometry(output)?;
                Some((output.clone(), (geo.loc.x, geo.loc.y)))
            })
            .collect();
        outputs.sort_by_key(|(_, position)| *position);
        outputs.into_iter().map(|(output, _)| output).collect()
    }

    /// Send the focused window, with its family, to another output. It keeps
    /// its place relative to the free area there, or fills it or snaps to the
    /// same side. Not while it's being moved or resized.
    pub fn move_focused_to_output(&mut self, target: OutputTarget) {
        let Some(window) = self.focused_window() else {
            return;
        };
        let family = self.transient_family(&window);
        if self
            .grabbed_window()
            .is_some_and(|grabbed| family.contains(&grabbed))
        {
            return;
        }
        let Some(from) = self.window_output(&window) else {
            return;
        };
        let Some(to) = neighbor(&self.outputs_in_order(), &from, target) else {
            return;
        };
        let (Some(from_area), Some(to_area)) =
            (self.output_area(&from, true), self.output_area(&to, true))
        else {
            return;
        };
        debug!(to = to.name(), "moving window to output");
        let moved = |rect: Rectangle<i32, Logical>, bar: i32| -> Point<i32, Logical> {
            let loc = to_area.loc + (rect.loc - from_area.loc);
            fit_in_area(loc, rect.size, bar, to_area)
        };
        let bar = decoration::layout_offset(&window, false);
        let (maximized, fullscreen) = fill_state(&window);
        let Some(location) = self.space.element_location(&window) else {
            return;
        };
        if maximized || fullscreen {
            if let Some(mut rect) = restore_geometry(&window) {
                rect.loc = moved(rect, bar);
                set_restore_geometry(&window, rect);
            }
            self.animate_geometry(&window);
            self.set_fill_state_on(&window, maximized, fullscreen, Some(&to));
        } else if !self.move_snapped_to(&window, to_area, |rect| moved(rect, bar)) {
            let rect = Rectangle::new(location, window.geometry().size);
            let loc = moved(rect, bar);
            self.animate_geometry(&window);
            self.map_window(window.clone(), loc, false);
            self.x11_window_moved(&window);
        }
        // The rest of the family goes the same way.
        if let Some(now) = self.space.element_location(&window) {
            let delta = now - location;
            for member in family.iter().filter(|member| **member != window) {
                if let Some(at) = self.space.element_location(member) {
                    self.map_window(member.clone(), at + delta, false);
                    self.x11_window_moved(member);
                }
            }
        }
        self.keep_transients_above(&window);
        self.refresh_pointer_focus();
        self.queue_redraw();
    }

    /// Go to another output: the pointer to its middle, unless a client holds
    /// it, and the focus to its top window. Not during a pointer grab.
    pub fn focus_output(&mut self, target: OutputTarget) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        if pointer.is_grabbed() {
            return;
        }
        let at = pointer.current_location();
        let Some(from) = self
            .space
            .outputs()
            .find(|output| {
                self.space
                    .output_geometry(output)
                    .is_some_and(|geo| geo.to_f64().contains(at))
            })
            .cloned()
        else {
            return;
        };
        let Some(to) = neighbor(&self.outputs_in_order(), &from, target) else {
            return;
        };
        let Some(geo) = self.space.output_geometry(&to) else {
            return;
        };
        debug!(to = to.name(), "focusing output");
        let middle = Point::<f64, Logical>::from((
            f64::from(geo.loc.x) + f64::from(geo.size.w) / 2.0,
            f64::from(geo.loc.y) + f64::from(geo.size.h) / 2.0,
        ));
        self.warp_pointer(middle);
        let top = self
            .space
            .elements()
            .rev()
            .find(|w| !is_override_redirect(w) && self.window_output(w).as_ref() == Some(&to))
            .cloned();
        self.focus_window(top, SERIAL_COUNTER.next_serial());
        self.queue_redraw();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::output::{PhysicalProperties, Subpixel};

    fn output(name: &str) -> Output {
        Output::new(
            name.to_string(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: String::new(),
                model: String::new(),
                serial_number: String::new(),
            },
        )
    }

    #[test]
    fn the_next_and_previous_output_go_around() {
        let outputs = [output("a"), output("b"), output("c")];
        let at = |from: usize, target| neighbor(&outputs, &outputs[from], target).unwrap().name();
        assert_eq!(at(0, OutputTarget::Next), "b");
        assert_eq!(at(2, OutputTarget::Next), "a");
        assert_eq!(at(0, OutputTarget::Prev), "c");
        assert!(neighbor(&outputs[..1], &outputs[0], OutputTarget::Next).is_none());
    }
}
