//! The color picker the desktop portal's `PickColor` asks for (`portal.rs`,
//! the Screenshot interface: GTK's color chooser, GIMP, Inkscape): the
//! pointer becomes a crosshair, a panel at the top of the output under it
//! says what to do, and a click answers with the color of the pixel under
//! it, as the screen shows it; Escape answers cancelled. Modal while it's
//! on: every key and press is its (`input.rs`), `Pila::under` hits nothing,
//! so no app gets the click. Locking cancels it; one at a time.
//!
//! The pixel is the output's scene drawn into a 1x1 texture with the
//! pointer at its origin and read back (`Captures::read_back`, finished by
//! `when_copied`): one pixel's worth of copying. Its panel is drawn as
//! Pila's other panels (`panel.rs`), its text on a thread.

use std::{cell::RefCell, rc::Rc};

use smithay::{
    backend::{allocator::Fourcc, renderer::gles::GlesRenderer},
    input::{keyboard::Keysym, pointer::CursorIcon, pointer::CursorImageStatus},
    reexports::calloop,
    utils::{Logical, Point, Rectangle, Scale, Size},
};
use tracing::{info, warn};

use crate::{
    capture::Source,
    config::Colors,
    panel::{self, Align, ContentImage, FrameBuffers, Item, TextKey},
    portal::{Refused, Reply},
    render::{self, PilaRenderElement},
    scale::output_scale,
    Pila,
};

/// The panel's size, its room around its line, and how far from the top of
/// its output it sits.
const WIDTH: i32 = 360;
const HEIGHT: i32 = 54;
const INSET: i32 = 16;
const FROM_TOP: i32 = 24;

/// A color, each channel from 0 to 1, as the portal answers it.
pub type Color = [f64; 3];

/// The color picker while it's on (`Pila::color_pick`).
#[derive(Default)]
pub struct ColorPick {
    reply: Option<Reply<Result<Color, Refused>>>,
    /// The output its panel was drawn on, redrawn when it goes.
    drawn: Option<String>,
    /// The content being drawn on a thread.
    pending: Option<TextKey>,
    buffers: Rc<RefCell<Buffers>>,
}

#[derive(Default)]
struct Buffers {
    frame: FrameBuffers,
    text: Option<ContentImage>,
}

/// What `Pila::scene` gives the renderer of its panel.
#[derive(Clone)]
pub struct ScenePick {
    pub output: String,
    pub rect: Rectangle<i32, Logical>,
    pub rounding: panel::Shape,
    buffers: Rc<RefCell<Buffers>>,
}

/// The panel's line and hint.
fn items(colors: &Colors) -> Vec<Item> {
    let (text, dimmed, _) = panel::text_colors(colors);
    vec![
        Item::Text {
            text: "Click to pick a color".into(),
            at: (INSET, (HEIGHT - 20) / 2),
            w: WIDTH - 2 * INSET - panel::hints_width(1),
            h: 20,
            font: panel::BODY,
            align: Align::Left,
            color: text,
        },
        Item::Hint {
            cap: "Esc".into(),
            what: "cancel".into(),
            at: (
                WIDTH - INSET - panel::hints_width(1) + 16,
                (HEIGHT - panel::CAP_HEIGHT) / 2,
            ),
            w: panel::hints_width(1) - 16,
            color: dimmed,
        },
    ]
}

impl Pila {
    /// The portal asks for a color: the picker is on until a click or
    /// Escape. Refused while locked or while another question is asked.
    pub(crate) fn start_color_pick(&mut self, reply: Reply<Result<Color, Refused>>) {
        if self.is_locked() || self.color_pick_open() || self.share_picker_open() {
            return reply.send(Err(Refused::Cancelled));
        }
        if crate::text::unavailable() {
            info!("no fonts: the color picker isn't shown");
            return reply.send(Err(Refused::Cancelled));
        }
        self.end_switch();
        self.hide_shortcuts();
        self.close_overview();
        self.close_window_menu();
        info!("portal: picking a color");
        self.color_pick.reply = Some(reply);
        self.color_pick.pending = None;
        *self.color_pick.buffers.borrow_mut() = Buffers::default();
        self.set_cursor(CursorImageStatus::Named(CursorIcon::Crosshair));
        self.redraw_color_pick();
        self.draw_color_pick_text();
    }

    pub fn color_pick_open(&self) -> bool {
        self.color_pick.reply.is_some()
    }

    /// End it: its panel goes, the pointer is the default again (the app
    /// under it sets its own as it re-enters).
    fn end_color_pick(&mut self) -> Option<Reply<Result<Color, Refused>>> {
        let reply = self.color_pick.reply.take()?;
        self.color_pick.pending = None;
        *self.color_pick.buffers.borrow_mut() = Buffers::default();
        self.set_cursor(CursorImageStatus::default_named());
        self.redraw_color_pick();
        Some(reply)
    }

    /// Escape, or the screen locked: answered cancelled.
    pub fn cancel_color_pick(&mut self) {
        if let Some(reply) = self.end_color_pick() {
            info!("portal: color picking cancelled");
            reply.send(Err(Refused::Cancelled));
        }
    }

    /// A key while it's on (it takes them all): Escape cancels.
    pub fn color_pick_key(&mut self, sym: Keysym) {
        if sym == Keysym::Escape {
            self.cancel_color_pick();
        }
    }

    /// A press at `location` while it's on (it takes them all): the color
    /// there is the answer.
    pub fn color_pick_press(&mut self, location: Point<f64, Logical>) {
        let Some(reply) = self.end_color_pick() else {
            return;
        };
        let Some(output) = self.output_at(location) else {
            return reply.send(Err(Refused::Cancelled));
        };
        let Some(geo) = self.space.output_geometry(&output) else {
            return reply.send(Err(Refused::Cancelled));
        };
        let scale = output_scale(&output);
        // The pixel under the pointer, its top-left the texture's.
        let origin = location.to_i32_floor();
        let input = self.frame_input();
        let clear = render::clear_color(&input.scene);
        let Pila {
            backend,
            blur,
            capture,
            ..
        } = self;
        let Some(backend) = backend.as_mut() else {
            return reply.send(Err(Refused::Failed));
        };
        let Some(mut renderer) = backend.renderer() else {
            return reply.send(Err(Refused::Failed));
        };
        let renderer: &mut GlesRenderer = &mut renderer;
        let elements: Vec<PilaRenderElement> = render::output_elements(
            renderer,
            blur,
            &input.scene,
            &output.name(),
            geo,
            origin,
            scale,
        );
        let color = Rc::new(RefCell::new([0u8; 4]));
        let read = color.clone();
        let copied = capture.read_back(
            renderer,
            Size::from((1, 1)),
            scale,
            Source::Elements {
                elements: &elements,
                clear,
            },
            Fourcc::Abgr8888,
            move |pixels| pixels.write_into(&mut read.borrow_mut()[..], 0, 4),
        );
        let copied = match copied {
            Ok(copied) => copied,
            Err(_) => {
                warn!("portal: can't read the color");
                return reply.send(Err(Refused::Failed));
            }
        };
        self.when_copied(
            copied,
            || true,
            move |_, result| {
                if result.is_err() {
                    warn!("portal: can't read the color");
                    return reply.send(Err(Refused::Failed));
                }
                let [r, g, b, _] = *color.borrow();
                let color = [r, g, b].map(|c| f64::from(c) / 255.0);
                info!(?color, "portal: color picked");
                reply.send(Ok(color));
            },
        );
    }

    /// Its panel for `Pila::scene`, while it's on: at the top of the output
    /// under the pointer.
    pub(crate) fn scene_color_pick(&self) -> Option<ScenePick> {
        self.color_pick.reply.as_ref()?;
        let output = self.output_at(self.pointer_location())?;
        let area = self.space.output_geometry(&output)?;
        let rect = Rectangle::new(
            area.loc + Point::from(((area.size.w - WIDTH) / 2, FROM_TOP)),
            Size::from((WIDTH, HEIGHT)),
        );
        Some(ScenePick {
            output: output.name(),
            rect,
            rounding: panel::rounding(&self.config),
            buffers: self.color_pick.buffers.clone(),
        })
    }

    /// Redraw the output its panel was on and the one it's on now (it
    /// follows the pointer from one to another).
    pub(crate) fn redraw_color_pick(&mut self) {
        let now = self.scene_color_pick().map(|view| view.output);
        if self.color_pick.drawn == now && now.is_none() {
            return;
        }
        for name in [self.color_pick.drawn.take(), now.clone()]
            .into_iter()
            .flatten()
        {
            let output = self.space.outputs().find(|o| o.name() == name).cloned();
            if let Some(output) = output {
                self.queue_redraw_output(&output);
            }
        }
        self.color_pick.drawn = now;
    }

    /// The pointer moved while it's on: its panel follows it to another
    /// output.
    pub(crate) fn color_pick_moved(&mut self) {
        if !self.color_pick_open() {
            return;
        }
        let now = self.scene_color_pick().map(|view| view.output);
        if now != self.color_pick.drawn {
            self.redraw_color_pick();
            self.draw_color_pick_text();
        }
    }

    /// Draw its panel's text on a thread, if what it says (its palette, its
    /// output's scale) changed.
    pub(crate) fn draw_color_pick_text(&mut self) {
        let Some(view) = self.scene_color_pick() else {
            return;
        };
        let scale = self
            .space
            .outputs()
            .find(|o| o.name() == view.output)
            .map(|o| crate::scale::buffer_scale(Scale::from(o.current_scale().fractional_scale())))
            .unwrap_or(1);
        let key = TextKey {
            items: items(&self.config.colors),
            caps: panel::key_caps(&self.config.colors),
            size: (WIDTH, HEIGHT),
            scale,
        };
        let drawn = view.buffers.borrow().text.as_ref().map(|t| &t.key) == Some(&key);
        if drawn || self.color_pick.pending.as_ref() == Some(&key) {
            return;
        }
        self.color_pick.pending = Some(key.clone());
        let (sender, channel) = calloop::channel::channel();
        let job = key.clone();
        let spawned = std::thread::Builder::new()
            .name("pila-color-pick".into())
            .spawn(move || {
                let _ = sender.send(panel::draw_text(&job));
            });
        if let Err(err) = spawned {
            warn!(?err, "can't draw the color picker's text");
            self.color_pick.pending = None;
            return;
        }
        let inserted = self
            .loop_handle
            .insert_source(channel, move |event, _, state| {
                let calloop::channel::Event::Msg(pixels) = event else {
                    return;
                };
                if state.color_pick.pending.as_ref() != Some(&key) {
                    return;
                }
                state.color_pick.pending = None;
                let Some(pixels) = pixels else {
                    return;
                };
                state.color_pick.buffers.borrow_mut().text =
                    Some(ContentImage::new(key.clone(), pixels));
                state.redraw_color_pick();
            });
        if let Err(err) = inserted {
            warn!(?err, "can't wait for the color picker's text");
            self.color_pick.pending = None;
        }
    }

    /// For the state dump: whether it's on, and its panel.
    pub fn color_pick_json(&self) -> serde_json::Value {
        match self.scene_color_pick() {
            None => serde_json::json!({ "open": false }),
            Some(view) => {
                let r = view.rect;
                serde_json::json!({
                    "open": true,
                    "output": view.output,
                    "rect": { "x": r.loc.x, "y": r.loc.y, "w": r.size.w, "h": r.size.h },
                    "text": view.buffers.borrow().text.is_some(),
                })
            }
        }
    }
}

/// Its panel's elements, front to back, relative to `origin`: drawn once
/// its text is there.
pub fn elements(
    renderer: &mut GlesRenderer,
    view: &ScenePick,
    colors: &Colors,
    origin: Point<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement> {
    let mut buffers = view.buffers.borrow_mut();
    let buffers = &mut *buffers;
    let Some(text) = buffers.text.as_mut() else {
        return Vec::new();
    };
    let mut elements: Vec<PilaRenderElement> = text
        .element(renderer, view.rect, origin, scale)
        .into_iter()
        .collect();
    elements.extend(panel::frame(
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
