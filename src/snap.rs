//! Snapping windows to half the screen, and moving/resizing them from the
//! keyboard.
//!
//! A snapped window fills the left or right half of its output's usable area
//! (below its title bar). It remembers where it was before (`SnapState`), so
//! snapping the same side again, Mod+Down, or dragging it away restores that.
//! xdg windows are told they're tiled (`TiledLeft`/`TiledRight` plus top and
//! bottom), so client-side decorations can drop their shadows.
//!
//! Dragging a window with the pointer at the left/right edge of an output
//! previews the half it would snap to (`Pila::snap_preview`, drawn above the
//! windows); the top edge previews maximizing. Releasing applies it
//! (`move_grab.rs`).

use std::cell::Cell;

use smithay::{
    desktop::Window,
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    utils::{Logical, Point, Rectangle},
};

use crate::{decoration, state::fill_state, Pila};

/// How far keyboard moves and resizes go per press.
pub const KEYBOARD_STEP: i32 = 40;
/// Pointer distance from an output's side edges that counts as "at the edge".
const EDGE: f64 = 2.0;
/// The same for the top edge, wider: nested, winit draws the pila window's
/// title bar right above the output, and a button released over it never
/// reaches pila, so the preview must show before the pointer gets there.
const EDGE_TOP: f64 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// Where a dragged window would go if released now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapTarget {
    Half(Side),
    Maximize,
}

/// A snapped window's side and where it was before.
#[derive(Default)]
struct SnapState(Cell<Option<(Side, Rectangle<i32, Logical>)>>);

/// Where a snapped window was before snapping; `None` if it isn't snapped.
pub fn snapped_from(window: &Window) -> Option<Rectangle<i32, Logical>> {
    snap_state(window).map(|(_, before)| before)
}

fn snap_state(window: &Window) -> Option<(Side, Rectangle<i32, Logical>)> {
    window
        .user_data()
        .get::<SnapState>()
        .and_then(|s| s.0.get())
}

fn set_snap_state(window: &Window, state: Option<(Side, Rectangle<i32, Logical>)>) {
    window.user_data().insert_if_missing(SnapState::default);
    if let Some(s) = window.user_data().get::<SnapState>() {
        s.0.set(state);
    }
}

/// The width a snapped window had before snapping.
pub fn snapped_before_width(window: &Window) -> Option<i32> {
    snap_state(window).map(|(_, before)| before.size.w)
}

/// The side a window is snapped to, if any.
pub fn snapped_side(window: &Window) -> Option<Side> {
    snap_state(window).map(|(side, _)| side)
}

/// The rectangle (window geometry, below a `bar`-high title bar) for the
/// `side` half of `area`.
pub fn half(area: Rectangle<i32, Logical>, side: Side, bar: i32) -> Rectangle<i32, Logical> {
    let left_w = area.size.w / 2;
    let (x, w) = match side {
        Side::Left => (area.loc.x, left_w),
        Side::Right => (area.loc.x + left_w, area.size.w - left_w),
    };
    Rectangle::new((x, area.loc.y + bar).into(), (w, area.size.h - bar).into())
}

/// Which snap the pointer at `pos` asks for, given the output it's on.
pub fn edge_target(
    output: Rectangle<i32, Logical>,
    pos: Point<f64, Logical>,
) -> Option<SnapTarget> {
    let geo = output.to_f64();
    if pos.x <= geo.loc.x + EDGE {
        Some(SnapTarget::Half(Side::Left))
    } else if pos.x >= geo.loc.x + geo.size.w - 1.0 - EDGE {
        Some(SnapTarget::Half(Side::Right))
    } else if pos.y <= geo.loc.y + EDGE_TOP {
        Some(SnapTarget::Maximize)
    } else {
        None
    }
}

impl Pila {
    /// The window's current rectangle: its location and the size it was
    /// last asked to have (or has).
    pub fn window_rect(&self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        let loc = self.space.element_location(window)?;
        let size = window
            .toplevel()
            .and_then(|t| t.with_pending_state(|s| s.size))
            .unwrap_or_else(|| window.geometry().size);
        Some(Rectangle::new(loc, size))
    }

    /// Give a window this geometry (location and size), tiled to `side` or
    /// floating.
    fn apply_rect(&mut self, window: &Window, rect: Rectangle<i32, Logical>, side: Option<Side>) {
        if let Some(toplevel) = window.toplevel() {
            toplevel.with_pending_state(|state| {
                use xdg_toplevel::State::*;
                for s in [TiledLeft, TiledRight, TiledTop, TiledBottom] {
                    state.states.unset(s);
                }
                if let Some(side) = side {
                    state.states.set(TiledTop);
                    state.states.set(TiledBottom);
                    state.states.set(match side {
                        Side::Left => TiledLeft,
                        Side::Right => TiledRight,
                    });
                }
                state.size = Some(rect.size);
            });
            if toplevel.is_initial_configure_sent() {
                toplevel.send_pending_configure();
            }
        } else if let Some(x11) = window.x11_surface() {
            let _ = x11.configure(rect);
        }
        self.space.map_element(window.clone(), rect.loc, true);
        self.x11_window_moved(window);
        self.queue_redraw();
    }

    /// Snap `window` to `side`, or restore it if it's already there.
    pub fn snap_window(&mut self, window: &Window, side: Side) {
        if self.is_locked() {
            return;
        }
        if snapped_side(window) == Some(side) {
            self.unsnap_window(window, None);
            return;
        }
        // Where it goes back to: what it remembered before an earlier snap
        // or maximize, else where it is now.
        let before = match (snap_state(window), fill_state(window)) {
            (Some((_, before)), _) => Some(before),
            (None, (false, false)) => self.window_rect(window),
            (None, _) => {
                crate::state::restore_geometry(window).or_else(|| self.window_rect(window))
            }
        };
        let Some(before) = before else {
            return;
        };
        self.animate_geometry(window);
        let (maximized, fullscreen) = fill_state(window);
        if maximized || fullscreen {
            self.clear_fill_state(window);
        }
        let Some(area) = self.usable_area_for(window) else {
            return;
        };
        let rect = half(area, side, decoration::layout_offset(window, false));
        set_snap_state(window, Some((side, before)));
        self.apply_rect(window, rect, Some(side));
    }

    /// Back to the size it had before snapping, at `at` (or where it was).
    pub fn unsnap_window(&mut self, window: &Window, at: Option<Point<i32, Logical>>) {
        let Some((_, before)) = snap_state(window) else {
            return;
        };
        set_snap_state(window, None);
        // Dragged out (`at`): it follows the pointer, no animation.
        if at.is_none() {
            self.animate_geometry(window);
        }
        let rect = Rectangle::new(at.unwrap_or(before.loc), before.size);
        self.apply_rect(window, rect, None);
    }

    /// The window stops being snapped but keeps its current geometry (it
    /// was resized, maximized, ...). Returns where it was before snapping.
    pub fn forget_snap(&mut self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        let (_, before) = snap_state(window)?;
        set_snap_state(window, None);
        if let Some(toplevel) = window.toplevel() {
            toplevel.with_pending_state(|state| {
                use xdg_toplevel::State::*;
                for s in [TiledLeft, TiledRight, TiledTop, TiledBottom] {
                    state.states.unset(s);
                }
            });
        }
        Some(before)
    }

    /// Mod+Down: undo a snap or a maximize.
    pub fn restore_window_size(&mut self, window: &Window) {
        if snapped_side(window).is_some() {
            self.unsnap_window(window, None);
        } else if fill_state(window).0 {
            let (_, fullscreen) = fill_state(window);
            self.set_fill_state(window, false, fullscreen);
        }
    }

    /// Mod+Shift+arrows: move by `delta`. A snapped window is unsnapped
    /// first (keeping its place); maximized/fullscreen ones don't move.
    pub fn move_window_by(&mut self, window: &Window, delta: Point<i32, Logical>) {
        let (maximized, fullscreen) = fill_state(window);
        if maximized || fullscreen || self.is_locked() {
            return;
        }
        if let Some(rect) = self.window_rect(window) {
            if snapped_side(window).is_some() {
                self.unsnap_window(window, Some(rect.loc + delta));
            } else {
                self.apply_rect(window, Rectangle::new(rect.loc + delta, rect.size), None);
            }
        }
    }

    /// Mod+Ctrl+arrows: grow or shrink by `delta` (width, height; right and
    /// bottom edge). Not a `Size`: shrinking makes it negative.
    pub fn resize_window_by(&mut self, window: &Window, delta: (i32, i32)) {
        let (maximized, fullscreen) = fill_state(window);
        if maximized || fullscreen || self.is_locked() {
            return;
        }
        self.forget_snap(window);
        if let Some(rect) = self.window_rect(window) {
            let size = (
                (rect.size.w + delta.0).max(KEYBOARD_STEP),
                (rect.size.h + delta.1).max(KEYBOARD_STEP),
            );
            self.apply_rect(window, Rectangle::new(rect.loc, size.into()), None);
        }
    }

    /// Mod+C: center in the output's usable area.
    pub fn center_window(&mut self, window: &Window) {
        let (maximized, fullscreen) = fill_state(window);
        if maximized || fullscreen || self.is_locked() {
            return;
        }
        self.forget_snap(window);
        let (Some(area), Some(rect)) = (self.usable_area_for(window), self.window_rect(window))
        else {
            return;
        };
        let bar = decoration::layout_offset(window, false);
        let loc = crate::state::centered_location(area, rect.size, bar);
        self.apply_rect(window, Rectangle::new(loc, rect.size), None);
    }

    /// The rectangle the window would get for `target` (for the preview).
    pub fn snap_target_rect(
        &self,
        window: &Window,
        target: SnapTarget,
    ) -> Option<Rectangle<i32, Logical>> {
        let area = self.usable_area_for(window)?;
        let bar = decoration::layout_offset(window, false);
        match target {
            SnapTarget::Half(side) => {
                // The preview covers the title bar too.
                let rect = half(area, side, bar);
                Some(Rectangle::new(
                    (rect.loc.x, rect.loc.y - bar).into(),
                    (rect.size.w, rect.size.h + bar).into(),
                ))
            }
            // Maximized, the bar is inside the area already.
            SnapTarget::Maximize => Some(area),
        }
    }

    /// Release at the end of a drag: apply what the preview showed.
    pub fn apply_snap_target(&mut self, window: &Window, target: SnapTarget) {
        tracing::debug!(?target, rect = ?self.window_rect(window), "drop at an output edge");
        match target {
            SnapTarget::Half(side) => self.snap_window(window, side),
            SnapTarget::Maximize => {
                let before = self.forget_snap(window);
                self.set_fill_state(window, true, false);
                if let Some(before) = before {
                    crate::state::set_restore_geometry(window, before);
                }
            }
        }
        tracing::debug!(fill = ?fill_state(window), rect = ?self.window_rect(window), "after the drop");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halves_split_the_area_below_the_title_bar() {
        let area = Rectangle::new((0, 30).into(), (1921, 1050).into());
        let left = half(area, Side::Left, 28);
        let right = half(area, Side::Right, 28);
        assert_eq!(left, Rectangle::new((0, 58).into(), (960, 1022).into()));
        assert_eq!(right, Rectangle::new((960, 58).into(), (961, 1022).into()));
    }

    #[test]
    fn edges_pick_the_snap_target() {
        let output = Rectangle::new((1920, 0).into(), (2560, 1440).into());
        let at = |x, y| edge_target(output, (x, y).into());
        assert_eq!(at(1920.0, 700.0), Some(SnapTarget::Half(Side::Left)));
        assert_eq!(at(4479.0, 700.0), Some(SnapTarget::Half(Side::Right)));
        assert_eq!(at(3000.0, 0.0), Some(SnapTarget::Maximize));
        assert_eq!(at(3000.0, 9.0), Some(SnapTarget::Maximize));
        assert_eq!(at(3000.0, 700.0), None);
    }
}
