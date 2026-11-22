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
    backend::{
        allocator::Fourcc,
        renderer::{
            element::{
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                solid::{SolidColorBuffer, SolidColorRenderElement},
                Kind,
            },
            gles::GlesRenderer,
        },
    },
    input::keyboard::{xkb, Keysym},
    reexports::calloop,
    utils::{Logical, Physical, Point, Rectangle, Scale, Size, Transform},
};

use crate::{
    config::{Action, Colors, Config, KeyCombo, ModKey, Trigger, WorkspaceTarget},
    render::PilaRenderElement,
    Pila,
};

/// The panel's border.
const BORDER: i32 = 1;
/// The room kept around a panel shrunk to fit its monitor (`fit`).
const FIT_MARGIN: i32 = 32;
/// A keycap's height, its rounding and its padding around the label.
const CAP_HEIGHT: i32 = 22;
const CAP_RADIUS: i32 = 4;
const CAP_PADDING: i32 = 7;
/// Between keycaps, where a "+" goes.
const CAP_GAP: i32 = 18;

/// The welcome: its width, where its rows start, each row's height, where
/// the keys and what they do go, and the room under the rows.
const WELCOME_WIDTH: i32 = 560;
const WELCOME_ROWS_AT: i32 = 120;
const WELCOME_ROW: i32 = 38;
const WELCOME_KEYS_AT: i32 = 90;
const WELCOME_WHAT_AT: i32 = 300;
const WELCOME_BOTTOM: i32 = 50;

/// The full cheatsheet: its width, its columns, where its sections start,
/// a section's heading and row heights, where what a row does goes (from
/// its column) and the room under the columns.
const SHORTCUTS_WIDTH: i32 = 860;
const COLUMNS_AT: [i32; 2] = [28, 448];
const SECTIONS_AT: i32 = 72;
const SECTION_HEADING: i32 = 22;
const SECTION_ROW: i32 = 32;
const SECTION_GAP: i32 = 14;
const SECTION_WHAT_AT: i32 = 200;
const SHORTCUTS_BOTTOM: i32 = 16;

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
    /// Where the panel was last drawn, redrawn when it goes.
    drawn: Option<Rectangle<i32, Logical>>,
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
    background: Option<SolidColorBuffer>,
    border: Option<SolidColorBuffer>,
    /// Everything on the panel, its text and keycaps, in one image the size
    /// of the panel: one texture, made on a thread
    /// (`Pila::draw_shortcuts_text`).
    text: Option<TextImage>,
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

/// How a text sits in its box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Align {
    Left,
    Center,
    Right,
}

/// Something drawn on the panel, at a place from its top-left (logical).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Item {
    /// A line of text in a box `w` by `h`, at `size` px, bold or not.
    Text {
        text: String,
        at: (i32, i32),
        w: i32,
        h: i32,
        size: u8,
        bold: bool,
        align: Align,
        color: [u32; 3],
    },
    /// Keycaps from left to right, a "+" between them, within `w`.
    Keys {
        caps: Vec<Cap>,
        at: (i32, i32),
        w: i32,
        size: u8,
    },
}

/// What a content image shows: its items in the panel's palette, the
/// panel's size and the buffer scale it's drawn at.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TextKey {
    pub(crate) items: Vec<Item>,
    /// A keycap's label, its fill, its edge and the "+" between them.
    pub(crate) caps: [[u32; 3]; 4],
    pub(crate) size: (i32, i32),
    pub(crate) scale: i32,
}

/// The panel's content drawn into one premultiplied ARGB8888 image.
struct TextImage {
    key: TextKey,
    pixels: Vec<u8>,
    /// Made from `pixels` by the first frame that shows it.
    buffer: Option<MemoryRenderBuffer>,
}

/// An opaque color as the content image's key keeps it.
pub(crate) fn bits(color: [f32; 3]) -> [u32; 3] {
    color.map(f32::to_bits)
}

/// The panel's palette: text, dimmed text and the accent.
fn text_colors(colors: &Colors) -> ([u32; 3], [u32; 3], [u32; 3]) {
    let [r, g, b, _] = crate::switcher_view::opaque(colors.snap_preview);
    (
        bits(colors.icon),
        bits(colors.icon_inactive),
        bits([r, g, b]),
    )
}

/// The welcome's items: the title, a line under it, its rows centered, and
/// `notes` at the bottom, for a panel `height` high.
fn welcome_items(rows: &[Row], notes: &[String], height: i32, colors: &Colors) -> Vec<Item> {
    let (text, dim, _) = text_colors(colors);
    let center = |text: String, y, h, size, bold, color| Item::Text {
        text,
        at: (0, y),
        w: WELCOME_WIDTH,
        h,
        size,
        bold,
        align: Align::Center,
        color,
    };
    let mut items = vec![
        center(Heading::Welcome.text().into(), 34, 40, 26, true, text),
        center("A few keys to get going".into(), 74, 20, 14, false, dim),
    ];
    for (i, row) in rows.iter().enumerate() {
        let y = WELCOME_ROWS_AT + i as i32 * WELCOME_ROW;
        items.push(Item::Keys {
            caps: row.keys.clone(),
            at: (WELCOME_KEYS_AT, y),
            w: WELCOME_WHAT_AT - WELCOME_KEYS_AT - 10,
            size: 14,
        });
        items.push(Item::Text {
            text: row.what.into(),
            at: (WELCOME_WHAT_AT, y),
            w: WELCOME_WIDTH - WELCOME_WHAT_AT - 20,
            h: CAP_HEIGHT,
            size: 15,
            bold: false,
            align: Align::Left,
            color: text,
        });
    }
    // With room at the sides: a long settings path ends in "…" before them.
    items.push(Item::Text {
        text: notes.join("  ·  "),
        at: (20, height - 36),
        w: WELCOME_WIDTH - 40,
        h: 20,
        size: 12,
        bold: false,
        align: Align::Center,
        color: dim,
    });
    items
}

/// The full cheatsheet's items: the title and `notes` across the top, then
/// its sections in two columns.
fn shortcuts_items(columns: &[Vec<Section>; 2], notes: &[String], colors: &Colors) -> Vec<Item> {
    let (text, dim, accent) = text_colors(colors);
    let x = COLUMNS_AT[0];
    let mut items = vec![
        Item::Text {
            text: Heading::Shortcuts.text().into(),
            at: (x, 20),
            w: 300,
            h: 34,
            size: 22,
            bold: true,
            align: Align::Left,
            color: text,
        },
        Item::Text {
            text: notes.join("  ·  "),
            at: (SHORTCUTS_WIDTH / 2, 27),
            w: SHORTCUTS_WIDTH / 2 - x,
            h: 20,
            size: 12,
            bold: false,
            align: Align::Right,
            color: dim,
        },
    ];
    for (column, sections) in COLUMNS_AT.iter().zip(columns) {
        let mut y = SECTIONS_AT;
        for section in sections {
            items.push(Item::Text {
                text: section.title.to_uppercase(),
                at: (*column, y),
                w: 380,
                h: 16,
                size: 11,
                bold: true,
                align: Align::Left,
                color: accent,
            });
            y += SECTION_HEADING;
            for row in &section.rows {
                items.push(Item::Keys {
                    caps: row.keys.clone(),
                    at: (*column, y),
                    w: SECTION_WHAT_AT - 10,
                    size: 13,
                });
                items.push(Item::Text {
                    text: row.what.into(),
                    at: (column + SECTION_WHAT_AT, y),
                    w: 380 - SECTION_WHAT_AT,
                    h: CAP_HEIGHT,
                    size: 14,
                    bold: false,
                    align: Align::Left,
                    color: dim,
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

/// Draw `src` (premultiplied, its alpha `alpha`) over the pixel at `i` of
/// premultiplied BGRA `pixels`.
fn blend(pixels: &mut [u8], i: usize, bgr: [f32; 3], alpha: f32) {
    if alpha <= 0.0 {
        return;
    }
    let keep = 1.0 - alpha;
    for (c, value) in bgr.iter().enumerate() {
        let dst = f32::from(pixels[i + c]) / 255.0;
        pixels[i + c] = ((value * alpha + dst * keep) * 255.0).round() as u8;
    }
    let dst = f32::from(pixels[i + 3]) / 255.0;
    pixels[i + 3] = ((alpha + dst * keep) * 255.0).round() as u8;
}

/// A rounded rect (physical pixels) filled with `color`, anti-aliased, over
/// the image `width` wide.
fn fill_rounded(
    pixels: &mut [u8],
    width: i32,
    rect: (f32, f32, f32, f32),
    radius: f32,
    color: [f32; 3],
) {
    let (x, y, w, h) = rect;
    let height = pixels.len() as i32 / 4 / width.max(1);
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    let bgr = [color[2], color[1], color[0]];
    for py in (y.floor() as i32).max(0)..((y + h).ceil() as i32).min(height) {
        for px in (x.floor() as i32).max(0)..((x + w).ceil() as i32).min(width) {
            // Distance from the pixel's center to the rounded rect.
            let qx = (px as f32 + 0.5 - cx).abs() - w / 2.0 + radius;
            let qy = (py as f32 + 0.5 - cy).abs() - h / 2.0 + radius;
            let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
            let d = outside + qx.max(qy).min(0.0) - radius;
            let coverage = (0.5 - d).clamp(0.0, 1.0);
            blend(pixels, ((py * width + px) * 4) as usize, bgr, coverage);
        }
    }
}

/// A rendered line of text over the image at (`x`, `y`) (physical pixels).
fn blit_text(pixels: &mut [u8], width: i32, text: &crate::text::Title, x: i32, y: i32) {
    let height = pixels.len() as i32 / 4 / width.max(1);
    let rows = text.pixels.len() as i32 / 4 / text.width.max(1);
    for row in 0..rows {
        let py = y + row;
        if py < 0 || py >= height {
            continue;
        }
        for col in 0..text.width {
            let px = x + col;
            if px < 0 || px >= width {
                continue;
            }
            let from = ((row * text.width + col) * 4) as usize;
            let a = f32::from(text.pixels[from + 3]) / 255.0;
            if a <= 0.0 {
                continue;
            }
            // Premultiplied already: un-premultiply to blend.
            let bgr = [0, 1, 2].map(|c| f32::from(text.pixels[from + c]) / 255.0 / a);
            blend(pixels, ((py * width + px) * 4) as usize, bgr, a);
        }
    }
}

/// `key`'s items drawn into one image (`None` until the fonts have loaded).
/// On a thread: shaping and rasterizing them all takes a few milliseconds.
pub(crate) fn draw_text(key: &TextKey) -> Option<Vec<u8>> {
    use crate::text::{render_text, Style};
    let scale = key.scale;
    let s = scale as f32;
    let (width, height) = (key.size.0 * scale, key.size.1 * scale);
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    let [label, fill, edge, plus] = key.caps.map(|c| c.map(f32::from_bits));
    for item in &key.items {
        match item {
            Item::Text {
                text,
                at,
                w,
                h,
                size,
                bold,
                align,
                color,
            } => {
                let style = Style {
                    size: f32::from(*size),
                    bold: *bold,
                };
                let line = render_text(text, color.map(f32::from_bits), *w, *h, scale, style)?;
                let room = w * scale - line.width;
                let x = at.0 * scale
                    + match align {
                        Align::Left => 0,
                        Align::Center => room / 2,
                        Align::Right => room,
                    };
                blit_text(&mut pixels, width, &line, x, at.1 * scale);
            }
            Item::Keys { caps, at, w, size } => {
                let style = Style {
                    size: f32::from(*size),
                    bold: false,
                };
                let end = (at.0 + w) * scale;
                let mut x = at.0 * scale;
                let y = at.1 * scale;
                for (i, cap) in caps.iter().enumerate() {
                    if i > 0 {
                        let sign = render_text("+", plus, CAP_GAP, CAP_HEIGHT, scale, style)?;
                        let room = CAP_GAP * scale - sign.width;
                        blit_text(&mut pixels, width, &sign, x + room / 2, y);
                        x += CAP_GAP * scale;
                    }
                    let text = render_text(cap, label, 200, CAP_HEIGHT, scale, style)?;
                    let cap_w = (text.width + 2 * CAP_PADDING * scale).max(CAP_HEIGHT * scale);
                    // Keys that don't fit their room stop at its edge.
                    if x + cap_w > end {
                        break;
                    }
                    let rect = (
                        x as f32,
                        y as f32,
                        cap_w as f32,
                        (CAP_HEIGHT * scale) as f32,
                    );
                    let radius = CAP_RADIUS as f32 * s;
                    fill_rounded(&mut pixels, width, rect, radius, edge);
                    let inner = (rect.0 + s, rect.1 + s, rect.2 - 2.0 * s, rect.3 - 2.0 * s);
                    fill_rounded(&mut pixels, width, inner, radius - s, fill);
                    blit_text(&mut pixels, width, &text, x + (cap_w - text.width) / 2, y);
                    x += cap_w;
                }
            }
        }
    }
    Some(pixels)
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
    /// The lines that go with them: where the settings are, how to close it.
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
    ("Quit Pila", |a| *a == Action::Quit),
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
            ("Quit", |a| *a == Action::Quit),
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
        let scale = self
            .space
            .outputs()
            .find(|o| o.name() == view.output)
            .map(|o| crate::scale::buffer_scale(Scale::from(o.current_scale().fractional_scale())))
            .unwrap_or(1);
        let colors = &self.config.colors;
        let items = match view.heading {
            Heading::Welcome => {
                let rows: Vec<Row> = view.rows().cloned().collect();
                welcome_items(&rows, &view.notes, view.layout_size.h, colors)
            }
            Heading::Shortcuts => shortcuts_items(&columns(&view.sections), &view.notes, colors),
        };
        let key = TextKey {
            items,
            caps: [
                bits(colors.icon),
                {
                    let [r, g, b, _] = crate::switcher_view::opaque(colors.bar);
                    bits([r, g, b])
                },
                bits(colors.hover),
                bits(colors.icon_inactive),
            ],
            size: (view.layout_size.w, view.layout_size.h),
            scale,
        };
        let drawn = view.buffers.borrow().text.as_ref().map(|t| &t.key) == Some(&key);
        if drawn || self.shortcuts_view.pending.as_ref() == Some(&key) {
            return;
        }
        self.shortcuts_view.pending = Some(key.clone());
        let (sender, channel) = calloop::channel::channel();
        let job = key.clone();
        let spawned = std::thread::Builder::new()
            .name("pila-shortcuts".into())
            .spawn(move || {
                let _ = sender.send(draw_text(&job));
            });
        if let Err(err) = spawned {
            tracing::warn!(?err, "can't draw the shortcuts' text");
            self.shortcuts_view.pending = None;
            return;
        }
        // One message, then the channel closes and its source goes.
        let inserted = self
            .loop_handle
            .insert_source(channel, move |event, _, state| {
                let calloop::channel::Event::Msg(pixels) = event else {
                    return;
                };
                if state.shortcuts_view.pending.as_ref() != Some(&key) {
                    return;
                }
                let Some(pixels) = pixels else {
                    // No fonts yet: their arrival asks again
                    // (`shortcuts_need_content`). With them, it failed: it
                    // stays pending, not asked again on every turn of the
                    // loop, until what it says changes.
                    if crate::text::loaded() {
                        tracing::warn!("can't draw the shortcuts' text");
                    } else {
                        state.shortcuts_view.pending = None;
                    }
                    return;
                };
                state.shortcuts_view.pending = None;
                state.shortcuts_view.buffers.borrow_mut().text = Some(TextImage {
                    key: key.clone(),
                    pixels,
                    buffer: None,
                });
                state.redraw_shortcuts_view();
            });
        if let Err(err) = inserted {
            tracing::warn!(?err, "can't wait for the shortcuts' text");
            self.shortcuts_view.pending = None;
        }
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
        {
            let mut buffers = self.shortcuts_view.buffers.borrow_mut();
            buffers.background = None;
            buffers.border = None;
        }
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

    /// Redraw where the panel was and where it is now.
    fn redraw_shortcuts_view(&mut self) {
        if let Some(rect) = self.shortcuts_view.drawn.take() {
            self.queue_redraw_rect(rect);
        }
        if let Some((rect, ..)) = self.shortcuts_layout() {
            self.shortcuts_view.drawn = Some(rect);
            self.queue_redraw_rect(rect);
        }
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
        let output = shown
            .output
            .as_ref()
            .and_then(|name| self.space.outputs().find(|o| &o.name() == name))
            .or_else(|| self.space.outputs().next())?
            .clone();
        let area = self.space.output_geometry(&output)?;
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
        notes.push("Any key or click closes this".into());
        let layout_size = panel_size(shown.heading, &sections);
        let scale = fit(layout_size, area.size);
        let size = Size::from((
            (f64::from(layout_size.w) * scale).round() as i32,
            (f64::from(layout_size.h) * scale).round() as i32,
        ));
        let loc = area.loc + Point::from(((area.size.w - size.w) / 2, (area.size.h - size.h) / 2));
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
            rounding: (self.config.corner_radius > 0).then_some((
                f64::from(self.config.corner_radius) * shrunk,
                self.config.rounded_corners,
            )),
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

/// The cheatsheet's elements, front to back, relative to `origin`.
pub fn elements(
    renderer: &mut GlesRenderer,
    view: &SceneShortcuts,
    colors: &Colors,
    origin: Point<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement> {
    let mut buffers = view.buffers.borrow_mut();
    let buffers = &mut *buffers;
    let physical = |loc: Point<i32, Logical>| -> Point<i32, Physical> {
        (loc - origin).to_physical_precise_round(scale)
    };
    let mut elements = Vec::new();
    // Drawn only with its text, which comes from a thread
    // (`Pila::draw_shortcuts_text`): never an empty panel.
    let Some(text) = buffers.text.as_mut() else {
        return elements;
    };
    let (width, height) = text.key.size;
    let text_scale = text.key.scale;
    let pixels = &text.pixels;
    let buffer = text.buffer.get_or_insert_with(|| {
        MemoryRenderBuffer::from_slice(
            pixels,
            Fourcc::Argb8888,
            (width * text_scale, height * text_scale),
            text_scale,
            Transform::Normal,
            None,
        )
    });
    // Shown at the panel's size: smaller than drawn where it was shrunk to
    // fit its monitor.
    let loc = (view.rect.loc - origin).to_f64().to_physical(scale);
    if let Ok(element) = MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        loc,
        buffer,
        None,
        // All of it, at its size as laid out (without a source, Smithay
        // takes `size` for it and crops).
        Some(Rectangle::from_size(view.layout_size.to_f64())),
        Some(view.rect.size),
        Kind::Unspecified,
    ) {
        elements.push(PilaRenderElement::Memory(element));
    }
    // The windows' corners, if their shaders compiled; else square.
    let shaders = view
        .rounding
        .and_then(|_| crate::rounded::Shaders::get(renderer));
    let mut solid = |slot: &mut Option<SolidColorBuffer>,
                     rect: Rectangle<i32, Logical>,
                     color: [f32; 4],
                     rounding: Option<(f64, crate::rounded::Corners)>| {
        let buffer = slot.get_or_insert_with(|| SolidColorBuffer::new(rect.size, color));
        buffer.update(rect.size, color);
        let element = SolidColorRenderElement::from_buffer(
            buffer,
            physical(rect.loc),
            scale,
            1.0,
            Kind::Unspecified,
        );
        elements.push(match (rounding, &shaders) {
            (Some((radius, corners)), Some(shaders)) => PilaRenderElement::RoundedSolid(
                crate::rounded::RoundedSolid::new(element, shaders, radius, corners, scale),
            ),
            _ => PilaRenderElement::Solid(element),
        });
    };
    solid(
        &mut buffers.background,
        view.rect,
        crate::switcher_view::opaque(colors.bar_inactive),
        view.rounding,
    );
    // One pixel around it, its corners a pixel wider to follow the curve.
    let outer = Rectangle::new(
        view.rect.loc - Point::from((BORDER, BORDER)),
        view.rect.size + Size::from((2 * BORDER, 2 * BORDER)),
    );
    let [r, g, b] = colors.hover;
    solid(
        &mut buffers.border,
        outer,
        [r, g, b, 1.0],
        view.rounding
            .map(|(radius, corners)| (radius + f64::from(BORDER), corners)),
    );
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

    /// A keycap is drawn over what's under it, its label over the cap.
    #[test]
    fn text_blends_over_what_is_under_it() {
        let mut pixels = vec![0u8; 4 * 4];
        fill_rounded(&mut pixels, 2, (0.0, 0.0, 2.0, 2.0), 0.0, [1.0, 0.0, 0.0]);
        // Opaque red, as BGRA.
        assert_eq!(&pixels[..4], &[0, 0, 255, 255]);
        let half_green = crate::text::Title {
            pixels: vec![0, 128, 0, 128],
            width: 1,
            whole: true,
        };
        blit_text(&mut pixels, 2, &half_green, 1, 1);
        let i = (2 + 1) * 4;
        assert_eq!(&pixels[i..i + 4], &[0, 128, 127, 255]);
    }
}
