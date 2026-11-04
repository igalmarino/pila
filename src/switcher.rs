//! Switching windows by recent use (Mod+Tab).
//!
//! `Pila::focus_history` lists windows most recently focused first; every
//! focus change through `focus_window` moves the window to the front, except
//! while switching. The first `cycle-focus` takes a snapshot of that order and
//! focuses (and raises) the second window; further presses step on, and
//! `cycle-focus-back` steps back. Releasing all of Ctrl/Alt/Super ends the
//! switch (`Pila::end_switch`, from the keyboard path): the chosen window
//! becomes the most recent. So a quick Mod+Tab toggles between the last two
//! windows.

use smithay::{
    desktop::Window,
    utils::{IsAlive, Serial},
};

use crate::Pila;

/// An Mod+Tab in progress.
pub struct Switch {
    /// Snapshot of the candidates, most recent first.
    order: Vec<Window>,
    index: usize,
    /// The current pick was minimized and is only shown while selected.
    shown_minimized: Option<Window>,
}

/// Whether `window` belongs in the switcher: X11 menus and tooltips don't.
fn switchable(window: &Window) -> bool {
    window
        .x11_surface()
        .is_none_or(|x11| !x11.is_override_redirect())
}

/// The next index when stepping `forward` or back through `len` windows.
pub fn step(index: usize, len: usize, forward: bool) -> usize {
    if len == 0 {
        return 0;
    }
    if forward {
        (index + 1) % len
    } else {
        (index + len - 1) % len
    }
}

impl Pila {
    /// Record that `window` got focus (called by `focus_window`).
    pub fn touch_focus_history(&mut self, window: &Window) {
        if self.switch.is_some() || !switchable(window) {
            return;
        }
        self.focus_history.retain(|w| w != window);
        self.focus_history.insert(0, window.clone());
    }

    /// Switchable windows on the active workspace (mapped or minimized),
    /// most recently focused first; windows never focused follow in stacking
    /// order (top first).
    fn switch_order(&mut self) -> Vec<Window> {
        let mut mapped: Vec<Window> = self.space.elements().rev().cloned().collect();
        let active = self.workspaces.active;
        mapped.extend(
            self.minimized
                .iter()
                .filter(|w| self.workspace_of(w) == active)
                .cloned(),
        );
        self.focus_history.retain(IsAlive::alive);
        let mut order: Vec<Window> = self
            .focus_history
            .iter()
            .filter(|w| mapped.contains(w))
            .cloned()
            .collect();
        let never_focused: Vec<Window> = mapped
            .into_iter()
            .filter(|w| switchable(w) && !order.contains(w))
            .collect();
        order.extend(never_focused);
        order
    }

    /// One step of Mod+Tab (or Mod+Shift+Tab when `!forward`).
    pub fn cycle_focus(&mut self, forward: bool, serial: Serial) {
        let mut switch = match self.switch.take() {
            Some(switch) => switch,
            None => {
                let order = self.switch_order();
                // Nothing to switch to: no window, or only the shown one. A
                // lone minimized window is still brought back.
                let only_shown = order.len() == 1 && !self.is_minimized(&order[0]);
                if order.is_empty() || only_shown {
                    return;
                }
                // Starting point: the focused window if it's in the list;
                // with none focused (e.g. the last shown one was just
                // minimized), the first step lands on the most recent.
                let focused = self.focused_window();
                let index = focused
                    .and_then(|f| order.iter().position(|w| *w == f))
                    .unwrap_or(if forward { order.len() - 1 } else { 0 });
                Switch {
                    order,
                    index,
                    shown_minimized: None,
                }
            }
        };
        // Skip windows that went away since the snapshot.
        switch.order.retain(|w| w.alive());
        if switch.order.is_empty() {
            return;
        }
        // A minimized window passed over goes back to being minimized.
        if let Some(previous) = switch.shown_minimized.take() {
            self.hide_window(&previous);
        }
        switch.index = step(switch.index, switch.order.len(), forward);
        let window = switch.order[switch.index].clone();
        if self.is_minimized(&window) {
            self.show_window(&window);
            switch.shown_minimized = Some(window.clone());
        }
        // `focus_window` must not reorder the history mid-switch.
        self.switch = Some(switch);
        self.focus_window(Some(window), serial);
    }

    /// The modifiers were released: the window switched to becomes the most
    /// recent one.
    pub fn end_switch(&mut self) {
        let Some(switch) = self.switch.take() else {
            return;
        };
        if let Some(window) = switch.order.get(switch.index).filter(|w| w.alive()) {
            let window = window.clone();
            self.touch_focus_history(&window);
        }
    }

    /// Forget a window that was unmapped or destroyed.
    pub fn forget_focus_history(&mut self, window: &Window) {
        self.focus_history.retain(|w| w != window);
    }
}

#[cfg(test)]
mod tests {
    use super::step;

    #[test]
    fn steps_wrap_around_both_ways() {
        assert_eq!(step(0, 3, true), 1);
        assert_eq!(step(2, 3, true), 0);
        assert_eq!(step(0, 3, false), 2);
        assert_eq!(step(1, 3, false), 0);
        assert_eq!(step(0, 0, true), 0);
    }
}
