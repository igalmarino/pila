//! A region of the screen to share or take (`share_picker.rs`'s Region):
//! the panel hides, the pointer becomes a crosshair and the user drags a
//! rectangle over one output (clipped to the output the drag starts on);
//! letting go shares it (Enter too), Escape goes back to the panel. Every
//! output is dimmed but the rectangle, outlined in the accent, and a guide at
//! the top of the output under the pointer says what to do and the
//! rectangle's size, in the stream's pixels. Still the picker's, so still
//! modal: its keys and presses come here first (`input.rs`).
//!
//! What's picked is a `capture::Target::Region`: a monitor-type stream of
//! the rectangle (the portal has no other kind for it), or a screenshot of
//! it. While a region is shared, its outline stays on the screen, drawn on
//! the outputs alone (`outline_elements`, by the backends beside the
//! pointer), never in a capture.

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use smithay::{
    backend::renderer::{element::solid::SolidColorBuffer, gles::GlesRenderer},
    input::{keyboard::Keysym, pointer::CursorIcon, pointer::CursorImageStatus},
    reexports::calloop,
    utils::{Logical, Point, Rectangle, Scale},
};
use tracing::{info, warn};

use crate::{
    capture::Target,
    config::Colors,
    panel::{self, ContentImage, FrameBuffers, Item, NoText, TextKey},
    render::PilaRenderElement,
    Pila,
};

/// The guide's width (a `panel::banner`).
const GUIDE_WIDTH: i32 = 440;
/// The outline's width, outside the rectangle.
pub const OUTLINE: i32 = 2;
/// A drag shorter than this either way is a click: nothing is selected.
const MIN_SIDE: i32 = 4;

/// The selection while it's on (`SharePicker::region`).
#[derive(Default)]
pub struct Selecting {
    drag: Option<Drag>,
    /// The outputs drawn on last (the guide's and the drag's), redrawn when
    /// they change.
    drawn: Vec<String>,
    /// The guide's content being drawn on a thread, one at a time.
    pending: Option<TextKey>,
    buffers: Rc<RefCell<Buffers>>,
}

/// A rectangle dragged (or being dragged) over an output.
#[derive(Clone)]
struct Drag {
    output: String,
    from: Point<f64, Logical>,
    to: Point<f64, Logical>,
    /// The button is still down.
    held: bool,
}

#[derive(Default)]
struct Buffers {
    frame: FrameBuffers,
    text: Option<ContentImage>,
    /// The dims, by output and side.
    dims: HashMap<String, SolidColorBuffer>,
    outline: HashMap<&'static str, SolidColorBuffer>,
}

/// What `Pila::scene` gives the renderer of the selection.
#[derive(Clone)]
pub struct SceneRegion {
    /// The guide: its output and where it shows.
    pub guide_output: String,
    pub guide: Rectangle<i32, Logical>,
    /// The rectangle, global, and its output, once dragged.
    pub rect: Option<(String, Rectangle<i32, Logical>)>,
    pub rounding: panel::Shape,
    buffers: Rc<RefCell<Buffers>>,
}

/// What of `geo` is outside `rect`: above, below, left and right of it.
fn around(
    geo: Rectangle<i32, Logical>,
    rect: Rectangle<i32, Logical>,
) -> [Rectangle<i32, Logical>; 4] {
    let (top, bottom) = (
        rect.loc.y - geo.loc.y,
        geo.loc.y + geo.size.h - rect.loc.y - rect.size.h,
    );
    let (left, right) = (
        rect.loc.x - geo.loc.x,
        geo.loc.x + geo.size.w - rect.loc.x - rect.size.w,
    );
    [
        Rectangle::new(geo.loc, (geo.size.w, top.max(0)).into()),
        Rectangle::new(
            (geo.loc.x, rect.loc.y + rect.size.h).into(),
            (geo.size.w, bottom.max(0)).into(),
        ),
        Rectangle::new(
            (geo.loc.x, rect.loc.y).into(),
            (left.max(0), rect.size.h).into(),
        ),
        Rectangle::new(
            (rect.loc.x + rect.size.w, rect.loc.y).into(),
            (right.max(0), rect.size.h).into(),
        ),
    ]
}

/// The rectangle from `from` to `to` (either corner first), its edges on
/// whole logical pixels.
fn rect_between(from: Point<f64, Logical>, to: Point<f64, Logical>) -> Rectangle<i32, Logical> {
    let (x0, x1) = (from.x.min(to.x).floor(), from.x.max(to.x).ceil());
    let (y0, y1) = (from.y.min(to.y).floor(), from.y.max(to.y).ceil());
    Rectangle::new(
        (x0 as i32, y0 as i32).into(),
        ((x1 - x0) as i32, (y1 - y0) as i32).into(),
    )
}

/// `point` kept inside `geo`.
fn clamp(point: Point<f64, Logical>, geo: Rectangle<i32, Logical>) -> Point<f64, Logical> {
    let geo = geo.to_f64();
    Point::from((
        point.x.clamp(geo.loc.x, geo.loc.x + geo.size.w),
        point.y.clamp(geo.loc.y, geo.loc.y + geo.size.h),
    ))
}

/// The guide's line: what to do, then the rectangle's size in the stream's
/// pixels (`size`) once there is one.
fn items(colors: &Colors, screenshot: bool, size: Option<(i32, i32)>) -> Vec<Item> {
    let act = if screenshot { "take" } else { "share" };
    let line = match size {
        Some((w, h)) => format!("{w} × {h}: let go to {act} it"),
        None => format!("Drag over what to {act}"),
    };
    vec![panel::banner(line, GUIDE_WIDTH, colors)]
}

impl Pila {
    /// Region picked in the panel: select one. Never without fonts (the
    /// guide is text; the panel itself needs them too).
    pub(crate) fn start_region_select(&mut self) {
        if self.share_picker.region.is_some() {
            return;
        }
        info!("portal: selecting a region");
        self.share_picker.region = Some(Selecting::default());
        self.set_cursor(CursorImageStatus::Named(CursorIcon::Crosshair));
        // The panel goes from every output: they all redraw.
        self.queue_redraw();
        self.draw_region_text();
    }

    pub fn region_selecting(&self) -> bool {
        self.share_picker.region.is_some()
    }

    /// End the selection: back to the panel (Escape), or the picker's
    /// request answered or gone.
    pub(crate) fn end_region_select(&mut self) {
        if self.share_picker.region.take().is_none() {
            return;
        }
        self.set_cursor(CursorImageStatus::default_named());
        self.queue_redraw();
    }

    /// A key while selecting: Escape goes back to the panel, Enter shares
    /// the rectangle if there's one.
    pub(crate) fn region_key(&mut self, sym: Keysym) {
        match sym {
            Keysym::Escape => {
                info!("portal: back to the picker");
                self.end_region_select();
                self.draw_share_picker_text();
            }
            Keysym::Return | Keysym::KP_Enter => self.confirm_region(),
            _ => {}
        }
    }

    /// A press starts a rectangle on the output under it.
    pub(crate) fn region_press(&mut self, location: Point<f64, Logical>) {
        let Some(output) = self.output_at(location) else {
            return;
        };
        let Some(geo) = self.space.output_geometry(&output) else {
            return;
        };
        let Some(selecting) = self.share_picker.region.as_mut() else {
            return;
        };
        let at = clamp(location, geo);
        selecting.drag = Some(Drag {
            output: output.name(),
            from: at,
            to: at,
            held: true,
        });
        self.redraw_region();
    }

    /// The pointer moved: the rectangle follows it while the button is down
    /// (clipped to its output), the guide goes to the output under it.
    pub(crate) fn region_moved(&mut self, location: Point<f64, Logical>) {
        let Some(selecting) = self.share_picker.region.as_ref() else {
            return;
        };
        if let Some(drag) = selecting.drag.as_ref().filter(|d| d.held) {
            let geo = self
                .space
                .outputs()
                .find(|o| o.name() == drag.output)
                .and_then(|o| self.space.output_geometry(o));
            let Some(geo) = geo else {
                return;
            };
            let to = clamp(location, geo);
            if let Some(drag) = self
                .share_picker
                .region
                .as_mut()
                .and_then(|s| s.drag.as_mut())
            {
                drag.to = to;
            }
            self.redraw_region();
            self.draw_region_text();
            return;
        }
        let guide = self.scene_region().map(|view| view.guide_output);
        if guide.is_some_and(|name| !selecting.drawn.contains(&name)) {
            self.redraw_region();
            self.draw_region_text();
        }
    }

    /// The button let go: a rectangle is shared, a click (nothing dragged)
    /// leaves none.
    pub(crate) fn region_release(&mut self) {
        let Some(selecting) = self.share_picker.region.as_mut() else {
            return;
        };
        let Some(drag) = selecting.drag.as_mut().filter(|d| d.held) else {
            return;
        };
        drag.held = false;
        if self.region_selected().is_some() {
            self.confirm_region();
        } else {
            if let Some(selecting) = self.share_picker.region.as_mut() {
                selecting.drag = None;
            }
            self.redraw_region();
            self.draw_region_text();
        }
    }

    /// The rectangle selected, global, and its output's name: none before
    /// a drag, or if it's too small to be one.
    fn region_selected(&self) -> Option<(String, Rectangle<i32, Logical>)> {
        let drag = self.share_picker.region.as_ref()?.drag.as_ref()?;
        let rect = rect_between(drag.from, drag.to);
        (rect.size.w >= MIN_SIDE && rect.size.h >= MIN_SIDE).then(|| (drag.output.clone(), rect))
    }

    /// Share (or take) the rectangle selected, if there's one.
    fn confirm_region(&mut self) {
        let Some((name, rect)) = self.region_selected() else {
            return;
        };
        let Some(output) = self.space.outputs().find(|o| o.name() == name).cloned() else {
            return;
        };
        let Some(geo) = self.space.output_geometry(&output) else {
            return;
        };
        let local = Rectangle::new(rect.loc - geo.loc, rect.size);
        info!(output = name, region = ?local, "portal: region picked");
        self.finish_share_pick(Some(Target::Region(output.downgrade(), local)));
    }

    /// The selection for `Pila::scene`, while it's on.
    pub(crate) fn scene_region(&self) -> Option<SceneRegion> {
        let selecting = self.share_picker.region.as_ref()?;
        let rect = self.region_selected().or_else(|| {
            // A drag too small to share yet still shows where it is.
            let drag = selecting.drag.as_ref()?;
            Some((drag.output.clone(), rect_between(drag.from, drag.to)))
        });
        // The guide stays on the drag's output while it's held, else it's on
        // the one under the pointer.
        let held = selecting.drag.as_ref().filter(|d| d.held);
        let output = match held {
            Some(drag) => self
                .space
                .outputs()
                .find(|o| o.name() == drag.output)
                .cloned(),
            None => self.output_at(self.pointer_location()),
        }
        .or_else(|| self.space.outputs().next().cloned())?;
        let area = self.space.output_geometry(&output)?;
        let guide = panel::banner_rect(area, GUIDE_WIDTH);
        Some(SceneRegion {
            guide_output: output.name(),
            guide,
            rect,
            rounding: panel::rounding(&self.config),
            buffers: selecting.buffers.clone(),
        })
    }

    /// Redraw the outputs it was drawn on and those it's on now.
    fn redraw_region(&mut self) {
        let now: Vec<String> = self
            .scene_region()
            .map(|view| {
                let mut names = vec![view.guide_output];
                names.extend(view.rect.map(|(name, _)| name));
                names
            })
            .unwrap_or_default();
        let before = self
            .share_picker
            .region
            .as_mut()
            .map(|s| std::mem::replace(&mut s.drawn, now.clone()))
            .unwrap_or_default();
        let outputs: Vec<_> = self
            .space
            .outputs()
            .filter(|o| now.contains(&o.name()) || before.contains(&o.name()))
            .cloned()
            .collect();
        for output in outputs {
            self.queue_redraw_output(&output);
        }
    }

    /// The size the rectangle selected is shared at, in the stream's pixels.
    fn region_size(&self) -> Option<(i32, i32)> {
        let (name, rect) = self.region_selected()?;
        let output = self.space.outputs().find(|o| o.name() == name)?;
        let size = crate::capture::region_pixels(rect.size, crate::scale::output_scale(output));
        Some((size.w, size.h))
    }

    /// Draw the guide's text on a thread if what it says changed: one at a
    /// time, the latest asked for drawn once the one drawing is done.
    pub(crate) fn draw_region_text(&mut self) {
        let Some(view) = self.scene_region() else {
            return;
        };
        let screenshot = self.share_picker_screenshot();
        let scale = self.panel_text_scale(&view.guide_output);
        let key = TextKey {
            items: items(&self.config.colors, screenshot, self.region_size()),
            caps: panel::key_caps(&self.config.colors),
            size: (GUIDE_WIDTH, panel::BANNER_HEIGHT),
            scale,
        };
        let Some(selecting) = self.share_picker.region.as_mut() else {
            return;
        };
        let drawn = selecting.buffers.borrow().text.as_ref().map(|t| &t.key) == Some(&key);
        if drawn || selecting.pending.is_some() {
            return;
        }
        selecting.pending = Some(key.clone());
        let buffers = selecting.buffers.clone();
        let (sender, channel) = calloop::channel::channel();
        let job = key.clone();
        let spawned = std::thread::Builder::new()
            .name("pila-region".into())
            .spawn(move || {
                let _ = sender.send(panel::draw_text(&job));
            });
        if let Err(err) = spawned {
            warn!(?err, "can't draw the region guide's text");
            selecting.pending = None;
            return;
        }
        let inserted = self
            .loop_handle
            .insert_source(channel, move |event, _, state| {
                let calloop::channel::Event::Msg(pixels) = event else {
                    return;
                };
                // Still this selection's: it shows, and the latest is drawn.
                let Some(selecting) = state.share_picker.region.as_mut() else {
                    return;
                };
                if !Rc::ptr_eq(&selecting.buffers, &buffers) {
                    return;
                }
                selecting.pending = None;
                // Drawn, the latest is asked (the selection may have moved
                // meanwhile); with no fonts, their coming asks
                // (`Pila::draw_panels_text`), or this, if they came while it
                // was drawn; failed, not until what it says changes.
                let again = pixels.is_ok() || NoText::ask_again(&pixels);
                if let Ok(pixels) = pixels {
                    buffers.borrow_mut().text = Some(ContentImage::new(key.clone(), pixels));
                }
                state.redraw_region();
                if again {
                    state.draw_region_text();
                }
            });
        if let Err(err) = inserted {
            warn!(?err, "can't wait for the region guide's text");
            if let Some(selecting) = self.share_picker.region.as_mut() {
                selecting.pending = None;
            }
        }
    }

    /// The regions shared now, global, by output: each one's outline is
    /// drawn on the screen.
    pub(crate) fn shared_regions(&self) -> Vec<(String, Rectangle<i32, Logical>)> {
        self.screencast
            .region_targets()
            .filter_map(|target| {
                let crate::capture::TargetArea { output, area, .. } = self.target_area(target)?;
                Some((output.name(), area))
            })
            .collect()
    }

    /// The regions shared changed (a share started or ended): the outputs
    /// whose outlines come or go are redrawn.
    pub(crate) fn update_region_outlines(&mut self) {
        let now = self.shared_regions();
        if now == self.share_picker.outlined {
            return;
        }
        let before = std::mem::replace(&mut self.share_picker.outlined, now.clone());
        let names: Vec<&String> = before.iter().chain(&now).map(|(name, _)| name).collect();
        let outputs: Vec<_> = self
            .space
            .outputs()
            .filter(|o| names.contains(&&o.name()))
            .cloned()
            .collect();
        for output in outputs {
            self.queue_redraw_output(&output);
        }
    }

    /// For the state dump: the selection, if it's on.
    pub fn region_json(&self) -> serde_json::Value {
        let rect = crate::introspect::rect;
        match self.scene_region() {
            None => serde_json::json!({ "selecting": false }),
            Some(view) => serde_json::json!({
                "selecting": true,
                "guide_output": view.guide_output,
                "guide": rect(view.guide),
                "output": view.rect.as_ref().map(|(name, _)| name.clone()),
                "rect": view.rect.map(|(_, r)| rect(r)),
                "size": self.region_size(),
                "text": view.buffers.borrow().text.is_some(),
            }),
        }
    }
}

/// The selection's elements for the output `output_name`, front to back,
/// relative to its top-left: the guide on its output, the rectangle's
/// outline, and the dim around the rectangle (over all of every other
/// output).
pub fn elements(
    renderer: &mut GlesRenderer,
    view: &SceneRegion,
    colors: &Colors,
    output_name: &str,
    output_geo: Rectangle<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement> {
    let origin = output_geo.loc;
    let mut buffers = view.buffers.borrow_mut();
    let buffers = &mut *buffers;
    let mut elements = Vec::new();
    if output_name == view.guide_output {
        if let Some(text) = buffers.text.as_mut() {
            elements.extend(text.element(renderer, view.guide, origin, scale));
            elements.extend(panel::frame(
                renderer,
                &mut buffers.frame,
                view.guide,
                view.rounding,
                colors,
                origin,
                scale,
            ));
        }
    }
    let rect = view
        .rect
        .as_ref()
        .filter(|(name, _)| name == output_name)
        .map(|(_, rect)| *rect);
    match rect {
        Some(rect) => {
            let sides = panel::outline_sides(rect, OUTLINE);
            let names = ["top", "bottom", "left", "right"];
            let accent = panel::accent(colors);
            elements.extend(panel::solids(
                renderer,
                &mut buffers.outline,
                names
                    .iter()
                    .zip(sides)
                    .map(|(name, side)| (*name, side, accent, None)),
                origin,
                scale,
            ));
            for (side, piece) in names.iter().zip(around(output_geo, rect)) {
                if !piece.is_empty() {
                    elements.push(panel::dim(
                        &mut buffers.dims,
                        &format!("{output_name} {side}"),
                        piece,
                        1.0,
                        origin,
                        scale,
                    ));
                }
            }
        }
        None => elements.push(panel::dim(
            &mut buffers.dims,
            output_name,
            output_geo,
            1.0,
            origin,
            scale,
        )),
    }
    elements
}

/// The outlines of the regions shared on `output_name` (`shared`: by
/// output, global), relative to `origin`: on the screen alone, the backends
/// add them beside the pointer.
pub fn outline_elements(
    buffers: &RefCell<HashMap<String, SolidColorBuffer>>,
    shared: &[(String, Rectangle<i32, Logical>)],
    colors: &Colors,
    output_name: &str,
    origin: Point<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement> {
    let mut buffers = buffers.borrow_mut();
    let accent = panel::accent(colors);
    let mut elements = Vec::new();
    for (i, (_, rect)) in shared
        .iter()
        .enumerate()
        .filter(|(_, (name, _))| name == output_name)
    {
        for (side, piece) in panel::outline_sides(*rect, OUTLINE).into_iter().enumerate() {
            elements.push(panel::keyed_solid(
                &mut buffers,
                format!("{i} {side}"),
                piece,
                accent,
                1.0,
                origin,
                scale,
            ));
        }
    }
    elements
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    #[test]
    fn a_drag_either_way_is_the_same_rectangle() {
        let a = Point::from((10.4, 20.6));
        let b = Point::from((110.2, 70.0));
        assert_eq!(rect_between(a, b), rect(10, 20, 101, 50));
        assert_eq!(rect_between(b, a), rect(10, 20, 101, 50));
    }

    #[test]
    fn the_dim_covers_everything_but_the_rectangle() {
        let geo = rect(100, 0, 800, 600);
        let inside = rect(200, 100, 300, 200);
        let pieces = around(geo, inside);
        let area: i32 = pieces.iter().map(|p| p.size.w * p.size.h).sum();
        assert_eq!(area, 800 * 600 - 300 * 200);
        assert!(pieces
            .iter()
            .all(|p| p.intersection(inside).is_none_or(|i| i.is_empty())));
    }

    #[test]
    fn the_outline_is_outside_the_rectangle() {
        let inside = rect(10, 10, 50, 40);
        for side in panel::outline_sides(inside, OUTLINE) {
            assert!(side.intersection(inside).is_none_or(|i| i.is_empty()));
        }
    }
}
