//! The overview (`overview`, Mod+W by default): the active workspace's
//! windows spread out on their outputs, none over another, live, over the
//! dimmed wallpaper; panels (upper layer surfaces) stay as they are. A click
//! on a window, or the arrows and Enter, picks it (focused and raised as it
//! closes); Escape, a click beside the windows or any other binding closes
//! it.
//!
//! Nothing is drawn anew: each window is drawn as an animating one is, its
//! frame mapped onto its place in the overview (`animation::FrameMap`), so
//! its title bar, corners, shadow and live content come along and its frames
//! are paced by the output it's shown on (`frame::send_frames`). The places
//! are laid out from the windows as they are whenever they're asked for
//! (`Pila::overview_slots`, `arrange`): a window opening, closing or
//! resizing meanwhile takes its place at once. This module draws only what's
//! behind them: the selected one's outline and the dimming
//! (`overview_elements`).
//!
//! Opening, the windows glide from where they are to their places and the
//! wallpaper dims (`[animations] overview`); closing, they glide back from
//! the places they had then (`Overview::closing`) while it's already closed
//! to input. A glide is an animation like the others (`step_animations`
//! ends it, `animating` keeps frames coming).

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use smithay::{
    backend::renderer::{
        element::{
            solid::{SolidColorBuffer, SolidColorRenderElement},
            Kind,
        },
        gles::GlesRenderer,
    },
    desktop::Window,
    input::keyboard::Keysym,
    output::Output,
    utils::{Logical, Point, Rectangle, Scale, Size},
};

use crate::{
    animation::{lerp_rect, Animation, FrameMap, Transform, WindowEffect},
    config::{Action, AnimationKind, Colors, OverviewAction},
    render::{OffscreenWindow, PilaRenderElement, SceneWindow},
    snap::Direction,
    Pila,
};

/// Around the windows, inside the part of the output panels leave.
const MARGIN: i32 = 48;
/// Between two windows, across and down.
const GAP: i32 = 32;
/// How far a press on a window moves before it's a drag.
const DRAG_THRESHOLD: f64 = 8.0;
/// A dragged window shrinks to fit this square (never grown), over the
/// first `SHRINK_DISTANCE` of the drag: small enough to aim at a
/// workspace's thumbnail.
const DRAGGED_MAX: f64 = 240.0;
const SHRINK_DISTANCE: f64 = 120.0;
/// The selected window's outline, around its frame.
const OUTLINE: i32 = 4;
/// The workspace strip: its thumbnails' height, a share of the output's
/// usable height; between them; above them.
const STRIP_SHARE: f64 = 0.12;
const STRIP_GAP: i32 = 16;
const STRIP_TOP: i32 = 24;
/// A thumbnail's border, and the active or hovered one's.
const THUMB_BORDER: i32 = 1;
const THUMB_BORDER_MARKED: i32 = 3;

/// The overview while it's open, or gliding closed.
#[derive(Default)]
pub struct Overview {
    open: Option<Open>,
    /// How far it's shown (0: closed, 1: open) while it opens or closes.
    glide: Option<Animation>,
    /// Closing: the places the windows glide back from.
    closing: Option<Vec<Slot>>,
    buffers: Rc<RefCell<Buffers>>,
    /// The pointer's visit to the hot corner (`hot_corner.rs`).
    pub(crate) hot_corner: crate::hot_corner::HotCorner,
    /// The workspace thumbnails' textures, by output and workspace: kept
    /// from one opening to the next, so opening makes none (each has its
    /// own damage tracker: only what changed meanwhile is drawn again).
    thumbnails: Rc<RefCell<Thumbnails>>,
}

/// Each workspace thumbnail's texture, by output name and workspace, with
/// the size it was made for.
type Thumbnails = HashMap<(String, usize), (Size<i32, smithay::utils::Physical>, OffscreenWindow)>;

/// A workspace's thumbnail in the strip on top of an output.
#[derive(Clone)]
pub struct Thumb {
    pub output: Output,
    pub workspace: usize,
    pub rect: Rectangle<i32, Logical>,
}

/// A thumbnail as the renderer gets it: what to draw in it, as the output
/// shows it, and where.
#[derive(Clone)]
pub struct SceneThumb {
    pub output: String,
    /// The output's place and size: the thumbnail shows all of it.
    pub output_geo: Rectangle<i32, Logical>,
    pub workspace: usize,
    pub rect: Rectangle<i32, Logical>,
    /// The active workspace's, bordered in the accent color.
    pub active: bool,
    /// Under the pointer.
    pub hovered: bool,
    /// The workspace's windows on that output, front to back, as they are.
    pub windows: Vec<SceneWindow>,
    /// The output's wallpaper (Bottom and Background layer surfaces).
    pub layers: Vec<crate::render::SceneLayer>,
}

#[derive(Default)]
struct Open {
    /// The window Enter picks: the focused one at first, then the one the
    /// arrows or the pointer go to.
    selected: Option<Window>,
    /// The places last laid out, to redraw the outputs when they change.
    laid_out: Vec<(Window, Rectangle<i32, Logical>)>,
    /// The workspace thumbnail under the pointer.
    hovered: Option<Rectangle<i32, Logical>>,
    /// A button held down on a window: its release picks it, or drops it
    /// on a workspace once it's dragged.
    press: Option<Press>,
}

/// A press on a window in the overview.
struct Press {
    window: Window,
    button: u32,
    /// Where it was pressed, and where the pointer was last (global).
    at: Point<f64, Logical>,
    last: Point<f64, Logical>,
    /// From the window's place to the pointer, and its place's size: it's
    /// drawn there under the pointer while it's dragged.
    grip: Point<f64, Logical>,
    size: Size<i32, Logical>,
    dragging: bool,
}

impl Press {
    /// Where the window is drawn while dragged, the pointer at `pointer`:
    /// shrunk as the drag goes on, the point it was taken by kept under
    /// the pointer.
    fn rect(&self, pointer: Point<f64, Logical>) -> Rectangle<i32, Logical> {
        dragged_rect(self.size, self.grip, self.at, pointer)
    }
}

/// Where a window's place of `size`, taken at `grip` from its top-left
/// where the pointer was `at`, is drawn with the pointer at `pointer`.
fn dragged_rect(
    size: Size<i32, Logical>,
    grip: Point<f64, Logical>,
    at: Point<f64, Logical>,
    pointer: Point<f64, Logical>,
) -> Rectangle<i32, Logical> {
    let scale = dragged_scale(size, pointer - at);
    let shrunk = size.to_f64().upscale(scale);
    Rectangle::new(
        (pointer - grip.upscale(scale)).to_i32_round(),
        (shrunk.w.round() as i32, shrunk.h.round() as i32).into(),
    )
}

/// How much smaller a window of `size` is drawn once dragged by `moved`:
/// from 1 to what fits `DRAGGED_MAX`, over `SHRINK_DISTANCE`.
fn dragged_scale(size: Size<i32, Logical>, moved: Point<f64, Logical>) -> f64 {
    let largest = f64::from(size.w.max(size.h).max(1));
    let small = (DRAGGED_MAX / largest).min(1.0);
    let t = (moved.x.hypot(moved.y) / SHRINK_DISTANCE).min(1.0);
    1.0 + (small - 1.0) * t
}

/// Kept between frames while it's open: an unchanged overview adds no
/// damage.
#[derive(Default)]
struct Buffers {
    /// Each output's dimming, by name.
    dim: HashMap<String, SolidColorBuffer>,
    outline: Option<SolidColorBuffer>,
    /// Each thumbnail's border, and what's under its windows.
    thumb_borders: HashMap<(String, usize), SolidColorBuffer>,
    thumb_background: Option<SolidColorBuffer>,
}

/// A window's place in the overview.
#[derive(Clone)]
pub struct Slot {
    pub window: Window,
    pub output: Output,
    /// Its frame (geometry and title bar) where it is.
    pub frame: Rectangle<i32, Logical>,
    /// Where its frame is shown in the overview.
    pub rect: Rectangle<i32, Logical>,
}

/// What `Pila::scene` gives the renderer of the overview open.
#[derive(Clone)]
pub struct SceneOverview {
    /// The outputs it dims, by name, and where they are.
    pub outputs: Vec<(String, Rectangle<i32, Logical>)>,
    /// The selected window's place.
    pub selected: Option<Rectangle<i32, Logical>>,
    /// Its outline's corners: the windows' radius and corners (`None`:
    /// square).
    pub rounding: Option<(f64, crate::rounded::Corners)>,
    /// How far it's shown (0 to 1): the dimming and outline fade with it.
    pub shown: f32,
    /// The workspace strips.
    pub thumbs: Vec<SceneThumb>,
    buffers: Rc<RefCell<Buffers>>,
    thumbnails: Rc<RefCell<Thumbnails>>,
}

impl SceneOverview {
    /// The windows its thumbnails on `output` show, which get frame
    /// callbacks from it.
    pub fn thumb_windows<'a>(&'a self, output: &'a str) -> impl Iterator<Item = &'a Window> {
        self.thumbs
            .iter()
            .filter(move |thumb| thumb.output == output)
            .flat_map(|thumb| thumb.windows.iter().map(|entry| &entry.window))
    }
}

/// An output's usable `area` (of an output of `size`) shared between the
/// strip of `count` workspace thumbnails on top (none for one workspace) and
/// the windows below: the thumbnails, of the output's shape, centered in a
/// row, and the windows' area inside `MARGIN`.
pub fn strip(
    area: Rectangle<i32, Logical>,
    size: Size<i32, Logical>,
    count: usize,
) -> (Vec<Rectangle<i32, Logical>>, Rectangle<i32, Logical>) {
    let inner = |top: i32| {
        Rectangle::new(
            (area.loc.x + MARGIN, top).into(),
            (
                area.size.w - 2 * MARGIN,
                area.loc.y + area.size.h - MARGIN - top,
            )
                .into(),
        )
    };
    if count < 2 || size.w <= 0 || size.h <= 0 {
        return (Vec::new(), inner(area.loc.y + MARGIN));
    }
    let n = count as f64;
    let mut h = f64::from(area.size.h) * STRIP_SHARE;
    let mut w = h * f64::from(size.w) / f64::from(size.h);
    let room = f64::from(area.size.w - 2 * MARGIN) - f64::from(STRIP_GAP) * (n - 1.0);
    if w * n > room {
        let fit = (room / (w * n)).max(0.0);
        (w, h) = (w * fit, h * fit);
    }
    let (w, h) = ((w.floor() as i32).max(1), (h.floor() as i32).max(1));
    let total = w * count as i32 + STRIP_GAP * (count as i32 - 1);
    let x = area.loc.x + (area.size.w - total) / 2;
    let y = area.loc.y + STRIP_TOP;
    let thumbs = (0..count as i32)
        .map(|i| Rectangle::new((x + i * (w + STRIP_GAP), y).into(), (w, h).into()))
        .collect();
    (thumbs, inner(y + h + MARGIN))
}

/// Places for frames of `sizes` in `area`, in rows, none over another: each
/// shrunk by the same scale (never grown), the order kept from row to row,
/// rows and windows centered, `GAP` between them. The number of rows is the
/// one that shows them largest (the fewest of equal ones).
pub fn arrange(
    area: Rectangle<i32, Logical>,
    sizes: &[Size<i32, Logical>],
) -> Vec<Rectangle<i32, Logical>> {
    let n = sizes.len();
    if n == 0 {
        return Vec::new();
    }
    let size = |i: usize| {
        let s = sizes[i];
        (f64::from(s.w.max(1)), f64::from(s.h.max(1)))
    };
    let (avail_w, avail_h) = (f64::from(area.size.w), f64::from(area.size.h));
    let gap = f64::from(GAP);
    // The rows of `rows` rows: as even as can be, the first ones longer.
    let split = |rows: usize| -> Vec<std::ops::Range<usize>> {
        let (per, extra) = (n / rows, n % rows);
        let mut start = 0;
        (0..rows)
            .map(|row| {
                let len = per + usize::from(row < extra);
                let range = start..start + len;
                start += len;
                range
            })
            .collect()
    };
    let scale_for = |rows: &[std::ops::Range<usize>]| -> f64 {
        let mut scale: f64 = 1.0;
        let mut heights = 0.0;
        for row in rows {
            let width: f64 = row.clone().map(|i| size(i).0).sum();
            let gaps = gap * (row.len() as f64 - 1.0);
            scale = scale.min((avail_w - gaps).max(1.0) / width);
            heights += row.clone().map(|i| size(i).1).fold(0.0, f64::max);
        }
        let gaps = gap * (rows.len() as f64 - 1.0);
        scale.min((avail_h - gaps).max(1.0) / heights)
    };
    let (rows, scale) = (1..=n)
        .map(|count| {
            let rows = split(count);
            let scale = scale_for(&rows);
            (rows, scale)
        })
        .fold(None::<(Vec<_>, f64)>, |best, (rows, scale)| match best {
            Some((_, best_scale)) if best_scale >= scale => best,
            _ => Some((rows, scale)),
        })
        .unwrap_or_default();
    let row_height =
        |row: &std::ops::Range<usize>| row.clone().map(|i| size(i).1 * scale).fold(0.0, f64::max);
    let total_h: f64 = rows.iter().map(row_height).sum::<f64>() + gap * (rows.len() as f64 - 1.0);
    let mut places = vec![Rectangle::default(); n];
    let mut y = f64::from(area.loc.y) + (avail_h - total_h) / 2.0;
    for row in &rows {
        let height = row_height(row);
        let width: f64 =
            row.clone().map(|i| size(i).0 * scale).sum::<f64>() + gap * (row.len() as f64 - 1.0);
        let mut x = f64::from(area.loc.x) + (avail_w - width) / 2.0;
        for i in row.clone() {
            let (w, h) = (size(i).0 * scale, size(i).1 * scale);
            places[i] = Rectangle::new(
                (x.round() as i32, (y + (height - h) / 2.0).round() as i32).into(),
                ((w.round() as i32).max(1), (h.round() as i32).max(1)).into(),
            );
            x += w + gap;
        }
        y += height + gap;
    }
    places
}

/// The place one step from `from` toward `direction` among `places`: the
/// nearest whose center lies that way, a sideways distance counting double.
pub fn step(
    places: &[Rectangle<i32, Logical>],
    from: usize,
    direction: Direction,
) -> Option<usize> {
    let center = |r: &Rectangle<i32, Logical>| {
        let r = r.to_f64();
        (r.loc.x + r.size.w / 2.0, r.loc.y + r.size.h / 2.0)
    };
    let (fx, fy) = center(places.get(from)?);
    places
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != from)
        .filter_map(|(i, place)| {
            let (x, y) = center(place);
            let (ahead, aside) = match direction {
                Direction::Left => (fx - x, y - fy),
                Direction::Right => (x - fx, y - fy),
                Direction::Up => (fy - y, x - fx),
                Direction::Down => (y - fy, x - fx),
            };
            (ahead > 0.0).then_some((i, ahead + 2.0 * aside.abs()))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

impl Action {
    /// Whether the overview stays open for it: going to a workspace or
    /// sending a window there, switching layouts and the overview's own.
    /// Any other action closes it first (`Pila::run_action`).
    pub(crate) fn keeps_overview(&self) -> bool {
        matches!(
            self,
            Action::Overview(_)
                | Action::Workspace(_)
                | Action::MoveToWorkspace(_)
                | Action::SwitchLayout(_)
        )
    }
}

impl Pila {
    pub fn overview_open(&self) -> bool {
        self.overview.open.is_some()
    }

    /// The `overview` action.
    pub(crate) fn overview_action(&mut self, action: OverviewAction) {
        match action {
            OverviewAction::Open => self.open_overview(),
            OverviewAction::Close => self.close_overview(),
            OverviewAction::Toggle if self.overview_open() => self.close_overview(),
            OverviewAction::Toggle => self.open_overview(),
        }
    }

    /// Open it, the focused window selected. Not while locked, nor behind
    /// a modal panel (by the hot corner, a swipe or `pila msg`).
    pub fn open_overview(&mut self) {
        if self.overview_open() || self.is_locked() || self.modal_panel_open() {
            return;
        }
        tracing::debug!("opening the overview");
        self.end_switch();
        self.close_window_menu();
        self.hide_shortcuts();
        self.dismiss_popup_grab();
        // From where a closing one is, if it's on its way back.
        let now = self.animation_time();
        let from = self.overview_shown(now);
        self.overview.closing = None;
        self.overview.glide = self
            .animation_duration(AnimationKind::Overview)
            .map(|duration| Animation::new(now, duration.mul_f64(1.0 - from), from, 1.0));
        if self.overview.glide.is_some() {
            crate::animation::log_overview_glide(true);
        }
        self.overview.open = Some(Open {
            selected: self.focused_window(),
            laid_out: Vec::new(),
            hovered: None,
            press: None,
        });
        self.overview_laid_out();
        // Every output: windows move, and a fullscreen window's panels show.
        self.queue_redraw();
        self.refresh_pointer_focus();
        let pointer_grabbed = self.seat.get_pointer().is_some_and(|p| p.is_grabbed());
        if !pointer_grabbed {
            self.set_cursor(smithay::input::pointer::CursorImageStatus::default_named());
        }
    }

    /// Close it, the windows gliding back where they are.
    pub fn close_overview(&mut self) {
        if !self.overview_open() {
            return;
        }
        tracing::debug!("closing the overview");
        let slots = self.overview_slots();
        let now = self.animation_time();
        let from = self.overview_shown(now);
        self.overview.open = None;
        self.overview.glide = self
            .animation_duration(AnimationKind::Overview)
            .map(|duration| Animation::new(now, duration.mul_f64(from), from, 0.0));
        if self.overview.glide.is_some() {
            crate::animation::log_overview_glide(false);
            self.overview.closing = Some(slots);
        } else {
            self.overview_closed(&slots);
        }
        self.queue_redraw();
        self.refresh_pointer_focus();
    }

    /// Done closing: what was kept for it goes.
    fn overview_closed(&mut self, slots: &[Slot]) {
        *self.overview.buffers.borrow_mut() = Buffers::default();
        for slot in slots {
            if !self.window_animates(&slot.window) {
                crate::animation::clear_textures(slot.window.user_data());
            }
        }
    }

    /// How far it's shown at `now`: 0 closed, 1 open, between while it
    /// glides.
    pub(crate) fn overview_shown(&self, now: std::time::Duration) -> f64 {
        match &self.overview.glide {
            Some(glide) => glide.value(now),
            None if self.overview_open() => 1.0,
            None => 0.0,
        }
    }

    /// Whether it opens or closes, though its time may be up
    /// (`animations_held`): every output is redrawn for it.
    pub(crate) fn overview_moving(&self) -> bool {
        self.overview.glide.is_some()
    }

    /// Whether it's still gliding at `now` (`animating`).
    pub(crate) fn overview_gliding(&self, now: std::time::Duration) -> bool {
        self.overview.glide.as_ref().is_some_and(|g| !g.done(now))
    }

    /// End a glide that's done (`step_animations`).
    pub(crate) fn end_finished_overview(&mut self, now: std::time::Duration) {
        if self.overview.glide.as_ref().is_some_and(|g| g.done(now)) {
            self.overview.glide = None;
            if let Some(slots) = self.overview.closing.take() {
                self.overview_closed(&slots);
            }
        }
    }

    /// Stop gliding at once (the session locked, `drop_animations`).
    pub(crate) fn stop_overview_glide(&mut self) {
        self.overview.glide = None;
        if let Some(slots) = self.overview.closing.take() {
            self.overview_closed(&slots);
            self.queue_redraw();
        }
    }

    /// Close it on `window`, focused and raised (brought back if minimized).
    fn pick_in_overview(&mut self, window: &Window) {
        self.close_overview();
        self.activate_toplevel(window);
    }

    /// A key pressed while it's open that's no binding: Escape closes it,
    /// the arrows go from window to window, Enter picks the selected one.
    pub(crate) fn overview_key(&mut self, sym: Keysym) {
        let direction = match sym {
            Keysym::Escape => return self.close_overview(),
            Keysym::Return | Keysym::KP_Enter | Keysym::space => {
                let selected = self.overview.open.as_ref().and_then(|o| o.selected.clone());
                match selected {
                    Some(window) => self.pick_in_overview(&window),
                    None => self.close_overview(),
                }
                return;
            }
            Keysym::Left => Direction::Left,
            Keysym::Right => Direction::Right,
            Keysym::Up => Direction::Up,
            Keysym::Down => Direction::Down,
            _ => return,
        };
        let slots = self.overview_slots();
        let places: Vec<_> = slots.iter().map(|s| s.rect).collect();
        let selected = self
            .overview
            .open
            .as_ref()
            .and_then(|o| o.selected.as_ref());
        let next = match selected.and_then(|w| slots.iter().position(|s| &s.window == w)) {
            Some(from) => step(&places, from, direction),
            // Nothing selected yet: the first one.
            None => (!slots.is_empty()).then_some(0),
        };
        if let Some(next) = next {
            self.select_in_overview(Some(slots[next].window.clone()));
        }
    }

    /// Select `window` (the arrows, the pointer over it), redrawing the
    /// outline where it was and where it is.
    fn select_in_overview(&mut self, window: Option<Window>) {
        let Some(open) = self.overview.open.as_mut() else {
            return;
        };
        if open.selected == window {
            return;
        }
        let before = std::mem::replace(&mut open.selected, window.clone());
        for slot in self.overview_slots() {
            if Some(&slot.window) == before.as_ref() || Some(&slot.window) == window.as_ref() {
                self.queue_redraw_rect(outline_rect(slot.rect));
            }
        }
    }

    /// The pointer moved to `location` while it's open: the window under it
    /// is selected.
    pub(crate) fn overview_pointer_moved(&mut self, location: Point<f64, Logical>) {
        if !self.overview_open() {
            return;
        }
        self.overview_drag(location);
        if let Some(window) = self.overview_window_at(location) {
            self.select_in_overview(Some(window));
        }
        let hovered = self.overview_thumb_at(location).map(|thumb| thumb.rect);
        let Some(open) = self.overview.open.as_mut() else {
            return;
        };
        let before = std::mem::replace(&mut open.hovered, hovered);
        if before != hovered {
            for rect in [before, hovered].into_iter().flatten() {
                self.queue_redraw_rect(thumb_border_rect(rect, THUMB_BORDER_MARKED));
            }
        }
    }

    /// A press while it's open: on a workspace's thumbnail it goes there;
    /// on a window it's held (`overview_release` picks it or drops it on a
    /// workspace); beside the windows (not on a panel) it closes it. `true`
    /// if it took the press, which then goes nowhere else (its release
    /// neither).
    pub(crate) fn overview_press(&mut self, location: Point<f64, Logical>, button: u32) -> bool {
        if !self.overview_open() {
            return false;
        }
        if self
            .overview
            .open
            .as_ref()
            .is_some_and(|o| o.press.is_some())
        {
            return true;
        }
        // A workspace's thumbnail goes there, and it stays open.
        if let Some(thumb) = self.overview_thumb_at(location) {
            self.switch_workspace(thumb.workspace);
            return true;
        }
        let slot = self
            .overview_slots()
            .into_iter()
            .find(|slot| slot.rect.to_f64().contains(location));
        if let Some(slot) = slot {
            if let Some(open) = self.overview.open.as_mut() {
                open.press = Some(Press {
                    window: slot.window,
                    button,
                    at: location,
                    last: location,
                    grip: location - slot.rect.loc.to_f64(),
                    size: slot.rect.size,
                    dragging: false,
                });
            }
            return true;
        }
        // Panels above it still take their clicks.
        if self.under(location).is_some() {
            return false;
        }
        self.close_overview();
        true
    }

    /// A button released while it's open: the one held on a window picks
    /// it, or, dragged, drops it: on another workspace's thumbnail it goes
    /// there, anywhere else back to its place. `true` if it was that one.
    pub(crate) fn overview_release(&mut self, location: Point<f64, Logical>, button: u32) -> bool {
        let Some(open) = self.overview.open.as_mut() else {
            return false;
        };
        if open
            .press
            .as_ref()
            .is_none_or(|press| press.button != button)
        {
            return false;
        }
        let Some(press) = open.press.take() else {
            return false;
        };
        if !press.dragging {
            self.pick_in_overview(&press.window);
            return true;
        }
        self.queue_redraw_rect(self.grown_by_shadow(press.rect(press.last)));
        if let Some(thumb) = self.overview_thumb_at(location) {
            self.move_window_to_workspace(&press.window, thumb.workspace);
        }
        // Wherever it went, the places changed (`overview_laid_out`).
        self.queue_redraw();
        true
    }

    /// The pointer moved with a window held: past `DRAG_THRESHOLD` it's
    /// dragged, drawn under the pointer.
    fn overview_drag(&mut self, location: Point<f64, Logical>) {
        let Some(press) = self.overview.open.as_mut().and_then(|o| o.press.as_mut()) else {
            return;
        };
        let moved = location - press.at;
        if !press.dragging && moved.x.hypot(moved.y) < DRAG_THRESHOLD {
            return;
        }
        press.dragging = true;
        // Where it was drawn last (its place, the first time) and now.
        let before = press.rect(press.last);
        press.last = location;
        let now = press.rect(location);
        for rect in [before, now] {
            self.queue_redraw_rect(self.grown_by_shadow(rect));
        }
    }

    /// `rect` grown by a window's shadow, for redraws.
    fn grown_by_shadow(&self, rect: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
        crate::redraw::grown(rect, self.shadow_margin())
    }

    /// The window shown at `location` (global), while it's open.
    pub fn overview_window_at(&self, location: Point<f64, Logical>) -> Option<Window> {
        if !self.overview_open() {
            return None;
        }
        self.overview_slots()
            .into_iter()
            .find(|slot| slot.rect.to_f64().contains(location))
            .map(|slot| slot.window)
    }

    /// A window committed while it's open: what it shows changed, and its
    /// size may move the others on its output.
    pub(crate) fn overview_committed(&mut self, window: &Window) {
        if !self.overview_open() {
            return;
        }
        let output = self
            .overview_slots()
            .into_iter()
            .find(|slot| &slot.window == window)
            .map(|slot| slot.output);
        if let Some(output) = output {
            self.queue_redraw_output(&output);
        }
        // Its thumbnail: a window on another workspace has no place of its
        // own on the `Space`.
        let workspace = self.workspace_of(window);
        let at = match self.space.element_geometry(window) {
            Some(geo) => geo,
            None => frame_at(window, crate::workspaces::space_location(window)),
        };
        let rects: Vec<_> = self
            .overview_strip()
            .into_iter()
            .filter(|thumb| {
                (thumb.workspace == workspace || crate::stacking::is_on_all_workspaces(window))
                    && self
                        .space
                        .output_geometry(&thumb.output)
                        .is_some_and(|geo| geo.overlaps(at))
            })
            .map(|thumb| thumb.rect)
            .collect();
        for rect in rects {
            self.queue_redraw_rect(rect);
        }
    }

    /// After each turn of the loop while it's open: an output whose places
    /// changed (a window opened, closed, moved or was minimized) is redrawn,
    /// and a selected window that's gone leaves nothing selected.
    pub(crate) fn overview_laid_out(&mut self) {
        let Some(open) = self.overview.open.as_ref() else {
            return;
        };
        let slots = self.overview_slots();
        let laid_out: Vec<_> = slots.iter().map(|s| (s.window.clone(), s.rect)).collect();
        if laid_out == open.laid_out {
            return;
        }
        let changed: Vec<Output> = slots
            .iter()
            .filter(|slot| {
                !open
                    .laid_out
                    .iter()
                    .any(|(w, rect)| *w == slot.window && *rect == slot.rect)
            })
            .map(|slot| slot.output.clone())
            .collect();
        let gone = open
            .laid_out
            .iter()
            .any(|(w, _)| !laid_out.iter().any(|(other, _)| other == w));
        let selected_gone = open
            .selected
            .as_ref()
            .is_some_and(|w| !laid_out.iter().any(|(other, _)| other == w));
        // A window held (pressed or dragged) that's gone is let go: its
        // release, still suppressed, goes nowhere.
        let held_gone = open
            .press
            .as_ref()
            .is_some_and(|p| !laid_out.iter().any(|(other, _)| *other == p.window));
        if let Some(open) = self.overview.open.as_mut() {
            open.laid_out = laid_out;
            if selected_gone {
                open.selected = None;
            }
            if held_gone {
                open.press = None;
            }
        }
        self.defer_redraws(|state| {
            if gone {
                state.queue_redraw();
            } else {
                for output in &changed {
                    state.queue_redraw_output(output);
                }
            }
        });
    }

    /// `output`'s workspace strip and the area its windows are laid out in.
    fn overview_areas(&self, output: &Output) -> Option<(Vec<Thumb>, Rectangle<i32, Logical>)> {
        let area = self.output_area(output, true)?;
        let size = self.space.output_geometry(output)?.size;
        let (rects, windows) = strip(area, size, self.config.workspaces);
        let thumbs = rects
            .into_iter()
            .enumerate()
            .map(|(workspace, rect)| Thumb {
                output: output.clone(),
                workspace,
                rect,
            })
            .collect();
        Some((thumbs, windows))
    }

    /// Every output's workspace thumbnails, while it's open.
    pub fn overview_strip(&self) -> Vec<Thumb> {
        if !self.overview_open() {
            return Vec::new();
        }
        self.overview_strips()
    }

    /// Every output's workspace thumbnails, open or not.
    fn overview_strips(&self) -> Vec<Thumb> {
        self.space
            .outputs()
            .filter_map(|output| self.overview_areas(output))
            .flat_map(|(thumbs, _)| thumbs)
            .collect()
    }

    /// The workspace thumbnail at `location` (global), while it's open.
    fn overview_thumb_at(&self, location: Point<f64, Logical>) -> Option<Thumb> {
        self.overview_strip()
            .into_iter()
            .find(|thumb| thumb.rect.to_f64().contains(location))
    }

    /// The windows it shows, each with its place: per output, the active
    /// workspace's windows on it (the minimized ones too), laid out in the
    /// part panels leave, in the order they're on screen.
    pub fn overview_slots(&self) -> Vec<Slot> {
        if !self.overview_open() {
            return Vec::new();
        }
        let active = self.workspaces.active;
        let first_output = self.space.outputs().next().cloned();
        let mapped = self
            .space
            .elements()
            .filter(|w| !crate::workspaces::is_override_redirect(w))
            .filter_map(|w| {
                let frame = frame_at(w, self.space.element_location(w)?);
                Some((w.clone(), self.window_output(w)?, frame))
            });
        let minimized = self
            .windows
            .minimized
            .iter()
            .filter(|w| self.workspace_of(w) == active)
            .filter_map(|w| {
                let frame = frame_at(w, crate::workspaces::space_location(w));
                let output = self
                    .output_overlapping(frame)
                    .or_else(|| first_output.clone())?;
                Some((w.clone(), output, frame))
            });
        let windows: Vec<(Window, Output, Rectangle<i32, Logical>)> =
            mapped.chain(minimized).collect();
        let mut slots = Vec::new();
        for output in self.space.outputs() {
            let Some((_, inner)) = self.overview_areas(output) else {
                continue;
            };
            let mut here: Vec<&(Window, Output, Rectangle<i32, Logical>)> =
                windows.iter().filter(|(_, o, _)| o == output).collect();
            // As they're on screen: top to bottom, then left to right.
            here.sort_by_key(|(_, _, frame)| {
                let center = crate::animation::center(*frame);
                (center.y as i32, center.x as i32)
            });
            let sizes: Vec<_> = here.iter().map(|(_, _, frame)| frame.size).collect();
            let places = arrange(inner, &sizes);
            slots.extend(
                here.into_iter()
                    .zip(places)
                    .map(|((window, output, frame), rect)| Slot {
                        window: window.clone(),
                        output: output.clone(),
                        frame: *frame,
                        rect,
                    }),
            );
        }
        slots
    }

    /// The scene's windows as it shows them: each on its place, `shown` of
    /// the way there from where it is (minimized ones added behind, fading
    /// with it); while it's open, those it doesn't show are left out.
    pub(crate) fn overview_scene_windows(
        &self,
        windows: Vec<SceneWindow>,
        focused: Option<&Window>,
    ) -> Vec<SceneWindow> {
        let now = self.animation_time();
        let shown = self.overview_shown(now);
        let open = self.overview_open();
        let slots = match &self.overview.closing {
            Some(slots) if !open => slots.clone(),
            _ => self.overview_slots(),
        };
        let effect = |slot: &Slot, frame: Rectangle<i32, Logical>, alpha: f32| {
            let to = lerp_rect(frame, slot.rect, shown);
            WindowEffect {
                transform: Transform {
                    alpha,
                    scale: 1.0,
                    offset: Point::default(),
                },
                center: to.loc + to.size.downscale(2.0).to_point(),
                output: Some(slot.output.clone()),
                frame: Some(FrameMap {
                    from: frame.to_f64(),
                    to,
                }),
            }
        };
        let mut drawn: Vec<SceneWindow> = windows
            .into_iter()
            .filter_map(|mut entry| {
                let Some(slot) = slots.iter().find(|s| s.window == entry.window) else {
                    return (!open).then_some(entry);
                };
                let frame = self
                    .space
                    .element_location(&entry.window)
                    .map_or(slot.frame, |loc| frame_at(&entry.window, loc));
                entry.effect = Some(effect(slot, frame, 1.0));
                Some(entry)
            })
            .collect();
        for slot in slots.iter().filter(|s| self.is_minimized(&s.window)) {
            let loc = crate::workspaces::space_location(&slot.window);
            if let Some(mut entry) = self.scene_window_at(&slot.window, loc, focused, None, None) {
                entry.effect = Some(effect(slot, slot.frame, shown as f32));
                drawn.push(entry);
            }
        }
        // A window dragged is under the pointer, in front of the others.
        let dragged = self
            .overview
            .open
            .as_ref()
            .and_then(|open| open.press.as_ref())
            .filter(|press| press.dragging);
        if let Some(press) = dragged {
            if let Some(index) = drawn.iter().position(|e| e.window == press.window) {
                let mut entry = drawn.remove(index);
                let rect = press.rect(press.last);
                if let Some(frame) = entry.effect.as_mut().and_then(|e| e.frame.as_mut()) {
                    frame.to = rect.to_f64();
                }
                if let Some(effect) = entry.effect.as_mut() {
                    effect.center = crate::animation::center(rect);
                }
                drawn.insert(0, entry);
            }
        }
        drawn
    }

    /// The workspace strips as `Pila::scene` gives them: each thumbnail
    /// with the windows of its workspace on its output and the output's
    /// wallpaper.
    fn scene_thumbs(&self) -> Vec<SceneThumb> {
        use smithay::wayland::shell::wlr_layer::Layer;
        let focused = self.focused_window();
        let hovered = self.overview.open.as_ref().and_then(|open| open.hovered);
        let active = self.workspaces.active;
        let layers = self.layer_surfaces(&[Layer::Bottom, Layer::Background]);
        // Front to back, each with its workspace (`None`: all of them).
        let mut windows: Vec<(Option<usize>, SceneWindow)> = self
            .space
            .elements()
            .rev()
            .filter(|w| !crate::workspaces::is_override_redirect(w))
            .filter_map(|w| {
                let workspace = (!crate::stacking::is_on_all_workspaces(w)).then_some(active);
                Some((
                    workspace,
                    self.scene_window(w, focused.as_ref(), None, None)?,
                ))
            })
            .collect();
        windows.extend(self.workspaces.parked.iter().rev().filter_map(|w| {
            let loc = crate::workspaces::space_location(w);
            let entry = self.scene_window_at(w, loc, focused.as_ref(), None, None)?;
            Some((Some(self.workspace_of(w)), entry))
        }));
        self.overview_strips()
            .into_iter()
            .filter_map(|thumb| {
                let output_geo = self.space.output_geometry(&thumb.output)?;
                Some(SceneThumb {
                    output: thumb.output.name(),
                    output_geo,
                    workspace: thumb.workspace,
                    rect: thumb.rect,
                    active: thumb.workspace == active,
                    hovered: hovered == Some(thumb.rect),
                    windows: windows
                        .iter()
                        .filter(|(workspace, entry)| {
                            workspace.is_none_or(|w| w == thumb.workspace)
                                && entry.bounds.overlaps(output_geo)
                        })
                        .map(|(_, entry)| entry.clone())
                        .collect(),
                    layers: layers
                        .iter()
                        .filter(|(_, location)| output_geo.contains(*location))
                        .map(|(layer, location)| crate::render::SceneLayer {
                            layer: layer.clone(),
                            location: *location,
                            blur: Vec::new(),
                            popup_blur: Vec::new(),
                            effect: None,
                        })
                        .collect(),
                })
            })
            .collect()
    }

    /// The overview for `Pila::scene`, while it's open or gliding closed.
    pub(crate) fn scene_overview(&self) -> Option<SceneOverview> {
        if !self.overview_open() && self.overview.closing.is_none() {
            return None;
        }
        let selected = self.overview.open.as_ref().and_then(|open| {
            let window = open.selected.as_ref()?;
            self.overview_slots()
                .into_iter()
                .find(|slot| &slot.window == window)
                .map(|slot| slot.rect)
        });
        Some(SceneOverview {
            outputs: self
                .space
                .outputs()
                .filter_map(|o| Some((o.name(), self.space.output_geometry(o)?)))
                .collect(),
            selected,
            rounding: crate::panel::rounding(&self.config)
                .map(|(radius, corners)| (radius + f64::from(OUTLINE), corners)),
            shown: self.overview_shown(self.animation_time()) as f32,
            thumbs: self.scene_thumbs(),
            buffers: self.overview.buffers.clone(),
            thumbnails: self.overview.thumbnails.clone(),
        })
    }

    /// For the state dump: whether it's open, the selected window and each
    /// window's place.
    pub fn overview_json(&self) -> serde_json::Value {
        let shown = self.overview_shown(self.animation_time());
        let Some(open) = self.overview.open.as_ref() else {
            return serde_json::json!({ "open": false, "shown": shown });
        };
        let rect = |r: Rectangle<i32, Logical>| serde_json::json!({ "x": r.loc.x, "y": r.loc.y, "w": r.size.w, "h": r.size.h });
        serde_json::json!({
            "open": true,
            "shown": shown,
            "selected": open.selected.as_ref().map(crate::foreign_toplevel::app_id),
            "dragging": open.press.as_ref().filter(|p| p.dragging).map(|p| crate::foreign_toplevel::app_id(&p.window)),
            "dragged": open.press.as_ref().filter(|p| p.dragging).map(|p| rect(p.rect(p.last))),
            "slots": self.overview_slots().iter().map(|slot| serde_json::json!({
                "app_id": crate::foreign_toplevel::app_id(&slot.window),
                "output": slot.output.name(),
                "rect": rect(slot.rect),
            })).collect::<Vec<_>>(),
            "strip": self.overview_strip().iter().map(|thumb| serde_json::json!({
                "output": thumb.output.name(),
                "workspace": thumb.workspace + 1,
                "rect": rect(thumb.rect),
                "active": thumb.workspace == self.workspaces.active,
            })).collect::<Vec<_>>(),
        })
    }
}

/// `window`'s frame (geometry and title bar) with its geometry at `loc`.
fn frame_at(window: &Window, loc: Point<i32, Logical>) -> Rectangle<i32, Logical> {
    let geo = Rectangle::new(loc, window.geometry().size);
    crate::decoration::title_bar_rect(window, loc).map_or(geo, |bar| bar.merge(geo))
}

/// A thumbnail's border around it, `width` wide.
fn thumb_border_rect(rect: Rectangle<i32, Logical>, width: i32) -> Rectangle<i32, Logical> {
    Rectangle::new(
        rect.loc - Point::from((width, width)),
        rect.size + Size::from((2 * width, 2 * width)),
    )
}

/// The selected window's outline around its place.
fn outline_rect(rect: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
    Rectangle::new(
        rect.loc - Point::from((OUTLINE, OUTLINE)),
        rect.size + Size::from((2 * OUTLINE, 2 * OUTLINE)),
    )
}

/// What it draws behind the windows on the area `visible` (an output's;
/// everywhere if `None`), front to back, relative to `origin`: the selected
/// window's outline, then the dimming over what's below (the wallpaper).
pub fn overview_elements(
    renderer: &mut GlesRenderer,
    view: &SceneOverview,
    colors: &Colors,
    visible: Option<Rectangle<i32, Logical>>,
    origin: Point<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement> {
    let mut buffers = view.buffers.borrow_mut();
    let buffers = &mut *buffers;
    let shows = |rect: Rectangle<i32, Logical>| visible.is_none_or(|area| area.overlaps(rect));
    let element = |buffer: &SolidColorBuffer, loc: Point<i32, Logical>| {
        SolidColorRenderElement::from_buffer(
            buffer,
            (loc - origin).to_physical_precise_round(scale),
            scale,
            view.shown,
            Kind::Unspecified,
        )
    };
    let mut elements = Vec::new();
    if let Some(rect) = view.selected.map(outline_rect).filter(|r| shows(*r)) {
        let color = crate::panel::accent(colors);
        let buffer = buffers
            .outline
            .get_or_insert_with(|| SolidColorBuffer::new(rect.size, color));
        buffer.update(rect.size, color);
        let solid = element(buffer, rect.loc);
        let shaders = view
            .rounding
            .and_then(|_| crate::rounded::Shaders::get(renderer));
        elements.push(match (view.rounding, shaders) {
            (Some((radius, corners)), Some(shaders)) => PilaRenderElement::RoundedSolid(
                crate::rounded::RoundedSolid::new(solid, &shaders, radius, corners, scale),
            ),
            _ => PilaRenderElement::Solid(solid),
        });
    }
    let mut thumbnails = view.thumbnails.borrow_mut();
    for thumb in view.thumbs.iter().filter(|thumb| shows(thumb.rect)) {
        // Gliding in from above the output.
        let rise = (1.0 - f64::from(view.shown))
            * f64::from(thumb.rect.loc.y + thumb.rect.size.h - thumb.output_geo.loc.y);
        let rect = thumb.rect.to_f64();
        let dst = Rectangle::new((rect.loc.x, rect.loc.y - rise).into(), rect.size);
        if let Some(element) = thumbnail(renderer, &mut thumbnails, buffers, thumb, colors, scale)
            .map(|(buffer, size)| {
                crate::render::texture_element(&buffer, size, dst, view.shown, origin, scale)
            })
        {
            elements.push(element);
        }
        let (width, color) = if thumb.active {
            (THUMB_BORDER_MARKED, crate::panel::accent(colors))
        } else if thumb.hovered {
            (THUMB_BORDER_MARKED, crate::panel::rgb(colors.hover))
        } else {
            (THUMB_BORDER, crate::panel::rgb(colors.hover))
        };
        let border = thumb_border_rect(thumb.rect, width);
        let buffer = buffers
            .thumb_borders
            .entry((thumb.output.clone(), thumb.workspace))
            .or_insert_with(|| SolidColorBuffer::new(border.size, color));
        buffer.update(border.size, color);
        let loc = border.loc - Point::from((0, rise.round() as i32));
        elements.push(PilaRenderElement::Solid(element(buffer, loc)));
    }
    drop(thumbnails);
    for (name, geo) in view.outputs.iter().filter(|(_, geo)| shows(*geo)) {
        elements.push(crate::panel::dim(
            &mut buffers.dim,
            name,
            *geo,
            view.shown,
            origin,
            scale,
        ));
    }
    elements
}

/// `thumb`'s texture, drawn as its workspace is now, and its size; made
/// (or made again, for a new size) as needed.
fn thumbnail(
    renderer: &mut GlesRenderer,
    thumbnails: &mut Thumbnails,
    buffers: &mut Buffers,
    thumb: &SceneThumb,
    colors: &Colors,
    scale: Scale<f64>,
) -> Option<(
    smithay::backend::renderer::element::texture::TextureRenderBuffer<
        smithay::backend::renderer::gles::GlesTexture,
    >,
    Size<i32, smithay::utils::Physical>,
)> {
    let fit = f64::from(thumb.rect.size.w) / f64::from(thumb.output_geo.size.w.max(1));
    let inner = Scale::from((scale.x * fit, scale.y * fit));
    let size = crate::render::texture_size(thumb.rect, scale);
    let key = (thumb.output.clone(), thumb.workspace);
    if thumbnails.get(&key).is_none_or(|(made, _)| *made != size) {
        thumbnails.insert(
            key.clone(),
            (size, OffscreenWindow::new(renderer, size, inner)?),
        );
    }
    let (_, target) = thumbnails.get_mut(&key)?;
    let background = buffers
        .thumb_background
        .get_or_insert_with(|| SolidColorBuffer::new(thumb.output_geo.size, colors.background));
    background.update(thumb.output_geo.size, colors.background);
    if !crate::render::draw_thumbnail(
        renderer,
        target,
        &thumb.windows,
        &thumb.layers,
        background,
        thumb.output_geo,
        inner,
    ) {
        return None;
    }
    Some((target.buffer().clone(), size))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    fn size(w: i32, h: i32) -> Size<i32, Logical> {
        (w, h).into()
    }

    #[test]
    fn places_never_overlap_stay_inside_keep_the_shape_and_never_grow() {
        let area = rect(10, 20, 1800, 1000);
        let shapes = [
            size(800, 600),
            size(1920, 1080),
            size(300, 900),
            size(640, 480),
            size(1200, 400),
        ];
        for n in 1..=20 {
            let sizes: Vec<_> = (0..n).map(|i| shapes[i % shapes.len()]).collect();
            let places = arrange(area, &sizes);
            assert_eq!(places.len(), n);
            for (i, (place, size)) in places.iter().zip(&sizes).enumerate() {
                assert!(area.contains_rect(*place), "{n}: {place:?} out of {area:?}");
                assert!(place.size.w <= size.w && place.size.h <= size.h, "grown");
                let (a, b) = (
                    f64::from(place.size.w) / f64::from(place.size.h),
                    f64::from(size.w) / f64::from(size.h),
                );
                assert!((a - b).abs() / b < 0.05, "{n}: shape {a} for {b}");
                for other in &places[i + 1..] {
                    assert!(!place.overlaps(*other), "{n}: {place:?} over {other:?}");
                }
            }
        }
    }

    #[test]
    fn small_windows_keep_their_size_centered() {
        let places = arrange(rect(0, 0, 1000, 800), &[size(200, 100), size(300, 100)]);
        assert_eq!(places[0].size, size(200, 100));
        assert_eq!(places[1].size, size(300, 100));
        // One row, centered: 200 + 32 + 300 = 532 wide.
        assert_eq!(places[0].loc, (234, 350).into());
        assert_eq!(places[1].loc, (234 + 200 + GAP, 350).into());
    }

    #[test]
    fn rows_are_chosen_to_show_windows_largest() {
        // Four screen-sized windows on a screen: two rows of two.
        let sizes = vec![size(1920, 1080); 4];
        let places = arrange(rect(0, 0, 1920, 1080), &sizes);
        assert_eq!(places[0].loc.y, places[1].loc.y);
        assert!(places[2].loc.y > places[0].loc.y);
        assert_eq!(places[2].loc.y, places[3].loc.y);
    }

    #[test]
    fn the_strip_is_a_row_of_thumbnails_of_the_outputs_shape_above_the_windows() {
        let area = rect(0, 30, 1920, 1050);
        let (thumbs, windows) = strip(area, size(1920, 1080), 4);
        assert_eq!(thumbs.len(), 4);
        let h = (1050.0 * STRIP_SHARE).floor() as i32;
        for (i, thumb) in thumbs.iter().enumerate() {
            assert_eq!(thumb.size.h, h);
            assert_eq!(
                thumb.size.w,
                (f64::from(h) * 1920.0 / 1080.0).floor() as i32
            );
            assert_eq!(thumb.loc.y, 30 + STRIP_TOP);
            if i > 0 {
                assert_eq!(thumb.loc.x, thumbs[i - 1].loc.x + thumb.size.w + STRIP_GAP);
            }
        }
        // Centered.
        let (left, right) = (thumbs[0].loc.x, 1920 - (thumbs[3].loc.x + thumbs[3].size.w));
        assert!((left - right).abs() <= 1, "{left} {right}");
        // The windows below, inside the margin.
        assert_eq!(windows.loc.y, 30 + STRIP_TOP + h + MARGIN);
        assert_eq!(windows.loc.y + windows.size.h, 30 + 1050 - MARGIN);
        assert_eq!((windows.loc.x, windows.size.w), (MARGIN, 1920 - 2 * MARGIN));
    }

    #[test]
    fn many_thumbnails_shrink_to_fit_and_one_workspace_has_none() {
        let area = rect(0, 0, 1000, 800);
        let (thumbs, _) = strip(area, size(1000, 800), 9);
        let last = thumbs.last().unwrap();
        assert!(thumbs[0].loc.x >= MARGIN && last.loc.x + last.size.w <= 1000 - MARGIN);
        let (thumbs, windows) = strip(area, size(1000, 800), 1);
        assert!(thumbs.is_empty());
        assert_eq!(
            windows,
            rect(MARGIN, MARGIN, 1000 - 2 * MARGIN, 800 - 2 * MARGIN)
        );
    }

    #[test]
    fn a_dragged_window_shrinks_as_it_goes_keeping_its_grip() {
        let size = size(800, 400);
        assert_eq!(dragged_scale(size, (0.0, 0.0).into()), 1.0);
        let half = dragged_scale(size, (SHRINK_DISTANCE / 2.0, 0.0).into());
        assert!((half - (1.0 + 0.3) / 2.0).abs() < 1e-9, "{half}");
        assert!((dragged_scale(size, (0.0, 500.0).into()) - 0.3).abs() < 1e-9);
        // A small one isn't grown.
        assert_eq!(
            dragged_scale(super::Size::from((100, 80)), (500.0, 0.0).into()),
            1.0
        );
        // Taken by its middle, it shrinks about the pointer.
        let rect = dragged_rect(
            size,
            (400.0, 200.0).into(),
            (0.0, 0.0).into(),
            (500.0, 0.0).into(),
        );
        assert_eq!(rect, Rectangle::new((380, -60).into(), (240, 120).into()));
    }

    #[test]
    fn a_step_goes_to_the_nearest_place_that_way() {
        // A grid of two rows:  0 1 2
        //                        3 4
        let places = [
            rect(0, 0, 100, 100),
            rect(200, 0, 100, 100),
            rect(400, 0, 100, 100),
            rect(100, 200, 100, 100),
            rect(300, 200, 100, 100),
        ];
        assert_eq!(step(&places, 0, Direction::Right), Some(1));
        assert_eq!(step(&places, 1, Direction::Left), Some(0));
        assert_eq!(step(&places, 0, Direction::Left), None);
        assert_eq!(step(&places, 2, Direction::Down), Some(4));
        assert_eq!(step(&places, 3, Direction::Up), Some(0));
        assert_eq!(step(&places, 3, Direction::Right), Some(4));
        assert_eq!(step(&places, 4, Direction::Down), None);
    }
}
