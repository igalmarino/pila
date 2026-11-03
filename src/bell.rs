//! The system bell (xdg-system-bell-v1): a terminal's `\a`, an app's error
//! beep. pila's bell is visual: the window that rang (or, when the client
//! names none, the focused one) flashes briefly, title bar included. The
//! flash is drawn just in front of that window (`render.rs`), so windows
//! stacked above it stay unaffected. X11 apps' bells don't reach pila
//! (Xwayland keeps them).

use std::{
    cell::{Cell, RefCell},
    time::{Duration, Instant},
};

use smithay::{
    backend::renderer::element::solid::SolidColorBuffer,
    delegate_xdg_system_bell,
    desktop::Window,
    reexports::{
        calloop::timer::{TimeoutAction, Timer},
        wayland_server::protocol::wl_surface::WlSurface,
    },
    utils::{Logical, Rectangle},
    wayland::{
        compositor::get_parent,
        xdg_system_bell::{XdgSystemBellHandler, XdgSystemBellState},
    },
};
use tracing::debug;

use crate::{decoration, Pila};

/// How long a window flashes.
const FLASH: Duration = Duration::from_millis(150);
/// A light veil (premultiplied: white at 18%).
const FLASH_COLOR: [f32; 4] = [0.18, 0.18, 0.18, 0.18];

/// A window's flash: its buffer (each window needs its own, element ids
/// must be unique in a frame) and when it ends.
#[derive(Default)]
struct BellFlash {
    buffer: RefCell<SolidColorBuffer>,
    until: Cell<Option<Instant>>,
}

/// The flash to draw over `window` this frame, if it's flashing: a buffer
/// and the rectangle it covers (window geometry plus title bar).
pub fn flash(
    window: &Window,
    location: smithay::utils::Point<i32, Logical>,
) -> Option<(SolidColorBuffer, Rectangle<i32, Logical>)> {
    let flash = window.user_data().get::<BellFlash>()?;
    if flash
        .until
        .get()
        .is_none_or(|until| Instant::now() >= until)
    {
        return None;
    }
    let mut rect = Rectangle::new(location, window.geometry().size);
    if let Some(bar) = decoration::title_bar_rect(window, location) {
        rect = rect.merge(bar);
    }
    let mut buffer = flash.buffer.borrow_mut();
    buffer.update(rect.size, FLASH_COLOR);
    Some((buffer.clone(), rect))
}

/// Create the global. Called once from `init_globals`.
pub fn init_global(dh: &smithay::reexports::wayland_server::DisplayHandle) {
    XdgSystemBellState::new::<Pila>(dh);
}

impl Pila {
    /// The window `surface` belongs to (a subsurface or popup counts as its
    /// window's).
    fn window_of(&self, surface: &WlSurface) -> Option<Window> {
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        let root = self.popup_root(&root).unwrap_or(root);
        self.window_for_surface(&root)
    }

    fn flash_window(&mut self, window: &Window) {
        window.user_data().insert_if_missing(BellFlash::default);
        let Some(flash) = window.user_data().get::<BellFlash>() else {
            return;
        };
        flash.until.set(Some(Instant::now() + FLASH));
        self.queue_redraw();
        // Redraw again once it's over, so it disappears.
        let _ = self
            .loop_handle
            .insert_source(Timer::from_duration(FLASH), |_, _, state| {
                state.queue_redraw();
                TimeoutAction::Drop
            });
    }
}

impl XdgSystemBellHandler for Pila {
    fn ring(&mut self, surface: Option<WlSurface>) {
        if self.is_locked() {
            return;
        }
        let window = surface
            .as_ref()
            .and_then(|s| self.window_of(s))
            .or_else(|| self.focused_window());
        debug!(window = window.is_some(), "bell");
        if let Some(window) = window {
            self.flash_window(&window);
        }
    }
}
delegate_xdg_system_bell!(Pila);
