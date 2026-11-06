//! Minimized windows: taken out of the `Space` (not drawn, no input) and kept
//! in `Pila::windows.minimized`, with where they were. They come back through Mod+Tab
//! (minimized windows stay in its recent-use order, `switcher.rs`, show while
//! selected, and are restored when the switch ends on them) or through a
//! taskbar using wlr-foreign-toplevel-management (`toplevel_management.rs`).
//! Where it was and its workspace are kept like a parked window's
//! (`workspaces.rs`); restored by a taskbar's unminimize, it comes back on
//! the active workspace.

use std::cell::Cell;

use smithay::{desktop::Window, utils::SERIAL_COUNTER};

use crate::{
    workspaces::{deactivate, leave_space, space_location},
    Pila,
};

/// In a minimized window's user data: it went with its parent, and comes
/// back with it (`transients.rs`).
#[derive(Default)]
struct WentWithParent(Cell<bool>);

fn went_with_parent(window: &Window) -> &Cell<bool> {
    window
        .user_data()
        .insert_if_missing(WentWithParent::default);
    &window.user_data().get::<WentWithParent>().unwrap().0
}

impl Pila {
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
        self.space.unmap_elem(window);
        deactivate(window);
        if let Some(x11) = window.x11_surface() {
            let _ = x11.set_suspended(true);
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
        went_with_parent(window).set(false);
        self.space
            .map_element(window.clone(), space_location(window), false);
        if let Some(x11) = window.x11_surface() {
            let _ = x11.set_suspended(false);
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
