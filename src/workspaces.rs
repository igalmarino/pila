//! Workspaces (virtual desktops): numbered sets of windows, one shown at a
//! time across all outputs. Their number is `workspaces` in the config.
//!
//! The `Space` holds exactly the active workspace's windows (plus X11
//! override-redirect windows, which belong to none). The other workspaces'
//! windows are parked in `Workspaces::parked`, back to front, each with its
//! workspace and location in its user data, like minimized windows
//! (`minimize.rs`); minimized windows remember their workspace the same way.
//! Switching parks the shown windows and maps the target's back in their old
//! order, then focuses the target's most recently used window. A window
//! being moved or resized with the pointer comes along.
//!
//! A window on all workspaces (`stacking.rs`) is never parked: it stays in the
//! `Space`, over what comes in, and a slide draws it fixed.
//!
//! Mod+Tab only goes through the active workspace's windows. Activating a
//! window elsewhere (a taskbar, xdg-activation) switches to its workspace.
//! Taskbars and panels see and switch workspaces through ext-workspace
//! (`ext_workspace.rs`).

use std::cell::Cell;

use smithay::{
    desktop::Window,
    utils::{IsAlive, Logical, Point, SERIAL_COUNTER},
};
use tracing::debug;

use crate::{
    config::WorkspaceTarget,
    grabs::{MoveSurfaceGrab, ResizeSurfaceGrab},
    stacking::{is_on_all_workspaces, leave_all_workspaces},
    Pila,
};

/// The workspaces' state (`Pila::workspaces`).
#[derive(Default)]
pub struct Workspaces {
    /// The workspace shown, counted from 0.
    pub active: usize,
    /// Windows on the other workspaces, out of the `Space`, back to front.
    pub parked: Vec<Window>,
}

/// What a window off the `Space` (parked or minimized) remembers: its
/// workspace and where it was.
#[derive(Default)]
struct OffSpace {
    index: Cell<usize>,
    at: Cell<Point<i32, Logical>>,
}

fn on_workspace(window: &Window) -> &OffSpace {
    window.user_data().insert_if_missing(OffSpace::default);
    window.user_data().get::<OffSpace>().unwrap()
}

/// Record that `window`, leaving the `Space` from `at`, is on workspace
/// `index`.
pub fn leave_space(window: &Window, index: usize, at: Point<i32, Logical>) {
    let data = on_workspace(window);
    data.index.set(index);
    data.at.set(at);
}

/// Change where `window`, off the `Space`, comes back to (e.g. it was
/// maximized meanwhile).
pub fn set_space_location(window: &Window, at: Point<i32, Logical>) {
    on_workspace(window).at.set(at);
}

/// Where `window` was when it left the `Space`.
pub fn space_location(window: &Window) -> Point<i32, Logical> {
    on_workspace(window).at.get()
}

/// Tell a window that just left the `Space` it's no longer activated: the
/// space only deactivates windows it holds.
pub fn deactivate(window: &Window) {
    window.set_activated(false);
    if let Some(toplevel) = window.toplevel() {
        toplevel.send_pending_configure();
    }
}

/// Whether `window` is an X11 menu or tooltip (on no workspace).
pub(crate) fn is_override_redirect(window: &Window) -> bool {
    window
        .x11_surface()
        .is_some_and(|x11| x11.is_override_redirect())
}

/// The workspace `target` names, from `active` out of `count`; `None` if
/// there's no such workspace.
pub fn resolve(target: WorkspaceTarget, active: usize, count: usize) -> Option<usize> {
    let index = match target {
        WorkspaceTarget::Index(index) => index,
        WorkspaceTarget::Next => active + 1,
        WorkspaceTarget::Prev => active.checked_sub(1)?,
    };
    (index < count).then_some(index)
}

impl Pila {
    /// The workspace `window` is on.
    pub fn workspace_of(&self, window: &Window) -> usize {
        if is_on_all_workspaces(window) {
            return self.workspaces.active;
        }
        let off_space = self.workspaces.parked.contains(window) || self.is_minimized(window);
        match window.user_data().get::<OffSpace>() {
            Some(data) if off_space => data.index.get(),
            _ => self.workspaces.active,
        }
    }

    /// Parked windows and minimized ones: managed windows off the `Space`.
    pub fn off_space_windows(&self) -> impl Iterator<Item = &Window> {
        self.workspaces
            .parked
            .iter()
            .chain(self.windows.minimized.iter())
    }

    /// Go to the workspace `target` names (a binding or gesture).
    pub fn go_to_workspace(&mut self, target: WorkspaceTarget) {
        let (active, count) = (self.workspaces.active, self.config.workspaces);
        if let Some(index) = resolve(target, active, count) {
            self.switch_workspace(index);
        }
    }

    /// Move the focused window to the workspace `target` names, staying on
    /// this one. Not while that window is being moved or resized.
    pub fn move_focused_to_workspace(&mut self, target: WorkspaceTarget) {
        let (active, count) = (self.workspaces.active, self.config.workspaces);
        let (Some(window), Some(index)) = (self.focused_window(), resolve(target, active, count))
        else {
            return;
        };
        let family = self.transient_family(&window);
        let grabbed = self.grabbed_window();
        if index == active || grabbed.is_some_and(|grabbed| family.contains(&grabbed)) {
            return;
        }
        debug!(index, "moving window to workspace");
        // A Mod+Tab in progress must not come back to the parked window.
        self.end_switch();
        leave_all_workspaces(&family);
        for member in &family {
            self.park(member, index);
        }
        self.focus_recent_window();
        self.refresh_pointer_focus();
        self.queue_redraw();
    }

    /// Show workspace `index`. Ignored while the session is locked.
    pub fn switch_workspace(&mut self, index: usize) {
        if self.is_locked() || index >= self.config.workspaces || index == self.workspaces.active {
            return;
        }
        debug!(index, "switching workspace");
        self.show_workspace(index);
    }

    /// If `window` is on another workspace, switch to that one (before
    /// activating the window).
    pub fn reveal_workspace_of(&mut self, window: &Window) {
        let index = self.workspace_of(window);
        if index != self.workspaces.active {
            self.switch_workspace(index);
        }
    }

    /// The window pila is moving or resizing with the pointer, if any.
    pub(crate) fn grabbed_window(&self) -> Option<Window> {
        self.seat.get_pointer()?.with_grab(|_, grab| {
            if let Some(grab) = grab.downcast_ref::<MoveSurfaceGrab>() {
                Some(grab.window.clone())
            } else {
                grab.downcast_ref::<ResizeSurfaceGrab>()
                    .map(|grab| grab.window().clone())
            }
        })?
    }

    /// Swap the shown windows for workspace `index`'s, then focus the most
    /// recent one there. A window being moved or resized comes along (so a
    /// drag can carry it to another workspace); other pointer grabs (a
    /// client's drag and drop, a click held down) carry on untouched.
    fn show_workspace(&mut self, index: usize) {
        // A Mod+Tab in progress ends on the window it's at.
        self.end_switch();
        self.dismiss_popup_grab();
        let grabbed = self.grabbed_window();
        // They stay where they are, in their order, as the others change.
        let staying: Vec<Window> = self
            .space
            .elements()
            .filter(|w| is_on_all_workspaces(w) || Some(*w) == grabbed.as_ref())
            .cloned()
            .collect();
        // Each output's picture of the windows leaving, for the slide.
        self.start_workspace_slide(index, staying.clone());
        let from = self.workspaces.active;
        let shown: Vec<Window> = self
            .space
            .elements()
            .filter(|w| !is_override_redirect(w) && !staying.contains(w))
            .cloned()
            .collect();
        for window in shown {
            self.park(&window, from);
        }
        self.workspaces.active = index;
        self.unpark_active();
        match grabbed {
            Some(window) => self.focus_window(Some(window), SERIAL_COUNTER.next_serial()),
            None => self.focus_arriving_window(),
        }
        // Over what came in, even the window that took the focus.
        self.raise_in_order(&staying);
        self.keep_on_top();
        self.refresh_pointer_focus();
        self.refresh_workspaces();
        self.queue_redraw();
    }

    /// Take a mapped window off the `Space` onto workspace `index`.
    pub(crate) fn park(&mut self, window: &Window, index: usize) {
        let Some(loc) = self.space.element_location(window) else {
            return;
        };
        leave_space(window, index, loc);
        self.forget_animations(window);
        self.space.unmap_elem(window);
        deactivate(window);
        if let Some(x11) = window.x11_surface() {
            let _ = x11.set_hidden(true);
        }
        self.workspaces.parked.push(window.clone());
    }

    /// Map the active workspace's parked windows back where they were, on
    /// top in their old order, and fit them to the outputs as they are now
    /// (one may have gone, or a panel changed).
    fn unpark_active(&mut self) {
        let active = self.workspaces.active;
        let (back, parked) = std::mem::take(&mut self.workspaces.parked)
            .into_iter()
            .partition(|w| on_workspace(w).index.get() == active);
        self.workspaces.parked = parked;
        let back: Vec<Window> = back;
        for window in back {
            self.space
                .map_element(window.clone(), space_location(&window), false);
            if let Some(x11) = window.x11_surface() {
                let _ = x11.set_hidden(false);
            }
            self.x11_window_moved(&window);
        }
        self.fixup_windows();
    }

    /// After a switch: the most recent of the workspace's own windows, or else
    /// whatever `focus_recent_window` finds.
    fn focus_arriving_window(&mut self) {
        let own = self
            .windows
            .focus_history
            .iter()
            .find(|w| !is_on_all_workspaces(w) && self.space.elements().any(|m| m == *w))
            .or_else(|| {
                self.space
                    .elements()
                    .rev()
                    .find(|w| !is_override_redirect(w) && !is_on_all_workspaces(w))
            })
            .cloned();
        match own {
            Some(window) => self.focus_window(Some(window), SERIAL_COUNTER.next_serial()),
            None => self.focus_recent_window(),
        }
    }

    /// Focus the most recently used window on screen (or the top one, or
    /// none), e.g. after the focused one was minimized or left the workspace.
    pub(crate) fn focus_recent_window(&mut self) {
        let next = self
            .windows
            .focus_history
            .iter()
            .find(|w| self.space.elements().any(|m| m == *w))
            .cloned()
            .or_else(|| {
                self.space
                    .elements()
                    .rev()
                    .find(|w| !is_override_redirect(w))
                    .cloned()
            });
        self.focus_window(next, SERIAL_COUNTER.next_serial());
    }

    /// Forget a window off the `Space` (parked or minimized) whose client
    /// went away.
    pub fn forget_off_space(&mut self, window: &Window) {
        self.workspaces.parked.retain(|w| w != window);
        self.windows.minimized.retain(|w| w != window);
        if let Some(hidden) = &mut self.windows.desktop_shown {
            hidden.windows.retain(|w| w != window);
        }
    }

    /// The number of workspaces changed (config reload): windows on
    /// workspaces that are gone move to the last one left.
    pub fn apply_workspace_count(&mut self) {
        let last = self.config.workspaces - 1;
        for window in self.off_space_windows() {
            let data = on_workspace(window);
            if data.index.get() > last {
                data.index.set(last);
            }
        }
        self.workspaces.parked.retain(IsAlive::alive);
        if self.workspaces.active > last {
            // Even while locked: the active workspace must exist.
            self.show_workspace(last);
        } else {
            // Windows moved onto the active workspace come on screen.
            self.unpark_active();
            self.refresh_pointer_focus();
            self.refresh_workspaces();
            self.queue_redraw();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::resolve;
    use crate::config::WorkspaceTarget::{Index, Next, Prev};

    #[test]
    fn targets_resolve_without_wrapping() {
        assert_eq!(resolve(Index(2), 0, 4), Some(2));
        assert_eq!(resolve(Index(4), 0, 4), None);
        assert_eq!(resolve(Next, 2, 4), Some(3));
        assert_eq!(resolve(Next, 3, 4), None);
        assert_eq!(resolve(Prev, 1, 4), Some(0));
        assert_eq!(resolve(Prev, 0, 4), None);
    }
}
