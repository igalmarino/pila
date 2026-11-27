//! The quit dialog: `confirm-quit` (Mod+Shift+E by default) asks before
//! Pila quits, in a modal panel (`panel.rs`). Enter, its Quit button or the
//! `confirm-quit` binding pressed again quit; Escape or Cancel close it.
//! `quit` itself never asks (`pila msg action quit`, a logout menu's).

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use serde_json::{json, Value};
use smithay::{
    backend::renderer::{element::solid::SolidColorBuffer, gles::GlesRenderer},
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point, Rectangle, Scale, Size},
};
use tracing::{debug, info};

use crate::{
    config::{Action, Colors},
    panel::{self, Align, ContentImage, FrameBuffers, Item, TextKey},
    render::PilaRenderElement,
    Pila,
};

/// Panel geometry (logical pixels): the two buttons at the bottom right.
const WIDTH: i32 = 520;
const NOTE_AT: i32 = 62;
const NOTE_HEIGHT: i32 = 20;
const BUTTON_HEIGHT: i32 = 34;
const BUTTON_WIDTH: i32 = 88;
const BUTTON_GAP: i32 = 10;

const TITLE: &str = "Quit Pila?";
const NOTE: &str = "Every window will close.";

/// The dialog's state (`Pila::quit_dialog`).
#[derive(Default)]
pub struct QuitDialog {
    /// The output it's on, by name, while it's shown.
    shown: Option<String>,
    buffers: Rc<RefCell<Buffers>>,
    pending_text: Option<TextKey>,
}

#[derive(Default)]
struct Buffers {
    frame: FrameBuffers,
    solids: HashMap<&'static str, SolidColorBuffer>,
    dims: HashMap<String, SolidColorBuffer>,
    text: Option<ContentImage>,
}

/// Where the panel and its buttons are, global.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub panel: Rectangle<i32, Logical>,
    pub cancel: Rectangle<i32, Logical>,
    pub quit: Rectangle<i32, Logical>,
}

/// The panel centered in `area`, narrower than `WIDTH` on an output that is.
pub fn layout(area: Rectangle<i32, Logical>) -> Layout {
    let width = WIDTH.min(area.size.w - 2 * panel::PADDING);
    let height = NOTE_AT + NOTE_HEIGHT + panel::FOOTER_GAP + BUTTON_HEIGHT + panel::PADDING;
    let panel = Rectangle::new(
        area.loc + Point::from(((area.size.w - width) / 2, (area.size.h - height) / 2)),
        Size::from((width, height)),
    );
    let buttons_y = height - panel::PADDING - BUTTON_HEIGHT;
    let quit = Rectangle::new(
        panel.loc + Point::from((width - panel::PADDING - BUTTON_WIDTH, buttons_y)),
        Size::from((BUTTON_WIDTH, BUTTON_HEIGHT)),
    );
    let cancel = Rectangle::new(
        quit.loc - Point::from((BUTTON_GAP + BUTTON_WIDTH, 0)),
        Size::from((BUTTON_WIDTH, BUTTON_HEIGHT)),
    );
    Layout {
        panel,
        cancel,
        quit,
    }
}

/// What `Pila::scene` gives the renderer while the dialog is shown.
#[derive(Clone)]
pub struct SceneQuit {
    pub output: String,
    pub layout: Layout,
    pub rounding: panel::Shape,
    buffers: Rc<RefCell<Buffers>>,
}

impl Pila {
    /// `confirm-quit`: show the dialog on the output under the pointer.
    /// Never while locked or while another modal panel asks; shown already,
    /// it stays as it is (a held binding repeating, a second `pila msg`).
    pub(crate) fn ask_quit(&mut self) {
        if self.quit_dialog_open() {
            return;
        }
        if self.is_locked() || self.modal_panel_open() {
            debug!("quit dialog refused: locked or another panel asks");
            return;
        }
        let Some(output) = self
            .output_at(self.pointer_location())
            .or_else(|| self.space.outputs().next().cloned())
        else {
            debug!("quit dialog refused: no output");
            return;
        };
        self.end_switch();
        self.hide_shortcuts();
        self.close_overview();
        self.close_window_menu();
        info!("asking whether to quit");
        self.quit_dialog.shown = Some(output.name());
        self.quit_dialog.pending_text = None;
        *self.quit_dialog.buffers.borrow_mut() = Buffers::default();
        // It dims every output.
        self.queue_redraw();
        self.draw_quit_dialog_text();
    }

    pub fn quit_dialog_open(&self) -> bool {
        self.quit_dialog.shown.is_some()
    }

    /// Escape, Cancel or a lock: Pila goes on.
    pub fn cancel_quit_dialog(&mut self) {
        if self.quit_dialog.shown.take().is_none() {
            return;
        }
        info!("quit cancelled");
        self.quit_dialog.pending_text = None;
        *self.quit_dialog.buffers.borrow_mut() = Buffers::default();
        self.queue_redraw();
        // A share or capture permission asked meanwhile waited for it.
        self.show_share_picker();
        self.show_capture_permission();
    }

    fn confirm_quit(&mut self) {
        info!("quit confirmed");
        self.loop_signal.stop();
    }

    /// A key while the dialog is shown (it takes them all): Enter or the
    /// `confirm-quit` binding quit, Escape cancels, the rest does nothing.
    pub fn quit_dialog_key(&mut self, sym: Keysym, mods: &ModifiersState) {
        if !self.quit_dialog_open() {
            return;
        }
        let plain = !mods.ctrl && !mods.alt && !mods.logo && !mods.shift;
        match sym {
            Keysym::Return | Keysym::KP_Enter if plain => self.confirm_quit(),
            Keysym::Escape if plain => self.cancel_quit_dialog(),
            _ if self.config.action_for(sym, mods) == Some(Action::ConfirmQuit) => {
                self.confirm_quit()
            }
            _ => {}
        }
    }

    /// A press at `location` while the dialog is shown (it takes them all):
    /// its buttons act, elsewhere nothing.
    pub fn quit_dialog_press(&mut self, location: Point<f64, Logical>) {
        let Some(view) = self.scene_quit_dialog() else {
            return;
        };
        if view.layout.quit.to_f64().contains(location) {
            self.confirm_quit();
        } else if view.layout.cancel.to_f64().contains(location) {
            self.cancel_quit_dialog();
        }
    }

    /// The dialog for `Pila::scene`, while it's shown.
    pub(crate) fn scene_quit_dialog(&self) -> Option<SceneQuit> {
        let shown = self.quit_dialog.shown.as_ref()?;
        let output = self
            .space
            .outputs()
            .find(|o| o.name() == *shown)
            .or_else(|| self.space.outputs().next())?;
        let area = self.space.output_geometry(output)?;
        Some(SceneQuit {
            output: output.name(),
            layout: layout(area),
            rounding: panel::rounding(&self.config),
            buffers: self.quit_dialog.buffers.clone(),
        })
    }

    /// Draw the dialog's text on a thread, unless it's drawn or on its way.
    pub(crate) fn draw_quit_dialog_text(&mut self) {
        let Some(view) = self.scene_quit_dialog() else {
            return;
        };
        let scale = self.panel_text_scale(&view.output);
        let key = TextKey {
            items: items(&view.layout, &self.config.colors),
            caps: panel::key_caps(&self.config.colors),
            size: (view.layout.panel.size.w, view.layout.panel.size.h),
            scale,
        };
        let drawn = view.buffers.borrow().text.as_ref().map(|t| &t.key) == Some(&key);
        self.draw_panel_text(
            key,
            drawn,
            "pila-quit-text",
            |state| &mut state.quit_dialog.pending_text,
            Self::draw_quit_dialog_text,
            |state, text| {
                state.quit_dialog.buffers.borrow_mut().text = Some(text);
                state.queue_redraw();
            },
        );
    }

    /// State dump for the tests (`introspect.rs`).
    pub fn quit_dialog_json(&self) -> Value {
        let rect = |r: Rectangle<i32, Logical>| json!({ "x": r.loc.x, "y": r.loc.y, "w": r.size.w, "h": r.size.h });
        let shown = self.scene_quit_dialog().map(|view| {
            json!({
                "output": view.output,
                "panel": rect(view.layout.panel),
                "cancel": rect(view.layout.cancel),
                "quit": rect(view.layout.quit),
                "text": view.buffers.borrow().text.is_some(),
            })
        });
        json!({ "shown": shown })
    }
}

fn items(layout: &Layout, colors: &Colors) -> Vec<Item> {
    let (text, dimmed, _) = panel::text_colors(colors);
    let width = layout.panel.size.w;
    let mut items = vec![
        panel::title(TITLE, width, colors),
        Item::Text {
            text: NOTE.into(),
            at: (panel::PADDING, NOTE_AT),
            w: width - 2 * panel::PADDING,
            h: NOTE_HEIGHT,
            font: panel::NOTE,
            align: Align::Left,
            color: dimmed,
        },
    ];
    let local =
        |r: Rectangle<i32, Logical>| (r.loc.x - layout.panel.loc.x, r.loc.y - layout.panel.loc.y);
    for (rect, label, color) in [
        (layout.quit, "Quit", panel::bits(colors.hover_icon)),
        (layout.cancel, "Cancel", text),
    ] {
        let (x, y) = local(rect);
        items.push(Item::Text {
            text: label.into(),
            at: (x, y + (BUTTON_HEIGHT - 20) / 2),
            w: rect.size.w,
            h: 20,
            font: panel::BODY,
            align: Align::Center,
            color,
        });
    }
    items
}

/// The dialog's render elements for `output_name`, front to back: the panel
/// on its output once its text is drawn, and the modal dim on every output.
pub fn elements(
    renderer: &mut GlesRenderer,
    view: &SceneQuit,
    colors: &Colors,
    output_name: &str,
    output_geo: Rectangle<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement> {
    let origin = output_geo.loc;
    let mut buffers = view.buffers.borrow_mut();
    let buffers = &mut *buffers;
    let dim = panel::dim(
        &mut buffers.dims,
        output_name,
        output_geo,
        1.0,
        origin,
        scale,
    );
    let Some(text) = buffers.text.as_mut().filter(|_| output_name == view.output) else {
        return vec![dim];
    };
    let layout = &view.layout;
    let mut elements: Vec<PilaRenderElement> = text
        .element(renderer, layout.panel, origin, scale)
        .into_iter()
        .collect();
    elements.extend(panel::solids(
        renderer,
        &mut buffers.solids,
        panel::buttons(layout.quit, layout.cancel, colors),
        origin,
        scale,
    ));
    elements.extend(panel::frame(
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

    /// Inside `area`, its buttons inside it, apart and under the note.
    fn assert_fits(area: Rectangle<i32, Logical>) -> Layout {
        let l = layout(area);
        assert!(area.contains_rect(l.panel), "{l:?} in {area:?}");
        for button in [l.cancel, l.quit] {
            assert!(l.panel.contains_rect(button), "{button:?} in {l:?}");
        }
        assert!(!l.cancel.overlaps(l.quit), "{l:?}");
        assert!(
            l.panel.loc.y + NOTE_AT + NOTE_HEIGHT < l.quit.loc.y,
            "{l:?}"
        );
        l
    }

    #[test]
    fn the_panel_is_centered_with_its_buttons_inside() {
        let wide = Rectangle::new((1920, 0).into(), (1920, 1080).into());
        let l = assert_fits(wide);
        assert_eq!(l.panel.size.w, WIDTH);
        assert_eq!(
            l.panel.loc.x + l.panel.size.w / 2,
            wide.loc.x + wide.size.w / 2
        );
        assert_fits(Rectangle::new((0, 0).into(), (480, 600).into()));
    }
}
