//! Urgency: a window asking for the user's attention without taking the
//! focus, until it gets it.
//!
//! A window turns urgent when its client activates it (xdg-activation) with a
//! token that carries no serial: no user input asked for it, so it's a "look
//! at me" (a chat message, a finished download), the convention niri follows
//! too (wayland-protocols issue 150). An X11 window turns urgent while its
//! `WM_HINTS` have the urgency hint. A window that has the keyboard focus is
//! never urgent: focusing one clears it (`SeatHandler::focus_changed`).
//!
//! It shows in an unfocused window's title bar (`[colors] bar-urgent`), as
//! ext-workspace's `urgent` state on the workspaces holding urgent windows
//! (`ext_workspace.rs`, which panels show), and in `pila msg windows`.

use std::cell::Cell;

use smithay::desktop::Window;

use crate::Pila;

/// The mark in a window's user data.
#[derive(Default)]
struct Urgent(Cell<bool>);

/// Marks a token made without a serial: it can only make its window urgent.
pub struct UrgentOnlyToken;

pub fn is_urgent(window: &Window) -> bool {
    window
        .user_data()
        .get::<Urgent>()
        .is_some_and(|urgent| urgent.0.get())
}

impl Pila {
    /// Mark `window` urgent or not; the focused window never is.
    pub fn set_urgent(&mut self, window: &Window, urgent: bool) {
        let urgent = urgent && self.focused_window().as_ref() != Some(window);
        let mark = window.user_data().get_or_insert(Urgent::default);
        if mark.0.replace(urgent) != urgent {
            tracing::debug!(
                app_id = crate::foreign_toplevel::title_and_app_id(window).1,
                urgent,
                "urgency"
            );
            self.refresh_workspaces();
            self.queue_redraw();
        }
    }

    /// The windows on any workspace that are urgent.
    pub fn urgent_windows(&self) -> impl Iterator<Item = &Window> {
        self.space
            .elements()
            .chain(self.off_space_windows())
            .filter(|window| is_urgent(window))
    }
}
