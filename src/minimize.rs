//! Minimized windows: taken out of the `Space` (not drawn, no input) and kept
//! in `Pila::minimized`, with where they were. They come back through Mod+Tab
//! (minimized windows stay in its recent-use order, `switcher.rs`, show while
//! selected, and are restored when the switch ends on them) or through a
//! taskbar using wlr-foreign-toplevel-management (`toplevel_management.rs`).
//! Where it was and its workspace are kept like a parked window's
//! (`workspaces.rs`); restored by a taskbar's unminimize, it comes back on
//! the active workspace.

use smithay::{desktop::Window, utils::SERIAL_COUNTER};

use crate::{
    workspaces::{deactivate, leave_space, space_location},
    Pila,
};

impl Pila {
    pub fn is_minimized(&self, window: &Window) -> bool {
        self.minimized.contains(window)
    }

    /// Minimize `window` (title bar button, Mod+N, or the client asking).
    /// Focus moves to the most recently used window still on screen.
    pub fn minimize_window(&mut self, window: &Window) {
        let was_focused = self.focused_window().as_ref() == Some(window);
        if !self.hide_window(window) {
            return;
        }
        if was_focused {
            self.focus_recent_window();
        }
    }

    /// Take a mapped window off screen. Returns false if it wasn't mapped.
    pub(crate) fn hide_window(&mut self, window: &Window) -> bool {
        let Some(loc) = self.space.element_location(window) else {
            return false;
        };
        leave_space(window, self.workspaces.active, loc);
        self.forget_animations(window);
        self.space.unmap_elem(window);
        deactivate(window);
        if let Some(x11) = window.x11_surface() {
            let _ = x11.set_suspended(true);
        }
        self.minimized.push(window.clone());
        self.queue_redraw();
        true
    }

    /// Put a minimized window back where it was (or centered, if that spot
    /// is no longer on any output) without focusing it.
    pub(crate) fn show_window(&mut self, window: &Window) {
        let Some(index) = self.minimized.iter().position(|w| w == window) else {
            return;
        };
        self.minimized.remove(index);
        self.space
            .map_element(window.clone(), space_location(window), false);
        if let Some(x11) = window.x11_surface() {
            let _ = x11.set_suspended(false);
        }
        // Back on screen even if its output went away meanwhile.
        self.fixup_windows();
        self.x11_window_moved(window);
        self.queue_redraw();
    }

    /// Restore and focus a minimized window (an X11 unminimize request).
    pub fn restore_window(&mut self, window: &Window) {
        self.show_window(window);
        self.focus_window(Some(window.clone()), SERIAL_COUNTER.next_serial());
    }
}
