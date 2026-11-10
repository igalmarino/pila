//! Minimized windows: taken out of the `Space` (not drawn, no input) and kept
//! in `Pila::windows.minimized`, with where they were. They come back through Mod+Tab
//! (minimized windows stay in its recent-use order, `switcher.rs`, show while
//! selected, and are restored when the switch ends on them) or through a
//! taskbar using wlr-foreign-toplevel-management (`toplevel_management.rs`).
//! Where it was and its workspace are kept like a parked window's
//! (`workspaces.rs`); restored by a taskbar's unminimize, it comes back on
//! the active workspace.

use std::cell::Cell;

use smithay::{
    desktop::Window,
    utils::{IsAlive, SERIAL_COUNTER},
};
use tracing::debug;

use crate::{
    workspaces::{deactivate, leave_space, space_location},
    Pila,
};

/// In a minimized window's user data: it went with its parent, and comes
/// back with it (`transients.rs`).
#[derive(Default)]
struct WentWithParent(Cell<bool>);

fn went_with_parent(window: &Window) -> &Cell<bool> {
    &window.user_data().get_or_insert(WentWithParent::default).0
}

/// What `show-desktop` took off screen, to bring back on that workspace.
pub struct DesktopShown {
    workspace: usize,
    /// Back to front.
    pub windows: Vec<Window>,
    focused: Option<Window>,
}

impl Pila {
    /// Minimize the workspace's windows; again, bring those back in their
    /// order. Opening a window or bringing one back forgets them, so the next
    /// time starts over. Those already minimized stay so.
    pub fn show_desktop(&mut self) {
        let active = self.workspaces.active;
        if let Some(shown) = self
            .windows
            .desktop_shown
            .take()
            .filter(|shown| shown.workspace == active && !shown.windows.is_empty())
        {
            for window in &shown.windows {
                self.show_window(window);
            }
            self.keep_on_top();
            match shown.focused.filter(IsAlive::alive) {
                Some(window) => self.focus_window(Some(window), SERIAL_COUNTER.next_serial()),
                None => self.focus_recent_window(),
            }
            self.refresh_pointer_focus();
            return;
        }
        let focused = self.focused_window();
        // Their children go with them.
        let windows: Vec<Window> = self
            .space
            .elements()
            .filter(|w| {
                !crate::workspaces::is_override_redirect(w) && self.transient_parent(w).is_none()
            })
            .cloned()
            .collect();
        if windows.is_empty() {
            return;
        }
        debug!(count = windows.len(), "showing the desktop");
        for window in &windows {
            self.hide_window(window);
        }
        self.windows.desktop_shown = Some(DesktopShown {
            workspace: active,
            windows,
            focused,
        });
        self.focus_recent_window();
        self.refresh_pointer_focus();
    }

    pub fn is_minimized(&self, window: &Window) -> bool {
        self.windows.minimized.contains(window)
    }

    /// Minimize `window` (title bar button, Mod+N, or the client asking).
    /// Focus moves to the most recently used window still on screen.
    pub fn minimize_window(&mut self, window: &Window) {
        let focused = self.focused_window();
        if !self.hide_window(window) {
            return;
        }
        // It, or a child that went with it.
        if focused.is_some_and(|focused| self.is_minimized(&focused)) {
            self.focus_recent_window();
        }
    }

    /// Take a mapped window off screen, its children with it. Returns false
    /// if it wasn't mapped.
    pub(crate) fn hide_window(&mut self, window: &Window) -> bool {
        let Some(loc) = self.space.element_location(window) else {
            return false;
        };
        leave_space(window, self.workspaces.active, loc);
        // Snapshot taken while it's still mapped.
        self.animate_minimize(window);
        self.unmap_window(window);
        deactivate(window);
        if let Some(x11) = window.x11_surface() {
            let _ = x11.set_hidden(true);
        }
        self.windows.minimized.push(window.clone());
        for child in self.transient_children(window) {
            if self.hide_window(&child) {
                went_with_parent(&child).set(true);
            }
        }
        self.queue_redraw();
        true
    }

    /// Put a minimized window back where it was (or centered, if that spot
    /// is no longer on any output) without focusing it, and the children
    /// that went with it.
    pub(crate) fn show_window(&mut self, window: &Window) {
        let Some(index) = self.windows.minimized.iter().position(|w| w == window) else {
            return;
        };
        self.windows.minimized.remove(index);
        self.windows.desktop_shown = None;
        went_with_parent(window).set(false);
        self.map_window(window.clone(), space_location(window), false);
        if let Some(x11) = window.x11_surface() {
            let _ = x11.set_hidden(false);
        }
        // Back on screen even if its output went away meanwhile.
        self.fixup_windows();
        self.animate_restore(window);
        self.x11_window_moved(window);
        self.keep_transients_above(window);
        for child in self.transient_children(window) {
            if went_with_parent(&child).get() {
                self.show_window(&child);
            }
        }
        self.queue_redraw();
    }

    /// Restore and focus a minimized window (an X11 unminimize request).
    pub fn restore_window(&mut self, window: &Window) {
        self.show_window(window);
        self.focus_window(Some(window.clone()), SERIAL_COUNTER.next_serial());
    }
}
