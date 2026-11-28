//! Snapping windows to half the screen, and moving/resizing them from the
//! keyboard.
//!
//! A snapped window fills a half of its output's usable area (left, right, top
//! or bottom) or a quarter (below its title bar). It remembers where it was before (`SnapState`), so
//! snapping the same side again, `restore`, or dragging it away restores that. The arrows
//! (`snap <direction>`, Mod+arrows) step from a half to a quarter and around (`step`).
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

use crate::{
    decoration,
    state::{fill_state, is_filled},
    Pila,
};

/// How far keyboard moves and resizes go per press.
pub const KEYBOARD_STEP: i32 = 40;
/// Pointer distance from an output's side edges that counts as "at the edge".
const EDGE: f64 = 2.0;
/// The same for the top edge, wider: nested, winit draws the pila window's
/// title bar right above the output, and a button released over it never
/// reaches pila, so the preview must show before the pointer gets there.
const EDGE_TOP: f64 = 10.0;
/// How near a corner, along either edge, a drag asks for a quarter.
const CORNER: f64 = 64.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Side {
    /// As `pila msg` and the action names say it.
    pub fn name(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
            Side::Top => "top",
            Side::Bottom => "bottom",
            Side::TopLeft => "top-left",
            Side::TopRight => "top-right",
            Side::BottomLeft => "bottom-left",
            Side::BottomRight => "bottom-right",
        }
    }

    fn touches_left(self) -> bool {
        !matches!(self, Side::Right | Side::TopRight | Side::BottomRight)
    }

    fn touches_right(self) -> bool {
        !matches!(self, Side::Left | Side::TopLeft | Side::BottomLeft)
    }

    fn touches_top(self) -> bool {
        !matches!(self, Side::Bottom | Side::BottomLeft | Side::BottomRight)
    }

    fn touches_bottom(self) -> bool {
        !matches!(self, Side::Top | Side::TopLeft | Side::TopRight)
    }

    /// Which part of the width and of the height it takes.
    fn spans(self) -> (Span, Span) {
        let span = |start, end| match (start, end) {
            (true, true) => Span::Whole,
            (true, false) => Span::Start,
            _ => Span::End,
        };
        (
            span(self.touches_left(), self.touches_right()),
            span(self.touches_top(), self.touches_bottom()),
        )
    }

    /// The side that takes these parts; `None` for the whole output.
    fn from_spans(h: Span, v: Span) -> Option<Side> {
        use Span::*;
        Some(match (h, v) {
            (Whole, Whole) => return None,
            (Start, Whole) => Side::Left,
            (End, Whole) => Side::Right,
            (Whole, Start) => Side::Top,
            (Whole, End) => Side::Bottom,
            (Start, Start) => Side::TopLeft,
            (End, Start) => Side::TopRight,
            (Start, End) => Side::BottomLeft,
            (End, End) => Side::BottomRight,
        })
    }
}

/// Part of a width or height: the first half, all of it, the second half.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Span {
    Start,
    Whole,
    End,
}

/// The arrow of `snap <direction>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// Where a window is, for `step`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    Floating,
    Maximized,
    Fullscreen,
    Snapped(Side),
}

/// What `snap <direction>` does to a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Snap(Side),
    Maximize,
    /// Back to the size from before the snap or maximize.
    Restore,
    Nothing,
}

/// One arrow's step on the grid of halves and quarters: left and right take
/// that half of the width (keeping the top or bottom of a quarter or half),
/// up and down go one row up or down (a half to a quarter, a quarter to a
/// half); toward the side a window is already against restores it, up past
/// the top maximizes and down past the bottom restores. From a floating
/// window up maximizes, as from a maximized one down restores.
pub fn step(from: Placement, dir: Direction) -> Step {
    use Direction::*;
    use Span::*;
    let (h, v) = match (from, dir) {
        (Placement::Snapped(side), _) => side.spans(),
        (_, Left) => return Step::Snap(Side::Left),
        (_, Right) => return Step::Snap(Side::Right),
        (Placement::Floating, Up) => return Step::Maximize,
        (Placement::Maximized, Down) => return Step::Restore,
        _ => return Step::Nothing,
    };
    let to = |h, v| Side::from_spans(h, v).map_or(Step::Maximize, Step::Snap);
    match (dir, v) {
        (Left, _) if h == Start => Step::Restore,
        (Left, _) => to(Start, v),
        (Right, _) if h == End => Step::Restore,
        (Right, _) => to(End, v),
        (Up, Start) => Step::Maximize,
        (Up, Whole) => to(h, Start),
        (Up, End) => to(h, Whole),
        (Down, End) => Step::Restore,
        (Down, Whole) => to(h, End),
        (Down, Start) if h == Whole => Step::Restore,
        (Down, Start) => to(h, Whole),
    }
}

/// Where a dragged window would go if released now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapTarget {
    Side(Side),
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
    window
        .user_data()
        .get_or_insert(SnapState::default)
        .0
        .set(state);
}

/// The side a window is snapped to, if any.
pub fn snapped_side(window: &Window) -> Option<Side> {
    snap_state(window).map(|(side, _)| side)
}

/// `area` less `gap` on each side (`gaps`): what a maximized window fills.
pub fn gapped(area: Rectangle<i32, Logical>, gap: u32) -> Rectangle<i32, Logical> {
    let gap = gap as i32;
    Rectangle::new(
        (area.loc.x + gap, area.loc.y + gap).into(),
        (
            (area.size.w - 2 * gap).max(1),
            (area.size.h - 2 * gap).max(1),
        )
            .into(),
    )
}

/// The rectangle (window geometry, below a `bar`-high title bar) for the
/// half or quarter of `area` that `side` names, `gap` from the area's edges
/// and from the other halves (`gaps`).
/// No side of `state` tiled (xdg-shell's `tiled_*`): not snapped.
fn unset_tiled(state: &mut smithay::wayland::shell::xdg::ToplevelState) {
    use xdg_toplevel::State::*;
    for s in [TiledLeft, TiledRight, TiledTop, TiledBottom] {
        state.states.unset(s);
    }
}

impl Pila {
    /// `side_rect` of `area` for `window`: under its title bar, with the
    /// configured gaps.
    fn window_side_rect(
        &self,
        window: &Window,
        area: Rectangle<i32, Logical>,
        side: Side,
    ) -> Rectangle<i32, Logical> {
        side_rect(
            area,
            side,
            decoration::layout_offset(window, false),
            self.config.gaps,
        )
    }
}

pub fn side_rect(
    area: Rectangle<i32, Logical>,
    side: Side,
    bar: i32,
    gap: u32,
) -> Rectangle<i32, Logical> {
    let area = gapped(area, gap);
    let gap = gap as i32;
    // Each at least a pixel, whatever the gap (`MAX_GAPS` on a small output).
    let left_w = ((area.size.w - gap) / 2).max(1);
    let top_h = ((area.size.h - gap) / 2).max(1);
    let (x, w) = match (side.touches_left(), side.touches_right()) {
        (true, true) => (area.loc.x, area.size.w),
        (true, false) => (area.loc.x, left_w),
        (false, _) => (
            area.loc.x + left_w + gap,
            (area.size.w - left_w - gap).max(1),
        ),
    };
    let (y, h) = match (side.touches_top(), side.touches_bottom()) {
        (true, true) => (area.loc.y, area.size.h),
        (true, false) => (area.loc.y, top_h),
        (false, _) => (area.loc.y + top_h + gap, (area.size.h - top_h - gap).max(1)),
    };
    Rectangle::new((x, y + bar).into(), (w, (h - bar).max(1)).into())
}

/// Which snap the pointer at `pos` asks for, given the output it's on.
pub fn edge_target(
    output: Rectangle<i32, Logical>,
    pos: Point<f64, Logical>,
) -> Option<SnapTarget> {
    let geo = output.to_f64();
    let (right, bottom) = (geo.loc.x + geo.size.w - 1.0, geo.loc.y + geo.size.h - 1.0);
    let at_left = pos.x <= geo.loc.x + EDGE;
    let at_right = pos.x >= right - EDGE;
    let at_top = pos.y <= geo.loc.y + EDGE_TOP;
    let at_bottom = pos.y >= bottom - EDGE;
    let near_left = pos.x <= geo.loc.x + CORNER;
    let near_right = pos.x >= right - CORNER;
    let near_top = pos.y <= geo.loc.y + CORNER;
    let near_bottom = pos.y >= bottom - CORNER;
    let corner = |side| Some(SnapTarget::Side(side));
    if (at_left && near_top) || (at_top && near_left) {
        corner(Side::TopLeft)
    } else if (at_right && near_top) || (at_top && near_right) {
        corner(Side::TopRight)
    } else if (at_left && near_bottom) || (at_bottom && near_left) {
        corner(Side::BottomLeft)
    } else if (at_right && near_bottom) || (at_bottom && near_right) {
        corner(Side::BottomRight)
    } else if at_left {
        corner(Side::Left)
    } else if at_right {
        corner(Side::Right)
    } else if at_top {
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
    /// floating, and raise it.
    fn apply_rect(&mut self, window: &Window, rect: Rectangle<i32, Logical>, side: Option<Side>) {
        self.place_rect(window, rect, side, true);
    }

    /// `apply_rect`, raising the window only with `raise`.
    fn place_rect(
        &mut self,
        window: &Window,
        rect: Rectangle<i32, Logical>,
        side: Option<Side>,
        raise: bool,
    ) {
        if let Some(toplevel) = window.toplevel() {
            toplevel.with_pending_state(|state| {
                use xdg_toplevel::State::*;
                unset_tiled(state);
                if let Some(side) = side {
                    if side.touches_top() {
                        state.states.set(TiledTop);
                    }
                    if side.touches_bottom() {
                        state.states.set(TiledBottom);
                    }
                    if side.touches_left() {
                        state.states.set(TiledLeft);
                    }
                    if side.touches_right() {
                        state.states.set(TiledRight);
                    }
                }
                state.size = Some(rect.size);
            });
            if toplevel.is_initial_configure_sent() {
                toplevel.send_pending_configure();
            }
        } else if let Some(x11) = window.x11_surface() {
            let _ = x11.configure(rect);
        }
        if raise || self.space.element_location(window).is_none() {
            self.map_window(window.clone(), rect.loc, true);
            self.keep_transients_above(window);
        } else {
            self.move_window(window, rect.loc);
        }
        self.x11_window_moved(window);
        self.queue_redraw_window(window);
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
        if is_filled(window) {
            self.clear_fill_state(window);
        }
        let Some(area) = self.usable_area_for(window) else {
            return;
        };
        let rect = self.window_side_rect(window, area, side);
        set_snap_state(window, Some((side, before)));
        self.apply_rect(window, rect, Some(side));
    }

    /// Snapped windows on the `Space` to their half or quarter of the usable
    /// area again, after it changed (a panel came, resized or went; the
    /// output was resized), as `refit_to_usable_area` does maximized ones:
    /// an app snapped at login before the bar was up stays under it
    /// otherwise. Where they are in the stack stays.
    pub(crate) fn refit_snapped(&mut self) {
        let windows: Vec<(Window, Side)> = self
            .space
            .elements()
            .filter_map(|window| snapped_side(window).map(|side| (window.clone(), side)))
            .collect();
        for (window, side) in windows {
            let Some(area) = self.usable_area_for(&window) else {
                continue;
            };
            let rect = self.window_side_rect(&window, area, side);
            if self.window_rect(&window) != Some(rect) {
                self.place_rect(&window, rect, Some(side), false);
            }
        }
    }

    /// If `window` is snapped: the same side of `area`, on another output, and it
    /// goes back to `moved(where it was before)`. Whether it was snapped.
    pub(crate) fn move_snapped_to(
        &mut self,
        window: &Window,
        area: Rectangle<i32, Logical>,
        moved: impl Fn(Rectangle<i32, Logical>) -> Point<i32, Logical>,
    ) -> bool {
        let Some((side, mut before)) = snap_state(window) else {
            return false;
        };
        before.loc = moved(before);
        self.animate_geometry(window);
        set_snap_state(window, Some((side, before)));
        let rect = self.window_side_rect(window, area, side);
        self.apply_rect(window, rect, Some(side));
        true
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
            toplevel.with_pending_state(unset_tiled);
        }
        Some(before)
    }

    /// `snap <direction>` (Mod+arrows): one `step` from where it is.
    pub fn snap_toward(&mut self, window: &Window, dir: Direction) {
        if self.is_locked() {
            return;
        }
        let (maximized, fullscreen) = fill_state(window);
        let from = match snapped_side(window) {
            Some(side) => Placement::Snapped(side),
            None if fullscreen => Placement::Fullscreen,
            None if maximized => Placement::Maximized,
            None => Placement::Floating,
        };
        match step(from, dir) {
            Step::Snap(side) => self.snap_window(window, side),
            Step::Maximize => self.set_fill_state(window, true, false),
            Step::Restore => self.restore_window_size(window),
            Step::Nothing => {}
        }
    }

    /// The `restore` action: undo a snap or a maximize.
    pub fn restore_window_size(&mut self, window: &Window) {
        if snapped_side(window).is_some() {
            self.unsnap_window(window, None);
        } else if let (true, fullscreen) = fill_state(window) {
            self.set_fill_state(window, false, fullscreen);
        }
    }

    /// Mod+Shift+arrows: move by `delta`. A snapped window is unsnapped
    /// first (keeping its place); maximized/fullscreen ones don't move.
    pub fn move_window_by(&mut self, window: &Window, delta: Point<i32, Logical>) {
        if is_filled(window) || self.is_locked() {
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
        if is_filled(window) || self.is_locked() {
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
        if is_filled(window) || self.is_locked() {
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
            SnapTarget::Side(side) => {
                // The preview covers the title bar too.
                let rect = side_rect(area, side, bar, self.config.gaps);
                Some(Rectangle::new(
                    (rect.loc.x, rect.loc.y - bar).into(),
                    (rect.size.w, rect.size.h + bar).into(),
                ))
            }
            // Maximized, the bar is inside the area already.
            SnapTarget::Maximize => Some(gapped(area, self.config.gaps)),
        }
    }

    /// Release at the end of a drag: apply what the preview showed.
    pub fn apply_snap_target(&mut self, window: &Window, target: SnapTarget) {
        tracing::debug!(?target, rect = ?self.window_rect(window), "drop at an output edge");
        match target {
            SnapTarget::Side(side) => self.snap_window(window, side),
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
        let left = side_rect(area, Side::Left, 28, 0);
        let right = side_rect(area, Side::Right, 28, 0);
        assert_eq!(left, Rectangle::new((0, 58).into(), (960, 1022).into()));
        assert_eq!(right, Rectangle::new((960, 58).into(), (961, 1022).into()));
        // Top and bottom: the whole width, each its title bar.
        let top = side_rect(area, Side::Top, 28, 0);
        let bottom = side_rect(area, Side::Bottom, 28, 0);
        assert_eq!(top, Rectangle::new((0, 58).into(), (1921, 497).into()));
        assert_eq!(bottom, Rectangle::new((0, 583).into(), (1921, 497).into()));
    }

    #[test]
    fn gaps_go_around_and_between_halves_and_quarters() {
        let area = Rectangle::new((0, 30).into(), (1920, 1050).into());
        assert_eq!(
            gapped(area, 10),
            Rectangle::new((10, 40).into(), (1900, 1030).into())
        );
        let left = side_rect(area, Side::Left, 28, 10);
        let right = side_rect(area, Side::Right, 28, 10);
        // 1900 less the gap between them, in two.
        assert_eq!(left, Rectangle::new((10, 68).into(), (945, 1002).into()));
        assert_eq!(right, Rectangle::new((965, 68).into(), (945, 1002).into()));
        let top_left = side_rect(area, Side::TopLeft, 28, 10);
        let bottom_left = side_rect(area, Side::BottomLeft, 28, 10);
        assert_eq!(top_left, Rectangle::new((10, 68).into(), (945, 482).into()));
        assert_eq!(
            bottom_left,
            Rectangle::new((10, 588).into(), (945, 482).into())
        );
        // Never smaller than a pixel, however large the gap.
        let tiny = Rectangle::new((0, 0).into(), (100, 100).into());
        assert!(!side_rect(tiny, Side::Left, 28, 128).is_empty());
    }

    #[test]
    fn edges_pick_the_snap_target() {
        let output = Rectangle::new((1920, 0).into(), (2560, 1440).into());
        let at = |x, y| edge_target(output, (x, y).into());
        assert_eq!(at(1920.0, 700.0), Some(SnapTarget::Side(Side::Left)));
        assert_eq!(at(4479.0, 700.0), Some(SnapTarget::Side(Side::Right)));
        assert_eq!(at(3000.0, 0.0), Some(SnapTarget::Maximize));
        assert_eq!(at(3000.0, 9.0), Some(SnapTarget::Maximize));
        assert_eq!(at(3000.0, 700.0), None);
        // Near a corner, along either edge.
        let corner = |side| Some(SnapTarget::Side(side));
        assert_eq!(at(1920.0, 0.0), corner(Side::TopLeft));
        assert_eq!(at(1920.0, 60.0), corner(Side::TopLeft));
        assert_eq!(at(1980.0, 0.0), corner(Side::TopLeft));
        assert_eq!(at(4479.0, 0.0), corner(Side::TopRight));
        assert_eq!(at(1920.0, 1439.0), corner(Side::BottomLeft));
        assert_eq!(at(1980.0, 1439.0), corner(Side::BottomLeft));
        assert_eq!(at(4479.0, 1439.0), corner(Side::BottomRight));
        // Along the bottom away from the corners: nothing.
        assert_eq!(at(3000.0, 1439.0), None);
    }

    #[test]
    fn arrows_step_through_halves_and_quarters() {
        use Direction::{Down, Left as L, Right as R, Up};
        use Placement::*;
        use Side::*;
        use Step::{Maximize, Nothing, Restore, Snap};
        let cases = [
            // Floating, maximized, fullscreen: left and right take a half.
            (Floating, [Snap(Left), Snap(Right), Maximize, Nothing]),
            (Maximized, [Snap(Left), Snap(Right), Nothing, Restore]),
            (Fullscreen, [Snap(Left), Snap(Right), Nothing, Nothing]),
            // Halves.
            (
                Snapped(Left),
                [Restore, Snap(Right), Snap(TopLeft), Snap(BottomLeft)],
            ),
            (
                Snapped(Right),
                [Snap(Left), Restore, Snap(TopRight), Snap(BottomRight)],
            ),
            (
                Snapped(Top),
                [Snap(TopLeft), Snap(TopRight), Maximize, Restore],
            ),
            (
                Snapped(Bottom),
                [Snap(BottomLeft), Snap(BottomRight), Maximize, Restore],
            ),
            // Quarters.
            (
                Snapped(TopLeft),
                [Restore, Snap(TopRight), Maximize, Snap(Left)],
            ),
            (
                Snapped(TopRight),
                [Snap(TopLeft), Restore, Maximize, Snap(Right)],
            ),
            (
                Snapped(BottomLeft),
                [Restore, Snap(BottomRight), Snap(Left), Restore],
            ),
            (
                Snapped(BottomRight),
                [Snap(BottomLeft), Restore, Snap(Right), Restore],
            ),
        ];
        for (from, expected) in cases {
            for (dir, want) in [L, R, Up, Down].into_iter().zip(expected) {
                assert_eq!(step(from, dir), want, "{from:?} {dir:?}");
            }
        }
    }
}
