//! The screen-share picker drawn by Pila itself: when an app asks the portal
//! Pila serves to share a screen (`portal.rs`), a panel in the middle of the
//! output under the pointer names the app and shows every output as a live
//! preview; the user picks one (a click, or the arrows) and shares it (Share,
//! Enter), or cancels (Cancel, Escape). Nothing is shared without it: this is
//! the consent.
//!
//! While it's shown it's modal: every key goes to it (`input.rs`), a click
//! does nothing but on its controls, and every output is dimmed under it.
//! Requests wait their turn in `ScreenCasts::waiting`; locking the screen
//! cancels them all, and the app closing its session takes its request away.
//!
//! Its texts are drawn on a thread into one image the size of the panel, as
//! the cheatsheet's are (`panel::draw_text`); the panel shows from
//! the start, its text once it's there. Each preview is its output's scene
//! drawn small into a texture of its own with its own damage tracker
//! (`render::OffscreenWindow`), from `after_frames` like captures
//! (`Pila::update_share_previews`): an output that didn't change costs no
//! drawing, and one that did redraws the picker's output.

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use smithay::{
    backend::renderer::{
        element::{
            solid::{SolidColorBuffer, SolidColorRenderElement},
            Kind,
        },
        gles::GlesRenderer,
    },
    input::keyboard::Keysym,
    reexports::calloop,
    utils::{Logical, Physical, Point, Rectangle, Scale, Size},
};
use tracing::{info, warn};

use crate::{
    config::Colors,
    panel::{bits, draw_text, Align, ContentImage, Font, FrameBuffers, Item, TextKey},
    portal::Refused,
    render::{OffscreenWindow, PilaRenderElement},
    screencast::Share,
    Pila,
};

/// The panel's room around its content, and where its lines go.
const PADDING: i32 = 28;
const TITLE_AT: i32 = 22;
const TITLE_HEIGHT: i32 = 30;
const NOTE_AT: i32 = 56;
const NOTE_HEIGHT: i32 = 20;
/// The tiles: a preview's box, the label under it, the room between them.
const TILES_AT: i32 = 100;
pub const TILE: (i32, i32) = (320, 180);
const LABEL_GAP: i32 = 14;
const LABEL_HEIGHT: i32 = 20;
const TILE_GAP: i32 = 64;
const ROW_GAP: i32 = 24;
const COLUMNS: usize = 3;
/// The selected tile's highlight: around the tile and its label.
const SELECTED_MARGIN: i32 = 6;
/// The buttons' corners: the cheatsheet's keys'.
const KEY_RADIUS: i32 = crate::panel::CAP_RADIUS;
/// The footer: the buttons, and the room above it.
const FOOTER_GAP: i32 = 32;
const BUTTON_HEIGHT: i32 = 36;
const SHARE_WIDTH: i32 = 120;
const CANCEL_WIDTH: i32 = 110;
const BUTTON_GAP: i32 = 12;
/// Room between the hints and the buttons.
const FOOTER_SPACE: i32 = 24;
/// The narrowest panel (one source): its footer, the hints then the buttons.
const MIN_WIDTH: i32 =
    2 * PADDING + HINTS_WIDTH + FOOTER_SPACE + CANCEL_WIDTH + BUTTON_GAP + SHARE_WIDTH;
/// A hint's keycap slot, before its text, and where each hint starts.
const HINT_KEYS: i32 = 60;
const HINT_TEXT: i32 = 70;
const HINT_SLOTS: [i32; 3] = [0, 120, 250];
/// All of the hints: the last one's keycap and text.
const HINTS_WIDTH: i32 = HINT_SLOTS[2] + HINT_KEYS - 6 + HINT_TEXT;

/// The picker while it's shown (`Pila::share_picker`).
#[derive(Default)]
pub struct SharePicker {
    shown: Option<Shown>,
    /// The content being drawn on a thread (`Pila::draw_share_picker_text`).
    pending: Option<TextKey>,
    buffers: Rc<RefCell<Buffers>>,
}

struct Shown {
    share: Share,
    /// The output the panel is on, by name.
    output: String,
    /// The outputs to pick from, by name, left to right.
    sources: Vec<String>,
    selected: usize,
}

/// Kept between frames while it's shown: an unchanged panel adds no damage.
#[derive(Default)]
struct Buffers {
    frame: FrameBuffers,
    /// The panel's other solids, by their place in `elements`.
    solids: HashMap<&'static str, SolidColorBuffer>,
    /// The dim over each output, by name.
    dims: HashMap<String, SolidColorBuffer>,
    text: Option<ContentImage>,
    /// Each output's preview, by name (`Pila::update_share_previews`), and
    /// the background drawn behind it (kept: a new one would be damage).
    previews: HashMap<String, OffscreenWindow>,
    backgrounds: HashMap<String, SolidColorBuffer>,
}

/// Where the panel and its parts are, global.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub panel: Rectangle<i32, Logical>,
    /// Each source's tile: the box its preview fits in.
    pub tiles: Vec<Rectangle<i32, Logical>>,
    pub share: Rectangle<i32, Logical>,
    pub cancel: Rectangle<i32, Logical>,
    /// The key hints, left of the buttons.
    pub hints: Rectangle<i32, Logical>,
}

/// The panel for `count` sources, in the middle of `area`: up to `COLUMNS`
/// tiles a row, each row centered.
pub fn layout(area: Rectangle<i32, Logical>, count: usize) -> Layout {
    let count = count.max(1);
    let columns = count.min(COLUMNS) as i32;
    let rows = count.div_ceil(COLUMNS) as i32;
    let tile_row = TILE.1 + LABEL_GAP + LABEL_HEIGHT;
    let width = (2 * PADDING + columns * TILE.0 + (columns - 1) * TILE_GAP).max(MIN_WIDTH);
    let height =
        TILES_AT + rows * tile_row + (rows - 1) * ROW_GAP + FOOTER_GAP + BUTTON_HEIGHT + PADDING;
    let panel = Rectangle::new(
        area.loc + Point::from(((area.size.w - width) / 2, (area.size.h - height) / 2)),
        Size::from((width, height)),
    );
    let tiles = (0..count)
        .map(|i| {
            let (row, column) = ((i / COLUMNS) as i32, (i % COLUMNS) as i32);
            let in_row = (count - row as usize * COLUMNS).min(COLUMNS) as i32;
            let row_width = in_row * TILE.0 + (in_row - 1) * TILE_GAP;
            let x = (width - row_width) / 2 + column * (TILE.0 + TILE_GAP);
            let y = TILES_AT + row * (tile_row + ROW_GAP);
            Rectangle::new(panel.loc + Point::from((x, y)), Size::from(TILE))
        })
        .collect();
    let buttons_y = height - PADDING - BUTTON_HEIGHT;
    let share = Rectangle::new(
        panel.loc + Point::from((width - PADDING - SHARE_WIDTH, buttons_y)),
        Size::from((SHARE_WIDTH, BUTTON_HEIGHT)),
    );
    let cancel = Rectangle::new(
        share.loc - Point::from((BUTTON_GAP + CANCEL_WIDTH, 0)),
        Size::from((CANCEL_WIDTH, BUTTON_HEIGHT)),
    );
    let hints = Rectangle::new(
        panel.loc + Point::from((PADDING, buttons_y)),
        Size::from((HINTS_WIDTH, BUTTON_HEIGHT)),
    );
    Layout {
        panel,
        tiles,
        share,
        cancel,
        hints,
    }
}

/// Where a preview of a `source`-sized output goes in its `tile`: as large
/// as fits, its shape kept, centered.
pub fn preview_rect(
    tile: Rectangle<i32, Logical>,
    source: Size<i32, Logical>,
) -> Rectangle<i32, Logical> {
    let (sw, sh) = (f64::from(source.w.max(1)), f64::from(source.h.max(1)));
    let fit = (f64::from(tile.size.w) / sw).min(f64::from(tile.size.h) / sh);
    let size = Size::from(((sw * fit).round() as i32, (sh * fit).round() as i32));
    Rectangle::new(
        tile.loc + Point::from(((tile.size.w - size.w) / 2, (tile.size.h - size.h) / 2)),
        size,
    )
}

/// The highlight around the selected `tile`: it and its label.
fn selected_rect(tile: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
    Rectangle::new(
        tile.loc - Point::from((SELECTED_MARGIN, SELECTED_MARGIN)),
        tile.size
            + Size::from((
                2 * SELECTED_MARGIN,
                LABEL_GAP + LABEL_HEIGHT + 2 * SELECTED_MARGIN,
            )),
    )
}

/// A key's effect on the picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEffect {
    Previous,
    Next,
    Share,
    Cancel,
    Nothing,
}

pub fn key_effect(sym: Keysym) -> KeyEffect {
    match sym {
        Keysym::Left | Keysym::Up | Keysym::ISO_Left_Tab => KeyEffect::Previous,
        Keysym::Right | Keysym::Down | Keysym::Tab => KeyEffect::Next,
        Keysym::Return | Keysym::KP_Enter | Keysym::space => KeyEffect::Share,
        Keysym::Escape => KeyEffect::Cancel,
        _ => KeyEffect::Nothing,
    }
}

/// What `Pila::scene` gives the renderer of the picker shown.
#[derive(Clone)]
pub struct ScenePicker {
    /// The output the panel is on, by name.
    pub output: String,
    pub layout: Layout,
    /// The sources' previews: output name and where it shows.
    pub previews: Vec<(String, Rectangle<i32, Logical>)>,
    pub selected: usize,
    /// The windows' corners (`corner-radius`, `rounded-corners`); `None`:
    /// square.
    pub rounding: Option<(f64, crate::rounded::Corners)>,
    buffers: Rc<RefCell<Buffers>>,
}

impl Pila {
    /// Show the picker for the first request waiting, unless one is shown.
    /// Never while locked: the requests are cancelled.
    pub(crate) fn show_share_picker(&mut self) {
        if self.share_picker.shown.is_some() || self.screencast.waiting.is_empty() {
            return;
        }
        if self.is_locked() {
            for share in self.screencast.waiting.drain(..) {
                share.reply.send(Err(Refused::Cancelled));
            }
            return;
        }
        let mut outputs: Vec<_> = self
            .space
            .outputs()
            .filter_map(|o| Some((self.space.output_geometry(o)?, o.name())))
            .collect();
        outputs.sort_by_key(|(geo, _)| (geo.loc.x, geo.loc.y));
        let Some(share) = self.screencast.waiting.pop_front() else {
            return;
        };
        if outputs.is_empty() {
            share.reply.send(Err(Refused::Cancelled));
            return;
        }
        let pointer = self.pointer_location();
        let under = outputs
            .iter()
            .position(|(geo, _)| geo.to_f64().contains(pointer))
            .unwrap_or(0);
        let output = outputs[under].1.clone();
        // Modal, in front of everything: nothing else of Pila's stays open.
        self.end_switch();
        self.hide_shortcuts();
        self.close_overview();
        self.close_window_menu();
        info!(
            app = share.app.as_deref().unwrap_or("?"),
            "portal: asking which screen to share"
        );
        self.share_picker.shown = Some(Shown {
            share,
            output,
            sources: outputs.into_iter().map(|(_, name)| name).collect(),
            selected: under,
        });
        *self.share_picker.buffers.borrow_mut() = Buffers::default();
        self.share_picker.pending = None;
        // The previews are drawn after the next frames, the first now.
        self.capture_due = true;
        self.wake.ping();
        self.queue_redraw();
        self.draw_share_picker_text();
    }

    pub fn share_picker_open(&self) -> bool {
        self.share_picker.shown.is_some()
    }

    /// Hide the picker, giving back its request; the next one waiting is
    /// shown by the caller.
    fn take_share_picker(&mut self) -> Option<Shown> {
        let shown = self.share_picker.shown.take()?;
        self.share_picker.pending = None;
        *self.share_picker.buffers.borrow_mut() = Buffers::default();
        self.queue_redraw();
        Some(shown)
    }

    /// The user cancelled (Cancel, Escape), or the screen locked: the app
    /// is told, and the next request is asked.
    pub fn cancel_share_picker(&mut self) {
        let Some(shown) = self.take_share_picker() else {
            return;
        };
        info!("portal: sharing cancelled");
        shown.share.reply.send(Err(Refused::Cancelled));
        self.show_share_picker();
    }

    /// The app closed the session whose picker is shown: it goes, and the
    /// next request is asked.
    pub(crate) fn close_share_picker_of(&mut self, session: &str) {
        if self
            .share_picker
            .shown
            .as_ref()
            .is_some_and(|shown| shown.share.session == session)
        {
            self.take_share_picker();
            self.show_share_picker();
        }
    }

    /// Share the selected output (Share, Enter).
    fn pick_share(&mut self) {
        let Some(shown) = self.take_share_picker() else {
            return;
        };
        let output = shown
            .sources
            .get(shown.selected)
            .and_then(|name| self.space.outputs().find(|o| &o.name() == name).cloned());
        match output {
            Some(output) => self.start_cast(shown.share, output),
            None => shown.share.reply.send(Err(Refused::Cancelled)),
        }
        self.show_share_picker();
    }

    fn select_share(&mut self, index: usize) {
        let Some(shown) = self.share_picker.shown.as_mut() else {
            return;
        };
        if index < shown.sources.len() && index != shown.selected {
            shown.selected = index;
            let output = shown.output.clone();
            self.queue_redraw_named_output(&output);
        }
    }

    /// A key pressed while it's shown (it takes them all: `input.rs`).
    pub fn share_picker_key(&mut self, sym: Keysym) {
        let Some(shown) = self.share_picker.shown.as_ref() else {
            return;
        };
        let (selected, count) = (shown.selected, shown.sources.len());
        match key_effect(sym) {
            KeyEffect::Previous => self.select_share((selected + count - 1) % count),
            KeyEffect::Next => self.select_share((selected + 1) % count),
            KeyEffect::Share => self.pick_share(),
            KeyEffect::Cancel => self.cancel_share_picker(),
            KeyEffect::Nothing => {}
        }
    }

    /// A press at `location` while it's shown (it takes them all): a tile
    /// selects its output, the buttons share or cancel, elsewhere nothing.
    pub fn share_picker_press(&mut self, location: Point<f64, Logical>) {
        let Some(view) = self.scene_share_picker() else {
            return;
        };
        let layout = &view.layout;
        if layout.share.to_f64().contains(location) {
            self.pick_share();
        } else if layout.cancel.to_f64().contains(location) {
            self.cancel_share_picker();
        } else if let Some(index) = layout
            .tiles
            .iter()
            .position(|tile| selected_rect(*tile).to_f64().contains(location))
        {
            self.select_share(index);
        }
    }

    fn queue_redraw_named_output(&mut self, name: &str) {
        let output = self.space.outputs().find(|o| o.name() == name).cloned();
        if let Some(output) = output {
            self.queue_redraw_output(&output);
        }
    }

    /// The picker for `Pila::scene`, while it's shown and not locked.
    pub(crate) fn scene_share_picker(&self) -> Option<ScenePicker> {
        let shown = self.share_picker.shown.as_ref()?;
        let output = self
            .space
            .outputs()
            .find(|o| o.name() == shown.output)
            .or_else(|| self.space.outputs().next())?;
        let area = self.space.output_geometry(output)?;
        let layout = layout(area, shown.sources.len());
        let previews = shown
            .sources
            .iter()
            .zip(&layout.tiles)
            .filter_map(|(name, tile)| {
                let source = self.space.outputs().find(|o| &o.name() == name)?;
                let geo = self.space.output_geometry(source)?;
                Some((name.clone(), preview_rect(*tile, geo.size)))
            })
            .collect();
        Some(ScenePicker {
            output: output.name(),
            layout,
            previews,
            selected: shown.selected,
            rounding: crate::panel::rounding(&self.config),
            buffers: self.share_picker.buffers.clone(),
        })
    }

    /// Draw the previews whose outputs changed, from `after_frames` (`input`
    /// is the scene of this pass, shared with the captures), and redraw the
    /// picker's output if one did.
    pub fn update_share_previews(&mut self, input: &mut Option<crate::render::FrameInput>) {
        let Some(view) = self.scene_share_picker() else {
            return;
        };
        let Some(panel_output) = self
            .space
            .outputs()
            .find(|o| o.name() == view.output)
            .cloned()
        else {
            return;
        };
        let panel_scale = crate::scale::output_scale(&panel_output);
        let sources: Vec<_> = view
            .previews
            .iter()
            .filter_map(|(name, rect)| {
                let output = self.space.outputs().find(|o| &o.name() == name)?;
                Some((name.clone(), *rect, self.space.output_geometry(output)?))
            })
            .collect();
        let input = &*input.get_or_insert_with(|| self.frame_input());
        let background = crate::render::clear_color(&input.scene);
        let Some(backend) = self.backend.as_mut() else {
            return;
        };
        let Some(mut renderer) = backend.renderer() else {
            return;
        };
        let renderer = &mut *renderer;
        let mut changed = false;
        let mut buffers = view.buffers.borrow_mut();
        for (name, rect, geo) in sources {
            let size: Size<i32, Physical> =
                rect.size.to_f64().to_physical(panel_scale).to_i32_round();
            if size.w <= 0 || size.h <= 0 {
                continue;
            }
            let scale = Scale::from(f64::from(size.w) / f64::from(geo.size.w.max(1)));
            let fits = buffers
                .previews
                .get(&name)
                .is_some_and(|preview| preview.size() == size);
            if !fits {
                match OffscreenWindow::new(renderer, size, scale) {
                    Some(preview) => {
                        buffers.previews.insert(name.clone(), preview);
                    }
                    None => continue,
                }
            }
            let mut elements = crate::render::scene_elements(
                renderer,
                &input.scene,
                geo.loc,
                scale,
                &mut |_, _, _, _| None,
                Some(geo),
            );
            let back = buffers
                .backgrounds
                .entry(name.clone())
                .or_insert_with(|| SolidColorBuffer::new((size.w, size.h), background));
            back.update((size.w, size.h), background);
            elements.push(PilaRenderElement::Solid(
                SolidColorRenderElement::from_buffer(
                    back,
                    (0, 0),
                    Scale::from(1.0),
                    1.0,
                    Kind::Unspecified,
                ),
            ));
            let Some(preview) = buffers.previews.get_mut(&name) else {
                continue;
            };
            if preview.draw_damaged(renderer, &elements) == Some(true) {
                changed = true;
            }
        }
        drop(buffers);
        if changed {
            self.queue_redraw_output(&panel_output);
        }
    }

    /// Draw the panel's texts if what they say changed: on a thread, into
    /// one image the panel shows once it's back.
    pub(crate) fn draw_share_picker_text(&mut self) {
        let Some(view) = self.scene_share_picker() else {
            return;
        };
        let Some(shown) = self.share_picker.shown.as_ref() else {
            return;
        };
        let scale = self
            .space
            .outputs()
            .find(|o| o.name() == view.output)
            .map(|o| crate::scale::buffer_scale(Scale::from(o.current_scale().fractional_scale())))
            .unwrap_or(1);
        let labels: Vec<String> = shown
            .sources
            .iter()
            .map(|name| {
                let size = self
                    .space
                    .outputs()
                    .find(|o| &o.name() == name)
                    .and_then(|o| o.current_mode())
                    .map(|mode| format!("  {} × {}", mode.size.w, mode.size.h))
                    .unwrap_or_default();
                format!("{name}{size}")
            })
            .collect();
        let key = TextKey {
            items: items(
                &view.layout,
                shown.share.app.as_deref(),
                &labels,
                &self.config.colors,
            ),
            caps: crate::panel::key_caps(&self.config.colors),
            size: (view.layout.panel.size.w, view.layout.panel.size.h),
            scale,
        };
        let drawn = view.buffers.borrow().text.as_ref().map(|t| &t.key) == Some(&key);
        if drawn || self.share_picker.pending.as_ref() == Some(&key) {
            return;
        }
        self.share_picker.pending = Some(key.clone());
        let (sender, channel) = calloop::channel::channel();
        let job = key.clone();
        let spawned = std::thread::Builder::new()
            .name("pila-share-picker".into())
            .spawn(move || {
                let _ = sender.send(draw_text(&job));
            });
        if let Err(err) = spawned {
            warn!(?err, "can't draw the share picker's text");
            self.share_picker.pending = None;
            return;
        }
        let inserted = self
            .loop_handle
            .insert_source(channel, move |event, _, state| {
                let calloop::channel::Event::Msg(pixels) = event else {
                    return;
                };
                if state.share_picker.pending.as_ref() != Some(&key) {
                    return;
                }
                state.share_picker.pending = None;
                let Some(pixels) = pixels else {
                    // No fonts: the panel goes on without its text.
                    return;
                };
                state.share_picker.buffers.borrow_mut().text =
                    Some(ContentImage::new(key.clone(), pixels));
                if let Some(view) = state.scene_share_picker() {
                    state.queue_redraw_named_output(&view.output);
                }
            });
        if let Err(err) = inserted {
            warn!(?err, "can't wait for the share picker's text");
            self.share_picker.pending = None;
        }
    }

    /// For the state dump: whether it's shown, for whom, where, its sources
    /// and controls.
    pub fn share_picker_json(&self) -> serde_json::Value {
        let rect = |r: Rectangle<i32, Logical>| serde_json::json!({ "x": r.loc.x, "y": r.loc.y, "w": r.size.w, "h": r.size.h });
        let (Some(view), Some(shown)) =
            (self.scene_share_picker(), self.share_picker.shown.as_ref())
        else {
            return serde_json::json!({ "shown": false, "waiting": self.screencast.waiting.len() });
        };
        serde_json::json!({
            "shown": true,
            "app": shown.share.app,
            "output": view.output,
            "sources": shown.sources,
            "selected": view.selected,
            "panel": rect(view.layout.panel),
            "tiles": view.layout.tiles.iter().map(|t| rect(*t)).collect::<Vec<_>>(),
            "share": rect(view.layout.share),
            "cancel": rect(view.layout.cancel),
            "text": view.buffers.borrow().text.is_some(),
            "previews": view.buffers.borrow().previews.len(),
            "waiting": self.screencast.waiting.len(),
        })
    }
}

/// The panel's texts, at places from its top-left.
fn items(layout: &Layout, app: Option<&str>, labels: &[String], colors: &Colors) -> Vec<Item> {
    let panel = layout.panel;
    let width = panel.size.w - 2 * PADDING;
    let local = |r: Rectangle<i32, Logical>| (r.loc.x - panel.loc.x, r.loc.y - panel.loc.y);
    let (text, dimmed) = (bits(colors.icon), bits(colors.icon_inactive));
    let title = match app {
        Some(app) => format!("Share a screen with {app}"),
        None => "An app asks to share a screen".to_string(),
    };
    let mut items = vec![
        Item::Text {
            text: title,
            at: (PADDING, TITLE_AT),
            w: width,
            h: TITLE_HEIGHT,
            font: Font {
                size: 20,
                bold: false,
            },
            align: Align::Left,
            color: text,
        },
        Item::Text {
            text: "It will see everything the screen shows until it stops sharing.".into(),
            at: (PADDING, NOTE_AT),
            w: width,
            h: NOTE_HEIGHT,
            font: Font {
                size: 13,
                bold: false,
            },
            align: Align::Left,
            color: dimmed,
        },
    ];
    for (tile, label) in layout.tiles.iter().zip(labels) {
        let (x, y) = local(*tile);
        items.push(Item::Text {
            text: label.clone(),
            at: (x, y + tile.size.h + LABEL_GAP),
            w: tile.size.w,
            h: LABEL_HEIGHT,
            font: Font {
                size: 14,
                bold: false,
            },
            align: Align::Center,
            color: text,
        });
    }
    let buttons_y = local(layout.share).1;
    for (rect, label) in [(layout.share, "Share"), (layout.cancel, "Cancel")] {
        let (x, y) = local(rect);
        items.push(Item::Text {
            text: label.into(),
            at: (x, y + (BUTTON_HEIGHT - LABEL_HEIGHT) / 2),
            w: rect.size.w,
            h: LABEL_HEIGHT,
            font: Font {
                size: 14,
                bold: false,
            },
            align: Align::Center,
            color: text,
        });
    }
    let hints_y = buttons_y + (BUTTON_HEIGHT - 22) / 2;
    for (slot, (cap, what)) in
        HINT_SLOTS
            .iter()
            .zip([("← →", "pick"), ("Enter", "share"), ("Esc", "cancel")])
    {
        items.push(Item::Keys {
            caps: vec![cap.to_string()],
            at: (PADDING + slot, hints_y),
            w: HINT_KEYS,
            size: 12,
        });
        items.push(Item::Text {
            text: what.into(),
            at: (PADDING + slot + HINT_KEYS - 6, hints_y + 1),
            w: HINT_TEXT,
            h: 20,
            font: Font {
                size: 13,
                bold: false,
            },
            align: Align::Left,
            color: dimmed,
        });
    }
    items
}

/// One of the panel's solids: its name (its buffer's key), where it is,
/// its color, and its corners.
type PanelSolid = (
    &'static str,
    Rectangle<i32, Logical>,
    [f32; 4],
    crate::panel::Shape,
);

/// The picker's elements for the output `output_name`, front to back,
/// relative to `origin`: the panel on its output, and the dim on every one.
pub fn elements(
    renderer: &mut GlesRenderer,
    view: &ScenePicker,
    colors: &Colors,
    output_name: &str,
    output_geo: Rectangle<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement> {
    let origin = output_geo.loc;
    let mut buffers = view.buffers.borrow_mut();
    let buffers = &mut *buffers;
    let dim = crate::panel::dim(
        &mut buffers.dims,
        output_name,
        output_geo,
        1.0,
        origin,
        scale,
    );
    if output_name != view.output {
        return vec![dim];
    }
    let layout = &view.layout;
    let mut elements = Vec::new();
    if let Some(text) = buffers.text.as_mut() {
        elements.extend(text.element(renderer, layout.panel, origin, scale));
    }
    for (name, rect) in &view.previews {
        if let Some(preview) = buffers.previews.get(name) {
            elements.push(crate::render::texture_element(
                preview.buffer(),
                preview.size(),
                rect.to_f64(),
                1.0,
                origin,
                scale,
            ));
        }
    }

    let accent = crate::panel::accent(colors);
    let hover = crate::panel::rgb(colors.hover);
    let key_fill = crate::panel::opaque(colors.bar);
    let inset = |rect: Rectangle<i32, Logical>, by: i32| {
        Rectangle::new(
            rect.loc + Point::from((by, by)),
            rect.size - Size::from((2 * by, 2 * by)),
        )
    };
    // The buttons as the cheatsheet's keys: rounded, Cancel with their
    // edge and fill, Share in the accent.
    let key = Some((f64::from(KEY_RADIUS), crate::rounded::Corners::All));
    let key_inner = Some((f64::from(KEY_RADIUS - 1), crate::rounded::Corners::All));
    let mut solids: Vec<PanelSolid> = Vec::new();
    // The selected tile on a block of the accent, as the switcher's.
    if let Some(tile) = layout.tiles.get(view.selected) {
        solids.push(("selected", selected_rect(*tile), accent, None));
    }
    solids.push(("share", layout.share, accent, key));
    solids.push(("cancel", inset(layout.cancel, 1), key_fill, key_inner));
    solids.push(("cancel-edge", layout.cancel, hover, key));
    for (name, rect, color, rounding) in solids {
        let buffer = buffers
            .solids
            .entry(name)
            .or_insert_with(|| SolidColorBuffer::new(rect.size, color));
        buffer.update(rect.size, color);
        elements.push(crate::render::solid_element(
            renderer,
            buffer,
            rect.loc - origin,
            rounding,
            1.0,
            scale,
        ));
    }
    elements.extend(crate::panel::frame(
        renderer,
        &mut buffers.frame,
        layout.panel,
        view.rounding,
        colors,
        origin,
        scale,
    ));
    elements.push(dim);
    elements
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rectangle<i32, Logical> {
        Rectangle::new((0, 0).into(), (1920, 1080).into())
    }

    #[test]
    fn the_panel_is_centered_with_its_tiles_and_buttons_inside() {
        for count in 1..=5 {
            let layout = layout(area(), count);
            assert_eq!(layout.tiles.len(), count);
            let panel = layout.panel;
            assert_eq!(panel.loc.x * 2 + panel.size.w, 1920, "centered");
            for rect in layout.tiles.iter().chain([&layout.share, &layout.cancel]) {
                assert!(panel.contains_rect(*rect), "{count}: {rect:?} in {panel:?}");
            }
            assert!(!layout.share.overlaps(layout.cancel));
            assert!(
                panel.contains_rect(layout.hints),
                "{count}: the hints in the panel"
            );
            assert!(
                !layout.hints.overlaps(layout.cancel) && !layout.hints.overlaps(layout.share),
                "{count}: the key hints run into the buttons: {:?} and {:?}",
                layout.hints,
                layout.cancel
            );
        }
        // Three to a row, then another row.
        let four = layout(area(), 4);
        assert_eq!(
            four.tiles[3].loc.y,
            four.tiles[0].loc.y + TILE.1 + LABEL_GAP + LABEL_HEIGHT + ROW_GAP
        );
    }

    #[test]
    fn a_preview_keeps_its_outputs_shape() {
        let tile = Rectangle::new((100, 100).into(), Size::from(TILE));
        let wide = preview_rect(tile, (2560, 1080).into());
        assert_eq!(wide.size.w, TILE.0);
        assert!(wide.size.h < TILE.1);
        let tall = preview_rect(tile, (1080, 1920).into());
        assert_eq!(tall.size.h, TILE.1);
        assert!(tile.contains_rect(wide) && tile.contains_rect(tall));
    }

    #[test]
    fn keys_move_share_or_cancel() {
        assert_eq!(key_effect(Keysym::Left), KeyEffect::Previous);
        assert_eq!(key_effect(Keysym::Tab), KeyEffect::Next);
        assert_eq!(key_effect(Keysym::Return), KeyEffect::Share);
        assert_eq!(key_effect(Keysym::Escape), KeyEffect::Cancel);
        assert_eq!(key_effect(Keysym::a), KeyEffect::Nothing);
    }
}
