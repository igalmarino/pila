//! The window switcher drawn by Pila itself: while a Mod+Tab (or Alt+Tab)
//! is held past `DELAY`, a panel in the middle of the switch's output shows
//! its windows in their order (`switcher.rs` does the switching; this only
//! shows it), each as a live preview over its title, the selected one in the
//! accent color; a click on one ends the switch there. The panel has the
//! windows' corners (`corner-radius`, `rounded-corners`).
//!
//! The panel is laid out from the switch as it is (`Pila::switcher_layout`):
//! tiles of `COLUMNS` to a row, `ROWS` rows shown with the selected one's in
//! view (`first_shown`). Each tile's preview is its window drawn into a
//! texture of its own (an `OffscreenWindow`, scaled to fit, popups left
//! out), with that texture's own damage tracker: a window that didn't change
//! costs no drawing, one that did redraws its preview, and the output sees a
//! single texture per tile. Its windows get frame callbacks from the
//! switcher's output while it's shown (`frame::send_frames`), and a commit of
//! one redraws its tile (`Pila::switcher_view_committed`), so a video keeps
//! playing in its tile. Titles are rasterized once per title, scale and
//! state (`text::render_title`), and kept with the textures while it's
//! shown; all of it goes when the switch ends.

use std::{cell::RefCell, collections::HashMap, rc::Rc, time::Duration};

use smithay::{
    backend::renderer::{
        element::{
            memory::MemoryRenderBuffer, solid::SolidColorBuffer,
            surface::WaylandSurfaceRenderElement, utils::RescaleRenderElement, AsRenderElements,
        },
        gles::GlesRenderer,
    },
    desktop::Window,
    reexports::calloop::{
        timer::{TimeoutAction, Timer},
        RegistrationToken,
    },
    utils::{Logical, Physical, Point, Rectangle, Scale, Size},
};

use crate::{
    config::Colors,
    render::{OffscreenWindow, PilaRenderElement},
    Pila,
};

/// How long a switch goes on before it's shown: a quick Mod+Tab, there and
/// back, shows nothing.
pub const DELAY: Duration = Duration::from_millis(150);
/// Tiles in a row, and rows shown at once.
pub const COLUMNS: usize = 5;
pub const ROWS: usize = 3;
/// A tile: its padding around a preview, the preview, and the title under it.
const TILE_PADDING: i32 = 8;
pub const PREVIEW: (i32, i32) = (184, 115);
const CAPTION: i32 = 20;
const CAPTION_GAP: i32 = 6;
pub const TILE: (i32, i32) = (
    PREVIEW.0 + 2 * TILE_PADDING,
    PREVIEW.1 + CAPTION_GAP + CAPTION + 2 * TILE_PADDING,
);
/// Between tiles, and around them all.
pub const SPACING: i32 = 4;
pub const PADDING: i32 = 12;

/// The switcher while a switch goes on.
#[derive(Default)]
pub struct SwitcherView {
    /// Shown: the switch went on past `DELAY`.
    shown: bool,
    /// The delay of the switch in progress.
    timer: Option<RegistrationToken>,
    /// Where the panel was last drawn, redrawn when it changes or goes.
    drawn: Option<Rectangle<i32, Logical>>,
    buffers: Rc<RefCell<Buffers>>,
}

/// Kept between frames while it's shown: an unchanged switcher adds no
/// damage.
#[derive(Default)]
struct Buffers {
    frame: crate::panel::FrameBuffers,
    selected: Option<SolidColorBuffer>,
    /// Each tile's preview texture, by window (a handful: a list).
    previews: Vec<(Window, OffscreenWindow)>,
    /// Titles by text, buffer scale and whether selected.
    titles: HashMap<(String, i32, bool), MemoryRenderBuffer>,
}

/// One window's tile, in global coordinates.
#[derive(Clone)]
pub struct SceneTile {
    pub window: Window,
    pub rect: Rectangle<i32, Logical>,
    pub selected: bool,
    pub title: String,
}

/// What `Pila::scene` gives the renderer of the switcher shown.
#[derive(Clone)]
pub struct SceneSwitcher {
    /// The output it's on, by name.
    pub output: String,
    pub rect: Rectangle<i32, Logical>,
    pub tiles: Vec<SceneTile>,
    /// Its corners, the windows' (`corner-radius`, `rounded-corners`);
    /// `None`: square.
    pub rounding: Option<(f64, crate::rounded::Corners)>,
    buffers: Rc<RefCell<Buffers>>,
}

impl SceneSwitcher {
    /// The windows it previews, which get frame callbacks from its output.
    pub fn windows(&self) -> impl Iterator<Item = &Window> {
        self.tiles.iter().map(|tile| &tile.window)
    }
}

/// The first row shown of `len` rows when at most `max` show and the
/// `selected` one must: from the top until it would fall off the end.
pub fn first_shown(len: usize, max: usize, selected: Option<usize>) -> usize {
    let selected = selected.unwrap_or(0);
    if max == 0 || selected < max {
        return 0;
    }
    (selected + 1 - max).min(len.saturating_sub(max))
}

/// The panel's size for `tiles` tiles: rows of `COLUMNS`, at most `ROWS`.
pub fn panel_size(tiles: usize) -> Size<i32, Logical> {
    let columns = tiles.clamp(1, COLUMNS) as i32;
    let rows = (tiles.div_ceil(COLUMNS)).clamp(1, ROWS) as i32;
    let across = |n: i32, tile: i32| 2 * PADDING + n * tile + (n - 1) * SPACING;
    (across(columns, TILE.0), across(rows, TILE.1)).into()
}

/// A title as a tile shows it: one line, control characters dropped; the
/// app id where the window has no title.
pub(crate) fn caption(title: &str, app_id: &str) -> String {
    let title: String = title.chars().filter(|c| !c.is_control()).collect();
    if title.trim().is_empty() {
        app_id.to_string()
    } else {
        title
    }
}

impl Pila {
    /// The switch started, stepped, lost a window or ended
    /// (`Pila::switch_changed`): shown after `DELAY`, redrawn as it
    /// changes, gone with it.
    pub(crate) fn switcher_view_changed(&mut self) {
        if self.windows.switch.is_none() {
            if let Some(timer) = self.switcher_view.timer.take() {
                self.loop_handle.remove(timer);
            }
            self.switcher_view.shown = false;
            *self.switcher_view.buffers.borrow_mut() = Buffers::default();
            self.redraw_switcher_view();
            return;
        }
        if self.switcher_view.shown {
            self.redraw_switcher_view();
            return;
        }
        if self.switcher_view.timer.is_some() {
            return;
        }
        let timer = Timer::from_duration(DELAY);
        self.switcher_view.timer = self
            .loop_handle
            .insert_source(timer, |_, _, state| {
                state.switcher_view.timer = None;
                if state.windows.switch.is_some() && !state.is_locked() {
                    state.switcher_view.shown = true;
                    state.redraw_switcher_view();
                }
                TimeoutAction::Drop
            })
            .ok();
    }

    /// Redraw where the panel was and where it is now.
    fn redraw_switcher_view(&mut self) {
        if let Some(rect) = self.switcher_view.drawn.take() {
            self.queue_redraw_rect(rect);
        }
        if !self.switcher_view.shown {
            return;
        }
        if let Some((rect, ..)) = self.switcher_layout() {
            self.switcher_view.drawn = Some(rect);
            self.queue_redraw_rect(rect);
        }
    }

    /// A window committed: its tile is redrawn if it's shown.
    pub(crate) fn switcher_view_committed(&mut self, window: &Window) {
        if !self.switcher_view.shown {
            return;
        }
        let tile = self
            .switcher_layout()
            .and_then(|(_, tiles, _)| tiles.into_iter().find(|t| &t.window == window));
        if let Some(tile) = tile {
            self.queue_redraw_rect(tile.rect);
        }
    }

    /// The panel, its tiles and its output, from the switch in progress.
    fn switcher_layout(&self) -> Option<(Rectangle<i32, Logical>, Vec<SceneTile>, String)> {
        let switch = self.windows.switch.as_ref()?;
        let (order, index) = switch.order();
        let (output, area) = self.output_named_or_first(switch.output())?;
        let rect = crate::panel::centered(area, panel_size(order.len()));
        let rows = order.len().div_ceil(COLUMNS);
        let first = first_shown(rows, ROWS, Some(index / COLUMNS));
        let tiles = order
            .iter()
            .enumerate()
            .skip(first * COLUMNS)
            .take(ROWS * COLUMNS)
            .map(|(i, window)| {
                let shown = i - first * COLUMNS;
                let (column, row) = ((shown % COLUMNS) as i32, (shown / COLUMNS) as i32);
                let at = rect.loc
                    + Point::from((
                        PADDING + column * (TILE.0 + SPACING),
                        PADDING + row * (TILE.1 + SPACING),
                    ));
                SceneTile {
                    window: window.clone(),
                    rect: Rectangle::new(at, TILE.into()),
                    selected: i == index,
                    title: caption(
                        &crate::foreign_toplevel::title(window),
                        &crate::foreign_toplevel::app_id(window),
                    ),
                }
            })
            .collect();
        Some((rect, tiles, output.name()))
    }

    /// The switcher for `Pila::scene`, while it's shown.
    pub(crate) fn scene_switcher(&self) -> Option<SceneSwitcher> {
        if !self.switcher_view.shown {
            return None;
        }
        let (rect, tiles, output) = self.switcher_layout()?;
        Some(SceneSwitcher {
            output,
            rect,
            tiles,
            rounding: crate::panel::rounding(&self.config),
            buffers: self.switcher_view.buffers.clone(),
        })
    }

    /// Whether the switcher is shown at `location` (global): nothing under
    /// it gets the pointer.
    pub fn switcher_view_covers(&self, location: Point<f64, Logical>) -> bool {
        self.switcher_view.shown
            && self
                .switcher_layout()
                .is_some_and(|(rect, ..)| rect.to_f64().contains(location))
    }

    /// The window of the tile at `location` (global), if the switcher is
    /// shown there.
    pub fn switcher_view_window_at(&self, location: Point<f64, Logical>) -> Option<Window> {
        if !self.switcher_view.shown || self.is_locked() {
            return None;
        }
        let (_, tiles, _) = self.switcher_layout()?;
        tiles
            .into_iter()
            .find(|tile| tile.rect.to_f64().contains(location))
            .map(|tile| tile.window)
    }

    /// For the state dump: whether it's shown, where, and its tiles' rects.
    pub fn switcher_view_json(&self) -> serde_json::Value {
        match self.scene_switcher() {
            None => serde_json::json!({ "shown": false }),
            Some(view) => {
                let rect = crate::introspect::rect;
                serde_json::json!({
                    "shown": true,
                    "output": view.output,
                    "rect": rect(view.rect),
                    "tiles": view.tiles.iter().map(|t| serde_json::json!({
                        "app_id": crate::foreign_toplevel::app_id(&t.window),
                        "rect": rect(t.rect),
                        "selected": t.selected,
                    })).collect::<Vec<_>>(),
                })
            }
        }
    }
}

/// The switcher's elements, front to back, relative to `origin`.
pub fn elements(
    renderer: &mut GlesRenderer,
    view: &SceneSwitcher,
    colors: &Colors,
    origin: Point<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement> {
    let mut buffers = view.buffers.borrow_mut();
    let buffers = &mut *buffers;
    let buffer_scale = crate::scale::buffer_scale(scale);
    let mut elements = Vec::new();
    // Textures of windows no longer shown go.
    buffers
        .previews
        .retain(|(window, _)| view.tiles.iter().any(|t| &t.window == window));

    for tile in &view.tiles {
        // The title, under the preview.
        let color = if tile.selected {
            colors.hover_icon
        } else {
            colors.icon
        };
        let at =
            tile.rect.loc + Point::from((TILE_PADDING, TILE_PADDING + PREVIEW.1 + CAPTION_GAP));
        elements.extend(crate::panel::label(
            renderer,
            &mut buffers.titles,
            (tile.title.clone(), buffer_scale, tile.selected),
            &tile.title,
            color,
            (PREVIEW.0, CAPTION),
            at,
            origin,
            scale,
        ));
        // The preview.
        let area = Rectangle::new(
            tile.rect.loc + Point::from((TILE_PADDING, TILE_PADDING)),
            PREVIEW.into(),
        );
        if let Some(element) = preview(
            renderer,
            &mut buffers.previews,
            &tile.window,
            area,
            origin,
            scale,
        ) {
            elements.push(element);
        }
        if tile.selected {
            let buffer = crate::panel::solid_buffer(
                &mut buffers.selected,
                tile.rect.size,
                crate::panel::accent(colors),
            );
            elements.push(crate::render::solid_element(
                renderer,
                buffer,
                tile.rect.loc - origin,
                None,
                1.0,
                scale,
            ));
        }
    }
    elements.extend(crate::panel::frame(
        renderer,
        &mut buffers.frame,
        view.rect,
        view.rounding,
        colors,
        origin,
        scale,
    ));
    elements
}

/// `window` drawn into its preview's texture, scaled to fit `area` (never
/// grown) and centered in it: the texture redrawn where the window changed.
pub(crate) fn preview(
    renderer: &mut GlesRenderer,
    previews: &mut Vec<(Window, OffscreenWindow)>,
    window: &Window,
    area: Rectangle<i32, Logical>,
    origin: Point<i32, Logical>,
    scale: Scale<f64>,
) -> Option<PilaRenderElement> {
    let geometry = window.geometry();
    if geometry.size.w <= 0 || geometry.size.h <= 0 {
        return None;
    }
    let fit = (f64::from(area.size.w) / f64::from(geometry.size.w))
        .min(f64::from(area.size.h) / f64::from(geometry.size.h))
        .min(1.0);
    let size = Size::<i32, Logical>::from((
        ((f64::from(geometry.size.w) * fit).round() as i32).max(1),
        ((f64::from(geometry.size.h) * fit).round() as i32).max(1),
    ));
    let rect = crate::panel::centered(area, size);
    let size_now = crate::render::texture_size(rect, scale);
    let index = match previews.iter().position(|(w, _)| w == window) {
        Some(i) if previews[i].1.fits(rect, scale) => i,
        found => {
            if let Some(i) = found {
                previews.remove(i);
            }
            previews.push((
                window.clone(),
                OffscreenWindow::new(renderer, size_now, scale)?,
            ));
            previews.len() - 1
        }
    };
    let target = &mut previews[index].1;
    // The window drawn as it is, its geometry at the texture's top-left,
    // then shrunk as a whole about that corner: a surface element scales
    // only its place, not its size.
    let location: Point<i32, Physical> = (Point::<i32, Logical>::default() - geometry.loc)
        .to_f64()
        .to_physical(scale)
        .to_i32_round();
    let surfaces: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
        window.render_elements(renderer, location, scale, 1.0);
    let surfaces: Vec<PilaRenderElement> = surfaces
        .into_iter()
        .map(|surface| {
            PilaRenderElement::Rescaled(RescaleRenderElement::from_element(
                surface,
                Point::default(),
                fit,
            ))
        })
        .collect();
    if !target.draw(renderer, &surfaces) {
        return None;
    }
    Some(crate::render::texture_element(
        target.buffer(),
        size_now,
        rect.to_f64(),
        1.0,
        origin,
        scale,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_selected_row_stays_in_view() {
        assert_eq!(first_shown(2, 3, Some(1)), 0);
        assert_eq!(first_shown(5, 3, Some(2)), 0);
        assert_eq!(first_shown(5, 3, Some(3)), 1);
        assert_eq!(first_shown(5, 3, Some(4)), 2);
        assert_eq!(first_shown(5, 3, None), 0);
    }

    #[test]
    fn the_panel_fits_its_tiles() {
        // One tile: a single cell with the padding around it.
        assert_eq!(
            panel_size(1),
            (2 * PADDING + TILE.0, 2 * PADDING + TILE.1).into()
        );
        // Seven: a full row of five, two rows.
        let size = panel_size(7);
        assert_eq!(size.w, 2 * PADDING + 5 * TILE.0 + 4 * SPACING);
        assert_eq!(size.h, 2 * PADDING + 2 * TILE.1 + SPACING);
        // Forty: three rows at most.
        assert_eq!(panel_size(40).h, 2 * PADDING + 3 * TILE.1 + 2 * SPACING);
    }

    #[test]
    fn a_caption_is_one_line_or_the_app_id() {
        assert_eq!(caption("Home\u{1b}[2J", "files"), "Home[2J");
        assert_eq!(caption("  ", "foot"), "foot");
        assert_eq!(caption("a shell", "foot"), "a shell");
    }
}
