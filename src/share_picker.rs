//! The screen-share picker drawn by Pila itself: when an app asks the portal
//! Pila serves to share a screen or a window (`portal.rs`), a panel in the
//! middle of the output under the pointer names the app and shows what it may
//! share as live previews: every output, every window (most recently used
//! first, minimized ones and those on other workspaces too), on two tabs,
//! Screens and Windows, when it may share both (Tab or a click switches, each
//! tab keeping its selection); the user picks one (a click, or the arrows)
//! and shares it (Share, Enter), or cancels (Cancel, Escape). Nothing is
//! shared without it: this is the consent.
//!
//! Where it may share a screen, it offers a region of one too (Region,
//! after the outputs): picking it hides the panel for the user to drag one
//! (`share_region.rs`).
//!
//! An app that asks for its share to be remembered gets a Remember toggle
//! under the tiles (R, or a click; off unless the app had one remembered):
//! with it on, the app gets what's picked again next time without asking
//! (`screen_grants.rs`).
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
    backend::renderer::{element::solid::SolidColorBuffer, gles::GlesRenderer},
    input::keyboard::Keysym,
    utils::{Logical, Physical, Point, Rectangle, Scale, Size},
};
use tracing::info;

use smithay::{desktop::Window, utils::IsAlive};

use crate::{
    config::Colors,
    panel::{bits, Align, ContentImage, FrameBuffers, Item, TextKey, BUTTON_GAP, BUTTON_HEIGHT},
    portal::Refused,
    render::{OffscreenWindow, PilaRenderElement},
    screencast::Share,
    Pila,
};

/// The panel's room around its content (Pila's titled panels'), and where
/// the note under its title goes.
const PADDING: i32 = crate::panel::PADDING;
const NOTE_AT: i32 = 56;
/// The tiles: a preview's box, the label under it, the room between them.
/// With tabs, the tabs are where the tiles would start, the tiles under them.
const TILES_AT: i32 = 100;
pub const TILE: (i32, i32) = (320, 180);
const LABEL_GAP: i32 = 14;
const LABEL_HEIGHT: i32 = 20;
const TILE_GAP: i32 = 64;
const ROW_GAP: i32 = 24;
const COLUMNS: usize = 3;
/// Windows' tiles: up to `WINDOW_COLUMNS` a row, `WINDOW_ROWS` rows shown
/// (the selected one's kept in view).
pub const WINDOW_TILE: (i32, i32) = (176, 100);
const WINDOW_GAP: i32 = 20;
const WINDOW_COLUMNS: usize = 5;
const WINDOW_ROWS: usize = 2;
/// The tabs (with both kinds): as tall as the buttons, side by side in the
/// middle, the room under them.
const TAB_WIDTH: i32 = 120;
const TABS_GAP: i32 = 28;
/// The selected tile's highlight: around the tile and its label.
const SELECTED_MARGIN: i32 = 6;
/// The Remember row (an app asking for its share to be remembered): under
/// the tiles, its box at its left, the label beside it.
const REMEMBER_GAP: i32 = 20;
const REMEMBER_HEIGHT: i32 = 28;
const REMEMBER_BOX: i32 = 20;
const REMEMBER_LABEL_AT: i32 = 32;
/// The narrowest panel (one source): room for its title, its note and the
/// buttons.
const MIN_WIDTH: i32 = 694;

/// A tab of the picker that offers both kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// The outputs and the region.
    Screens,
    Windows,
}

impl Tab {
    pub fn name(self) -> &'static str {
        match self {
            Tab::Screens => "Screens",
            Tab::Windows => "Windows",
        }
    }

    fn other(self) -> Tab {
        match self {
            Tab::Screens => Tab::Windows,
            Tab::Windows => Tab::Screens,
        }
    }
}

/// The picker while it's shown (`Pila::share_picker`).
#[derive(Default)]
pub struct SharePicker {
    shown: Option<Shown>,
    /// Selecting a region (Region picked), the panel hidden meanwhile.
    pub(crate) region: Option<crate::share_region::Selecting>,
    /// The regions shared whose outlines are drawn (`Pila::shared_regions`),
    /// and their buffers.
    pub(crate) outlined: Vec<(String, Rectangle<i32, Logical>)>,
    pub(crate) outline_buffers: Rc<RefCell<HashMap<String, SolidColorBuffer>>>,
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
    /// What there is to pick: the outputs, left to right, a region of one,
    /// then the windows.
    sources: Vec<Source>,
    selected: usize,
    /// The kind shown (with both: its tab), and what's selected on the
    /// other tab, back when it's shown again.
    tab: Tab,
    other_selected: usize,
    /// The Remember toggle, if the app asks for its share to be remembered.
    remember: Option<bool>,
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
    /// A region of an output, to drag once picked (`share_region.rs`).
    Region,
    Window(Window),
}

impl Shown {
    /// How many screens (the outputs and the region), then windows, it
    /// offers.
    fn counts(&self) -> (usize, usize) {
        let outputs = self
            .sources
            .iter()
            .filter(|s| matches!(s, Source::Output(_) | Source::Region))
            .count();
        (outputs, self.sources.len() - outputs)
    }

    /// Whether it offers both kinds, on tabs.
    fn tabbed(&self) -> bool {
        let (outputs, windows) = self.counts();
        outputs > 0 && windows > 0
    }

    /// The sources on the tab shown (all of them without tabs).
    fn on_tab(&self) -> std::ops::Range<usize> {
        let (outputs, _) = self.counts();
        match self.tab {
            _ if !self.tabbed() => 0..self.sources.len(),
            Tab::Screens => 0..outputs,
            Tab::Windows => outputs..self.sources.len(),
        }
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
    /// The tabs' (named as the buttons are: `panel::buttons`).
    tabs: HashMap<&'static str, SolidColorBuffer>,
    /// The dim over each output, by name.
    dims: HashMap<String, SolidColorBuffer>,
    text: Option<ContentImage>,
    /// Each output's preview, by name (`Pila::update_share_previews`), and
    /// the background drawn behind it (kept: a new one would be damage).
    previews: HashMap<String, OffscreenWindow>,
    backgrounds: HashMap<String, SolidColorBuffer>,
    /// The region tile's picture: a selection on a plain screen.
    region: HashMap<&'static str, SolidColorBuffer>,
    /// Each window's preview shown (`switcher_view::preview`).
    windows: Vec<(Window, OffscreenWindow)>,
}

/// Where the panel and its parts are, global.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub panel: Rectangle<i32, Logical>,
    /// The tiles shown (the box a preview fits in), and the source each
    /// one is: the outputs' (and the region's), or the windows' rows shown.
    pub tiles: Vec<Rectangle<i32, Logical>>,
    pub shown: Vec<usize>,
    /// The tabs and where they are (none with one kind of source), and the
    /// one shown.
    pub tabs: Vec<(Tab, Rectangle<i32, Logical>)>,
    pub tab: Option<Tab>,
    pub share: Rectangle<i32, Logical>,
    pub cancel: Rectangle<i32, Logical>,
    /// The Remember row, with the toggle.
    pub remember: Option<Rectangle<i32, Logical>>,
}

/// The panel for `outputs` outputs and `windows` windows (rows of them from
/// `first_row` shown), in the middle of `area`. With both kinds, they're on
/// tabs, `tab` shown, the panel the same on either. The outputs' rows are
/// centered, three to a row; the windows' rows a grid in the middle. With
/// `remember`, the Remember row under them.
pub fn layout(
    area: Rectangle<i32, Logical>,
    outputs: usize,
    windows: usize,
    tab: Tab,
    first_row: usize,
    remember: bool,
) -> Layout {
    let tabbed = outputs > 0 && windows > 0;
    let outputs_shown = if outputs == 0 && windows == 0 {
        1
    } else {
        outputs
    };
    // The kind shown.
    let shows = match tab {
        _ if tabbed => tab,
        _ if outputs == 0 && windows > 0 => Tab::Windows,
        _ => Tab::Screens,
    };
    let row_width =
        |n: usize, tile: (i32, i32), gap: i32| n as i32 * tile.0 + (n as i32 - 1).max(0) * gap;
    let tile_row = |tile: (i32, i32)| tile.1 + LABEL_GAP + LABEL_HEIGHT;
    let rows_height =
        |rows: usize, tile: (i32, i32)| rows as i32 * tile_row(tile) + (rows as i32 - 1) * ROW_GAP;
    let window_rows = windows.div_ceil(WINDOW_COLUMNS);
    // What each kind takes, the panel sized for both: it doesn't change
    // between tabs.
    let screens = (outputs > 0 || windows == 0).then(|| {
        (
            row_width(outputs_shown.min(COLUMNS), TILE, TILE_GAP),
            rows_height(outputs_shown.div_ceil(COLUMNS), TILE),
        )
    });
    let window_grid = (windows > 0).then(|| {
        (
            row_width(windows.min(WINDOW_COLUMNS), WINDOW_TILE, WINDOW_GAP),
            rows_height(window_rows.min(WINDOW_ROWS), WINDOW_TILE),
        )
    });
    let content = [screens, window_grid]
        .into_iter()
        .flatten()
        .fold((0, 0), |(w, h), (cw, ch)| (w.max(cw), h.max(ch)));
    let width = (2 * PADDING + content.0).max(MIN_WIDTH);
    let top = if tabbed {
        TILES_AT + BUTTON_HEIGHT + TABS_GAP
    } else {
        TILES_AT
    };
    let remember_row = if remember {
        REMEMBER_GAP + REMEMBER_HEIGHT
    } else {
        0
    };
    let height =
        top + content.1 + remember_row + crate::panel::FOOTER_GAP + BUTTON_HEIGHT + PADDING;
    let panel = crate::panel::centered(area, Size::from((width, height)));
    // `count` tiles of `tile` in rows of `columns` from the top, sources
    // `sources`: each row centered, or (`grid`) the rows as a grid in the
    // middle.
    let mut tiles = Vec::new();
    let mut shown = Vec::new();
    let mut place = |count: usize,
                     columns: usize,
                     tile: (i32, i32),
                     gap: i32,
                     grid: bool,
                     sources: std::ops::Range<usize>| {
        let mut sources = sources;
        for i in 0..count {
            let (row, column) = ((i / columns) as i32, (i % columns) as i32);
            let in_row = if grid {
                count.min(columns)
            } else {
                (count - row as usize * columns).min(columns)
            };
            let x = (width - row_width(in_row, tile, gap)) / 2 + column * (tile.0 + gap);
            let y = top + row * (tile_row(tile) + ROW_GAP);
            tiles.push(Rectangle::new(
                panel.loc + Point::from((x, y)),
                Size::from(tile),
            ));
            shown.extend(sources.next());
        }
    };
    match shows {
        Tab::Screens => place(outputs_shown, COLUMNS, TILE, TILE_GAP, false, 0..outputs),
        Tab::Windows => {
            let first_row = first_row.min(window_rows.saturating_sub(WINDOW_ROWS));
            let from = first_row * WINDOW_COLUMNS;
            let count = (windows - from).min(WINDOW_COLUMNS * WINDOW_ROWS);
            let first = outputs + from;
            place(
                count,
                WINDOW_COLUMNS,
                WINDOW_TILE,
                WINDOW_GAP,
                true,
                first..first + count,
            );
        }
    }
    // The tabs side by side in the middle, above the tiles.
    let tabs = if tabbed {
        let x = (width - 2 * TAB_WIDTH - BUTTON_GAP) / 2;
        [Tab::Screens, Tab::Windows]
            .into_iter()
            .enumerate()
            .map(|(i, tab)| {
                let at = Point::from((x + i as i32 * (TAB_WIDTH + BUTTON_GAP), TILES_AT));
                (
                    tab,
                    Rectangle::new(panel.loc + at, Size::from((TAB_WIDTH, BUTTON_HEIGHT))),
                )
            })
            .collect()
    } else {
        Vec::new()
    };
    let (share, cancel) = crate::panel::footer_buttons(panel);
    let remember = remember.then(|| {
        Rectangle::new(
            panel.loc + Point::from((PADDING, top + content.1 + REMEMBER_GAP)),
            Size::from((width - 2 * PADDING, REMEMBER_HEIGHT)),
        )
    });
    Layout {
        panel,
        tiles,
        shown,
        tabs,
        tab: tabbed.then_some(shows),
        share,
        cancel,
        remember,
    }
}

/// The Remember toggle's box, in its `row`.
fn remember_box(row: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
    Rectangle::new(
        row.loc + Point::from((0, (row.size.h - REMEMBER_BOX) / 2)),
        Size::from((REMEMBER_BOX, REMEMBER_BOX)),
    )
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
    /// Tab and Shift+Tab: the other tab, or without tabs, as Next and
    /// Previous.
    NextTab,
    PreviousTab,
    Share,
    Cancel,
    /// The Remember toggle, where it's shown.
    Remember,
    Nothing,
}

pub fn key_effect(sym: Keysym) -> KeyEffect {
    match sym {
        Keysym::Left | Keysym::Up => KeyEffect::Previous,
        Keysym::Right | Keysym::Down => KeyEffect::Next,
        Keysym::Tab => KeyEffect::NextTab,
        Keysym::ISO_Left_Tab => KeyEffect::PreviousTab,
        Keysym::Return | Keysym::KP_Enter | Keysym::space => KeyEffect::Share,
        Keysym::Escape => KeyEffect::Cancel,
        Keysym::r | Keysym::R => KeyEffect::Remember,
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
    /// The Remember toggle, if it's shown: on or off.
    pub remember: Option<bool>,
    /// The windows' corners (`corner-radius`, `rounded-corners`); `None`:
    /// square.
    pub rounding: Option<(f64, crate::rounded::Corners)>,
    /// Selecting a region: drawn instead of the panel.
    pub region: Option<crate::share_region::SceneRegion>,
    /// The region tile shown, if it is.
    pub region_tile: Option<Rectangle<i32, Logical>>,
    buffers: Rc<RefCell<Buffers>>,
}

impl ScenePicker {
    /// The windows it shows: they draw for its output while it's shown.
    pub fn windows(&self) -> impl Iterator<Item = &Window> {
        // None draw for it while a region is selected: the panel's hidden.
        self.windows
            .iter()
            .filter(|_| self.region.is_none())
            .map(|(window, _)| window)
    }
}

impl Pila {
    /// Show the picker for the first request waiting, unless one is shown.
    /// Never while locked: the requests are cancelled. While another of
    /// Pila's panels that takes every key is open (the color picker, an
    /// app's shortcuts), the requests wait for it to close.
    pub(crate) fn show_share_picker(&mut self) {
        if self.share_picker.shown.is_some() || self.screencast.waiting.is_empty() {
            return;
        }
        // Its own isn't (just above): another one.
        if !self.is_locked() && self.modal_panel_open() {
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
            sources.push(Source::Region);
        }
        if wanted.windows {
            let windows = self
                .windows
                .focus_history
                .iter()
                .filter(|window| crate::screen_grants::shareable(window));
            sources.extend(windows.cloned().map(Source::Window));
        }
        if sources.is_empty() {
            info!("portal: nothing the app may pick");
            ask.cancel();
            return self.show_share_picker();
        }
        // The output under the pointer selected first, else the window used
        // last; with both, the window used last is the Windows tab's.
        let (selected, tab) = if wanted.monitors {
            (under, Tab::Screens)
        } else {
            (0, Tab::Windows)
        };
        let other_selected = if wanted.monitors {
            outputs.len() + 1
        } else {
            0
        };
        // Modal, in front of everything: nothing else of Pila's stays open.
        self.close_transient_panels();
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
        // Offered if the app asks for the share to be remembered: on if it
        // had one remembered (that isn't there now).
        let remember = match &ask {
            Ask::Cast(share) if share.persist > crate::screen_grants::PERSIST_NONE => Some(
                share
                    .restore
                    .as_ref()
                    .is_some_and(|id| self.screen_grant(id, &share.app_id).is_some()),
            ),
            _ => None,
        };
        self.share_picker.shown = Some(Shown {
            ask,
            app,
            output,
            sources,
            selected,
            tab,
            other_selected,
            remember,
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
        let looked_up = app_id.clone();
        self.off_loop(
            "pila-app-name",
            move || crate::portal::app_name(&looked_up, &crate::portal::data_dirs()),
            move |state, name| {
                let Some(name) = name else {
                    return;
                };
                let Some(shown) = state.share_picker.shown.as_mut() else {
                    return;
                };
                if shown.app.as_deref() == Some(app_id.as_str()) && name != app_id {
                    shown.app = Some(name);
                    state.draw_share_picker_text();
                }
            },
        );
    }

    pub fn share_picker_open(&self) -> bool {
        self.share_picker.shown.is_some()
    }

    /// Hide the picker, giving back its request; the next one waiting is
    /// shown by the caller.
    fn take_share_picker(&mut self) -> Option<Shown> {
        let shown = self.share_picker.shown.take()?;
        self.end_region_select();
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
        self.show_waiting_panels();
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
            self.show_waiting_panels();
        }
    }

    /// Share what's selected (Share, Enter); the region, once dragged.
    fn pick_share(&mut self) {
        let Some(shown) = self.share_picker.shown.as_ref() else {
            return;
        };
        let target = match shown.sources.get(shown.selected) {
            Some(Source::Region) => return self.start_region_select(),
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
        self.finish_share_pick(target);
    }

    /// Share or take `target`, picked (`None`: it went meanwhile, and the
    /// app is told it was cancelled), and ask the next request.
    pub(crate) fn finish_share_pick(&mut self, target: Option<crate::capture::Target>) {
        let Some(shown) = self.take_share_picker() else {
            return;
        };
        match (target, shown.ask) {
            (Some(target), Ask::Cast(share)) => {
                // Remembered if the toggle's on; otherwise the grant the app
                // asked to restore, not there, goes.
                let remembered = match shown.remember {
                    Some(true) => self.new_screen_grant(&share, shown.app.as_deref(), &target),
                    _ => {
                        if let Some(id) = &share.restore {
                            if self.screen_grant(id, &share.app_id).is_some() {
                                self.forget_screen_grant(id);
                            }
                        }
                        None
                    }
                };
                self.start_cast(share, target, remembered)
            }
            (Some(target), Ask::Screenshot(shot)) => self.take_screenshot(Some(target), shot.reply),
            (None, ask) => ask.cancel(),
        }
        self.show_waiting_panels();
    }

    /// Show `tab` (with both kinds), what was selected on it selected again.
    fn show_share_tab(&mut self, tab: Tab) {
        let Some(shown) = self.share_picker.shown.as_mut() else {
            return;
        };
        if !shown.tabbed() || shown.tab == tab {
            return;
        }
        shown.tab = tab;
        std::mem::swap(&mut shown.selected, &mut shown.other_selected);
        let output = shown.output.clone();
        self.queue_redraw_named_output(&output);
        // Other tiles and labels; the outputs' previews drawn after the next
        // frames, now.
        self.draw_share_picker_text();
        self.capture_due = true;
        self.wake.ping();
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

    /// Turn the Remember toggle on or off, where it's shown.
    fn toggle_share_remember(&mut self) {
        let Some(shown) = self.share_picker.shown.as_mut() else {
            return;
        };
        let Some(remember) = shown.remember.as_mut() else {
            return;
        };
        *remember = !*remember;
        let output = shown.output.clone();
        self.queue_redraw_named_output(&output);
    }

    /// Whether what it asks for is a screenshot.
    pub(crate) fn share_picker_screenshot(&self) -> bool {
        self.share_picker
            .shown
            .as_ref()
            .is_some_and(|shown| matches!(shown.ask, Ask::Screenshot(_)))
    }

    /// A key pressed while it's shown (it takes them all: `input.rs`).
    pub fn share_picker_key(&mut self, sym: Keysym) {
        if self.region_selecting() {
            return self.region_key(sym);
        }
        let Some(shown) = self.share_picker.shown.as_ref() else {
            return;
        };
        // Round the tab shown.
        let range = shown.on_tab();
        let (first, count) = (range.start, range.len().max(1));
        let at = shown.selected.saturating_sub(first) % count;
        let (tabbed, tab) = (shown.tabbed(), shown.tab);
        match key_effect(sym) {
            KeyEffect::NextTab | KeyEffect::PreviousTab if tabbed => {
                self.show_share_tab(tab.other())
            }
            KeyEffect::Previous | KeyEffect::PreviousTab => {
                self.select_share(first + (at + count - 1) % count)
            }
            KeyEffect::Next | KeyEffect::NextTab => self.select_share(first + (at + 1) % count),
            KeyEffect::Share => self.pick_share(),
            KeyEffect::Cancel => self.cancel_share_picker(),
            KeyEffect::Remember => self.toggle_share_remember(),
            KeyEffect::Nothing => {}
        }
    }

    /// A press at `location` while it's shown (it takes them all): a tab
    /// shows it, a tile selects its source, the buttons share or cancel,
    /// the Remember row turns it on or off, elsewhere nothing.
    pub fn share_picker_press(&mut self, location: Point<f64, Logical>) {
        if self.region_selecting() {
            return self.region_press(location);
        }
        let Some(view) = self.scene_share_picker() else {
            return;
        };
        let layout = &view.layout;
        if layout.share.to_f64().contains(location) {
            self.pick_share();
        } else if layout.cancel.to_f64().contains(location) {
            self.cancel_share_picker();
        } else if layout
            .remember
            .is_some_and(|row| row.to_f64().contains(location))
        {
            self.toggle_share_remember();
        } else if let Some((tab, _)) = layout
            .tabs
            .iter()
            .find(|(_, rect)| rect.to_f64().contains(location))
        {
            self.show_share_tab(*tab);
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

    /// The pointer moved while it's shown: a region being selected follows
    /// it.
    pub(crate) fn share_picker_moved(&mut self, location: Point<f64, Logical>) {
        if self.region_selecting() {
            self.region_moved(location);
        }
    }

    /// A button let go while it's shown (its press was the picker's).
    pub(crate) fn share_picker_release(&mut self) {
        if self.region_selecting() {
            self.region_release();
        }
    }

    /// The picker for `Pila::scene`, while it's shown and not locked.
    pub(crate) fn scene_share_picker(&self) -> Option<ScenePicker> {
        let shown = self.share_picker.shown.as_ref()?;
        let (output, area) = self.output_named_or_first(Some(&shown.output))?;
        let (outputs, windows) = shown.counts();
        let layout = layout(
            area,
            outputs,
            windows,
            shown.tab,
            shown.first_window_row(),
            shown.remember.is_some(),
        );
        let mut previews = Vec::new();
        let mut shown_windows = Vec::new();
        let mut region_tile = None;
        for (tile, index) in layout.tiles.iter().zip(&layout.shown) {
            match shown.sources.get(*index) {
                Some(Source::Region) => region_tile = Some(*tile),
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
            remember: shown.remember,
            rounding: crate::panel::rounding(&self.config),
            region: self.scene_region(),
            region_tile,
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
            elements.push(crate::panel::keyed_solid(
                &mut buffers.backgrounds,
                name.clone(),
                Rectangle::from_size((size.w, size.h).into()),
                background,
                1.0,
                Point::default(),
                Scale::from(1.0),
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
        let scale = self.panel_text_scale(&view.output);
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
                Some(Source::Region) => "Region".to_string(),
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
        self.draw_panel_text(
            key,
            drawn,
            "pila-share-picker",
            |state| &mut state.share_picker.pending,
            Self::draw_share_picker_text,
            |state, text| {
                state.share_picker.buffers.borrow_mut().text = Some(text);
                if let Some(view) = state.scene_share_picker() {
                    state.queue_redraw_named_output(&view.output);
                }
            },
        );
    }

    /// For the state dump: whether it's shown, for whom, where, its sources
    /// and controls.
    pub fn share_picker_json(&self) -> serde_json::Value {
        let rect = crate::introspect::rect;
        let (Some(view), Some(shown)) =
            (self.scene_share_picker(), self.share_picker.shown.as_ref())
        else {
            return serde_json::json!({
                "shown": false,
                "waiting": self.screencast.waiting.len(),
                "region": self.region_json(),
            });
        };
        serde_json::json!({
            "shown": true,
            "app": shown.app,
            "screenshot": matches!(shown.ask, Ask::Screenshot(_)),
            "output": view.output,
            // Outputs by name, windows by app id.
            "sources": shown.sources.iter().map(|source| match source {
                Source::Output(name) => name.clone(),
                Source::Region => "region".to_string(),
                Source::Window(window) => crate::foreign_toplevel::app_id(window),
            }).collect::<Vec<_>>(),
            "kinds": shown.sources.iter().map(|source| match source {
                Source::Output(_) => "output",
                Source::Region => "region",
                Source::Window(_) => "window",
            }).collect::<Vec<_>>(),
            "region": self.region_json(),
            "selected": view.selected,
            "panel": rect(view.layout.panel),
            "tiles": view.layout.tiles.iter().map(|t| rect(*t)).collect::<Vec<_>>(),
            "tile_sources": view.layout.shown,
            "tabs": view.layout.tabs.iter().map(|(tab, _)| tab.name()).collect::<Vec<_>>(),
            "tab_rects": view.layout.tabs.iter().map(|(_, r)| rect(*r)).collect::<Vec<_>>(),
            "tab": view.layout.tab.map(Tab::name),
            "window_previews": view.buffers.borrow().windows.len(),
            "share": rect(view.layout.share),
            "cancel": rect(view.layout.cancel),
            "remember": shown.remember,
            "remember_rect": view.layout.remember.map(rect),
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
    use crate::panel::BODY;
    let panel = layout.panel;
    let local = |r: Rectangle<i32, Logical>| (r.loc.x - panel.loc.x, r.loc.y - panel.loc.y);
    let (text, _, _) = crate::panel::text_colors(colors);
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
        crate::panel::note(note, NOTE_AT, panel.size.w, colors),
    ];
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
    // Share's label on the accent, as the switcher's selected title, and
    // the tab shown's.
    let on_accent = bits(colors.hover_icon);
    let act = if screenshot { "Take" } else { "Share" };
    let tabs = layout.tabs.iter().map(|(tab, rect)| {
        let color = if Some(*tab) == layout.tab {
            on_accent
        } else {
            text
        };
        (*rect, tab.name(), color)
    });
    items.extend(crate::panel::button_labels(
        panel,
        (layout.share, act),
        (layout.cancel, "Cancel"),
        colors,
    ));
    items.extend(
        tabs.map(|(rect, label, color)| crate::panel::button_label(panel, rect, label, color)),
    );
    if let Some(row) = layout.remember {
        let (x, y) = local(row);
        let label = match app {
            Some(app) => format!("Remember: share this with {app} again without asking"),
            None => "Remember: share this with the app again without asking".to_string(),
        };
        items.push(Item::Text {
            text: label,
            at: (
                x + REMEMBER_LABEL_AT,
                y + (REMEMBER_HEIGHT - LABEL_HEIGHT) / 2,
            ),
            w: row.size.w - REMEMBER_LABEL_AT,
            h: LABEL_HEIGHT,
            font: BODY,
            align: Align::Left,
            color: text,
        });
    }
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
    if let Some(region) = &view.region {
        return crate::share_region::elements(
            renderer,
            region,
            colors,
            output_name,
            output_geo,
            scale,
        );
    }
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

    // The region's tile: a selection outlined in the accent on a plain
    // screen of the output's shape.
    if let Some(tile) = view.region_tile {
        let screen = preview_rect(tile, output_geo.size);
        let selection = Rectangle::new(
            screen.loc + Point::from((screen.size.w / 4, screen.size.h / 4)),
            screen.size / 2,
        );
        let accent = crate::panel::accent(colors);
        // A selection's outline, inside it.
        let edge = 2;
        let inside = Rectangle::new(
            selection.loc + Point::from((edge, edge)),
            selection.size - Size::from((2 * edge, 2 * edge)),
        );
        let names = ["region-top", "region-bottom", "region-left", "region-right"];
        let sides = names
            .into_iter()
            .zip(crate::panel::outline_sides(inside, edge));
        let solids = sides
            .into_iter()
            .map(|(name, rect)| (name, rect, accent, None))
            .chain([(
                "region-screen",
                screen,
                crate::panel::opaque(colors.bar),
                None,
            )]);
        elements.extend(crate::panel::solids(
            renderer,
            &mut buffers.region,
            solids,
            origin,
            scale,
        ));
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
    // The Remember toggle's box: edged as Cancel, the accent inside when
    // it's on.
    let toggle = layout
        .remember
        .map(remember_box)
        .into_iter()
        .flat_map(|square| {
            let mark = Rectangle::new(
                square.loc + Point::from((5, 5)),
                square.size - Size::from((10, 10)),
            );
            let on = view.remember == Some(true);
            let fill = if on {
                crate::panel::accent(colors)
            } else {
                crate::panel::opaque(colors.bar)
            };
            let [fill_box, edge] =
                crate::panel::secondary_button(["remember-fill", "remember-edge"], square, colors);
            [
                (
                    "remember-mark",
                    mark,
                    fill,
                    Some((2.0, crate::rounded::Corners::All)),
                ),
                fill_box,
                edge,
            ]
        });
    let solids = selected
        .into_iter()
        .chain(toggle)
        .chain(crate::panel::buttons(layout.share, layout.cancel, colors));
    elements.extend(crate::panel::solids(
        renderer,
        &mut buffers.solids,
        solids,
        origin,
        scale,
    ));
    // The tabs as the buttons: the one shown as Share, the other as Cancel.
    let active = layout.tabs.iter().find(|(tab, _)| Some(*tab) == layout.tab);
    let inactive = layout.tabs.iter().find(|(tab, _)| Some(*tab) != layout.tab);
    if let (Some((_, active)), Some((_, inactive))) = (active, inactive) {
        elements.extend(crate::panel::solids(
            renderer,
            &mut buffers.tabs,
            crate::panel::buttons(*active, *inactive, colors),
            origin,
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

    /// The picker's title and type are the other titled panels'
    /// (`panel.rs`): its title where theirs is, in the same font.
    #[test]
    fn it_has_the_titled_panels_title() {
        let colors = crate::config::Config::parse("", &|_| None, crate::config::ModKey::Super)
            .unwrap()
            .colors;
        let monitors = crate::portal::Sources {
            monitors: true,
            windows: false,
        };
        let picker = items(
            &layout(area(), 1, 0, Tab::Screens, 0, false),
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
    }

    fn area() -> Rectangle<i32, Logical> {
        Rectangle::new((0, 0).into(), (1920, 1080).into())
    }

    /// The panel's parts inside it, none on another.
    fn assert_inside(layout: &Layout, what: &str) {
        let panel = layout.panel;
        let tabs = layout.tabs.iter().map(|(_, rect)| *rect);
        let controls: Vec<_> = [layout.share, layout.cancel]
            .into_iter()
            .chain(tabs)
            .chain(layout.remember)
            .collect();
        for rect in &controls {
            assert!(panel.contains_rect(*rect), "{what}: {rect:?} in {panel:?}");
        }
        for (i, a) in controls.iter().enumerate() {
            for b in &controls[i + 1..] {
                assert!(!a.overlaps(*b), "{what}: {a:?} on {b:?}");
            }
        }
        for tile in &layout.tiles {
            let around = selected_rect(*tile);
            assert!(panel.contains_rect(around), "{what}: {tile:?} in {panel:?}");
            for rect in &controls {
                assert!(!around.overlaps(*rect), "{what}: {tile:?} on {rect:?}");
            }
        }
    }

    #[test]
    fn the_panel_is_centered_with_its_tiles_and_buttons_inside() {
        for count in 1..=5 {
            let layout = layout(area(), count, 0, Tab::Screens, 0, false);
            assert_eq!(layout.tiles.len(), count);
            assert!(layout.tabs.is_empty(), "no tabs with one kind");
            let panel = layout.panel;
            assert_eq!(panel.loc.x * 2 + panel.size.w, 1920, "centered");
            assert_inside(&layout, &format!("{count} outputs"));
        }
        // Three to a row, then another row.
        let four = layout(area(), 4, 0, Tab::Screens, 0, false);
        assert_eq!(
            four.tiles[3].loc.y,
            four.tiles[0].loc.y + TILE.1 + LABEL_GAP + LABEL_HEIGHT + ROW_GAP
        );
    }

    /// The Remember row, where it's offered: under the tiles, above the
    /// footer, the panel taller by it.
    #[test]
    fn the_remember_row_is_between_the_tiles_and_the_footer() {
        for (outputs, windows, tab) in [(1, 0, Tab::Screens), (2, 7, Tab::Windows)] {
            let without = layout(area(), outputs, windows, tab, 0, false);
            let with = layout(area(), outputs, windows, tab, 0, true);
            assert_eq!(without.remember, None);
            let row = with.remember.expect("the row");
            assert_inside(&with, "with Remember");
            assert_eq!(
                with.panel.size.h,
                without.panel.size.h + REMEMBER_GAP + REMEMBER_HEIGHT
            );
            assert!(with.tiles.iter().all(|t| t.loc.y < row.loc.y));
            assert!(row.loc.y + row.size.h < with.share.loc.y);
            assert!(row.contains_rect(remember_box(row)));
        }
    }

    /// With windows too, the outputs and the windows are on tabs, the panel
    /// the same on both: on Screens the outputs alone, on Windows at most two
    /// rows of windows, scrolled to the selected one's; it fits a 1280x800
    /// logical monitor (1920x1200 at 1.5).
    #[test]
    fn outputs_and_windows_are_on_tabs() {
        let small = Rectangle::new((0, 0).into(), (1280, 800).into());
        for outputs in [2, 4] {
            let screens = layout(small, outputs, 12, Tab::Screens, 0, false);
            let windows = layout(small, outputs, 12, Tab::Windows, 0, false);
            for (layout, tab) in [(&screens, Tab::Screens), (&windows, Tab::Windows)] {
                assert_eq!(
                    layout.tabs.iter().map(|(t, _)| *t).collect::<Vec<_>>(),
                    [Tab::Screens, Tab::Windows]
                );
                assert_eq!(layout.tab, Some(tab));
                assert!(small.contains_rect(layout.panel), "{:?}", layout.panel);
                assert_inside(layout, &format!("{outputs} outputs, {tab:?}"));
                // The tabs above the tiles.
                let tabs_bottom = layout.tabs[0].1.loc.y + layout.tabs[0].1.size.h;
                assert!(layout.tiles.iter().all(|t| t.loc.y > tabs_bottom));
            }
            assert_eq!(screens.panel, windows.panel, "the panel doesn't move");
            assert_eq!(screens.shown, (0..outputs).collect::<Vec<_>>());
            assert_eq!(screens.tiles[0].size, Size::from(TILE));
            assert_eq!(windows.shown, (outputs..outputs + 10).collect::<Vec<_>>());
        }
        // Scrolled: the second and third rows of windows.
        let scrolled = layout(small, 2, 12, Tab::Windows, 1, false);
        assert_eq!(scrolled.shown, [7, 8, 9, 10, 11, 12, 13]);
        assert_eq!(scrolled.shown.len(), scrolled.tiles.len());
        // Past the end: the last two rows.
        assert_eq!(
            layout(small, 2, 12, Tab::Windows, 5, false).shown,
            scrolled.shown
        );
    }

    /// Windows alone: no tabs, their rows a grid in the middle.
    #[test]
    fn windows_alone_have_no_tabs() {
        let layout = layout(area(), 0, 3, Tab::Windows, 0, false);
        assert!(layout.tabs.is_empty());
        assert_eq!(layout.tab, None);
        assert_eq!(layout.shown, [0, 1, 2]);
        assert_inside(&layout, "3 windows");
        let panel = layout.panel;
        let left = layout.tiles[0].loc.x - panel.loc.x;
        let right = panel.loc.x + panel.size.w - (layout.tiles[2].loc.x + WINDOW_TILE.0);
        assert!((left - right).abs() <= 1, "centered: {left} and {right}");
        // A second row starts under the first's first tile.
        let seven = super::layout(area(), 0, 7, Tab::Windows, 0, false);
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
        assert_eq!(key_effect(Keysym::Right), KeyEffect::Next);
        assert_eq!(key_effect(Keysym::Tab), KeyEffect::NextTab);
        assert_eq!(key_effect(Keysym::ISO_Left_Tab), KeyEffect::PreviousTab);
        assert_eq!(key_effect(Keysym::Return), KeyEffect::Share);
        assert_eq!(key_effect(Keysym::Escape), KeyEffect::Cancel);
        assert_eq!(key_effect(Keysym::r), KeyEffect::Remember);
        assert_eq!(key_effect(Keysym::a), KeyEffect::Nothing);
    }
}
