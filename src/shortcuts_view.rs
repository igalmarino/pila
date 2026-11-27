//! The shortcuts cheatsheet drawn by Pila itself: a panel in the middle of
//! an output listing the common bindings as they're bound now (Mod resolved,
//! a rebound key as the user wrote it, an unbound action left out). Shown on
//! the first run, when Pila has just written the config from the example
//! (the welcome screen), and by `show-shortcuts` (Mod+? by default).
//!
//! Any key that isn't a modifier, or a click, hides it: a key that's a
//! binding still runs it (Mod+Return hides it and opens a terminal), any
//! other key and a click on the panel go nowhere else (`input.rs`). Its
//! texts are drawn on a thread into one image the size of the panel
//! (`Pila::draw_shortcuts_text`), a single texture, and the panel shows once
//! it's there; all of it goes when it's hidden.

use std::{cell::RefCell, rc::Rc};

use smithay::{
    backend::renderer::gles::GlesRenderer,
    input::keyboard::{xkb, Keysym},
    utils::{Logical, Point, Rectangle, Scale, Size},
};

use crate::{
    config::{Action, Colors, Config, KeyCombo, ModKey, Trigger, WorkspaceTarget},
    panel::{self, Align, ContentImage, FrameBuffers, Item, TextKey},
    render::PilaRenderElement,
    Pila,
};

/// The room kept around a panel shrunk to fit its monitor (`fit`).
const FIT_MARGIN: i32 = 32;

/// The welcome: its width, where its rows start, each row's height, where
/// the keys and what they do go, and the room under the rows (the
/// footer's: where the settings are).
const WELCOME_WIDTH: i32 = 560;
const WELCOME_ROWS_AT: i32 = 120;
const WELCOME_ROW: i32 = 38;
const WELCOME_KEYS_AT: i32 = 90;
const WELCOME_WHAT_AT: i32 = 300;
const WELCOME_BOTTOM: i32 = panel::FOOTER - (WELCOME_ROW - panel::CAP_HEIGHT);

/// The full cheatsheet: its width, its columns, where its sections start,
/// a section's heading and row heights, where what a row does goes (from
/// its column) and the room under the columns (`PADDING` under the last
/// row's keys: it has no footer, its keys are in the manual).
const SHORTCUTS_WIDTH: i32 = 860;
const COLUMNS_AT: [i32; 2] = [panel::PADDING, 448];
const SECTIONS_AT: i32 = 72;
const SECTION_HEADING: i32 = 22;
const SECTION_ROW: i32 = 32;
const SECTION_GAP: i32 = 14;
const SECTION_WHAT_AT: i32 = 200;
const SHORTCUTS_BOTTOM: i32 = panel::PADDING - (SECTION_ROW - panel::CAP_HEIGHT);

/// What the panel is shown as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heading {
    /// The first run: Pila just wrote the config from the example. A few
    /// keys to get going.
    Welcome,
    /// Asked for with `show-shortcuts`: all of them, in sections.
    Shortcuts,
}

impl Heading {
    fn text(self) -> &'static str {
        match self {
            Heading::Welcome => "Welcome to Pila",
            Heading::Shortcuts => "Shortcuts",
        }
    }
}

/// The cheatsheet while it's shown.
#[derive(Default)]
pub struct ShortcutsView {
    shown: Option<Shown>,
    /// The content being drawn on a thread (`Pila::draw_shortcuts_text`).
    pending: Option<TextKey>,
    buffers: Rc<RefCell<Buffers>>,
}

struct Shown {
    heading: Heading,
    /// The output it's on, by name: the one under the pointer when it was
    /// shown (`None` before there was one: the first).
    output: Option<String>,
}

/// Kept between frames while it's shown: an unchanged panel adds no damage.
#[derive(Default)]
struct Buffers {
    frame: FrameBuffers,
    /// The dim over each output, by name.
    dims: std::collections::HashMap<
        String,
        smithay::backend::renderer::element::solid::SolidColorBuffer,
    >,
    /// Everything on the panel, its text and keycaps, in one image the size
    /// of the panel: one texture, made on a thread
    /// (`Pila::draw_shortcuts_text`).
    text: Option<ContentImage>,
}

/// One of a row's keys: what its keycap says ("Super", "Q", "1…4").
pub type Cap = String;

/// A row: the keycaps of its keys, as bound now, and what they do.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub keys: Vec<Cap>,
    pub what: &'static str,
}

/// A section of the full cheatsheet.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub title: &'static str,
    pub rows: Vec<Row>,
}

/// The welcome's items: the title, a line under it, its rows centered, and
/// the footer with `notes`, for a panel `height` high.
fn welcome_items(rows: &[Row], notes: &[String], height: i32, colors: &Colors) -> Vec<Item> {
    let (text, dim, _) = panel::text_colors(colors);
    let center = |text: String, y, h, font, color| Item::Text {
        text,
        at: (0, y),
        w: WELCOME_WIDTH,
        h,
        font,
        align: Align::Center,
        color,
    };
    let mut items = vec![
        center(
            Heading::Welcome.text().into(),
            34,
            40,
            panel::WELCOME_TITLE,
            text,
        ),
        center("A few keys to get going".into(), 74, 20, panel::NOTE, dim),
    ];
    for (i, row) in rows.iter().enumerate() {
        let y = WELCOME_ROWS_AT + i as i32 * WELCOME_ROW;
        items.push(Item::Keys {
            caps: row.keys.clone(),
            at: (WELCOME_KEYS_AT, y),
            w: WELCOME_WHAT_AT - WELCOME_KEYS_AT - 10,
            size: panel::KEY,
        });
        items.push(Item::Text {
            text: row.what.into(),
            at: (WELCOME_WHAT_AT, y),
            w: WELCOME_WIDTH - WELCOME_WHAT_AT - 20,
            h: panel::CAP_HEIGHT,
            font: panel::BODY,
            align: Align::Left,
            color: text,
        });
    }
    if !notes.is_empty() {
        let y = height - panel::PADDING - panel::FOOTER_HEIGHT;
        items.push(panel::footer(
            &notes.join("  ·  "),
            y,
            WELCOME_WIDTH,
            colors,
        ));
    }
    items
}

/// The full cheatsheet's items: the title and its sections in two
/// columns.
fn shortcuts_items(columns: &[Vec<Section>; 2], colors: &Colors) -> Vec<Item> {
    let (text, _, accent) = panel::text_colors(colors);
    let mut items = vec![panel::title(
        Heading::Shortcuts.text(),
        SHORTCUTS_WIDTH,
        colors,
    )];
    for (column, sections) in COLUMNS_AT.iter().zip(columns) {
        let mut y = SECTIONS_AT;
        for section in sections {
            items.push(Item::Text {
                text: section.title.to_uppercase(),
                at: (*column, y),
                w: 380,
                h: 16,
                font: panel::SECTION,
                align: Align::Left,
                color: accent,
            });
            y += SECTION_HEADING;
            for row in &section.rows {
                items.push(Item::Keys {
                    caps: row.keys.clone(),
                    at: (*column, y),
                    w: SECTION_WHAT_AT - 10,
                    size: panel::KEY,
                });
                items.push(Item::Text {
                    text: row.what.into(),
                    at: (column + SECTION_WHAT_AT, y),
                    w: 380 - SECTION_WHAT_AT,
                    h: panel::CAP_HEIGHT,
                    font: panel::BODY,
                    align: Align::Left,
                    color: text,
                });
                y += SECTION_ROW;
            }
            y += SECTION_GAP;
        }
    }
    items
}

/// A column's height: its sections' headings and rows, a gap between them.
fn column_height(sections: &[Section]) -> i32 {
    let rows: usize = sections.iter().map(|s| s.rows.len()).sum();
    sections.len() as i32 * (SECTION_HEADING + SECTION_GAP) - SECTION_GAP
        + rows as i32 * SECTION_ROW
}

/// What `Pila::scene` gives the renderer of the panel shown.
#[derive(Clone)]
pub struct SceneShortcuts {
    /// The output it's on, by name.
    pub output: String,
    pub rect: Rectangle<i32, Logical>,
    /// Its size as laid out (`panel_size`), which its content is drawn at:
    /// `rect` is that, shrunk to fit its monitor (`fit`).
    pub layout_size: Size<i32, Logical>,
    pub heading: Heading,
    /// Its sections: the welcome's one, untitled; the full cheatsheet's in
    /// two columns, the first section the first column.
    pub sections: Vec<Section>,
    /// What the footer says beside its hint: where the settings are (the
    /// welcome's).
    pub notes: Vec<String>,
    /// Its corners, the windows' (`corner-radius`, `rounded-corners`);
    /// `None`: square.
    pub rounding: Option<(f64, crate::rounded::Corners)>,
    buffers: Rc<RefCell<Buffers>>,
}

impl SceneShortcuts {
    /// Every row, section after section.
    pub fn rows(&self) -> impl Iterator<Item = &Row> {
        self.sections.iter().flat_map(|section| &section.rows)
    }
}

/// Whether a binding's action counts as a row's.
type Matches = fn(&Action) -> bool;

/// The welcome's rows: what they do, and the bindings that count as it. A
/// row whose action has no binding is left out.
const WELCOME: &[(&str, Matches)] = &[
    ("Open a terminal", |a| *a == Action::Terminal),
    ("Open the launcher", |a| *a == Action::Launcher),
    ("Switch windows", |a| *a == Action::CycleFocus),
    ("Close a window", |a| *a == Action::CloseFocused),
    ("Quit Pila", Action::quits),
    ("All the shortcuts", |a| *a == Action::ShowShortcuts),
];

/// The full cheatsheet's sections, the first in the left column, the others
/// in the right one. "Mouse" has Mod's drags, which aren't bindings.
const SECTIONS: &[(&str, &[(&str, Matches)])] = &[
    (
        "Windows",
        &[
            ("Terminal", |a| *a == Action::Terminal),
            ("Launcher", |a| *a == Action::Launcher),
            ("Switch windows", |a| *a == Action::CycleFocus),
            ("Overview", |a| {
                *a == Action::Overview(crate::config::OverviewAction::Toggle)
            }),
            ("Close", |a| *a == Action::CloseFocused),
            ("Maximize", |a| *a == Action::ToggleMaximize),
            ("Fullscreen", |a| *a == Action::ToggleFullscreen),
            ("Minimize", |a| *a == Action::Minimize),
            ("Snap to halves", |a| matches!(a, Action::SnapToward(_))),
        ],
    ),
    (
        "Workspaces",
        &[
            ("Go to one", |a| {
                matches!(a, Action::Workspace(WorkspaceTarget::Index(_)))
            }),
            ("Move the window there", |a| {
                matches!(a, Action::MoveToWorkspace(WorkspaceTarget::Index(_)))
            }),
        ],
    ),
    ("Mouse", &[]),
    (
        "Pila",
        &[
            ("Show the desktop", |a| *a == Action::ShowDesktop),
            ("Quit", Action::quits),
        ],
    ),
];

/// The modifier `mod_key` is, as the cheatsheet names it.
fn mod_name(mod_key: ModKey) -> &'static str {
    match mod_key {
        ModKey::Alt => "Alt",
        ModKey::Super => "Super",
        ModKey::Ctrl => "Ctrl",
    }
}

/// A key as the cheatsheet names it: its character where it has one
/// (`slash` is "/", `d` is "D"), arrows as arrows, else its xkb name.
fn key_name(sym: Keysym) -> String {
    match sym {
        Keysym::Left => return "←".into(),
        Keysym::Right => return "→".into(),
        Keysym::Up => return "↑".into(),
        Keysym::Down => return "↓".into(),
        _ => {}
    }
    let name = xkb::keysym_get_name(sym);
    match sym.key_char() {
        Some(c) if c.is_alphabetic() => c.to_uppercase().collect(),
        Some(c) if c.is_ascii_graphic() => c.to_string(),
        _ => name,
    }
}

/// `combo`'s keycaps: its modifiers, then its key; a tap of a modifier is
/// "Super (tap)", a wheel's notch "wheel ↓".
pub fn combo_caps(combo: &KeyCombo) -> Vec<Cap> {
    let mods = combo.mods;
    if let Trigger::Key(sym) = combo.trigger {
        if crate::config::tap_sym(sym).is_some() {
            let name = if mods.logo {
                "Super"
            } else if mods.alt {
                "Alt"
            } else {
                "Ctrl"
            };
            return vec![format!("{name} (tap)")];
        }
    }
    let mut caps: Vec<Cap> = [
        (mods.logo, "Super"),
        (mods.ctrl, "Ctrl"),
        (mods.alt, "Alt"),
        (mods.shift, "Shift"),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, name)| name.to_string())
    .collect();
    caps.push(match combo.trigger {
        Trigger::Key(sym) => key_name(sym),
        Trigger::Wheel(direction) => {
            use crate::config::WheelDirection;
            match direction {
                WheelDirection::Up => "wheel ↑",
                WheelDirection::Down => "wheel ↓",
                WheelDirection::Left => "wheel ←",
                WheelDirection::Right => "wheel →",
            }
            .into()
        }
    });
    caps
}

/// `combo` as text: its keycaps joined, "Super+Shift+Q".
pub fn combo_label(combo: &KeyCombo) -> String {
    combo_caps(combo).join("+")
}

/// Combinations that differ only in their last key, a run of numbers from 1
/// ("Super 1", "Super 2" ...) or arrows, as one: "Super 1…9", "Super ← →".
fn merged(all: &[Vec<Cap>]) -> Option<Vec<Cap>> {
    const ARROWS: [&str; 4] = ["←", "→", "↑", "↓"];
    let (first, rest) = all.split_first()?;
    let (_, prefix) = first.split_last()?;
    let lasts: Option<Vec<&str>> = all
        .iter()
        .map(|caps| {
            let (last, others) = caps.split_last()?;
            (others == prefix).then_some(last.as_str())
        })
        .collect();
    let lasts = lasts.filter(|_| !rest.is_empty())?;
    let numbered = lasts
        .iter()
        .enumerate()
        .all(|(i, last)| *last == (i + 1).to_string());
    let last = if numbered {
        format!("1…{}", lasts.len())
    } else if lasts.iter().all(|last| ARROWS.contains(last)) {
        let mut arrows: Vec<&str> = lasts;
        arrows.sort_by_key(|a| ARROWS.iter().position(|b| b == a));
        arrows.dedup();
        arrows.join(" ")
    } else {
        return None;
    };
    let mut caps = prefix.to_vec();
    caps.push(last);
    Some(caps)
}

/// The keycaps of what `matches` binds in `config`: the combination to name
/// first (keys before a tap of a modifier, named keys like Tab before
/// letters like J, Mod's before other modifiers'), or a run of them as one
/// (`merged`). `None` when nothing binds it.
fn keys_for(config: &Config, matches: Matches) -> Option<Vec<Cap>> {
    let mut bound: Vec<&KeyCombo> = config
        .bindings
        .iter()
        .filter(|b| matches(&b.action))
        .map(|b| &b.combo)
        .collect();
    bound.sort_by_key(|combo| {
        let (tap, letter) = match combo.trigger {
            Trigger::Key(sym) => (
                crate::config::tap_sym(sym).is_some(),
                sym.key_char().is_some_and(char::is_alphabetic),
            ),
            Trigger::Wheel(_) => (false, false),
        };
        let with_mod = match config.mod_key {
            ModKey::Alt => combo.mods.alt,
            ModKey::Super => combo.mods.logo,
            ModKey::Ctrl => combo.mods.ctrl,
        };
        (tap, letter, !with_mod)
    });
    let all: Vec<Vec<Cap>> = bound.iter().map(|c| combo_caps(c)).collect();
    // A run in the order bound (Mod+1, Mod+2 ...), before the ranking.
    let mut in_order: Vec<Vec<Cap>> = all.clone();
    in_order.sort_by_key(|caps| caps.last().and_then(|k| k.parse::<u32>().ok()));
    merged(&in_order).or_else(|| all.into_iter().next())
}

/// The welcome's rows from `config`: the few keys to get going, as bound
/// now.
pub fn welcome_rows(config: &Config) -> Vec<Row> {
    WELCOME
        .iter()
        .filter_map(|(what, matches)| {
            Some(Row {
                keys: keys_for(config, *matches)?,
                what,
            })
        })
        .collect()
}

/// The full cheatsheet's sections from `config`, as bound now; a section
/// with nothing bound is left out.
pub fn sections(config: &Config) -> Vec<Section> {
    let mod_name = mod_name(config.mod_key);
    SECTIONS
        .iter()
        .map(|(title, rows)| {
            let mut rows: Vec<Row> = rows
                .iter()
                .filter_map(|(what, matches)| {
                    Some(Row {
                        keys: keys_for(config, *matches)?,
                        what,
                    })
                })
                .collect();
            if *title == "Mouse" {
                rows = vec![
                    Row {
                        keys: vec![mod_name.into(), "drag".into()],
                        what: "Move a window",
                    },
                    Row {
                        keys: vec![mod_name.into(), "right drag".into()],
                        what: "Resize it",
                    },
                ];
            }
            Section { title, rows }
        })
        .filter(|section| !section.rows.is_empty())
        .collect()
}

/// The full cheatsheet's columns: its first section, then the others.
fn columns(sections: &[Section]) -> [Vec<Section>; 2] {
    match sections.split_first() {
        Some((first, rest)) => [vec![first.clone()], rest.to_vec()],
        None => [Vec::new(), Vec::new()],
    }
}

/// `path` with the home directory as "~".
fn home_relative(path: &std::path::Path) -> String {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty());
    match home.and_then(|home| path.strip_prefix(home).ok().map(|p| p.to_owned())) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// How much a panel `size` is shrunk to fit `area` with `FIT_MARGIN`
/// around it: 1 where it fits (it's never grown).
fn fit(size: Size<i32, Logical>, area: Size<i32, Logical>) -> f64 {
    let room = |area: i32, size: i32| f64::from(area - 2 * FIT_MARGIN) / f64::from(size.max(1));
    room(area.w, size.w)
        .min(room(area.h, size.h))
        .clamp(0.1, 1.0)
}

/// The panel's size as `heading` with `sections`.
fn panel_size(heading: Heading, sections: &[Section]) -> Size<i32, Logical> {
    match heading {
        Heading::Welcome => {
            let rows: usize = sections.iter().map(|s| s.rows.len()).sum();
            let height = WELCOME_ROWS_AT + rows as i32 * WELCOME_ROW + WELCOME_BOTTOM;
            (WELCOME_WIDTH, height).into()
        }
        Heading::Shortcuts => {
            let [left, right] = columns(sections);
            let height =
                SECTIONS_AT + column_height(&left).max(column_height(&right)) + SHORTCUTS_BOTTOM;
            (SHORTCUTS_WIDTH, height).into()
        }
    }
}

impl Pila {
    /// Show the cheatsheet as `heading`, on the output under the pointer;
    /// never while locked.
    pub fn show_shortcuts(&mut self, heading: Heading) {
        if self.is_locked() {
            return;
        }
        // It's all text: with no fonts it would be an empty panel taking the
        // keys and clicks.
        if crate::text::unavailable() {
            tracing::info!("no fonts: the shortcuts aren't shown");
            return;
        }
        self.redraw_shortcuts_view();
        let output = self
            .space
            .output_under(self.pointer_location())
            .next()
            .map(|output| output.name());
        self.shortcuts_view.shown = Some(Shown { heading, output });
        *self.shortcuts_view.buffers.borrow_mut() = Buffers::default();
        tracing::info!(?heading, "showing the shortcuts");
        self.redraw_shortcuts_view();
        self.draw_shortcuts_text();
    }

    /// Draw the panel's texts again if what they say changed (shown, the
    /// fonts loaded, the config reloaded): on a thread, into one image that
    /// the panel shows once it's back (it's drawn only with its text). The
    /// output's scale is the one now; a panel whose output changes scale
    /// while it's shown has its text scaled.
    pub(crate) fn draw_shortcuts_text(&mut self) {
        let Some(view) = self.scene_shortcuts() else {
            return;
        };
        let scale = self.panel_text_scale(&view.output);
        let colors = &self.config.colors;
        let items = match view.heading {
            Heading::Welcome => {
                let rows: Vec<Row> = view.rows().cloned().collect();
                welcome_items(&rows, &view.notes, view.layout_size.h, colors)
            }
            Heading::Shortcuts => shortcuts_items(&columns(&view.sections), colors),
        };
        let key = TextKey {
            items,
            caps: crate::panel::key_caps(colors),
            size: (view.layout_size.w, view.layout_size.h),
            scale,
        };
        let drawn = view.buffers.borrow().text.as_ref().map(|t| &t.key) == Some(&key);
        self.draw_panel_text(
            key,
            drawn,
            "pila-shortcuts",
            |state| &mut state.shortcuts_view.pending,
            Self::draw_shortcuts_text,
            |state, text| {
                state.shortcuts_view.buffers.borrow_mut().text = Some(text);
                state.redraw_shortcuts_view();
            },
        );
    }

    /// Whether the cheatsheet is shown with no content and none on the way,
    /// and could have it: the fonts are loaded (`after_frames` asks then, so
    /// an output or the fonts coming after it's shown bring it, without
    /// building it anew on every turn of the loop).
    pub(crate) fn shortcuts_need_content(&self) -> bool {
        self.shortcuts_shown()
            && self.shortcuts_view.pending.is_none()
            && self.shortcuts_view.buffers.borrow().text.is_none()
            && crate::text::loaded()
    }

    /// The config was reloaded: the cheatsheet shown takes its keys, colors
    /// and corners as they are now. Its background and border are made anew,
    /// so that a change of shape alone (the corners) is drawn.
    pub(crate) fn shortcuts_config_changed(&mut self) {
        if !self.shortcuts_shown() {
            return;
        }
        self.shortcuts_view.buffers.borrow_mut().frame = FrameBuffers::default();
        self.redraw_shortcuts_view();
        self.draw_shortcuts_text();
    }

    /// Hide the cheatsheet, if it's shown.
    pub fn hide_shortcuts(&mut self) {
        if self.shortcuts_view.shown.take().is_none() {
            return;
        }
        self.shortcuts_view.pending = None;
        *self.shortcuts_view.buffers.borrow_mut() = Buffers::default();
        self.redraw_shortcuts_view();
    }

    /// `show-shortcuts`: show it, or hide it if it's shown.
    pub fn toggle_shortcuts(&mut self) {
        if self.shortcuts_shown() {
            self.hide_shortcuts();
        } else {
            self.show_shortcuts(Heading::Shortcuts);
        }
    }

    pub fn shortcuts_shown(&self) -> bool {
        self.shortcuts_view.shown.is_some()
    }

    /// Redraw every output: the panel dims them all while it's shown.
    fn redraw_shortcuts_view(&mut self) {
        self.queue_redraw();
    }

    /// The panel (where it shows, and its size as laid out), its sections,
    /// notes and output, while it's shown.
    #[allow(clippy::type_complexity)]
    fn shortcuts_layout(
        &self,
    ) -> Option<(
        Rectangle<i32, Logical>,
        Size<i32, Logical>,
        Vec<Section>,
        Vec<String>,
        String,
    )> {
        let shown = self.shortcuts_view.shown.as_ref()?;
        let (output, area) = self.output_named_or_first(shown.output.as_deref())?;
        let sections = match shown.heading {
            Heading::Welcome => vec![Section {
                title: "",
                rows: welcome_rows(&self.config),
            }],
            Heading::Shortcuts => sections(&self.config),
        };
        let mut notes = Vec::new();
        if shown.heading == Heading::Welcome {
            if let Some(path) = &self.config_path {
                notes.push(format!("Settings: {}", home_relative(path)));
            }
        }
        let layout_size = panel_size(shown.heading, &sections);
        let scale = fit(layout_size, area.size);
        let size = Size::from((
            (f64::from(layout_size.w) * scale).round() as i32,
            (f64::from(layout_size.h) * scale).round() as i32,
        ));
        let loc = crate::panel::centered(area, size).loc;
        Some((
            Rectangle::new(loc, size),
            layout_size,
            sections,
            notes,
            output.name(),
        ))
    }

    /// The cheatsheet for `Pila::scene`, while it's shown.
    pub(crate) fn scene_shortcuts(&self) -> Option<SceneShortcuts> {
        let heading = self.shortcuts_view.shown.as_ref()?.heading;
        let (rect, layout_size, sections, notes, output) = self.shortcuts_layout()?;
        // Its corners shrink with it.
        let shrunk = f64::from(rect.size.w) / f64::from(layout_size.w.max(1));
        Some(SceneShortcuts {
            output,
            rect,
            layout_size,
            heading,
            sections,
            notes,
            rounding: crate::panel::rounding(&self.config)
                .map(|(radius, corners)| (radius * shrunk, corners)),
            buffers: self.shortcuts_view.buffers.clone(),
        })
    }

    /// Whether the cheatsheet is shown at `location` (global): nothing under
    /// it gets the pointer.
    pub fn shortcuts_view_covers(&self, location: Point<f64, Logical>) -> bool {
        self.shortcuts_layout()
            .is_some_and(|(rect, ..)| rect.to_f64().contains(location))
    }

    /// For the state dump: whether it's shown, as what, where and its rows.
    pub fn shortcuts_view_json(&self) -> serde_json::Value {
        match self.scene_shortcuts() {
            None => serde_json::json!({ "shown": false }),
            Some(view) => {
                let r = view.rect;
                serde_json::json!({
                    "shown": true,
                    "text": self.shortcuts_view.buffers.borrow().text.is_some(),
                    "heading": view.heading.text(),
                    "output": view.output,
                    "rect": { "x": r.loc.x, "y": r.loc.y, "w": r.size.w, "h": r.size.h },
                    "rows": view.rows().map(|row| serde_json::json!([row.keys.join("+"), row.what])).collect::<Vec<_>>(),
                    "sections": view.sections.iter().map(|s| s.title).collect::<Vec<_>>(),
                    "notes": view.notes,
                })
            }
        }
    }
}

/// The cheatsheet's elements for the output `output_name` at `output_geo`,
/// front to back: the panel on its output, and the dim on every one, once
/// its content is there.
pub fn elements(
    renderer: &mut GlesRenderer,
    view: &SceneShortcuts,
    colors: &Colors,
    output_name: &str,
    output_geo: Rectangle<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement> {
    let origin = output_geo.loc;
    let mut buffers = view.buffers.borrow_mut();
    let buffers = &mut *buffers;
    let mut elements = Vec::new();
    // Drawn only with its text, which comes from a thread
    // (`Pila::draw_shortcuts_text`): never an empty panel.
    let Some(text) = buffers.text.as_mut() else {
        return elements;
    };
    if output_name == view.output {
        // Shown at the panel's size: smaller than drawn where it was shrunk
        // to fit its monitor.
        elements.extend(text.element(renderer, view.rect, origin, scale));
        elements.extend(crate::panel::frame(
            renderer,
            &mut buffers.frame,
            view.rect,
            view.rounding,
            colors,
            origin,
            scale,
        ));
    }
    elements.push(crate::panel::dim(
        &mut buffers.dims,
        output_name,
        output_geo,
        1.0,
        origin,
        scale,
    ));
    elements
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn config(text: &str, mod_key: ModKey) -> Config {
        Config::parse(text, &|_| None, mod_key).unwrap()
    }

    fn label(text: &str, mod_key: ModKey) -> String {
        let config = config(
            &format!("[bindings]\n\"{text}\" = \"show-shortcuts\"\n"),
            mod_key,
        );
        let binding = config
            .bindings
            .iter()
            .rev()
            .find(|b| b.action == Action::ShowShortcuts)
            .unwrap();
        combo_label(&binding.combo)
    }

    #[test]
    fn keys_are_named_as_they_are_pressed() {
        assert_eq!(label("Mod+Shift+q", ModKey::Super), "Super+Shift+Q");
        assert_eq!(label("Mod+Shift+q", ModKey::Alt), "Alt+Shift+Q");
        assert_eq!(
            label("Shift+Ctrl+Mod+slash", ModKey::Super),
            "Super+Ctrl+Shift+/"
        );
        assert_eq!(label("Mod+Return", ModKey::Super), "Super+Return");
        assert_eq!(label("Mod+Left", ModKey::Super), "Super+←");
        assert_eq!(label("Mod+3", ModKey::Super), "Super+3");
        assert_eq!(label("Mod", ModKey::Super), "Super (tap)");
        assert_eq!(label("Mod+WheelScrollDown", ModKey::Alt), "Alt+wheel ↓");
        assert_eq!(label("XF86AudioMute", ModKey::Super), "XF86AudioMute");
    }

    fn find<'a>(rows: impl IntoIterator<Item = &'a Row>, what: &str) -> Option<String> {
        rows.into_iter()
            .find(|row| row.what == what)
            .map(|row| row.keys.join("+"))
    }

    #[test]
    fn the_welcome_has_the_keys_to_get_going() {
        let rows = welcome_rows(&config("", ModKey::Super));
        let whats: Vec<&str> = rows.iter().map(|row| row.what).collect();
        assert_eq!(
            whats,
            [
                "Open a terminal",
                "Open the launcher",
                "Switch windows",
                "Close a window",
                "Quit Pila",
                "All the shortcuts"
            ]
        );
        // The combination to name first: Super+D over a tap of Super,
        // Super+Tab over Alt+Tab and Super+J.
        assert_eq!(find(&rows, "Open the launcher").as_deref(), Some("Super+D"));
        assert_eq!(find(&rows, "Switch windows").as_deref(), Some("Super+Tab"));
        assert_eq!(
            find(&rows, "All the shortcuts").as_deref(),
            Some("Super+Shift+/")
        );
    }

    #[test]
    fn the_defaults_make_the_cheatsheet() {
        let sections = sections(&config("", ModKey::Super));
        let titles: Vec<&str> = sections.iter().map(|s| s.title).collect();
        assert_eq!(titles, ["Windows", "Workspaces", "Mouse", "Pila"]);
        let rows = sections.iter().flat_map(|s| &s.rows);
        let all: Vec<&Row> = rows.collect();
        let find = |what| find(all.iter().copied(), what);
        assert_eq!(find("Terminal").as_deref(), Some("Super+Return"));
        assert_eq!(find("Overview").as_deref(), Some("Super+W"));
        assert_eq!(find("Snap to halves").as_deref(), Some("Super+← → ↑ ↓"));
        assert_eq!(find("Go to one").as_deref(), Some("Super+1…4"));
        assert_eq!(
            find("Move the window there").as_deref(),
            Some("Super+Shift+1…4")
        );
        assert_eq!(find("Move a window").as_deref(), Some("Super+drag"));
        assert_eq!(find("Quit").as_deref(), Some("Super+Shift+E"));
    }

    #[test]
    fn the_cheatsheet_follows_the_bindings() {
        let config = config(
            "[bindings]\n\"Mod+Return\" = \"none\"\n\"Mod+Shift+F12\" = \"quit\"\n\"Mod+Shift+E\" = \"none\"\n",
            ModKey::Alt,
        );
        let rows = welcome_rows(&config);
        assert!(rows.iter().all(|row| row.what != "Open a terminal"));
        assert_eq!(find(&rows, "Quit Pila").as_deref(), Some("Alt+Shift+F12"));
        // Alt as Mod: the mouse's too.
        let sections = sections(&config);
        let mouse = sections.iter().find(|s| s.title == "Mouse").unwrap();
        assert_eq!(mouse.rows[0].keys, ["Alt", "drag"]);
    }

    #[test]
    fn a_run_of_numbers_or_arrows_is_one_key() {
        let caps = |v: &[&[&str]]| -> Vec<Vec<Cap>> {
            v.iter()
                .map(|c| c.iter().map(|s| s.to_string()).collect())
                .collect()
        };
        assert_eq!(
            merged(&caps(&[&["Super", "1"], &["Super", "2"], &["Super", "3"]])),
            Some(vec!["Super".into(), "1…3".into()])
        );
        assert_eq!(
            merged(&caps(&[&["Super", "↓"], &["Super", "←"]])),
            Some(vec!["Super".into(), "← ↓".into()])
        );
        assert_eq!(merged(&caps(&[&["Super", "1"]])), None);
        assert_eq!(merged(&caps(&[&["Super", "1"], &["Super", "3"]])), None);
        assert_eq!(merged(&caps(&[&["Super", "1"], &["Alt", "2"]])), None);
    }

    /// The welcome's footer says where the settings are, at its right, in
    /// the footer's row; no key hints (no panel has them).
    #[test]
    fn the_welcome_footer_says_where_the_settings_are() {
        let config = config("", ModKey::Super);
        let notes = vec!["Settings: ~/.config/pila/config.toml".to_string()];
        let height = 400;
        let welcome = welcome_items(&welcome_rows(&config), &notes, height, &config.colors);
        let footer = welcome.iter().find_map(|item| match item {
            Item::Text {
                text, at, align, ..
            } if *text == notes[0] => Some((at.1, *align)),
            _ => None,
        });
        assert_eq!(
            footer,
            Some((
                height - panel::PADDING - panel::FOOTER_HEIGHT + 1,
                Align::Right
            )),
            "{welcome:?}"
        );
        // Without a settings file to name, nothing there.
        let bare = welcome_items(&welcome_rows(&config), &[], height, &config.colors);
        assert_eq!(bare.len(), welcome.len() - 1);
    }

    #[test]
    fn the_panels_fit_their_rows() {
        let row = |what| Row {
            keys: vec!["Super".into(), "D".into()],
            what,
        };
        let welcome = [Section {
            title: "",
            rows: vec![row("a"), row("b")],
        }];
        assert_eq!(
            panel_size(Heading::Welcome, &welcome),
            (
                WELCOME_WIDTH,
                WELCOME_ROWS_AT + 2 * WELCOME_ROW + WELCOME_BOTTOM
            )
                .into()
        );
        // The taller column: three rows on the left, two sections of one on
        // the right.
        let section = |title, n| Section {
            title,
            rows: (0..n).map(|_| row("x")).collect(),
        };
        let sheet = [section("A", 3), section("B", 1), section("C", 1)];
        let left = SECTION_HEADING + 3 * SECTION_ROW;
        let right = 2 * SECTION_HEADING + SECTION_GAP + 2 * SECTION_ROW;
        assert_eq!(
            panel_size(Heading::Shortcuts, &sheet).h,
            SECTIONS_AT + left.max(right) + SHORTCUTS_BOTTOM
        );
    }

    /// A panel that fits is drawn as it is; one that doesn't shrinks to its
    /// monitor with room around it (the welcome on 640x400 logical: 1280x800
    /// at scale 2), never grown.
    #[test]
    fn a_panel_too_big_for_its_monitor_shrinks() {
        let welcome = Size::from((WELCOME_WIDTH, 398));
        assert_eq!(fit(welcome, (1280, 800).into()), 1.0);
        let shrunk = fit(welcome, (640, 400).into());
        assert!(shrunk < 1.0);
        assert!(f64::from(welcome.h) * shrunk <= f64::from(400 - 2 * FIT_MARGIN) + 0.5);
        let sheet = Size::from((SHORTCUTS_WIDTH, 374));
        assert!(f64::from(sheet.w) * fit(sheet, (640, 400).into()) <= 576.5);
    }
}
