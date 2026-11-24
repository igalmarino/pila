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
    desktop::Window,
    reexports::{
        calloop::timer::{TimeoutAction, Timer},
        wayland_server::protocol::wl_surface::WlSurface,
    },
    utils::{Logical, Rectangle},
    wayland::xdg_system_bell::{XdgSystemBellHandler, XdgSystemBellState},
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
    /// A timer is set to redraw it once it's over: one per window, however
    /// often it rings.
    timer: Cell<bool>,
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
    fn flash_window(&mut self, window: &Window) {
        let flash = window.user_data().get_or_insert(BellFlash::default);
        flash.until.set(Some(Instant::now() + FLASH));
        // Where it shows: its window, title bar included.
        self.queue_redraw_window(window);
        if flash.timer.replace(true) {
            return;
        }
        // Redraw it once it's over, so it disappears: later than set if it
        // rang again meanwhile.
        let window = window.clone();
        let inserted =
            self.loop_handle
                .insert_source(Timer::from_duration(FLASH), move |_, _, state| {
                    let Some(flash) = window.user_data().get::<BellFlash>() else {
                        return TimeoutAction::Drop;
                    };
                    let now = Instant::now();
                    match flash.until.get() {
                        Some(until) if until > now => TimeoutAction::ToDuration(until - now),
                        _ => {
                            flash.timer.set(false);
                            state.queue_redraw_window(&window);
                            TimeoutAction::Drop
                        }
                    }
                });
        if inserted.is_err() {
            flash.timer.set(false);
        }
    }
}

impl XdgSystemBellHandler for Pila {
    fn ring(&mut self, surface: Option<WlSurface>) {
        if self.is_locked() {
            return;
        }
        let window = surface
            .as_ref()
            .and_then(|s| self.window_of_surface(s))
            .or_else(|| self.focused_window());
        debug!(window = window.is_some(), "bell");
        if let Some(window) = window {
            self.flash_window(&window);
        }
    }
}
