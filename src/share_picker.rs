//! The screen-share picker drawn by Pila itself: when an app asks the portal
//! Pila serves to share a screen or a window (`portal.rs`), a panel in the
//! middle of the output under the pointer names the app and shows what it may
//! share as live previews: every output, every window (most recently used
//! first, minimized ones and those on other workspaces too), in sections when
//! it may share both; the user picks one (a click, or the arrows) and shares
//! it (Share, Enter), or cancels (Cancel, Escape). Nothing is shared without
//! it: this is the consent.
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
//! drawing, and one that did redraws the picker's output. A window's preview
//! is the switcher's (`switcher_view::preview`): drawn as the panel is, the
//! window drawing for the panel's output while it's shown.

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

use smithay::{desktop::Window, utils::IsAlive};

use crate::{
    config::Colors,
    panel::{bits, draw_text, Align, ContentImage, FrameBuffers, Item, TextKey},
    portal::Refused,
    render::{OffscreenWindow, PilaRenderElement},
    screencast::Share,
    Pila,
};

/// The panel's room around its content (Pila's titled panels'), and where
/// the note under its title goes.
const PADDING: i32 = crate::panel::PADDING;
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
/// With windows too, outputs' tiles are smaller, closer together.
const SMALL_TILE: (i32, i32) = (240, 135);
const SMALL_GAP: i32 = 40;
/// Windows' tiles: up to `WINDOW_COLUMNS` a row, `WINDOW_ROWS` rows shown
/// (the selected one's kept in view).
pub const WINDOW_TILE: (i32, i32) = (176, 100);
const WINDOW_GAP: i32 = 20;
const WINDOW_COLUMNS: usize = 5;
const WINDOW_ROWS: usize = 2;
/// A section's heading (with both kinds): its line and the room under it,
/// and the room above the second one.
const SECTION_HEADING: i32 = 22;
const SECTION_GAP: i32 = 28;
/// The selected tile's highlight: around the tile and its label.
const SELECTED_MARGIN: i32 = 6;
/// The footer: the buttons, as tall as they are, the hints beside them.
const BUTTON_HEIGHT: i32 = 36;
const SHARE_WIDTH: i32 = 120;
const CANCEL_WIDTH: i32 = 110;
const BUTTON_GAP: i32 = 12;
/// Room between the hints and the buttons.
const FOOTER_SPACE: i32 = 24;
/// The narrowest panel (one source): its footer, the hints then the buttons.
const MIN_WIDTH: i32 =
    2 * PADDING + HINTS_WIDTH + FOOTER_SPACE + CANCEL_WIDTH + BUTTON_GAP + SHARE_WIDTH;
/// The key hints in the footer, and their width.
const HINTS: [(&str, &str); 3] = [("← →", "pick"), ("Enter", "share"), ("Esc", "cancel")];
const SCREENSHOT_HINTS: [(&str, &str); 3] = [("← →", "pick"), ("Enter", "take"), ("Esc", "cancel")];
const HINTS_WIDTH: i32 = crate::panel::hints_width(HINTS.len());

/// The picker while it's shown (`Pila::share_picker`).
#[derive(Default)]
pub struct SharePicker {
    shown: Option<Shown>,
    /// The content being drawn on a thread (`Pila::draw_share_picker_text`).
    pending: Option<TextKey>,
    buffers: Rc<RefCell<Buffers>>,
}

struct Shown {
    ask: Ask,
    /// Who asks, as the panel names it: the portal's name for the app, else
    /// the app id of the window it asked from (`Ask::parent`), then its
    /// desktop entry's name once read (`Pila::name_share_picker_app`).
    app: Option<String>,
    /// The output the panel is on, by name.
    output: String,
    /// What there is to pick: the outputs, left to right, then the windows.
    sources: Vec<Source>,
    selected: usize,
}

/// What the picker asks about: a share, or a screenshot.
pub enum Ask {
    /// To share (`screencast.rs`).
    Cast(Share),
    /// To take a screenshot (`screenshot.rs`).
    Screenshot(crate::screenshot::Shot),
}

impl Ask {
    /// Who asks, as the picker names it.
    fn app(&self) -> Option<&str> {
        match self {
            Ask::Cast(share) => share.app.as_deref(),
            Ask::Screenshot(shot) => shot.app.as_deref(),
        }
    }

    /// The window it was asked from, as the portal said it.
    fn parent(&self) -> Option<&crate::portal::ParentWindow> {
        match self {
            Ask::Cast(share) => share.parent.as_ref(),
            Ask::Screenshot(shot) => shot.parent.as_ref(),
        }
    }

    /// What may be picked: a screenshot may be of either.
    fn sources(&self) -> crate::portal::Sources {
        match self {
            Ask::Cast(share) => share.sources,
            Ask::Screenshot(_) => crate::portal::Sources {
                monitors: true,
                windows: true,
            },
        }
    }

    /// The portal session it's for (a share's).
    pub(crate) fn session(&self) -> Option<&str> {
        match self {
            Ask::Cast(share) => Some(&share.session),
            Ask::Screenshot(_) => None,
        }
    }

    /// Nothing was picked: the app is told.
    pub(crate) fn cancel(self) {
        match self {
            Ask::Cast(share) => share.reply.send(Err(Refused::Cancelled)),
            Ask::Screenshot(shot) => shot.reply.send(Err(Refused::Cancelled)),
        }
    }
}

/// Something the picker offers.
#[derive(Clone, PartialEq)]
enum Source {
    /// An output, by name.
    Output(String),
    Window(Window),
}

impl Shown {
    /// How many outputs, then windows, it offers.
    fn counts(&self) -> (usize, usize) {
        let outputs = self
            .sources
            .iter()
            .filter(|s| matches!(s, Source::Output(_)))
            .count();
        (outputs, self.sources.len() - outputs)
    }

    /// The first row of windows shown: from the top, until the selected
    /// window's would fall off.
    fn first_window_row(&self) -> usize {
        let (outputs, windows) = self.counts();
        let selected = self.selected.checked_sub(outputs);
        crate::switcher_view::first_shown(
            windows.div_ceil(WINDOW_COLUMNS),
            WINDOW_ROWS,
            selected.map(|i| i / WINDOW_COLUMNS),
        )
    }
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
    /// Each window's preview shown (`switcher_view::preview`).
    windows: Vec<(Window, OffscreenWindow)>,
}

/// Where the panel and its parts are, global.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub panel: Rectangle<i32, Logical>,
    /// The tiles shown (the box a preview fits in), and the source each
    /// one is: the outputs', then the windows' rows shown.
    pub tiles: Vec<Rectangle<i32, Logical>>,
    pub shown: Vec<usize>,
    /// The sections' headings and where they go, from the panel's top
    /// (none with one kind of source).
    pub headings: Vec<(&'static str, i32)>,
    pub share: Rectangle<i32, Logical>,
    pub cancel: Rectangle<i32, Logical>,
    /// The key hints, left of the buttons.
    pub hints: Rectangle<i32, Logical>,
}

/// The panel for `outputs` outputs and `windows` windows (rows of them from
/// `first_row` shown), in the middle of `area`. With one kind, its rows are
/// centered, the outputs' three to a row as large tiles; with both, under
/// their headings from the left, the outputs' smaller.
pub fn layout(
    area: Rectangle<i32, Logical>,
    outputs: usize,
    windows: usize,
    first_row: usize,
) -> Layout {
    let both = outputs > 0 && windows > 0;
    let outputs_shown = if outputs == 0 && windows == 0 {
        1
    } else {
        outputs
    };
    let (tile, gap) = if both {
        (SMALL_TILE, SMALL_GAP)
    } else {
        (TILE, TILE_GAP)
    };
    let row_width =
        |n: usize, tile: (i32, i32), gap: i32| n as i32 * tile.0 + (n as i32 - 1).max(0) * gap;
    let window_columns = windows.min(WINDOW_COLUMNS);
    let width = (2 * PADDING
        + row_width(outputs_shown.min(COLUMNS), tile, gap).max(row_width(
            window_columns,
            WINDOW_TILE,
            WINDOW_GAP,
        )))
    .max(MIN_WIDTH);
    let mut tiles = Vec::new();
    let mut shown = Vec::new();
    let mut headings = Vec::new();
    // `count` tiles of `tile` in rows of `columns`, from `y` (local): under
    // a heading from the left; else each row centered, or (`grid`) the rows
    // as a grid in the middle; the bottom of the last row.
    let mut place = |count: usize,
                     columns: usize,
                     tile: (i32, i32),
                     gap: i32,
                     y: i32,
                     grid: bool,
                     sources: &mut dyn Iterator<Item = usize>| {
        let rows = count.div_ceil(columns).max(1) as i32;
        let tile_row = tile.1 + LABEL_GAP + LABEL_HEIGHT;
        for i in 0..count {
            let (row, column) = ((i / columns) as i32, (i % columns) as i32);
            let in_row = if grid {
                count.min(columns)
            } else {
                (count - row as usize * columns).min(columns)
            };
            let x = if both {
                PADDING
            } else {
                (width - row_width(in_row, tile, gap)) / 2
            } + column * (tile.0 + gap);
            let y = y + row * (tile_row + ROW_GAP);
            tiles.push(Rectangle::new(Point::from((x, y)), Size::from(tile)));
            shown.extend(sources.next());
        }
        y + rows * tile_row + (rows - 1) * ROW_GAP
    };
    let mut y = TILES_AT;
    if outputs > 0 || windows == 0 {
        if both {
            headings.push(("Screens", y));
            y += SECTION_HEADING;
        }
        y = place(
            outputs_shown,
            COLUMNS,
            tile,
            gap,
            y,
            false,
            &mut (0..outputs),
        );
    }
    if windows > 0 {
        if both {
            y += SECTION_GAP;
            headings.push(("Windows", y));
            y += SECTION_HEADING;
        }
        let rows = windows.div_ceil(WINDOW_COLUMNS);
        let first_row = first_row.min(rows.saturating_sub(WINDOW_ROWS));
        let from = first_row * WINDOW_COLUMNS;
        let count = (windows - from).min(WINDOW_COLUMNS * WINDOW_ROWS);
        y = place(
            count,
            WINDOW_COLUMNS,
            WINDOW_TILE,
            WINDOW_GAP,
            y,
            true,
            &mut (outputs + from..outputs + from + count),
        );
    }
    let height = y + crate::panel::FOOTER_GAP + BUTTON_HEIGHT + PADDING;
    let panel = Rectangle::new(
        area.loc + Point::from(((area.size.w - width) / 2, (area.size.h - height) / 2)),
        Size::from((width, height)),
    );
    for tile in &mut tiles {
        tile.loc += panel.loc;
    }
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
        shown,
        headings,
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
    /// The outputs' previews: output name and where it shows.
    pub previews: Vec<(String, Rectangle<i32, Logical>)>,
    /// The windows shown and the tile each one's preview fits in.
    pub windows: Vec<(Window, Rectangle<i32, Logical>)>,
    pub selected: usize,
    /// The windows' corners (`corner-radius`, `rounded-corners`); `None`:
    /// square.
    pub rounding: Option<(f64, crate::rounded::Corners)>,
    buffers: Rc<RefCell<Buffers>>,
}

impl ScenePicker {
    /// The windows it shows: they draw for its output while it's shown.
    pub fn windows(&self) -> impl Iterator<Item = &Window> {
        self.windows.iter().map(|(window, _)| window)
    }
}

impl Pila {
    /// Show the picker for the first request waiting, unless one is shown.
    /// Never while locked: the requests are cancelled.
    pub(crate) fn show_share_picker(&mut self) {
        if self.share_picker.shown.is_some() || self.screencast.waiting.is_empty() {
            return;
        }
        if self.is_locked() {
            for ask in self.screencast.waiting.drain(..) {
                ask.cancel();
            }
            return;
        }
        let mut outputs: Vec<_> = self
            .space
            .outputs()
            .filter_map(|o| Some((self.space.output_geometry(o)?, o.name())))
            .collect();
        outputs.sort_by_key(|(geo, _)| (geo.loc.x, geo.loc.y));
        let Some(ask) = self.screencast.waiting.pop_front() else {
            return;
        };
        if outputs.is_empty() {
            return ask.cancel();
        }
        let wanted = ask.sources();
        let pointer = self.pointer_location();
        let under = outputs
            .iter()
            .position(|(geo, _)| geo.to_f64().contains(pointer))
            .unwrap_or(0);
        let output = outputs[under].1.clone();
        // What it may share: the outputs, then the windows that can be
        // (a foreign-toplevel identifier, a size), most recently used first.
        let mut sources: Vec<Source> = Vec::new();
        if wanted.monitors {
            sources.extend(outputs.iter().map(|(_, name)| Source::Output(name.clone())));
        }
        if wanted.windows {
            let windows = self.windows.focus_history.iter().filter(|window| {
                window.alive()
                    && crate::foreign_toplevel::identifier(window).is_some()
                    && !window.geometry().size.is_empty()
            });
            sources.extend(windows.cloned().map(Source::Window));
        }
        if sources.is_empty() {
            info!("portal: nothing the app may pick");
            ask.cancel();
            return self.show_share_picker();
        }
        // The output under the pointer selected first, else the window used
        // last.
        let selected = if wanted.monitors { under } else { 0 };
        // Modal, in front of everything: nothing else of Pila's stays open.
        self.end_switch();
        self.hide_shortcuts();
        self.close_overview();
        self.close_window_menu();
        // An app the portal doesn't name (one outside a sandbox) goes by the
        // window it asked from, if it said which.
        let mut app = ask.app().map(str::to_string);
        let mut look_up = None;
        if app.is_none() {
            let window = ask
                .parent()
                .and_then(|parent| self.window_of_parent(parent));
            if let Some(window) = window {
                let app_id = crate::foreign_toplevel::app_id(&window);
                if !app_id.is_empty() {
                    app = Some(app_id.clone());
                    look_up = Some(app_id);
                }
            }
        }
        info!(
            app = app.as_deref().unwrap_or("?"),
            screenshot = matches!(ask, Ask::Screenshot(_)),
            "portal: asking what to share"
        );
        self.share_picker.shown = Some(Shown {
            ask,
            app,
            output,
            sources,
            selected,
        });
        *self.share_picker.buffers.borrow_mut() = Buffers::default();
        self.share_picker.pending = None;
        // The previews are drawn after the next frames, the first now.
        self.capture_due = true;
        self.wake.ping();
        self.queue_redraw();
        self.draw_share_picker_text();
        if let Some(app_id) = look_up {
            self.name_share_picker_app(app_id);
        }
    }

    /// The window a portal request says it comes from: the one exported as
    /// its xdg-foreign handle, or the X11 window of its id (any X11 client
    /// could name another's, but one outside a sandbox, the only kind named
    /// this way, can do as much harm on its own).
    pub(crate) fn window_of_parent(&self, parent: &crate::portal::ParentWindow) -> Option<Window> {
        match parent {
            crate::portal::ParentWindow::Wayland(handle) => {
                let surface = self.globals.xdg_foreign.exported_surface(handle)?;
                self.window_for_surface(surface)
            }
            crate::portal::ParentWindow::X11(id) => self
                .space
                .elements()
                .chain(self.off_space_windows())
                .find(|w| w.x11_surface().is_some_and(|x| x.window_id() == *id))
                .cloned(),
        }
    }

    /// Name the app asking by `app_id`'s desktop entry, read on a thread
    /// (files aren't read on the event loop): the panel says it once it's
    /// there, if it still asks for that app.
    fn name_share_picker_app(&mut self, app_id: String) {
        let (sender, channel) = calloop::channel::channel();
        let looked_up = app_id.clone();
        let spawned = std::thread::Builder::new()
            .name("pila-app-name".into())
            .spawn(move || {
                let name = crate::portal::app_name(&looked_up, &crate::portal::data_dirs());
                let _ = sender.send(name);
            });
        if let Err(err) = spawned {
            warn!(?err, "portal: can't look up the app's name");
            return;
        }
        let inserted = self
            .loop_handle
            .insert_source(channel, move |event, _, state| {
                let calloop::channel::Event::Msg(Some(name)) = event else {
                    return;
                };
                let Some(shown) = state.share_picker.shown.as_mut() else {
                    return;
                };
                if shown.app.as_deref() == Some(app_id.as_str()) && name != app_id {
                    shown.app = Some(name);
                    state.draw_share_picker_text();
                }
            });
        if let Err(err) = inserted {
            warn!(?err, "portal: can't wait for the app's name");
        }
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
        shown.ask.cancel();
        self.show_share_picker();
    }

    /// The app closed the session whose picker is shown: it goes, and the
    /// next request is asked.
    pub(crate) fn close_share_picker_of(&mut self, session: &str) {
        if self
            .share_picker
            .shown
            .as_ref()
            .is_some_and(|shown| shown.ask.session() == Some(session))
        {
            self.take_share_picker();
            self.show_share_picker();
        }
    }

    /// Share what's selected (Share, Enter).
    fn pick_share(&mut self) {
        let Some(shown) = self.take_share_picker() else {
            return;
        };
        let target = match shown.sources.get(shown.selected) {
            Some(Source::Output(name)) => self
                .space
                .outputs()
                .find(|o| &o.name() == name)
                .map(|o| crate::capture::Target::Output(o.downgrade())),
            Some(Source::Window(window)) => crate::foreign_toplevel::identifier(window)
                .filter(|_| window.alive())
                .map(crate::capture::Target::Toplevel),
            None => None,
        };
        match (target, shown.ask) {
            (Some(target), Ask::Cast(share)) => self.start_cast(share, target),
            (Some(target), Ask::Screenshot(shot)) => self.take_screenshot(Some(target), shot.reply),
            (None, ask) => ask.cancel(),
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
            // The windows' rows may scroll: other labels.
            self.draw_share_picker_text();
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
            .zip(&layout.shown)
            .find(|(tile, _)| selected_rect(**tile).to_f64().contains(location))
            .map(|(_, index)| *index)
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
        let (outputs, windows) = shown.counts();
        let layout = layout(area, outputs, windows, shown.first_window_row());
        let mut previews = Vec::new();
        let mut shown_windows = Vec::new();
        for (tile, index) in layout.tiles.iter().zip(&layout.shown) {
            match shown.sources.get(*index) {
                Some(Source::Output(name)) => {
                    let Some(source) = self.space.outputs().find(|o| &o.name() == name) else {
                        continue;
                    };
                    let Some(geo) = self.space.output_geometry(source) else {
                        continue;
                    };
                    previews.push((name.clone(), preview_rect(*tile, geo.size)));
                }
                Some(Source::Window(window)) if window.alive() => {
                    shown_windows.push((window.clone(), *tile));
                }
                _ => {}
            }
        }
        Some(ScenePicker {
            output: output.name(),
            layout,
            previews,
            windows: shown_windows,
            selected: shown.selected,
            rounding: crate::panel::rounding(&self.config),
            buffers: self.share_picker.buffers.clone(),
        })
    }

    /// A window committed: if the picker shows it, its output is redrawn,
    /// so the preview stays live.
    pub(crate) fn share_picker_committed(&mut self, window: &Window) {
        let Some(view) = self.scene_share_picker() else {
            return;
        };
        if view.windows().any(|w| w == window) {
            self.queue_redraw_named_output(&view.output);
        }
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
        // The labels of the tiles shown: an output's name and size, a
        // window's title (its app id without one).
        let labels: Vec<String> = view
            .layout
            .shown
            .iter()
            .map(|index| match shown.sources.get(*index) {
                Some(Source::Output(name)) => {
                    let size = self
                        .space
                        .outputs()
                        .find(|o| &o.name() == name)
                        .and_then(|o| o.current_mode())
                        .map(|mode| format!("  {} × {}", mode.size.w, mode.size.h))
                        .unwrap_or_default();
                    format!("{name}{size}")
                }
                Some(Source::Window(window)) => crate::switcher_view::caption(
                    &crate::foreign_toplevel::title(window),
                    &crate::foreign_toplevel::app_id(window),
                ),
                None => String::new(),
            })
            .collect();
        let key = TextKey {
            items: items(
                &view.layout,
                shown.app.as_deref(),
                shown.ask.sources(),
                matches!(shown.ask, Ask::Screenshot(_)),
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
            "app": shown.app,
            "screenshot": matches!(shown.ask, Ask::Screenshot(_)),
            "output": view.output,
            // Outputs by name, windows by app id.
            "sources": shown.sources.iter().map(|source| match source {
                Source::Output(name) => name.clone(),
                Source::Window(window) => crate::foreign_toplevel::app_id(window),
            }).collect::<Vec<_>>(),
            "kinds": shown.sources.iter().map(|source| match source {
                Source::Output(_) => "output",
                Source::Window(_) => "window",
            }).collect::<Vec<_>>(),
            "selected": view.selected,
            "panel": rect(view.layout.panel),
            "tiles": view.layout.tiles.iter().map(|t| rect(*t)).collect::<Vec<_>>(),
            "tile_sources": view.layout.shown,
            "headings": view.layout.headings.iter().map(|(text, _)| *text).collect::<Vec<_>>(),
            "window_previews": view.buffers.borrow().windows.len(),
            "share": rect(view.layout.share),
            "cancel": rect(view.layout.cancel),
            "text": view.buffers.borrow().text.is_some(),
            "previews": view.buffers.borrow().previews.len(),
            "waiting": self.screencast.waiting.len(),
        })
    }
}

/// The panel's texts, at places from its top-left.
fn items(
    layout: &Layout,
    app: Option<&str>,
    sources: crate::portal::Sources,
    screenshot: bool,
    labels: &[String],
    colors: &Colors,
) -> Vec<Item> {
    use crate::panel::{BODY, NOTE};
    let panel = layout.panel;
    let width = panel.size.w - 2 * PADDING;
    let local = |r: Rectangle<i32, Logical>| (r.loc.x - panel.loc.x, r.loc.y - panel.loc.y);
    let (text, dimmed, accent) = crate::panel::text_colors(colors);
    let what = match (sources.monitors, sources.windows) {
        (true, false) => "a screen",
        (false, true) => "a window",
        _ => "a screen or a window",
    };
    let title = match app {
        _ if screenshot => match app {
            Some(app) => format!("Take a screenshot for {app}"),
            None => "An app asks for a screenshot".to_string(),
        },
        Some(app) if sources.monitors && sources.windows => format!("Share with {app}"),
        Some(app) => format!("Share {what} with {app}"),
        None => format!("An app asks to share {what}"),
    };
    let note = if screenshot {
        "It will get a picture of what you pick, without the pointer."
    } else if sources.windows {
        "It will see everything in what you pick until it stops sharing."
    } else {
        "It will see everything the screen shows until it stops sharing."
    };
    let mut items = vec![
        crate::panel::title(&title, panel.size.w, colors),
        Item::Text {
            text: note.into(),
            at: (PADDING, NOTE_AT),
            w: width,
            h: NOTE_HEIGHT,
            font: NOTE,
            align: Align::Left,
            color: dimmed,
        },
    ];
    for (heading, y) in &layout.headings {
        items.push(Item::Text {
            text: heading.to_uppercase(),
            at: (PADDING, *y),
            w: width,
            h: 16,
            font: crate::panel::SECTION,
            align: Align::Left,
            color: accent,
        });
    }
    for (tile, label) in layout.tiles.iter().zip(labels) {
        let (x, y) = local(*tile);
        items.push(Item::Text {
            text: label.clone(),
            at: (x, y + tile.size.h + LABEL_GAP),
            w: tile.size.w,
            h: LABEL_HEIGHT,
            font: BODY,
            align: Align::Center,
            color: text,
        });
    }
    // Share's label on the accent, as the switcher's selected title.
    let on_accent = bits(colors.hover_icon);
    let act = if screenshot { "Take" } else { "Share" };
    for (rect, label, color) in [
        (layout.share, act, on_accent),
        (layout.cancel, "Cancel", text),
    ] {
        let (x, y) = local(rect);
        items.push(Item::Text {
            text: label.into(),
            at: (x, y + (BUTTON_HEIGHT - LABEL_HEIGHT) / 2),
            w: rect.size.w,
            h: LABEL_HEIGHT,
            font: BODY,
            align: Align::Center,
            color,
        });
    }
    let hints_y = local(layout.share).1 + (BUTTON_HEIGHT - crate::panel::FOOTER_HEIGHT) / 2;
    let hints = if screenshot {
        &SCREENSHOT_HINTS
    } else {
        &HINTS
    };
    items.extend(crate::panel::footer(
        hints,
        None,
        hints_y,
        panel.size.w,
        colors,
    ));
    items
}

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

    // The windows' previews, drawn now, each fitted in its tile; those no
    // longer shown go.
    buffers
        .windows
        .retain(|(window, _)| view.windows.iter().any(|(w, _)| w == window));
    for (window, tile) in &view.windows {
        if let Some(element) = crate::switcher_view::preview(
            renderer,
            &mut buffers.windows,
            window,
            *tile,
            origin,
            scale,
        ) {
            elements.push(element);
        }
    }

    // The selected tile on a block of the accent, as the switcher's (its
    // tile, among those shown), and the buttons.
    let selected = layout
        .shown
        .iter()
        .position(|index| *index == view.selected)
        .and_then(|slot| layout.tiles.get(slot))
        .map(|tile| {
            (
                "selected",
                selected_rect(*tile),
                crate::panel::accent(colors),
                None,
            )
        });
    let solids =
        selected
            .into_iter()
            .chain(crate::panel::buttons(layout.share, layout.cancel, colors));
    elements.extend(crate::panel::solids(
        renderer,
        &mut buffers.solids,
        solids,
        origin,
        scale,
    ));
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

    /// The picker's title, type and footer are the other titled panels'
    /// (`panel.rs`): its title where theirs is, in the same font, and its
    /// footer's hints.
    #[test]
    fn it_has_the_titled_panels_title_and_footer() {
        let colors = crate::config::Config::parse("", &|_| None, crate::config::ModKey::Super)
            .unwrap()
            .colors;
        let monitors = crate::portal::Sources {
            monitors: true,
            windows: false,
        };
        let picker = items(
            &layout(area(), 1, 0, 0),
            Some("App"),
            monitors,
            false,
            &["A".into()],
            &colors,
        );
        let Some(Item::Text { at, font, .. }) = picker.first() else {
            panic!("no title first: {picker:?}");
        };
        assert_eq!(*at, (crate::panel::PADDING, crate::panel::TITLE_AT));
        assert_eq!(*font, crate::panel::TITLE);
        let hints = picker
            .iter()
            .filter(|item| matches!(item, Item::Hint { .. }))
            .count();
        assert_eq!(hints, HINTS.len());
    }

    fn area() -> Rectangle<i32, Logical> {
        Rectangle::new((0, 0).into(), (1920, 1080).into())
    }

    #[test]
    fn the_panel_is_centered_with_its_tiles_and_buttons_inside() {
        for count in 1..=5 {
            let layout = layout(area(), count, 0, 0);
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
        let four = layout(area(), 4, 0, 0);
        assert_eq!(
            four.tiles[3].loc.y,
            four.tiles[0].loc.y + TILE.1 + LABEL_GAP + LABEL_HEIGHT + ROW_GAP
        );
    }

    /// With windows too, the outputs and the windows are in sections, at
    /// most two rows of windows shown, scrolled to the selected one's; the
    /// panel fits a 1280x800 logical monitor (1920x1200 at 1.5).
    #[test]
    fn outputs_and_windows_are_in_sections() {
        let small = Rectangle::new((0, 0).into(), (1280, 800).into());
        let layout = layout(small, 2, 12, 0);
        assert_eq!(
            layout.headings.iter().map(|(h, _)| *h).collect::<Vec<_>>(),
            ["Screens", "Windows"]
        );
        assert_eq!(layout.tiles.len(), 2 + 10, "two rows of five windows");
        assert_eq!(layout.shown, (0..12).collect::<Vec<_>>());
        let panel = layout.panel;
        assert!(small.contains_rect(panel), "{panel:?} on 1280x800");
        for tile in &layout.tiles {
            assert!(
                panel.contains_rect(selected_rect(*tile)),
                "{tile:?} in {panel:?}"
            );
            assert!(!tile.overlaps(layout.share) && !tile.overlaps(layout.hints));
        }
        // The windows under their heading, below the outputs.
        let (_, windows_at) = layout.headings[1];
        assert!(layout.tiles[2].loc.y - panel.loc.y > windows_at);
        assert!(layout.tiles[2].loc.y > layout.tiles[0].loc.y + SMALL_TILE.1);
        // Scrolled: the second and third rows of windows.
        let scrolled = super::layout(small, 2, 12, 1);
        assert_eq!(scrolled.shown, [0, 1, 7, 8, 9, 10, 11, 12, 13]);
        assert_eq!(scrolled.shown.len(), scrolled.tiles.len());
        // Past the end: the last two rows.
        assert_eq!(super::layout(small, 2, 12, 5).shown, scrolled.shown);
    }

    /// Windows alone: no headings, their rows a grid in the middle.
    #[test]
    fn windows_alone_have_no_headings() {
        let layout = layout(area(), 0, 3, 0);
        assert!(layout.headings.is_empty());
        assert_eq!(layout.shown, [0, 1, 2]);
        let panel = layout.panel;
        let left = layout.tiles[0].loc.x - panel.loc.x;
        let right = panel.loc.x + panel.size.w - (layout.tiles[2].loc.x + WINDOW_TILE.0);
        assert!((left - right).abs() <= 1, "centered: {left} and {right}");
        // A second row starts under the first's first tile.
        let seven = super::layout(area(), 0, 7, 0);
        assert_eq!(seven.tiles[5].loc.x, seven.tiles[0].loc.x);
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
