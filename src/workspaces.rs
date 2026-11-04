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

/// The workspace a window off the `Space` belongs to (parked or minimized),
/// and where a parked window was.
#[derive(Default)]
struct OnWorkspace {
    index: Cell<usize>,
    at: Cell<Point<i32, Logical>>,
}

fn on_workspace(window: &Window) -> &OnWorkspace {
    window.user_data().insert_if_missing(OnWorkspace::default);
    window.user_data().get::<OnWorkspace>().unwrap()
}

/// Remember that `window`, now off the `Space`, belongs to workspace `index`.
pub fn set_workspace(window: &Window, index: usize) {
    on_workspace(window).index.set(index);
}

/// Whether `window` is an X11 menu or tooltip (on no workspace).
fn is_override_redirect(window: &Window) -> bool {
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
        let off_space = self.workspaces.parked.contains(window) || self.is_minimized(window);
        match window.user_data().get::<OnWorkspace>() {
            Some(data) if off_space => data.index.get(),
            _ => self.workspaces.active,
        }
    }

    /// Parked windows and minimized ones: managed windows off the `Space`.
    pub fn off_space_windows(&self) -> impl Iterator<Item = &Window> {
        self.workspaces.parked.iter().chain(self.minimized.iter())
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
        if index == active || self.grabbed_window().as_ref() == Some(&window) {
            return;
        }
        debug!(index, "moving window to workspace");
        // A Mod+Tab in progress must not come back to the parked window.
        self.end_switch();
        self.park(&window, index);
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
    fn grabbed_window(&self) -> Option<Window> {
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
        let carried = self.grabbed_window();
        let from = self.workspaces.active;
        let shown: Vec<Window> = self
            .space
            .elements()
            .filter(|w| !is_override_redirect(w) && Some(*w) != carried.as_ref())
            .cloned()
            .collect();
        for window in shown {
            self.park(&window, from);
        }
        self.workspaces.active = index;
        self.unpark_active();
        match carried {
            Some(window) => self.focus_window(Some(window), SERIAL_COUNTER.next_serial()),
            None => self.focus_recent_window(),
        }
        self.refresh_pointer_focus();
        self.refresh_workspaces();
        self.queue_redraw();
    }

    /// Take a mapped window off the `Space` onto workspace `index`.
    fn park(&mut self, window: &Window, index: usize) {
        let Some(loc) = self.space.element_location(window) else {
            return;
        };
        let data = on_workspace(window);
        data.index.set(index);
        data.at.set(loc);
        self.space.unmap_elem(window);
        // The space only deactivates windows it holds.
        window.set_activated(false);
        if let Some(toplevel) = window.toplevel() {
            toplevel.send_pending_configure();
        }
        if let Some(x11) = window.x11_surface() {
            let _ = x11.set_suspended(true);
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
            let loc = on_workspace(&window).at.get();
            self.space.map_element(window.clone(), loc, false);
            if let Some(x11) = window.x11_surface() {
                let _ = x11.set_suspended(false);
            }
            self.x11_window_moved(&window);
        }
        self.fixup_windows();
    }

    /// Focus the most recently used window on screen (or the top one, or
    /// none), e.g. after the focused one was minimized or left the workspace.
    pub(crate) fn focus_recent_window(&mut self) {
        let next = self
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

    /// Forget a parked window whose client went away.
    pub fn forget_parked(&mut self, window: &Window) {
        self.workspaces.parked.retain(|w| w != window);
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
