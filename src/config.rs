//! Configuration: `~/.config/pila/config.toml` (or `$XDG_CONFIG_HOME/pila`,
//! or `--config PATH`), then `PILA_*` environment overrides. See
//! `config.example.toml` for every option; it's `EXAMPLE`, which `main`
//! writes to the default path when nothing is there (`create_default_file`).
//!
//! The defaults are `EXAMPLE`'s active values, and nowhere else: a setting
//! the user's file leaves out is taken from it (`Config::parse_with`), and
//! one neither sets is an error (`need`), not a value written here. Only what
//! depends on the machine (`mod`, `terminal`, the keymap) is worked out here
//! when unset. The default bindings and gestures are `EXAMPLE`'s `[bindings]`
//! and `[gestures]`, which come before the user's (Mod is Super on a TTY and
//! Alt nested, unless configured; see `mod`/`PILA_MOD`), told to users in
//! `docs/MANUAL.md` and the man page, which tests check list every one; the
//! actions are `NAMED_ACTIONS` and `ACTIONS_WITH_ARGUMENT`, which
//! `config.example.toml` lists (checked too).
//! The mouse bindings: Mod+left drag moves a window, Mod+right drag resizes it
//! from the nearest corner. A binding's key may also be a mouse wheel's notch
//! (`WheelScrollDown`, ...: `Trigger::Wheel`, run by `input.rs`).
//!
//! `[bindings]` in the config adds to or replaces the default bindings
//! (`"Mod+Shift+Return" = "spawn kitty -e htop"`, `"Mod+J" = "none"`); the
//! modifiers of a binding must match the pressed ones exactly. The VT switch
//! comes from the keymap (`XF86_Switch_VT_N` keysyms), so it's handled in the
//! keyboard filter and can't be rebound; the mouse bindings aren't
//! configurable yet.
//!
//! A bare modifier (`"Mod"`, `"Super"`, `"Alt"`, `"Ctrl"`) is a tap: it runs
//! when that key is released with no other key, button, scroll, gesture or
//! touch since its press (`input.rs`), never on the press (`action_for`
//! skips modifier keys; `tap_action` looks taps up).
//!
//! Nested, Alt is the default because a host desktop such as GNOME grabs
//! Super while pila runs inside it.

use crate::edid::Monitor;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use smithay::{
    input::keyboard::{xkb, Keysym, ModifiersState},
    utils::Transform,
};
use tracing::{info, warn};

use crate::snap::{Direction, Side, KEYBOARD_STEP};

/// The largest `corner-radius` (logical pixels). A window's corners are
/// further limited by its size and the title bar's height (`rounded.rs`).
pub const MAX_CORNER_RADIUS: u32 = 64;

/// The largest `gaps` (logical pixels).
pub const MAX_GAPS: u32 = 128;

/// The most workspaces `workspaces` may ask for.
pub const MAX_WORKSPACES: usize = 32;

/// Which way a swipe went (the way the fingers moved).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwipeDirection {
    Left,
    Right,
    Up,
    Down,
}

/// A touchpad swipe pila handles itself, e.g. `swipe-3-left`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gesture {
    pub fingers: u32,
    pub direction: SwipeDirection,
}

impl Gesture {
    fn parse(text: &str) -> Result<Self, String> {
        let bad = || {
            format!(
                "unknown gesture {text:?} (swipe-<fingers>-<left|right|up|down>, 3 to 5 fingers)"
            )
        };
        let rest = text.trim().strip_prefix("swipe-").ok_or_else(bad)?;
        let (fingers, direction) = rest.split_once('-').ok_or_else(bad)?;
        let fingers: u32 = fingers.parse().map_err(|_| bad())?;
        // Two-finger swipes are scrolling; libinput doesn't report them.
        if !(3..=5).contains(&fingers) {
            return Err(bad());
        }
        let direction = match direction {
            "left" => SwipeDirection::Left,
            "right" => SwipeDirection::Right,
            "up" => SwipeDirection::Up,
            "down" => SwipeDirection::Down,
            _ => return Err(bad()),
        };
        Ok(Self { fingers, direction })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModKey {
    Alt,
    Super,
    Ctrl,
}

impl ModKey {
    pub fn is_pressed(self, mods: &ModifiersState) -> bool {
        match self {
            ModKey::Alt => mods.alt,
            ModKey::Super => mods.logo,
            ModKey::Ctrl => mods.ctrl,
        }
    }

    fn parse(name: &str) -> Result<Self, String> {
        match name.to_ascii_lowercase().as_str() {
            "alt" => Ok(ModKey::Alt),
            "super" | "logo" | "win" => Ok(ModKey::Super),
            "ctrl" | "control" => Ok(ModKey::Ctrl),
            other => Err(format!("unknown modifier {other:?} (alt, super or ctrl)")),
        }
    }

    /// The keysym a tap of this modifier is bound by (`tap_sym`).
    fn tap_sym(self) -> Keysym {
        match self {
            ModKey::Alt => Keysym::Alt_L,
            ModKey::Super => Keysym::Super_L,
            ModKey::Ctrl => Keysym::Control_L,
        }
    }
}

/// The modifiers of a binding. Caps Lock and Num Lock are ignored.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub logo: bool,
}

impl Mods {
    fn add(&mut self, key: ModKey) {
        match key {
            ModKey::Alt => self.alt = true,
            ModKey::Super => self.logo = true,
            ModKey::Ctrl => self.ctrl = true,
        }
    }
}

impl From<&ModifiersState> for Mods {
    fn from(m: &ModifiersState) -> Self {
        Mods {
            ctrl: m.ctrl,
            alt: m.alt,
            shift: m.shift,
            logo: m.logo,
        }
    }
}

/// The keysym a tap of the modifier key `sym` is bound by (both sides are
/// the same tap), or `None` if `sym` isn't Alt, Super or Ctrl.
pub fn tap_sym(sym: Keysym) -> Option<Keysym> {
    let key = match sym {
        Keysym::Alt_L | Keysym::Alt_R | Keysym::Meta_L | Keysym::Meta_R => ModKey::Alt,
        Keysym::Super_L | Keysym::Super_R => ModKey::Super,
        Keysym::Control_L | Keysym::Control_R => ModKey::Ctrl,
        _ => return None,
    };
    Some(key.tap_sym())
}

/// A key binding of `[bindings]` (or a default one).
#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
    pub combo: KeyCombo,
    pub action: Action,
    /// It runs again while its key is held (`repeat = true`), at the
    /// keyboard's repeat delay and rate.
    pub repeat: bool,
}

/// A key combination, e.g. `Mod+Shift+Q`, a modifier tapped alone (`Mod`:
/// its own modifier and `tap_sym`), or modifiers with a mouse wheel step
/// (`Mod+WheelScrollDown`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyCombo {
    pub mods: Mods,
    pub trigger: Trigger,
}

/// What completes a binding's modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// The unshifted (latin) keysym, lowercase for letters.
    Key(Keysym),
    /// A step of a mouse wheel (`input.rs::wheel_bindings`).
    Wheel(WheelDirection),
}

/// Which way a wheel step goes, as bindings name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelDirection {
    Up,
    Down,
    Left,
    Right,
}

impl WheelDirection {
    fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "wheelscrollup" => Some(Self::Up),
            "wheelscrolldown" => Some(Self::Down),
            "wheelscrollleft" => Some(Self::Left),
            "wheelscrollright" => Some(Self::Right),
            _ => None,
        }
    }
}

/// The keysym a binding's or an app shortcut's key `name` stands for
/// (`NoSymbol` for none): case-insensitive, which gives the lowercase keysym
/// for letters, as the unshifted sym the keyboard filter compares against;
/// a key by its code (`0x41`, XK_A) the same way, through its name. A name
/// with a NUL in it is none: xkbcommon's lookup panics on it, and a config
/// or `app-shortcuts.json` may have one (`"\u0000"`).
pub fn key_by_name(name: &str) -> Keysym {
    if name.contains('\0') {
        return Keysym::NoSymbol;
    }
    let sym = xkb::keysym_from_name(name, xkb::KEYSYM_CASE_INSENSITIVE);
    if sym.raw() == 0 {
        return sym;
    }
    xkb::keysym_from_name(&xkb::keysym_get_name(sym), xkb::KEYSYM_CASE_INSENSITIVE)
}

impl KeyCombo {
    fn parse(text: &str, mod_key: ModKey) -> Result<Self, String> {
        let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let key = parts.pop().filter(|k| !k.is_empty());
        let key = key.ok_or_else(|| format!("{text:?} has no key"))?;
        // A modifier alone is a tap of it (`tap_sym`).
        if parts.is_empty() {
            let modifier = match key.to_ascii_lowercase().as_str() {
                "mod" => Some(mod_key),
                name => ModKey::parse(name).ok(),
            };
            if let Some(modifier) = modifier {
                let mut mods = Mods::default();
                mods.add(modifier);
                return Ok(KeyCombo {
                    mods,
                    trigger: Trigger::Key(modifier.tap_sym()),
                });
            }
        }
        let mut mods = Mods::default();
        for part in parts {
            match part.to_ascii_lowercase().as_str() {
                "mod" => mods.add(mod_key),
                "shift" => mods.shift = true,
                name => match ModKey::parse(name) {
                    Ok(key) => mods.add(key),
                    Err(_) => return Err(format!("unknown modifier {part:?} in {text:?}")),
                },
            }
        }
        if let Some(direction) = WheelDirection::parse(key) {
            return Ok(KeyCombo {
                mods,
                trigger: Trigger::Wheel(direction),
            });
        }
        let sym = key_by_name(key);
        if sym.raw() == 0 {
            return Err(format!("unknown key {key:?} in {text:?}"));
        }
        Ok(KeyCombo {
            mods,
            trigger: Trigger::Key(sym),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Spawn(String),
    /// Spawn the configured terminal / launcher (resolved when run, so a
    /// changed `terminal` keeps working with the default binding).
    Terminal,
    Launcher,
    CloseFocused,
    /// Mod+Tab: by recent use (`switcher.rs`); `CycleFocusBack` the other way.
    CycleFocus,
    CycleFocusBack,
    Minimize,
    /// Snap to a half or a quarter of the output (`snap.rs`).
    Snap(Side),
    /// One step on the grid of halves and quarters (`snap::step`).
    SnapToward(Direction),
    /// Undo a snap or a maximize.
    Restore,
    /// Move by (x, y) / resize by (w, h) logical pixels.
    MoveWindow(i32, i32),
    ResizeWindow(i32, i32),
    Center,
    ToggleMaximize,
    ToggleFullscreen,
    Quit,
    /// Ask first, in the quit dialog (`quit_dialog.rs`).
    ConfirmQuit,
    /// Go to a workspace, or move the focused window there (`workspaces.rs`).
    Workspace(WorkspaceTarget),
    MoveToWorkspace(WorkspaceTarget),
    /// Keep the focused window above the others, or on all workspaces
    /// (`stacking.rs`).
    ToggleAlwaysOnTop,
    ToggleAllWorkspaces,
    /// Minimize the workspace's windows, or bring them back (`minimize.rs`).
    ShowDesktop,
    /// Show Pila's shortcuts cheatsheet, or hide it (`shortcuts_view.rs`).
    ShowShortcuts,
    /// Open or close the overview (`overview.rs`).
    Overview(OverviewAction),
    /// Go to another output, or send the focused window there (`stacking.rs`).
    FocusOutput(OutputTarget),
    MoveToOutput(OutputTarget),
    /// Switch the keyboard layout (`keyboard_layout.rs`).
    SwitchLayout(LayoutTarget),
    /// Turn every monitor's display off until any input, or back on
    /// (`output_power.rs`).
    PowerOffMonitors,
    PowerOnMonitors,
    /// Switch to virtual terminal N (Ctrl+Alt+F<N> on a TTY; not configurable).
    ChangeVt(i32),
}

/// What `overview` does: open it if it's closed and close it if it's open,
/// or only one of them (`overview open`, `overview close`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverviewAction {
    Toggle,
    Open,
    Close,
}

/// Which output an output action goes to: by position, around the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputTarget {
    Next,
    Prev,
}

/// Which keyboard layout `switch-layout` goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutTarget {
    /// Counted from 0 (the config counts from 1).
    Index(usize),
    /// The next / previous one, wrapping around.
    Next,
    Prev,
}

/// Which workspace a workspace action goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceTarget {
    /// Counted from 0 (the config counts from 1).
    Index(usize),
    /// The next / previous one, without wrapping around.
    Next,
    Prev,
}

impl WorkspaceTarget {
    /// `1`..`MAX_WORKSPACES`, `next` or `prev` (after `workspace`).
    fn parse(text: &str) -> Result<Self, String> {
        match text.trim() {
            "next" => Ok(Self::Next),
            "prev" => Ok(Self::Prev),
            number => match number.parse::<usize>() {
                Ok(n @ 1..=MAX_WORKSPACES) => Ok(Self::Index(n - 1)),
                _ => Err(format!(
                    "unknown workspace {number:?} (1 to {MAX_WORKSPACES}, next or prev)"
                )),
            },
        }
    }
}

impl Action {
    /// Whether `pila msg action` may run it with the session locked: what
    /// gets past nothing. Idling locks and then turns the monitors off, and
    /// a lock screen may switch the keyboard layout to type a password.
    pub(crate) fn runs_while_locked(&self) -> bool {
        matches!(
            self,
            Action::PowerOffMonitors | Action::PowerOnMonitors | Action::SwitchLayout(_)
        )
    }

    /// `quit` or `confirm-quit`: neither a client inhibiting shortcuts
    /// nor (in debug builds, to escape a nested session) the lock stops it.
    pub(crate) fn quits(&self) -> bool {
        matches!(self, Action::Quit | Action::ConfirmQuit)
    }

    /// `None` for `"none"`, which removes a binding.
    pub(crate) fn parse(text: &str) -> Result<Option<Self>, String> {
        let text = text.trim();
        if let Some(command) = text.strip_prefix("spawn ") {
            return Ok(Some(Action::Spawn(command.trim().to_string())));
        }
        // "workspace 2", "workspace-next", "move-to-workspace-prev", ...
        for (prefix, make) in [
            ("move-to-workspace", Action::MoveToWorkspace as fn(_) -> _),
            ("workspace", Action::Workspace),
        ] {
            if let Some(target) = text
                .strip_prefix(prefix)
                .and_then(|rest| rest.strip_prefix(' ').or_else(|| rest.strip_prefix('-')))
            {
                return WorkspaceTarget::parse(target).map(|t| Some(make(t)));
            }
        }
        // "switch-layout next", "switch-layout 2".
        if let Some(target) = text.strip_prefix("switch-layout ") {
            return match target.trim() {
                "next" => Ok(Some(Action::SwitchLayout(LayoutTarget::Next))),
                "prev" => Ok(Some(Action::SwitchLayout(LayoutTarget::Prev))),
                n => match n.parse::<usize>() {
                    Ok(n) if n >= 1 => Ok(Some(Action::SwitchLayout(LayoutTarget::Index(n - 1)))),
                    _ => Err(format!(
                        "switch-layout {n:?}: must be next, prev or a number from 1"
                    )),
                },
            };
        }
        // "snap left", ... ("snap-left" is a named action).
        if let Some(dir) = text.strip_prefix("snap ") {
            return match dir.trim() {
                "left" => Ok(Some(Action::SnapToward(Direction::Left))),
                "right" => Ok(Some(Action::SnapToward(Direction::Right))),
                "up" => Ok(Some(Action::SnapToward(Direction::Up))),
                "down" => Ok(Some(Action::SnapToward(Direction::Down))),
                other => Err(format!("snap {other:?}: must be left, right, up or down")),
            };
        }
        // "focus-output next", "move-to-output-prev".
        for (prefix, make) in [
            ("move-to-output", Action::MoveToOutput as fn(_) -> _),
            ("focus-output", Action::FocusOutput),
        ] {
            if let Some(target) = text
                .strip_prefix(prefix)
                .and_then(|rest| rest.strip_prefix(' ').or_else(|| rest.strip_prefix('-')))
            {
                return match target.trim() {
                    "next" => Ok(Some(make(OutputTarget::Next))),
                    "prev" => Ok(Some(make(OutputTarget::Prev))),
                    other => Err(format!("{prefix} {other:?}: must be next or prev")),
                };
            }
        }
        // "overview open", "overview close" ("overview" toggles).
        if let Some(what) = text.strip_prefix("overview ") {
            return match what.trim() {
                "open" => Ok(Some(Action::Overview(OverviewAction::Open))),
                "close" => Ok(Some(Action::Overview(OverviewAction::Close))),
                other => Err(format!("overview {other:?}: must be open or close")),
            };
        }
        if text == "none" {
            return Ok(None);
        }
        if let Some((_, action)) = NAMED_ACTIONS.iter().find(|(name, _)| *name == text) {
            return Ok(Some(action.clone()));
        }
        Err(format!("unknown action {text:?} ({})", action_list()))
    }
}

/// The actions a word alone names (`Action::parse`), in the order
/// `action_list` gives them. Adding one here is all parsing needs;
/// `every_action_is_in_the_example_config` makes sure `config.example.toml`
/// lists it too.
const NAMED_ACTIONS: &[(&str, Action)] = &[
    ("terminal", Action::Terminal),
    ("launcher", Action::Launcher),
    ("close", Action::CloseFocused),
    ("cycle-focus", Action::CycleFocus),
    ("cycle-focus-back", Action::CycleFocusBack),
    ("minimize", Action::Minimize),
    ("maximize", Action::ToggleMaximize),
    ("fullscreen", Action::ToggleFullscreen),
    ("snap-left", Action::Snap(Side::Left)),
    ("snap-right", Action::Snap(Side::Right)),
    ("snap-top", Action::Snap(Side::Top)),
    ("snap-bottom", Action::Snap(Side::Bottom)),
    ("snap-top-left", Action::Snap(Side::TopLeft)),
    ("snap-top-right", Action::Snap(Side::TopRight)),
    ("snap-bottom-left", Action::Snap(Side::BottomLeft)),
    ("snap-bottom-right", Action::Snap(Side::BottomRight)),
    ("restore", Action::Restore),
    ("move-left", Action::MoveWindow(-KEYBOARD_STEP, 0)),
    ("move-right", Action::MoveWindow(KEYBOARD_STEP, 0)),
    ("move-up", Action::MoveWindow(0, -KEYBOARD_STEP)),
    ("move-down", Action::MoveWindow(0, KEYBOARD_STEP)),
    ("grow-width", Action::ResizeWindow(KEYBOARD_STEP, 0)),
    ("shrink-width", Action::ResizeWindow(-KEYBOARD_STEP, 0)),
    ("grow-height", Action::ResizeWindow(0, KEYBOARD_STEP)),
    ("shrink-height", Action::ResizeWindow(0, -KEYBOARD_STEP)),
    ("center", Action::Center),
    ("toggle-always-on-top", Action::ToggleAlwaysOnTop),
    ("toggle-all-workspaces", Action::ToggleAllWorkspaces),
    ("show-desktop", Action::ShowDesktop),
    ("show-shortcuts", Action::ShowShortcuts),
    ("overview", Action::Overview(OverviewAction::Toggle)),
    ("power-off-monitors", Action::PowerOffMonitors),
    ("power-on-monitors", Action::PowerOnMonitors),
    ("quit", Action::Quit),
    ("confirm-quit", Action::ConfirmQuit),
];

/// The actions that take an argument, as written (`Action::parse` reads
/// them by their prefix).
const ACTIONS_WITH_ARGUMENT: &[&str] = &[
    "spawn <command>",
    "focus-output <next|prev>",
    "move-to-output <next|prev>",
    "workspace <n|next|prev>",
    "move-to-workspace <n|next|prev>",
    "switch-layout <n|next|prev>",
    "snap <left|right|up|down>",
    "overview <open|close>",
];

/// Every action as a config may write it, for messages.
fn action_list() -> String {
    let names: Vec<&str> = ACTIONS_WITH_ARGUMENT[..1]
        .iter()
        .copied()
        .chain(NAMED_ACTIONS.iter().map(|(name, _)| *name))
        .chain(ACTIONS_WITH_ARGUMENT[1..].iter().copied())
        .collect();
    format!("{} or none", names.join(", "))
}

/// One example of each kind of action (`Action::examples`, for a test that
/// runs them all: `tests/lock.rs`), from the same table as an exhaustive
/// match, so a new kind of action doesn't build until it's in the table.
macro_rules! action_examples {
    ($($kind:pat => $example:expr,)*) => {
        impl Action {
            pub(crate) fn examples() -> Vec<Action> {
                vec![$($example),*]
            }
        }

        #[allow(dead_code)]
        fn every_kind_of_action_has_an_example(action: &Action) {
            match action {
                $($kind)|* => {}
            }
        }
    };
}

action_examples! {
    Action::Spawn(_) => Action::Spawn("true".into()),
    Action::Terminal => Action::Terminal,
    Action::Launcher => Action::Launcher,
    Action::CloseFocused => Action::CloseFocused,
    Action::CycleFocus => Action::CycleFocus,
    Action::CycleFocusBack => Action::CycleFocusBack,
    Action::Minimize => Action::Minimize,
    Action::Snap(_) => Action::Snap(Side::Left),
    Action::SnapToward(_) => Action::SnapToward(Direction::Up),
    Action::Restore => Action::Restore,
    Action::MoveWindow(..) => Action::MoveWindow(KEYBOARD_STEP, 0),
    Action::ResizeWindow(..) => Action::ResizeWindow(KEYBOARD_STEP, 0),
    Action::Center => Action::Center,
    Action::ToggleMaximize => Action::ToggleMaximize,
    Action::ToggleFullscreen => Action::ToggleFullscreen,
    Action::Quit => Action::Quit,
    Action::ConfirmQuit => Action::ConfirmQuit,
    Action::Workspace(_) => Action::Workspace(WorkspaceTarget::Index(1)),
    Action::MoveToWorkspace(_) => Action::MoveToWorkspace(WorkspaceTarget::Index(1)),
    Action::ToggleAlwaysOnTop => Action::ToggleAlwaysOnTop,
    Action::ToggleAllWorkspaces => Action::ToggleAllWorkspaces,
    Action::ShowDesktop => Action::ShowDesktop,
    Action::ShowShortcuts => Action::ShowShortcuts,
    Action::Overview(_) => Action::Overview(OverviewAction::Toggle),
    Action::FocusOutput(_) => Action::FocusOutput(OutputTarget::Next),
    Action::MoveToOutput(_) => Action::MoveToOutput(OutputTarget::Next),
    Action::SwitchLayout(_) => Action::SwitchLayout(LayoutTarget::Next),
    Action::PowerOffMonitors => Action::PowerOffMonitors,
    Action::PowerOnMonitors => Action::PowerOnMonitors,
    Action::ChangeVt(_) => Action::ChangeVt(2),
}

/// A `[bindings]` value: the action (`"Mod+X" = "close"`), or a table with
/// it and whether it repeats while the key is held
/// (`"XF86AudioRaiseVolume" = { action = "spawn ...", repeat = true }`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct BindingFile {
    action: String,
    repeat: bool,
}

impl<'de> Deserialize<'de> for BindingFile {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Table {
            action: String,
            #[serde(default)]
            repeat: bool,
        }

        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = BindingFile;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an action, or a table with `action` and `repeat`")
            }

            fn visit_str<E: serde::de::Error>(self, action: &str) -> Result<BindingFile, E> {
                Ok(BindingFile {
                    action: action.to_string(),
                    repeat: false,
                })
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                map: A,
            ) -> Result<BindingFile, A::Error> {
                let table = Table::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                Ok(BindingFile {
                    action: table.action,
                    repeat: table.repeat,
                })
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

/// `[keyboard]`: passed to xkbcommon, plus key repeat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyboardConfig {
    /// Empty when unset (the layout, variant and model depend on the
    /// machine): xkbcommon's defaults (and `XKB_DEFAULT_*`).
    pub layout: String,
    pub variant: String,
    pub model: String,
    pub options: Option<String>,
    /// Milliseconds before a held key repeats.
    pub repeat_delay: i32,
    /// Repeats per second.
    pub repeat_rate: i32,
    /// Num Lock on at startup, and with each new keymap (a reload that
    /// changes `[keyboard]`): xkb starts every keymap with it off.
    pub numlock: bool,
}

/// `[keyboard]` as written.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct KeyboardFile {
    layout: Option<String>,
    variant: Option<String>,
    model: Option<String>,
    options: Option<String>,
    repeat_delay: Option<i32>,
    repeat_rate: Option<i32>,
    numlock: Option<bool>,
}

impl KeyboardFile {
    fn or_example(self, example: Self) -> Self {
        or_example!(
            self,
            example,
            KeyboardFile {
                layout,
                variant,
                model,
                options,
                repeat_delay,
                repeat_rate,
                numlock
            }
        )
    }

    fn validate(self) -> Result<KeyboardConfig, String> {
        // xkbcommon panics on a name with a NUL in it ("\u0000").
        for (key, value) in [
            ("layout", &self.layout),
            ("variant", &self.variant),
            ("model", &self.model),
            ("options", &self.options),
        ] {
            if value.as_deref().is_some_and(|v| v.contains('\0')) {
                return Err(format!("keyboard.{key} has a NUL character in it"));
            }
        }
        Ok(KeyboardConfig {
            layout: self.layout.unwrap_or_default(),
            variant: self.variant.unwrap_or_default(),
            model: self.model.unwrap_or_default(),
            options: self.options,
            repeat_delay: need(self.repeat_delay, "keyboard.repeat-delay")?,
            repeat_rate: need(self.repeat_rate, "keyboard.repeat-rate")?,
            numlock: need(self.numlock, "keyboard.numlock")?,
        })
    }
}

/// The keymap `[keyboard]` asks for, for messages: `layout "en"`, with the
/// variant, model and options when set.
pub fn keymap_description(kb: &KeyboardConfig) -> String {
    let mut parts = vec![format!("layout \"{}\"", kb.layout)];
    if !kb.variant.is_empty() {
        parts.push(format!("variant \"{}\"", kb.variant));
    }
    if !kb.model.is_empty() {
        parts.push(format!("model \"{}\"", kb.model));
    }
    if let Some(options) = kb.options.as_ref().filter(|o| !o.is_empty()) {
        parts.push(format!("options \"{options}\""));
    }
    parts.join(", ")
}

/// Whether xkbcommon can build the keymap `[keyboard]` asks for (empty
/// fields fall back to its defaults and `XKB_DEFAULT_*`, as when pila
/// starts). The file can't tell a misspelt layout; this can.
pub fn check_keymap(kb: &KeyboardConfig) -> Result<(), String> {
    let mut context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    // The message below says what's wrong; xkbcommon's own lines (include
    // paths searched, ...) only when asked for (XKB_LOG_LEVEL).
    if std::env::var_os("XKB_LOG_LEVEL").is_none() {
        context.set_log_level(xkb::LogLevel::Critical);
    }
    xkb::Keymap::new_from_names(
        &context,
        "",
        &kb.model,
        &kb.layout,
        &kb.variant,
        kb.options.clone(),
        xkb::KEYMAP_COMPILE_NO_FLAGS,
    )
    .map(|_| ())
    .ok_or_else(|| {
        format!(
            "[keyboard]: {}: xkbcommon can't build that keymap",
            keymap_description(kb)
        )
    })
}

/// Pointer acceleration (`accel-profile`): libinput's adaptive curve, or a
/// constant factor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccelProfile {
    Adaptive,
    Flat,
}

/// How a clickpad decides which button a click is (`click-method`): by
/// where it's pressed, or by how many fingers are down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClickMethod {
    ButtonAreas,
    Clickfinger,
}

/// `[input.touchpad]` / `[input.mouse]`: libinput settings for one kind of
/// device, on a TTY (nested, the host handles input). `None` leaves
/// libinput's default; options a device doesn't support are skipped.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct PointerConfig {
    /// Tap to click (touchpads; on in `EXAMPLE`).
    pub tap: Option<bool>,
    pub natural_scroll: Option<bool>,
    /// -1 (slowest) to 1 (fastest).
    pub accel_speed: Option<f64>,
    pub accel_profile: Option<AccelProfile>,
    /// Ignore the touchpad briefly while typing.
    pub disable_while_typing: Option<bool>,
    pub click_method: Option<ClickMethod>,
    /// Pressing left and right together is a middle click.
    pub middle_emulation: Option<bool>,
}

impl PointerConfig {
    fn or_example(self, example: Self) -> Self {
        or_example!(
            self,
            example,
            PointerConfig {
                tap,
                natural_scroll,
                accel_speed,
                accel_profile,
                disable_while_typing,
                click_method,
                middle_emulation
            }
        )
    }

    fn validate(self, section: &str) -> Result<Self, String> {
        if let Some(speed) = self.accel_speed {
            if !(-1.0..=1.0).contains(&speed) {
                return Err(format!(
                    "[input.{section}] accel-speed {speed} is outside -1..1"
                ));
            }
        }
        Ok(self)
    }
}

/// The animations pila can play (`animation.rs`), each with its own
/// `[animations]` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationKind {
    /// A window appears.
    WindowOpen,
    /// A window closes.
    WindowClose,
    /// A window is minimized or restored.
    Minimize,
    /// A window is maximized, made fullscreen or snapped, or back.
    Maximize,
    /// A launcher or notification (layer surface) appears or goes.
    Layer,
    /// The workspace changes.
    Workspace,
    /// The overview opens or closes.
    Overview,
}

impl AnimationKind {
    const ALL: [AnimationKind; 7] = [
        AnimationKind::WindowOpen,
        AnimationKind::WindowClose,
        AnimationKind::Minimize,
        AnimationKind::Maximize,
        AnimationKind::Layer,
        AnimationKind::Workspace,
        AnimationKind::Overview,
    ];

    /// Its key in `[animations]`.
    fn key(self) -> &'static str {
        match self {
            AnimationKind::WindowOpen => "window-open",
            AnimationKind::WindowClose => "window-close",
            AnimationKind::Minimize => "minimize",
            AnimationKind::Maximize => "maximize",
            AnimationKind::Layer => "layer",
            AnimationKind::Workspace => "workspace",
            AnimationKind::Overview => "overview",
        }
    }
}

/// The longest an animation may be configured to last, in milliseconds.
pub const MAX_ANIMATION_MS: u32 = 1000;
/// The range of `[animations] speed`.
const ANIMATION_SPEEDS: std::ops::RangeInclusive<f64> = 0.25..=4.0;

/// `[animations]`, validated.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimationsConfig {
    /// Each kind's duration in milliseconds (`AnimationKind::ALL` order);
    /// 0 when it's off.
    durations: [u32; 7],
    /// Multiplies every duration (2: twice as slow).
    speed: f64,
    /// `enabled`: off, everything that would animate changes at once.
    enabled: bool,
}

impl AnimationsConfig {
    /// How long `kind` lasts, or `None` if it's off.
    pub fn duration(&self, kind: AnimationKind) -> Option<std::time::Duration> {
        let index = AnimationKind::ALL.iter().position(|k| *k == kind)?;
        let ms = f64::from(self.durations[index]) * self.speed;
        (ms >= 1.0).then(|| std::time::Duration::from_secs_f64(ms / 1000.0))
    }

    /// `ms` at the configured speed, for a change with no `[animations]` entry
    /// of its own (the cursor growing, `shake_cursor.rs`); zero with
    /// animations off.
    pub fn scaled(&self, ms: u32) -> std::time::Duration {
        if !self.enabled {
            return std::time::Duration::ZERO;
        }
        self.at_speed(ms)
    }

    /// `ms` at the configured speed, animations on or off: how long
    /// something stays before it animates (the cursor stays big a moment).
    pub fn at_speed(&self, ms: u32) -> std::time::Duration {
        std::time::Duration::from_secs_f64(f64::from(ms) * self.speed / 1000.0)
    }
}

/// One animation's entry in `[animations]`, as written: its duration in
/// milliseconds, or `false` (off) / `true` (`EXAMPLE`'s duration).
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(
    untagged,
    expecting = "a duration in ms (e.g. 180), or false to turn it off"
)]
enum AnimationFile {
    On(bool),
    Duration(u32),
}

/// `[animations]` as written.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct AnimationsFile {
    enabled: Option<bool>,
    speed: Option<f64>,
    window_open: Option<AnimationFile>,
    window_close: Option<AnimationFile>,
    minimize: Option<AnimationFile>,
    maximize: Option<AnimationFile>,
    layer: Option<AnimationFile>,
    workspace: Option<AnimationFile>,
    overview: Option<AnimationFile>,
}

impl AnimationsFile {
    fn or_example(self, example: Self) -> Self {
        // `true`: the example's duration.
        let entry = |user: Option<AnimationFile>, example| match user {
            None | Some(AnimationFile::On(true)) => example,
            user => user,
        };
        AnimationsFile {
            enabled: self.enabled.or(example.enabled),
            speed: self.speed.or(example.speed),
            window_open: entry(self.window_open, example.window_open),
            window_close: entry(self.window_close, example.window_close),
            minimize: entry(self.minimize, example.minimize),
            maximize: entry(self.maximize, example.maximize),
            layer: entry(self.layer, example.layer),
            workspace: entry(self.workspace, example.workspace),
            overview: entry(self.overview, example.overview),
        }
    }

    fn validate(self) -> Result<AnimationsConfig, String> {
        let speed = need(self.speed, "animations.speed")?;
        if !ANIMATION_SPEEDS.contains(&speed) {
            return Err(format!(
                "[animations] speed {speed} is outside {}..{}",
                ANIMATION_SPEEDS.start(),
                ANIMATION_SPEEDS.end()
            ));
        }
        let all_on = need(self.enabled, "animations.enabled")?;
        let entries = [
            self.window_open,
            self.window_close,
            self.minimize,
            self.maximize,
            self.layer,
            self.workspace,
            self.overview,
        ];
        let mut durations = [0; 7];
        for ((kind, entry), duration) in AnimationKind::ALL.iter().zip(entries).zip(&mut durations)
        {
            let key = format!("animations.{}", kind.key());
            let ms = match need(entry, &key)? {
                AnimationFile::On(true) => {
                    return Err(format!("{key} = true: no duration to take"))
                }
                AnimationFile::On(false) => 0,
                AnimationFile::Duration(ms @ 1..=MAX_ANIMATION_MS) => ms,
                AnimationFile::Duration(ms) => {
                    return Err(format!(
                        "[animations] {} = {ms}: a duration is 1 to {MAX_ANIMATION_MS} ms \
                         (false turns it off)",
                        kind.key()
                    ))
                }
            };
            if all_on {
                *duration = ms;
            }
        }
        Ok(AnimationsConfig {
            durations,
            speed,
            enabled: all_on,
        })
    }
}

/// `[input]`.
#[derive(Debug, Clone, PartialEq)]
pub struct InputConfig {
    pub touchpad: PointerConfig,
    pub mouse: PointerConfig,
    /// The least time between two runs of a mouse wheel binding, in
    /// milliseconds: the notches of a quick turn inside it are dropped
    /// (`input.rs::wheel_bindings`). 0 runs one a notch.
    pub wheel_binding_cooldown: u32,
}

/// The longest `wheel-binding-cooldown`, in milliseconds.
const MAX_WHEEL_BINDING_COOLDOWN: u32 = 1000;

/// `[[output]]`: settings for one monitor, matched by connector name
/// (`eDP-1`, `DP-1`, `HDMI-A-1`, ...; `winit` is the nested window) or by the
/// monitor's make and model, with or without its serial (`GSM 27GL850`).
#[derive(Debug, Clone, PartialEq)]
pub struct OutputConfig {
    pub name: String,
    /// The mode to set instead of the monitor's preferred one (TTY only).
    pub mode: Option<ModeSpec>,
    /// Logical pixels are this many physical ones; a multiple of 1/120 (what
    /// fractional-scale can express), from 0.5 to 8.
    pub scale: Option<f64>,
    /// Top-left corner in the global logical space (TTY only). Outputs
    /// without one go right of the others.
    pub position: Option<(i32, i32)>,
    /// Rotation (counter-clockwise, as in wl_output) and flipping, for a
    /// monitor mounted sideways (TTY only).
    pub transform: Option<Transform>,
    /// `false` leaves the monitor off: no `wl_output`, no windows (TTY only).
    pub enabled: Option<bool>,
    /// Variable refresh rate, where the monitor supports it (TTY only).
    pub adaptive_sync: Option<bool>,
}

impl OutputConfig {
    /// A section with nothing set: the defaults.
    pub fn named(name: &str) -> Self {
        OutputConfig {
            name: name.to_string(),
            mode: None,
            scale: None,
            position: None,
            transform: None,
            enabled: None,
            adaptive_sync: None,
        }
    }

    /// This section with `over`'s set fields on top (what a client set
    /// through wlr-output-management over what the file says).
    pub fn overridden_by(&self, over: &OutputConfig) -> OutputConfig {
        OutputConfig {
            name: self.name.clone(),
            mode: over.mode.or(self.mode),
            scale: over.scale.or(self.scale),
            position: over.position.or(self.position),
            transform: over.transform.or(self.transform),
            enabled: over.enabled.or(self.enabled),
            adaptive_sync: over.adaptive_sync.or(self.adaptive_sync),
        }
    }
}

/// A TOML error as one line that starts with where and what ("line 3,
/// column 3: unknown field ..."): notifications show only their first lines,
/// and toml's own text starts with a snippet of the file.
fn toml_error(text: &str, err: &toml::de::Error) -> String {
    let message = err.message().trim();
    let place = err.span().map(|span| {
        let before = &text[..span.start.min(text.len())];
        let line = before.matches('\n').count() + 1;
        let column = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
        format!("line {line}, column {column}: ")
    });
    // A key combination outside [bindings], most likely from taking the `#`
    // off a binding above the table's header: the fields that table expected
    // don't help then.
    let misplaced = message
        .strip_prefix("unknown field `")
        .and_then(|rest| rest.split('`').next())
        .filter(|field| field.contains('+'))
        .map(|field| format!("unknown field `{field}`: key bindings go under [bindings]"));
    let message = misplaced.unwrap_or_else(|| message.to_string());
    format!("{}{message}", place.unwrap_or_default())
}

/// `transform` in `[[output]]`: wl_output's names.
pub(crate) fn parse_transform(text: &str) -> Result<Transform, String> {
    Ok(match text {
        "normal" => Transform::Normal,
        "90" => Transform::_90,
        "180" => Transform::_180,
        "270" => Transform::_270,
        "flipped" => Transform::Flipped,
        "flipped-90" => Transform::Flipped90,
        "flipped-180" => Transform::Flipped180,
        "flipped-270" => Transform::Flipped270,
        other => {
            return Err(format!(
                "unknown transform {other:?} (normal, 90, 180, 270, flipped, flipped-90, \
                 flipped-180 or flipped-270)"
            ))
        }
    })
}

/// A video mode: `"2560x1440"` or `"2560x1440@59.95"` (refresh in Hz).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModeSpec {
    pub width: i32,
    pub height: i32,
    pub refresh_hz: Option<f64>,
}

impl ModeSpec {
    fn parse(text: &str) -> Result<Self, String> {
        let bad = || format!("bad mode {text:?} (e.g. \"1920x1080\" or \"1920x1080@60\")");
        let (size, refresh) = match text.split_once('@') {
            Some((size, refresh)) => (size, Some(refresh)),
            None => (text, None),
        };
        let (w, h) = size.split_once('x').ok_or_else(bad)?;
        let width = w.trim().parse().map_err(|_| bad())?;
        let height = h.trim().parse().map_err(|_| bad())?;
        let refresh_hz = match refresh {
            Some(r) => Some(
                r.trim()
                    .trim_end_matches("Hz")
                    .parse::<f64>()
                    .ok()
                    .filter(|r| *r > 0.0)
                    .ok_or_else(bad)?,
            ),
            None => None,
        };
        if width <= 0 || height <= 0 {
            return Err(bad());
        }
        Ok(Self {
            width,
            height,
            refresh_hz,
        })
    }

    /// Which of `modes` (size and refresh in mHz) this asks for: the right
    /// size, at the refresh closest to the one given, else the `preferred`
    /// one if it has the right size, else the fastest.
    pub fn pick(&self, modes: &[((i32, i32), i32)], preferred: Option<usize>) -> Option<usize> {
        let fits = |i: &usize| modes[*i].0 == (self.width, self.height);
        let candidates = (0..modes.len()).filter(fits);
        match self.refresh_hz {
            Some(hz) => candidates.min_by(|a, b| {
                let off = |i: &usize| (modes[*i].1 as f64 / 1000.0 - hz).abs();
                off(a).total_cmp(&off(b))
            }),
            None => preferred
                .filter(fits)
                .or_else(|| candidates.max_by_key(|i| modes[*i].1)),
        }
    }
}

/// The farthest an output may be placed from the origin, on each axis, in
/// logical pixels: far past any desk, and far from where adding an output's
/// size to its position overflows. `[[output]]` and display tools are held
/// to it.
pub const MAX_OUTPUT_POSITION: i32 = 1_000_000;

/// Whether an output position is within `MAX_OUTPUT_POSITION`.
pub fn output_position_in_range((x, y): (i32, i32)) -> bool {
    let range = -MAX_OUTPUT_POSITION..=MAX_OUTPUT_POSITION;
    range.contains(&x) && range.contains(&y)
}

/// `[[output]]` as written in the file.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct OutputFile {
    name: String,
    mode: Option<String>,
    scale: Option<f64>,
    position: Option<[i32; 2]>,
    transform: Option<String>,
    enabled: Option<bool>,
    #[serde(rename = "adaptive-sync")]
    adaptive_sync: Option<bool>,
}

impl OutputFile {
    fn validate(self) -> Result<OutputConfig, String> {
        if self.name.is_empty() {
            return Err("an [[output]] has no name".into());
        }
        let scale = match self.scale {
            Some(scale) if !(0.5..=8.0).contains(&scale) => {
                return Err(format!(
                    "output {:?}: scale {scale} is outside 0.5..8",
                    self.name
                ))
            }
            // Clients get it in 1/120ths: render at exactly what they're told.
            Some(scale) => Some((scale * 120.0).round() / 120.0),
            None => None,
        };
        if let Some([x, y]) = self.position {
            if !output_position_in_range((x, y)) {
                return Err(format!(
                    "output {:?}: position [{x}, {y}] is outside -{MAX_OUTPUT_POSITION}..{MAX_OUTPUT_POSITION}",
                    self.name
                ));
            }
        }
        let mode = match &self.mode {
            Some(mode) => Some(
                ModeSpec::parse(mode).map_err(|err| format!("output {:?}: {err}", self.name))?,
            ),
            None => None,
        };
        let transform = match &self.transform {
            Some(text) => Some(
                parse_transform(text).map_err(|err| format!("output {:?}: {err}", self.name))?,
            ),
            None => None,
        };
        Ok(OutputConfig {
            name: self.name,
            mode,
            scale,
            position: self.position.map(|[x, y]| (x, y)),
            transform,
            enabled: self.enabled,
            adaptive_sync: self.adaptive_sync,
        })
    }
}

/// An `[environment]` value as written: the variable's value, or `false` to
/// remove it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(
    untagged,
    expecting = "a string (the variable's value), or false (to remove it)"
)]
enum EnvFile {
    Set(String),
    Remove(bool),
}

/// The variables Pila gives what it starts itself (`Pila::session_env`,
/// `Pila::spawn`), which `[environment]` can't change: the session needs
/// them as they are.
pub const PILA_ENV: [&str; 7] = [
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "XAUTHORITY",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_TYPE",
    "PILA_SOCKET",
    "PILA_OUTPUT",
];

/// `[environment]`, validated: each variable with its value, `None` to
/// remove it; Pila's own (`PILA_ENV`) left out, with a warning each.
fn environment(
    entries: BTreeMap<String, EnvFile>,
    warnings: &mut Vec<String>,
) -> Result<Vec<(String, Option<String>)>, String> {
    let mut environment = Vec::new();
    for (name, value) in entries {
        if name.is_empty() || name.contains(['=', '\0']) {
            return Err(format!(
                "[environment] {name:?}: a variable's name can't be empty or have \"=\""
            ));
        }
        let value = match value {
            EnvFile::Set(value) if value.contains('\0') => {
                return Err(format!("[environment] {name}: a value can't have a NUL"))
            }
            EnvFile::Set(value) => Some(value),
            EnvFile::Remove(false) => None,
            EnvFile::Remove(true) => {
                return Err(format!(
                    "[environment] {name} = true: a value is a string (\"true\"), or false \
                     to remove the variable"
                ))
            }
        };
        if PILA_ENV.contains(&name.as_str()) {
            warnings.push(format!(
                "[environment] {name}: Pila sets it itself for what it starts; left as Pila sets it"
            ));
            continue;
        }
        environment.push((name, value));
    }
    Ok(environment)
}

/// `user`'s fields, each one it leaves unset taken from `example`'s.
macro_rules! or_example {
    ($user:expr, $example:expr, $Type:ident { $($field:ident),* $(,)? }) => {{
        let (user, example) = ($user, $example);
        $Type { $($field: user.$field.or(example.$field)),* }
    }};
}
use or_example;

/// A setting's value, which the user's file or else `EXAMPLE` sets: there are
/// no defaults in the code, so one neither sets is an error (a test makes
/// sure `EXAMPLE` sets every one).
fn need<T>(value: Option<T>, key: &str) -> Result<T, String> {
    value.ok_or_else(|| {
        format!("{key} isn't set, and the built-in config.example.toml doesn't set it either")
    })
}

/// `value` of `key` if it's within `range`; else an error that says the
/// range, in `unit` ("pixels"; empty: none).
fn in_range<T: PartialOrd + std::fmt::Display>(
    value: T,
    key: &str,
    range: std::ops::RangeInclusive<T>,
    unit: &str,
) -> Result<T, String> {
    if range.contains(&value) {
        return Ok(value);
    }
    let unit = if unit.is_empty() {
        String::new()
    } else {
        format!(" {unit}")
    };
    Err(format!(
        "{key} = {value}: must be {} to {}{unit}",
        range.start(),
        range.end()
    ))
}

/// The file's layout, before validation: everything optional, as the user's
/// file may leave anything out (`EXAMPLE` fills it in).
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct ConfigFile {
    #[serde(rename = "mod")]
    mod_key: Option<String>,
    terminal: Option<String>,
    launcher: Option<String>,
    autostart: Option<Vec<String>>,
    keyboard: KeyboardFile,
    default_bindings: Option<bool>,
    bindings: BTreeMap<String, BindingFile>,
    gestures: BTreeMap<String, String>,
    output: Vec<OutputFile>,
    input: InputFile,
    workspaces: Option<usize>,
    corner_radius: Option<u32>,
    rounded_corners: Option<String>,
    gaps: Option<u32>,
    other_gpus: Option<bool>,
    direct_scanout: Option<bool>,
    tearing: Option<bool>,
    render_late: Option<bool>,
    tool_configs: Option<bool>,
    shadow: ShadowFile,
    colors: ColorsFile,
    animations: AnimationsFile,
    cursor: CursorFile,
    xwayland: XwaylandFile,
    overview: OverviewFile,
    idle: IdleFile,
    wallpaper: WallpaperFile,
    environment: BTreeMap<String, EnvFile>,
    permissions: PermissionsFile,
    #[serde(rename = "permission")]
    permission_rules: Vec<PermissionRuleFile>,
    #[serde(rename = "window-rule")]
    window_rules: Vec<crate::rules::WindowRuleFile>,
    #[serde(rename = "layer-rule")]
    layer_rules: Vec<crate::rules::LayerRule>,
}

/// A color from the config: `#rrggbb`, or `#rrggbbaa` if `alpha` allows it.
/// Premultiplied, as the renderer takes it.
pub(crate) fn parse_color(name: &str, text: &str, alpha: bool) -> Result<[f32; 4], String> {
    let invalid = || {
        let form = if alpha {
            "#rrggbb or #rrggbbaa"
        } else {
            "#rrggbb"
        };
        format!("{name} = {text:?}: must be {form}")
    };
    let digits = text.strip_prefix('#').ok_or_else(invalid)?;
    if !digits.is_ascii() || !(digits.len() == 6 || (alpha && digits.len() == 8)) {
        return Err(invalid());
    }
    let channel = |i: usize| {
        u8::from_str_radix(&digits[i..i + 2], 16)
            .map(|v| f32::from(v) / 255.0)
            .map_err(|_| invalid())
    };
    let a = if digits.len() == 8 { channel(6)? } else { 1.0 };
    Ok([channel(0)? * a, channel(2)? * a, channel(4)? * a, a])
}

/// The colors of what pila draws itself (`[colors]`). Premultiplied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Colors {
    pub bar: [f32; 4],
    pub bar_inactive: [f32; 4],
    /// The title bar of an unfocused window asking for attention (`urgency.rs`).
    pub bar_urgent: [f32; 4],
    /// Title and buttons of the focused window.
    pub icon: [f32; 3],
    pub icon_inactive: [f32; 3],
    /// A title bar button under the pointer.
    pub hover: [f32; 3],
    pub hover_icon: [f32; 3],
    pub close_hover: [f32; 3],
    pub close_hover_icon: [f32; 3],
    /// Behind all windows and layer surfaces.
    pub background: [f32; 4],
    /// Where a dragged window would snap to (`snap.rs`).
    pub snap_preview: [f32; 4],
}

impl Colors {
    /// Every color by its key in `[colors]`, premultiplied RGBA (those
    /// with no alpha of their own opaque).
    pub fn named(&self) -> [(&'static str, [f32; 4]); 11] {
        let opaque = |[r, g, b]: [f32; 3]| [r, g, b, 1.0];
        [
            ("bar", self.bar),
            ("bar-inactive", self.bar_inactive),
            ("bar-urgent", self.bar_urgent),
            ("icon", opaque(self.icon)),
            ("icon-inactive", opaque(self.icon_inactive)),
            ("hover", opaque(self.hover)),
            ("hover-icon", opaque(self.hover_icon)),
            ("close-hover", opaque(self.close_hover)),
            ("close-hover-icon", opaque(self.close_hover_icon)),
            ("background", self.background),
            ("snap-preview", self.snap_preview),
        ]
    }
}

/// `[colors]` as written: unset ones are `EXAMPLE`'s.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct ColorsFile {
    bar: Option<String>,
    bar_inactive: Option<String>,
    bar_urgent: Option<String>,
    icon: Option<String>,
    icon_inactive: Option<String>,
    hover: Option<String>,
    hover_icon: Option<String>,
    close_hover: Option<String>,
    close_hover_icon: Option<String>,
    background: Option<String>,
    snap_preview: Option<String>,
}

impl ColorsFile {
    fn or_example(self, example: Self) -> Self {
        or_example!(
            self,
            example,
            ColorsFile {
                bar,
                bar_inactive,
                bar_urgent,
                icon,
                icon_inactive,
                hover,
                hover_icon,
                close_hover,
                close_hover_icon,
                background,
                snap_preview
            }
        )
    }

    fn validate(self) -> Result<Colors, String> {
        let rgba = |name: &str, text: Option<String>, alpha: bool| {
            let name = format!("colors.{name}");
            parse_color(&name, &need(text, &name)?, alpha)
        };
        let rgb = |name: &str, text: Option<String>| {
            rgba(name, text, false).map(|[r, g, b, _]| [r, g, b])
        };
        Ok(Colors {
            bar: rgba("bar", self.bar, false)?,
            bar_inactive: rgba("bar-inactive", self.bar_inactive, false)?,
            bar_urgent: rgba("bar-urgent", self.bar_urgent, false)?,
            icon: rgb("icon", self.icon)?,
            icon_inactive: rgb("icon-inactive", self.icon_inactive)?,
            hover: rgb("hover", self.hover)?,
            hover_icon: rgb("hover-icon", self.hover_icon)?,
            close_hover: rgb("close-hover", self.close_hover)?,
            close_hover_icon: rgb("close-hover-icon", self.close_hover_icon)?,
            background: rgba("background", self.background, false)?,
            snap_preview: rgba("snap-preview", self.snap_preview, true)?,
        })
    }
}

/// The range of `[cursor] shake-magnification`.
pub const SHAKE_MAGNIFICATIONS: std::ops::RangeInclusive<f64> = 1.5..=5.0;

/// `[cursor]`, validated.
#[derive(Debug, Clone, PartialEq)]
pub struct CursorConfig {
    /// Shaking the pointer makes the cursor bigger for a moment
    /// (`shake_cursor.rs`).
    pub shake_to_find: bool,
    /// How many times bigger it gets at most.
    pub shake_magnification: f64,
    /// The xcursor theme; `None`: `XCURSOR_THEME`, else `default`
    /// (`cursor::CursorTheme::load`). Set, it's also the `XCURSOR_THEME` of
    /// what Pila starts.
    pub theme: Option<String>,
    /// The cursor's size in logical pixels; `None`: `XCURSOR_SIZE`, else 24.
    /// Set, it's also the `XCURSOR_SIZE` of what Pila starts.
    pub size: Option<u32>,
}

/// The cursor sizes `[cursor] size` takes, in logical pixels.
pub const CURSOR_SIZES: std::ops::RangeInclusive<u32> = 8..=256;

/// `[cursor]` as written: unset ones are `EXAMPLE`'s.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct CursorFile {
    shake_to_find: Option<bool>,
    shake_magnification: Option<f64>,
    theme: Option<String>,
    size: Option<u32>,
}

impl CursorFile {
    fn or_example(self, example: Self) -> Self {
        or_example!(
            self,
            example,
            CursorFile {
                shake_to_find,
                shake_magnification,
                theme,
                size
            }
        )
    }

    fn validate(self) -> Result<CursorConfig, String> {
        let key = "cursor.shake-magnification";
        let magnification = in_range(
            need(self.shake_magnification, key)?,
            key,
            SHAKE_MAGNIFICATIONS,
            "",
        )?;
        if self
            .theme
            .as_deref()
            .is_some_and(|theme| theme.trim().is_empty())
        {
            return Err("cursor.theme is empty: name a theme, or leave it out".to_string());
        }
        if let Some(size) = self.size {
            in_range(size, "cursor.size", CURSOR_SIZES, "")?;
        }
        Ok(CursorConfig {
            shake_to_find: need(self.shake_to_find, "cursor.shake-to-find")?,
            shake_magnification: magnification,
            theme: self.theme,
            size: self.size,
        })
    }
}

/// The `[xwayland]` `idle-timeout`s Pila takes, in seconds.
pub const XWAYLAND_IDLE_TIMEOUTS: std::ops::RangeInclusive<u32> = 1..=3600;

/// `[xwayland]`: how Pila runs Xwayland (`xwayland.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XwaylandConfig {
    /// X11 apps can run: an X display is reserved for them and Xwayland
    /// started when one connects. Off, nothing X is set up, and turned off
    /// while Xwayland runs it stays for its apps but isn't started again.
    pub enabled: bool,
    /// Stop Xwayland this long after its last X11 client leaves (it starts
    /// again with the next one). Unset, it runs until Pila exits once
    /// started.
    pub idle_timeout: Option<u32>,
}

/// `[xwayland]` as written.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct XwaylandFile {
    enabled: Option<bool>,
    idle_timeout: Option<u32>,
}

impl XwaylandFile {
    fn or_example(self, example: Self) -> Self {
        or_example!(
            self,
            example,
            XwaylandFile {
                enabled,
                idle_timeout
            }
        )
    }

    fn validate(self) -> Result<XwaylandConfig, String> {
        if let Some(seconds) = self.idle_timeout {
            in_range(
                seconds,
                "xwayland.idle-timeout",
                XWAYLAND_IDLE_TIMEOUTS,
                "seconds",
            )?;
        }
        Ok(XwaylandConfig {
            enabled: need(self.enabled, "xwayland.enabled")?,
            idle_timeout: self.idle_timeout,
        })
    }
}

/// `[overview]` (`overview.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverviewConfig {
    /// The pointer pushed into a corner of the screen opens it (or closes
    /// it).
    pub hot_corner: bool,
    /// Which corner.
    pub hot_corner_position: Corner,
    /// How far (logical pixels of motion) the pointer must be pushed past
    /// the corner before it counts; 0: reaching it is enough.
    pub hot_corner_pressure: u32,
    /// How long (milliseconds) the pointer must stay in the corner.
    pub hot_corner_delay: u32,
}

/// A corner of the screen (`hot-corner-position`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    /// Which way out of the screen the corner is: -1 or 1 on each axis.
    pub fn outward(self) -> (f64, f64) {
        match self {
            Corner::TopLeft => (-1.0, -1.0),
            Corner::TopRight => (1.0, -1.0),
            Corner::BottomLeft => (-1.0, 1.0),
            Corner::BottomRight => (1.0, 1.0),
        }
    }
}

/// `hot-corner-pressure`'s range, in logical pixels.
pub const HOT_CORNER_PRESSURES: std::ops::RangeInclusive<u32> = 0..=2000;
/// `hot-corner-delay`'s range, in milliseconds.
pub const HOT_CORNER_DELAYS: std::ops::RangeInclusive<u32> = 0..=5000;

/// `[overview]` as written.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct OverviewFile {
    hot_corner: Option<bool>,
    hot_corner_position: Option<Corner>,
    hot_corner_pressure: Option<u32>,
    hot_corner_delay: Option<u32>,
}

impl OverviewFile {
    fn or_example(self, example: Self) -> Self {
        or_example!(
            self,
            example,
            OverviewFile {
                hot_corner,
                hot_corner_position,
                hot_corner_pressure,
                hot_corner_delay
            }
        )
    }

    fn validate(self) -> Result<OverviewConfig, String> {
        let key = "overview.hot-corner-pressure";
        let pressure = in_range(
            need(self.hot_corner_pressure, key)?,
            key,
            HOT_CORNER_PRESSURES,
            "pixels",
        )?;
        let key = "overview.hot-corner-delay";
        let delay = in_range(
            need(self.hot_corner_delay, key)?,
            key,
            HOT_CORNER_DELAYS,
            "milliseconds",
        )?;
        Ok(OverviewConfig {
            hot_corner: need(self.hot_corner, "overview.hot-corner")?,
            hot_corner_position: need(self.hot_corner_position, "overview.hot-corner-position")?,
            hot_corner_pressure: pressure,
            hot_corner_delay: delay,
        })
    }
}

/// `[permissions]`, validated (`permissions.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermissionsConfig {
    /// Enforce screen capture permissions for direct Wayland capture protocols.
    pub enforce: bool,
    /// Light a monitor's top edge while it's captured (`capture_indicator.rs`).
    pub indicator: bool,
    /// Show what's captured in the system tray, with a Stop for each (`tray.rs`).
    pub tray: bool,
}

/// `[permissions]` as written.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct PermissionsFile {
    enforce: Option<bool>,
    indicator: Option<bool>,
    tray: Option<bool>,
}

impl PermissionsFile {
    fn or_example(self, example: Self) -> Self {
        or_example!(
            self,
            example,
            PermissionsFile {
                enforce,
                indicator,
                tray
            }
        )
    }

    fn validate(self) -> Result<PermissionsConfig, String> {
        Ok(PermissionsConfig {
            enforce: need(self.enforce, "permissions.enforce")?,
            indicator: need(self.indicator, "permissions.indicator")?,
            tray: need(self.tray, "permissions.tray")?,
        })
    }
}

/// What a `[[permission]]` rule decides for matching executables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionAccess {
    Allow,
    Ask,
    Deny,
}

/// A validated `[[permission]]` rule (`permissions.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRule {
    /// Executable path pattern (`*` wildcards).
    pub executable: String,
    pub access: PermissionAccess,
}

/// `[[permission]]` as written.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct PermissionRuleFile {
    executable: Option<String>,
    access: Option<PermissionAccess>,
}

impl PermissionRuleFile {
    fn validate(self, index: usize) -> Result<PermissionRule, String> {
        let executable = self
            .executable
            .filter(|e| !e.trim().is_empty())
            .ok_or_else(|| {
                format!("permission {index}: no executable (e.g. executable = \"/usr/bin/grim\")")
            })?;
        let access = self
            .access
            .ok_or_else(|| format!("permission {index}: no access (allow, ask or deny)"))?;
        Ok(PermissionRule { executable, access })
    }
}

/// A time in seconds as written, or `false` for never.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(
    untagged,
    expecting = "a time in seconds (e.g. 600), or false for never"
)]
enum SecondsFile {
    Never(bool),
    Seconds(u32),
}

/// `[idle]` as written: an old file's. Turning the monitors off after a
/// while is swayidle's job (the example's `autostart`); the key is still
/// read, so such a file keeps working, and a time in it gets a warning.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct IdleFile {
    monitors_off: Option<SecondsFile>,
}

impl IdleFile {
    /// The warning for a time Pila no longer acts on, with the swayidle
    /// command that does it.
    fn warning(&self) -> Option<String> {
        let seconds = match self.monitors_off? {
            SecondsFile::Never(false) => return None,
            SecondsFile::Never(true) => 600,
            SecondsFile::Seconds(seconds) => seconds,
        };
        Some(format!(
            "idle.monitors-off: Pila no longer turns the monitors off itself, swayidle does: \
             remove [idle] and put \"swayidle -w timeout {seconds} 'pila msg action \
             power-off-monitors'\" in `autostart` (the example's also locks the screen)"
        ))
    }
}

/// `[wallpaper]` as written: an old file's. The wallpaper is swaybg's job
/// (`autostart`); the section is still read, so such a file keeps working,
/// and an image in it gets a warning with the swaybg line that draws it.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct WallpaperFile {
    image: Option<String>,
    fit: Option<String>,
}

impl WallpaperFile {
    fn warning(&self) -> Option<String> {
        let image = self.image.as_deref().filter(|image| !image.is_empty())?;
        // `~/` outside the quotes, for the shell to expand.
        let (tilde, path) = match image.strip_prefix("~/") {
            Some(rest) => ("~/", rest),
            None => ("", image),
        };
        let quoted = format!("{tilde}'{}'", path.replace('\'', r"'\''"));
        // swaybg's modes have the names `fit` took.
        let mode = self.fit.as_deref().unwrap_or("fill");
        Some(format!(
            "[wallpaper]: Pila no longer draws the wallpaper, swaybg does: remove [wallpaper] \
             and put \"swaybg -i {quoted} -m {mode}\" in `autostart`"
        ))
    }
}

/// The largest `[shadow]` `size` and, either way, `offset` (logical pixels).
const MAX_SHADOW: i32 = 64;

/// The shadows pila draws under windows (`[shadow]`; `rounded.rs`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowConfig {
    pub enabled: bool,
    /// How far it reaches past the window, in logical pixels.
    pub size: i32,
    pub offset: (i32, i32),
    /// Premultiplied, under the focused window.
    pub color: [f32; 4],
    pub inactive_color: [f32; 4],
}

/// `[shadow]` as written: unset ones are `EXAMPLE`'s.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct ShadowFile {
    enabled: Option<bool>,
    size: Option<i32>,
    offset: Option<[i32; 2]>,
    color: Option<String>,
    inactive_color: Option<String>,
}

impl ShadowFile {
    fn or_example(self, example: Self) -> Self {
        or_example!(
            self,
            example,
            ShadowFile {
                enabled,
                size,
                offset,
                color,
                inactive_color
            }
        )
    }

    fn validate(self) -> Result<ShadowConfig, String> {
        let size = in_range(
            need(self.size, "shadow.size")?,
            "shadow.size",
            0..=MAX_SHADOW,
            "",
        )?;
        let [x, y] = need(self.offset, "shadow.offset")?;
        let offset = (x, y);
        if [offset.0, offset.1].iter().any(|v| v.abs() > MAX_SHADOW) {
            return Err(format!(
                "shadow.offset = [{}, {}]: must be -{MAX_SHADOW} to {MAX_SHADOW}",
                offset.0, offset.1
            ));
        }
        let color = |name: &str, text: Option<String>| {
            let name = format!("shadow.{name}");
            parse_color(&name, &need(text, &name)?, true)
        };
        Ok(ShadowConfig {
            enabled: need(self.enabled, "shadow.enabled")?,
            size,
            offset,
            color: color("color", self.color)?,
            inactive_color: color("inactive-color", self.inactive_color)?,
        })
    }
}

/// `[input]` as written: unset ones are `EXAMPLE`'s.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct InputFile {
    touchpad: Option<PointerConfig>,
    mouse: Option<PointerConfig>,
    #[serde(rename = "wheel-binding-cooldown")]
    wheel_binding_cooldown: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub mod_key: ModKey,
    /// The Mod used when neither the file nor `PILA_MOD` sets one (depends on
    /// the backend; kept so a reload parses the same way).
    pub default_mod: ModKey,
    /// `None`: the first of `TERMINALS` that is installed (`terminal_command`).
    pub terminal: Option<String>,
    /// `None`: no launcher (`launcher = ""`; unset, `EXAMPLE`'s
    /// `fuzzel`).
    pub launcher: Option<String>,
    /// Commands run at startup (when no `-c` is given; unset, `EXAMPLE`'s
    /// `waybar`).
    pub autostart: Vec<String>,
    pub keyboard: KeyboardConfig,
    pub bindings: Vec<Binding>,
    /// Touchpad swipes pila handles (the others reach the clients).
    pub gestures: Vec<(Gesture, Action)>,
    pub outputs: Vec<OutputConfig>,
    pub input: InputConfig,
    /// How many workspaces there are (`workspaces.rs`).
    pub workspaces: usize,
    /// Window corner radius in logical pixels; 0 is square (`rounded.rs`).
    pub corner_radius: u32,
    /// Which of a window's corners it rounds: the top ones (as GTK's own
    /// windows) or all four.
    pub rounded_corners: crate::rounded::Corners,
    /// Room around maximized windows, and around and between snapped ones,
    /// in logical pixels (`snap::side_rect`, `snap::gapped`).
    pub gaps: u32,
    /// Whether monitors on the seat's other GPUs are used, drawn on the
    /// primary and copied over (`udev/secondary.rs`). Off in `EXAMPLE`:
    /// untried on real hardware.
    pub other_gpus: bool,
    /// Whether a client's buffer that is all an output shows (a fullscreen
    /// video or game) goes to the screen as it is, with no frame of Pila's
    /// own (`udev/draw.rs::frame_flags`).
    pub direct_scanout: bool,
    /// Whether a fullscreen window that asks for it (wp-tearing-control: a
    /// game with vsync off) has its frames shown at once, torn, rather than at
    /// the next vblank (`tearing.rs`).
    pub tearing: bool,
    /// Whether a frame is drawn as close to its vblank as it safely can be,
    /// for less latency (`udev/render_late.rs`).
    pub render_late: bool,
    /// Whether Pila writes its own configuration for the tools it starts
    /// where the user has none, at startup (`tool_configs.rs`).
    pub tool_configs: bool,
    pub shadow: ShadowConfig,
    pub colors: Colors,
    /// Which animations play and how long they last (`animation.rs`).
    pub animations: AnimationsConfig,
    /// `[cursor]`: shake to find it.
    pub cursor: CursorConfig,
    /// `[xwayland]`: stopping it when unused.
    pub xwayland: XwaylandConfig,
    /// `[overview]`: how it opens besides its binding (`overview.rs`).
    pub overview: OverviewConfig,
    /// `[permissions]` and `[[permission]]`: screen capture permissions
    /// (`permissions.rs`).
    pub permissions: PermissionsConfig,
    pub permission_rules: Vec<PermissionRule>,
    /// `[[window-rule]]` and `[[layer-rule]]`, in order (`rules.rs`).
    pub window_rules: Vec<crate::rules::WindowRule>,
    pub layer_rules: Vec<crate::rules::LayerRule>,
    /// Mistakes that leave the file valid but surely not doing what was
    /// meant (an `[[output]]` named twice): shown by `--check-config` and in
    /// a notification when it's loaded.
    pub warnings: Vec<String>,
    /// `[environment]`: variables for everything Pila starts, set (`Some`)
    /// or removed (`None`), before Pila's own (`PILA_ENV`).
    pub environment: Vec<(String, Option<String>)>,
}

/// Terminals tried in order when `terminal` isn't set.
pub const TERMINALS: &[&str] = &[
    "kitty",
    "foot",
    "alacritty",
    "ghostty",
    "wezterm",
    "konsole",
    "gnome-terminal",
    "xterm",
];

/// Whether `program` is an executable file in a `PATH` directory.
pub(crate) fn in_path(program: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        std::fs::metadata(dir.join(program))
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    })
}

/// A non-empty `PILA_*` environment variable.
fn env_override(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

impl Default for Config {
    /// `EXAMPLE` as it is.
    fn default() -> Self {
        Self::from_toml("", &|_| None).expect("the example is valid")
    }
}

/// `config.example.toml`: every option, the defaults active and the examples
/// commented out. It's the only place the defaults are written: what a user's
/// file leaves out is taken from it (`Config::parse_with`). Pila writes it to
/// the default path when nothing is there (`create_default_file`).
pub const EXAMPLE: &str = include_str!("../config.example.toml");

/// Write `EXAMPLE` to `path`, creating its directories, unless something is
/// there already (a file, or a symlink, even a broken one). `Ok(true)` when it
/// wrote it.
pub fn create_default_file(path: &Path) -> std::io::Result<bool> {
    create_new_file(path, EXAMPLE)
}

/// `EXAMPLE`'s own `launcher` and `autostart` commands, parsed once.
#[derive(Default, Deserialize)]
struct ExampleCommands {
    #[serde(default)]
    launcher: Option<String>,
    #[serde(default)]
    autostart: Vec<String>,
}

fn example_commands_parsed() -> &'static ExampleCommands {
    static PARSED: std::sync::OnceLock<ExampleCommands> = std::sync::OnceLock::new();
    // Tested to parse (`example_config_is_valid`); the rest is ignored here.
    PARSED.get_or_init(|| toml::from_str(EXAMPLE).unwrap_or_default())
}

/// `EXAMPLE`'s own `autostart` commands: the defaults, which a file made from
/// it (`create_default_file`) holds too, as opposed to commands the user
/// chose (`Pila::autostart_installed`).
pub fn example_autostart() -> &'static [String] {
    &example_commands_parsed().autostart
}

/// Whether `command` is, as written, one of `EXAMPLE`'s own commands: its
/// `launcher` or one of its `autostart` (`tool_configs::with_configs`).
pub fn is_example_command(command: &str) -> bool {
    let example = example_commands_parsed();
    example.launcher.as_deref() == Some(command) || example.autostart.iter().any(|c| c == command)
}

/// Write `contents` to `path`, creating its directories, unless something is
/// there already (a file, or a symlink, even a broken one). `Ok(true)` when it
/// wrote it.
pub fn create_new_file(path: &Path, contents: &str) -> std::io::Result<bool> {
    use std::io::Write;
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    // `create_new` (O_EXCL): never over what's there, a link included, even
    // one whose file isn't there yet (it's the user's).
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(err) => return Err(err),
    };
    file.write_all(contents.as_bytes())?;
    Ok(true)
}

/// The user's configuration directory: `$XDG_CONFIG_HOME`, else `~/.config`.
pub fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
}

impl Config {
    /// The config file pila reads without `--config`.
    pub fn default_path() -> Option<PathBuf> {
        Some(config_home()?.join("pila/config.toml"))
    }

    /// The file pila reads: `explicit` (`--config`) or the default path.
    pub fn path(explicit: Option<&Path>) -> Option<PathBuf> {
        explicit.map(Path::to_path_buf).or_else(Self::default_path)
    }

    /// Read `path` and apply `PILA_*` overrides. A missing file means the
    /// defaults; an unreadable or invalid one is an error.
    pub fn read(path: Option<&Path>, default_mod: ModKey) -> Result<Self, String> {
        let text = match path {
            Some(path) => match std::fs::read_to_string(path) {
                Ok(text) => {
                    info!(path = %path.display(), "loading config");
                    text
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(err) => return Err(format!("can't read {}: {err}", path.display())),
            },
            None => String::new(),
        };
        if text.is_empty() {
            info!("no config file; using the defaults (see config.example.toml)");
        }
        Self::parse(&text, &env_override, default_mod)
    }

    /// `read` at startup: an invalid file is logged and ignored, since a typo
    /// shouldn't leave you without a desktop. The error comes back too, to be
    /// shown once there's a desktop to show it on.
    pub fn load(path: Option<&Path>, default_mod: ModKey) -> (Self, Option<String>) {
        match Self::read(path, default_mod) {
            Ok(config) => (config, None),
            Err(err) => {
                warn!(%err, "invalid config; using defaults");
                let config = Self::parse("", &env_override, default_mod).unwrap_or_default();
                (config, Some(err))
            }
        }
    }

    /// Parse a config file's contents with Alt as the default Mod; `env`
    /// looks up `PILA_*` overrides.
    pub fn from_toml(text: &str, env: &dyn Fn(&str) -> Option<String>) -> Result<Self, String> {
        Self::parse(text, env, ModKey::Alt)
    }

    /// `from_toml` with the Mod to use when none is configured.
    pub fn parse(
        text: &str,
        env: &dyn Fn(&str) -> Option<String>,
        default_mod: ModKey,
    ) -> Result<Self, String> {
        Self::parse_with(text, EXAMPLE, env, default_mod)
    }

    /// `parse` with `example` for the defaults: every setting `text` leaves
    /// out is `example`'s, and the bindings and gestures are `example`'s and
    /// then `text`'s.
    fn parse_with(
        text: &str,
        example: &str,
        env: &dyn Fn(&str) -> Option<String>,
        default_mod: ModKey,
    ) -> Result<Self, String> {
        let file: ConfigFile = toml::from_str(text).map_err(|err| toml_error(text, &err))?;
        let example: ConfigFile = toml::from_str(example).map_err(|err| {
            format!(
                "the built-in config.example.toml: {}",
                toml_error(example, &err)
            )
        })?;

        let mod_key = match env("PILA_MOD").or(file.mod_key).or(example.mod_key) {
            Some(name) => ModKey::parse(&name)?,
            None => default_mod,
        };
        let terminal = env("PILA_TERMINAL").or(file.terminal).or(example.terminal);
        // "" (or blank) is none.
        let launcher = env("PILA_LAUNCHER")
            .or(file.launcher)
            .or(example.launcher)
            .filter(|launcher| !launcher.trim().is_empty());
        // Unset in both: nothing to start.
        let autostart = file.autostart.or(example.autostart).unwrap_or_default();

        let workspaces = in_range(
            need(file.workspaces.or(example.workspaces), "workspaces")?,
            "workspaces",
            1..=MAX_WORKSPACES,
            "",
        )?;
        let corner_radius = in_range(
            need(
                file.corner_radius.or(example.corner_radius),
                "corner-radius",
            )?,
            "corner-radius",
            0..=MAX_CORNER_RADIUS,
            "",
        )?;
        let gaps = in_range(
            need(file.gaps.or(example.gaps), "gaps")?,
            "gaps",
            0..=MAX_GAPS,
            "",
        )?;
        let rounded_corners = match need(
            file.rounded_corners.or(example.rounded_corners),
            "rounded-corners",
        )?
        .as_str()
        {
            "top" => crate::rounded::Corners::Top,
            "all" => crate::rounded::Corners::All,
            other => {
                return Err(format!(
                    "rounded-corners = {other:?}: must be \"top\" or \"all\""
                ))
            }
        };
        let flag = |user: Option<bool>, example: Option<bool>, key| need(user.or(example), key);
        let other_gpus = flag(file.other_gpus, example.other_gpus, "other-gpus")?;
        let direct_scanout = flag(
            file.direct_scanout,
            example.direct_scanout,
            "direct-scanout",
        )?;
        let tearing = flag(file.tearing, example.tearing, "tearing")?;
        let render_late = flag(file.render_late, example.render_late, "render-late")?;
        let tool_configs = flag(file.tool_configs, example.tool_configs, "tool-configs")?;

        let keyboard = file.keyboard.or_example(example.keyboard).validate()?;
        let shadow = file.shadow.or_example(example.shadow).validate()?;
        let colors = file.colors.or_example(example.colors).validate()?;
        let animations = file.animations.or_example(example.animations).validate()?;
        let cursor = file.cursor.or_example(example.cursor).validate()?;
        let xwayland = file.xwayland.or_example(example.xwayland).validate()?;
        let overview = file.overview.or_example(example.overview).validate()?;
        let permissions = file
            .permissions
            .or_example(example.permissions)
            .validate()?;
        let permission_rules = file
            .permission_rules
            .into_iter()
            .enumerate()
            .map(|(i, rule)| rule.validate(i + 1))
            .collect::<Result<Vec<_>, _>>()?;
        let idle_warning = file.idle.warning();
        let wallpaper_warning = file.wallpaper.warning();
        let window_rules = file
            .window_rules
            .into_iter()
            .enumerate()
            .map(|(i, rule)| rule.validate(i + 1, workspaces))
            .collect::<Result<Vec<_>, _>>()?;

        // The example's first, then the user's: a user binding replaces one
        // of the example's for the same combo, and "none" removes it. Mod+N
        // and Mod+Shift+N only exist for the workspaces there are (up to 9),
        // so the other digits still reach the apps. `default-bindings =
        // false` leaves out the example's and the digits, gestures included:
        // only the user's are left (a greeter's compositor, a scheme of one's
        // own).
        let with_defaults = need(
            file.default_bindings.or(example.default_bindings),
            "default-bindings",
        )?;
        let mut bindings: Vec<Binding> = Vec::new();
        let workspace_bindings: Vec<(String, String)> = (1..=workspaces.min(9))
            .flat_map(|n| {
                [
                    (format!("Mod+{n}"), format!("workspace {n}")),
                    (format!("Mod+Shift+{n}"), format!("move-to-workspace {n}")),
                ]
            })
            .collect();
        let defaults = example
            .bindings
            .iter()
            .map(|(k, b)| (k.as_str(), b.action.as_str(), b.repeat))
            .chain(
                workspace_bindings
                    .iter()
                    .map(|(k, a)| (k.as_str(), a.as_str(), false)),
            )
            .filter(|_| with_defaults);
        let user = file
            .bindings
            .iter()
            .map(|(k, b)| (k.as_str(), b.action.as_str(), b.repeat));
        for (written, action, repeat) in defaults.chain(user) {
            let combo = KeyCombo::parse(written, mod_key)?;
            let action =
                Action::parse(action).map_err(|err| format!("binding {written:?}: {err}"))?;
            let held_key = matches!(combo.trigger, Trigger::Key(sym) if tap_sym(sym).is_none());
            if repeat && !held_key {
                return Err(format!(
                    "binding {written:?}: only a key can repeat, not a mouse wheel's notch \
                     or a modifier alone"
                ));
            }
            bindings.retain(|b| b.combo != combo);
            if let Some(action) = action {
                bindings.push(Binding {
                    combo,
                    action,
                    repeat,
                });
            }
        }

        // Same for gestures.
        let mut gestures: Vec<(Gesture, Action)> = Vec::new();
        let defaults = example
            .gestures
            .iter()
            .map(|(g, a)| (g.as_str(), a.as_str()))
            .filter(|_| with_defaults);
        let user = file.gestures.iter().map(|(g, a)| (g.as_str(), a.as_str()));
        for (written, action) in defaults.chain(user) {
            let gesture = Gesture::parse(written)?;
            let action =
                Action::parse(action).map_err(|err| format!("gesture {written:?}: {err}"))?;
            gestures.retain(|(g, _)| *g != gesture);
            if let Some(action) = action {
                gestures.push((gesture, action));
            }
        }

        let outputs = file
            .output
            .into_iter()
            .map(OutputFile::validate)
            .collect::<Result<Vec<_>, _>>()?;
        let mut warnings = Vec::new();
        // The example's, then the user's over them.
        let mut entries = example.environment;
        entries.extend(file.environment);
        let environment = environment(entries, &mut warnings)?;
        warnings.extend(wallpaper_warning);
        warnings.extend(idle_warning);
        for (i, output) in outputs.iter().enumerate() {
            // Once a name, at its second section.
            let before = outputs[..i].iter().filter(|o| o.name == output.name);
            if before.count() == 1 {
                warnings.push(format!(
                    "[[output]] {:?} is there more than once: only the first is used, put its settings in one",
                    output.name
                ));
            }
        }

        // Each device's options one by one: an unset one is the example's.
        let pointer = |user: Option<PointerConfig>, example: Option<PointerConfig>, section| {
            let config = match (user, example) {
                (Some(user), Some(example)) => user.or_example(example),
                (user, example) => need(user.or(example), &format!("[input.{section}]"))?,
            };
            config.validate(section)
        };
        let input = InputConfig {
            touchpad: pointer(file.input.touchpad, example.input.touchpad, "touchpad")?,
            mouse: pointer(file.input.mouse, example.input.mouse, "mouse")?,
            wheel_binding_cooldown: in_range(
                need(
                    file.input
                        .wheel_binding_cooldown
                        .or(example.input.wheel_binding_cooldown),
                    "input.wheel-binding-cooldown",
                )?,
                "input.wheel-binding-cooldown",
                0..=MAX_WHEEL_BINDING_COOLDOWN,
                "ms",
            )?,
        };

        Ok(Self {
            mod_key,
            default_mod,
            terminal,
            launcher,
            autostart,
            keyboard,
            bindings,
            gestures,
            outputs,
            input,
            workspaces,
            corner_radius,
            rounded_corners,
            gaps,
            other_gpus,
            direct_scanout,
            tearing,
            render_late,
            tool_configs,
            shadow,
            colors,
            animations,
            cursor,
            xwayland,
            overview,
            permissions,
            permission_rules,
            window_rules,
            layer_rules: file.layer_rules,
            warnings,
            environment,
        })
    }

    /// The terminal to run: the configured one, else the first of
    /// `TERMINALS` that is installed (`None` if none is).
    pub fn terminal_command(&self) -> Option<String> {
        if let Some(terminal) = &self.terminal {
            return Some(terminal.clone());
        }
        let found = TERMINALS.iter().find(|t| in_path(t)).map(|t| t.to_string());
        if found.is_none() {
            warn!(
                tried = ?TERMINALS,
                "no terminal installed; set `terminal` in the config"
            );
        }
        found
    }

    /// The `[[output]]` settings for the output called `name` showing
    /// `monitor`, if any: the most specific section, the monitor itself
    /// before its model before the connector.
    pub fn output(&self, name: &str, monitor: Option<&Monitor>) -> Option<&OutputConfig> {
        let by_monitor = |matches: fn(&Monitor, &str) -> bool| {
            let monitor = monitor?;
            self.outputs.iter().find(|o| matches(monitor, &o.name))
        };
        by_monitor(Monitor::is_exactly)
            .or_else(|| by_monitor(Monitor::is_model))
            .or_else(|| self.outputs.iter().find(|o| o.name == name))
    }

    /// The action for a swipe, if pila handles it.
    pub fn gesture_action(&self, gesture: Gesture) -> Option<Action> {
        self.gestures
            .iter()
            .find(|(g, _)| *g == gesture)
            .map(|(_, action)| action.clone())
    }

    /// Whether pila handles any swipe with this many fingers (then it keeps
    /// the whole gesture from the clients).
    pub fn handles_swipes(&self, fingers: u32) -> bool {
        self.gestures.iter().any(|(g, _)| g.fingers == fingers)
    }

    /// The action bound to `sym` with exactly the pressed modifiers.
    ///
    /// `sym` is the unshifted (latin) keysym, so Shift comes from `mods`.
    pub fn action_for(&self, sym: Keysym, mods: &ModifiersState) -> Option<Action> {
        // A modifier's press is never a binding: a tap runs on its release.
        if tap_sym(sym).is_some() {
            return None;
        }
        self.bound(KeyCombo {
            mods: mods.into(),
            trigger: Trigger::Key(sym),
        })
    }

    /// Whether the binding of `sym` with exactly the pressed modifiers runs
    /// again while the key is held (`repeat = true`).
    pub fn repeats(&self, sym: Keysym, mods: &ModifiersState) -> bool {
        let combo = KeyCombo {
            mods: mods.into(),
            trigger: Trigger::Key(sym),
        };
        self.bindings.iter().any(|b| b.combo == combo && b.repeat)
    }

    /// The action bound to a wheel step that way with exactly the pressed
    /// modifiers.
    pub fn wheel_action(&self, direction: WheelDirection, mods: &ModifiersState) -> Option<Action> {
        self.bound(KeyCombo {
            mods: mods.into(),
            trigger: Trigger::Wheel(direction),
        })
    }

    /// The action of tapping the modifier key `sym` alone, `mods` being the
    /// modifiers its press left (its own).
    pub fn tap_action(&self, sym: Keysym, mods: &ModifiersState) -> Option<Action> {
        self.bound(KeyCombo {
            mods: mods.into(),
            trigger: Trigger::Key(tap_sym(sym)?),
        })
    }

    fn bound(&self, combo: KeyCombo) -> Option<Action> {
        let action = self
            .bindings
            .iter()
            .find(|b| b.combo == combo)
            .map(|b| b.action.clone())?;
        // Mod+D does nothing without a launcher (and isn't swallowed).
        match action {
            Action::Launcher if self.launcher.is_none() => None,
            action => Some(action),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory for a test's files, removed when it's done.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("pila-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_default_file_is_the_example_and_its_directories_are_made() {
        let dir = TempDir::new("default-file");
        let path = dir.0.join("config/pila/config.toml");
        assert!(create_default_file(&path).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), EXAMPLE);
    }

    #[test]
    fn the_default_file_never_replaces_what_is_there() {
        let dir = TempDir::new("default-file-kept");
        let path = dir.0.join("config.toml");
        std::fs::write(&path, "mod = \"ctrl\"\n").unwrap();
        assert!(!create_default_file(&path).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "mod = \"ctrl\"\n");

        // A dotfiles manager's link to a file that isn't there (yet).
        let link = dir.0.join("linked.toml");
        std::os::unix::fs::symlink(dir.0.join("dotfiles/pila.toml"), &link).unwrap();
        assert!(!create_default_file(&link).unwrap());
        assert!(!dir.0.join("dotfiles").exists());
    }

    #[test]
    fn a_default_file_that_cant_be_written_is_an_error() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("default-file-ro");
        std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0o555)).unwrap();
        // Root writes anyway: nothing to test then.
        if std::fs::write(dir.0.join("probe"), "").is_ok() {
            return;
        }
        assert!(create_default_file(&dir.0.join("pila/config.toml")).is_err());
    }

    /// `config.example.toml` lists every action (its comment above
    /// `[bindings]`), so a new one can't be left out of it.
    #[test]
    fn every_action_is_in_the_example_config() {
        let example = EXAMPLE;
        let names = NAMED_ACTIONS
            .iter()
            .map(|(name, _)| *name)
            .chain(ACTIONS_WITH_ARGUMENT.iter().copied());
        for name in names {
            assert!(
                example.contains(name),
                "config.example.toml doesn't list {name:?}"
            );
        }
    }

    #[test]
    fn every_named_action_parses_to_itself() {
        for (name, action) in NAMED_ACTIONS {
            assert_eq!(Action::parse(name), Ok(Some(action.clone())), "{name}");
        }
        assert_eq!(Action::parse("none"), Ok(None));
        let err = Action::parse("explode").unwrap_err();
        assert!(err.contains("spawn <command>, terminal,") && err.ends_with("quit, confirm-quit, focus-output <next|prev>, move-to-output <next|prev>, workspace <n|next|prev>, move-to-workspace <n|next|prev>, switch-layout <n|next|prev>, snap <left|right|up|down>, overview <open|close> or none)"), "{err}");
        assert_eq!(
            Action::parse("snap up"),
            Ok(Some(Action::SnapToward(Direction::Up)))
        );
        let err = Action::parse("snap sideways").unwrap_err();
        assert!(err.contains("must be left, right, up or down"), "{err}");
    }

    #[test]
    fn the_examples_are_one_of_each_kind_of_action() {
        let kinds: std::collections::HashSet<_> = Action::examples()
            .iter()
            .map(std::mem::discriminant)
            .collect();
        assert_eq!(kinds.len(), Action::examples().len());
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn mods(ctrl: bool, alt: bool, shift: bool, logo: bool) -> ModifiersState {
        ModifiersState {
            ctrl,
            alt,
            shift,
            logo,
            ..Default::default()
        }
    }

    #[test]
    fn defaults_bind_mod_return_to_the_terminal() {
        let config = Config::from_toml("", &no_env).unwrap();
        assert_eq!(config.mod_key, ModKey::Alt);
        assert_eq!(config.terminal, None);
        let alt = mods(false, true, false, false);
        assert_eq!(
            config.action_for(Keysym::Return, &alt),
            Some(Action::Terminal)
        );
        let alt_shift = mods(false, true, true, false);
        assert_eq!(
            config.action_for(Keysym::q, &alt_shift),
            Some(Action::CloseFocused)
        );
    }

    #[test]
    fn modifiers_must_match_exactly() {
        let config = Config::from_toml("", &no_env).unwrap();
        // Ctrl+Alt+Return isn't Mod+Return; plain Return isn't bound.
        let ctrl_alt = mods(true, true, false, false);
        assert_eq!(config.action_for(Keysym::Return, &ctrl_alt), None);
        assert_eq!(
            config.action_for(Keysym::Return, &ModifiersState::default()),
            None
        );
    }

    #[test]
    fn caps_lock_does_not_break_bindings() {
        let config = Config::from_toml("", &no_env).unwrap();
        let alt_caps = ModifiersState {
            alt: true,
            caps_lock: true,
            ..Default::default()
        };
        assert_eq!(
            config.action_for(Keysym::m, &alt_caps),
            Some(Action::ToggleMaximize)
        );
    }

    #[test]
    fn launcher_and_autostart_left_out_are_the_examples() {
        // As every setting: fuzzel, swaybg, waybar, mako and swayidle come
        // from config.example.toml, not the code.
        let config = Config::from_toml("", &no_env).unwrap();
        assert_eq!(config.launcher.as_deref(), Some("fuzzel"));
        let programs: Vec<_> = config
            .autostart
            .iter()
            .filter_map(|command| command.split_whitespace().next())
            .collect();
        assert_eq!(programs, ["swaybg", "waybar", "mako", "swayidle"]);
        // swaybg alone: Pila gives it its wallpaper (`tool_configs.rs`).
        assert_eq!(config.autostart[0], "swaybg");
        assert!(config.autostart[3].contains("'swaylock -f'"));
        assert!(config.autostart[3].contains("'pila msg action power-off-monitors'"));
        // "" (or blank) is no launcher, and [] starts nothing; PILA_LAUNCHER
        // still wins over the file.
        for text in [r#"launcher = """#, r#"launcher = "  ""#] {
            let config = Config::from_toml(text, &no_env).unwrap();
            assert_eq!(config.launcher, None, "{text}");
        }
        let config = Config::from_toml("autostart = []", &no_env).unwrap();
        assert!(config.autostart.is_empty());
        let env = |name: &str| (name == "PILA_LAUNCHER").then(|| "fuzzel".to_string());
        let config = Config::from_toml(r#"launcher = """#, &env).unwrap();
        assert_eq!(config.launcher.as_deref(), Some("fuzzel"));
    }

    #[test]
    fn environment_has_wayland_defaults_a_file_can_remove() {
        let value = |config: &Config, name: &str| {
            config
                .environment
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, value)| value.clone())
        };
        let defaults = Config::default();
        for (name, expected) in [
            ("QT_QPA_PLATFORM", "wayland;xcb"),
            ("ELECTRON_OZONE_PLATFORM_HINT", "auto"),
            ("MOZ_ENABLE_WAYLAND", "1"),
            ("_JAVA_AWT_WM_NONREPARENTING", "1"),
        ] {
            assert_eq!(
                value(&defaults, name),
                Some(Some(expected.to_string())),
                "{name}"
            );
        }
        // A file's own value wins; false removes a default.
        let config = Config::from_toml(
            "[environment]\nQT_QPA_PLATFORM = \"xcb\"\nMOZ_ENABLE_WAYLAND = false\n",
            &no_env,
        )
        .unwrap();
        assert_eq!(value(&config, "QT_QPA_PLATFORM"), Some(Some("xcb".into())));
        assert_eq!(value(&config, "MOZ_ENABLE_WAYLAND"), Some(None));
        assert_eq!(
            value(&config, "ELECTRON_OZONE_PLATFORM_HINT"),
            Some(Some("auto".into()))
        );
    }

    #[test]
    fn environment_sets_removes_and_leaves_pilas_own() {
        let config = Config::from_toml(
            "[environment]\nLANGUAGE = \"zh_CN:zh\"\nQT_QPA_PLATFORMTHEME = false\nEMPTY = \"\"\n",
            &no_env,
        )
        .unwrap();
        let user = |config: &Config| -> Vec<(String, Option<String>)> {
            let defaults = Config::default().environment;
            config
                .environment
                .iter()
                .filter(|entry| !defaults.contains(entry))
                .cloned()
                .collect()
        };
        assert_eq!(
            user(&config),
            [
                ("EMPTY".to_string(), Some(String::new())),
                ("LANGUAGE".to_string(), Some("zh_CN:zh".to_string())),
                ("QT_QPA_PLATFORMTHEME".to_string(), None),
            ]
        );
        assert!(config.warnings.is_empty());
        // Pila's own: left out, with a warning, not an error.
        for name in PILA_ENV {
            let text = format!("[environment]\n{name} = \"x\"\nKEPT = \"y\"\n");
            let config = Config::from_toml(&text, &no_env).unwrap();
            assert_eq!(user(&config), [("KEPT".to_string(), Some("y".to_string()))]);
            assert_eq!(config.warnings.len(), 1, "{:?}", config.warnings);
            assert!(config.warnings[0].contains(name), "{:?}", config.warnings);
        }
        let err = |text: &str| Config::from_toml(text, &no_env).unwrap_err();
        assert!(err("[environment]\n\"A=B\" = \"x\"").contains("can't be empty or have"));
        assert!(err("[environment]\n\"\" = \"x\"").contains("can't be empty or have"));
        assert!(err("[environment]\nA = true").contains("a value is a string"));
        assert!(err("[environment]\nA = 3").contains("a string (the variable's value)"));
    }

    #[test]
    fn launcher_binding_needs_a_launcher() {
        let alt = mods(false, true, false, false);
        let none = Config::from_toml(r#"launcher = """#, &no_env).unwrap();
        assert_eq!(none.action_for(Keysym::d, &alt), None);
        let some = Config::from_toml(r#"launcher = "fuzzel""#, &no_env).unwrap();
        assert_eq!(some.action_for(Keysym::d, &alt), Some(Action::Launcher));
    }

    #[test]
    fn a_modifier_alone_is_a_tap() {
        let logo = mods(false, false, false, true);
        let alt = mods(false, true, false, false);
        let ctrl = mods(true, false, false, false);
        let config = Config::from_toml(
            r#"
            mod = "super"
            launcher = "fuzzel"
            [bindings]
            "Alt" = "terminal"
            "ctrl" = "maximize"
            "#,
            &no_env,
        )
        .unwrap();
        // Never on the press, whichever side.
        assert_eq!(config.action_for(Keysym::Super_L, &logo), None);
        assert_eq!(config.action_for(Keysym::Super_R, &logo), None);
        assert_eq!(
            config.tap_action(Keysym::Super_R, &logo),
            Some(Action::Launcher)
        );
        assert_eq!(
            config.tap_action(Keysym::Alt_R, &alt),
            Some(Action::Terminal)
        );
        assert_eq!(
            config.tap_action(Keysym::Control_L, &ctrl),
            Some(Action::ToggleMaximize)
        );
        // With another modifier held, it isn't a tap.
        assert_eq!(
            config.tap_action(Keysym::Super_L, &mods(false, false, true, true)),
            None
        );
        assert_eq!(config.tap_action(Keysym::d, &logo), None);

        // "Mod" follows `mod`; "none" removes it; without a launcher the
        // default does nothing.
        let nested = Config::from_toml(r#"launcher = "fuzzel""#, &no_env).unwrap();
        assert_eq!(nested.mod_key, ModKey::Alt);
        assert_eq!(
            nested.tap_action(Keysym::Alt_L, &alt),
            Some(Action::Launcher)
        );
        assert_eq!(nested.tap_action(Keysym::Super_L, &logo), None);
        let unbound = Config::from_toml(
            "launcher = \"fuzzel\"\n[bindings]\n\"Mod\" = \"none\"\n",
            &no_env,
        )
        .unwrap();
        assert_eq!(unbound.tap_action(Keysym::Alt_L, &alt), None);
        let no_launcher = Config::from_toml(r#"launcher = """#, &no_env).unwrap();
        assert_eq!(no_launcher.tap_action(Keysym::Alt_L, &alt), None);

        // Shift isn't a tap, and a modifier as the key of a combo is no key.
        for text in ["Shift", "Mod+Alt", "Mod+Super"] {
            assert!(KeyCombo::parse(text, ModKey::Alt).is_err(), "{text}");
        }
    }

    #[test]
    fn a_key_by_its_code_is_the_one_the_keyboard_gives() {
        // 0x41 is XK_A; the keyboard filter compares unshifted keysyms.
        let combo = KeyCombo::parse("Ctrl+0x41", ModKey::Alt).unwrap();
        assert_eq!(combo.trigger, Trigger::Key(Keysym::a));
        // A NUL in a name (TOML's "\u0000"): an error, not a crash.
        assert!(KeyCombo::parse("Ctrl+a\0", ModKey::Alt).is_err());
    }

    #[test]
    fn a_binding_can_repeat() {
        let none = ModifiersState::default();
        let config = Config::from_toml(
            "[bindings]\n\
             \"XF86AudioRaiseVolume\" = { action = \"spawn up\", repeat = true }\n\
             \"XF86AudioLowerVolume\" = { action = \"spawn down\" }\n\
             \"XF86AudioMute\" = \"spawn mute\"\n",
            &no_env,
        )
        .unwrap();
        let up = Keysym::XF86_AudioRaiseVolume;
        assert_eq!(
            config.action_for(up, &none),
            Some(Action::Spawn("up".into()))
        );
        assert!(config.repeats(up, &none));
        assert!(!config.repeats(Keysym::XF86_AudioLowerVolume, &none));
        assert!(!config.repeats(Keysym::XF86_AudioMute, &none));

        // "none" removes a default binding, and with it any repeat.
        let alt = mods(false, true, false, false);
        let defaults = Config::from_toml("", &no_env).unwrap();
        assert_eq!(
            defaults.action_for(Keysym::Tab, &alt),
            Some(Action::CycleFocus)
        );
        let removed = Config::from_toml(
            "[bindings]\n\"Alt+Tab\" = { action = \"none\", repeat = true }\n",
            &no_env,
        )
        .unwrap();
        assert_eq!(removed.action_for(Keysym::Tab, &alt), None);
        assert!(!removed.repeats(Keysym::Tab, &alt));

        // A wheel's notch or a tap has no held key to repeat.
        for combo in ["Mod+WheelScrollUp", "Mod"] {
            let text =
                format!("[bindings]\n\"{combo}\" = {{ action = \"close\", repeat = true }}\n");
            let err = Config::from_toml(&text, &no_env).unwrap_err();
            assert!(err.contains("only a key can repeat"), "{combo}: {err}");
        }
        let err = Config::from_toml(
            "[bindings]\n\"Mod+X\" = { action = \"close\", repat = true }\n",
            &no_env,
        )
        .unwrap_err();
        assert!(err.contains("unknown field `repat`"), "{err}");
    }

    #[test]
    fn file_sets_mod_key_and_user_bindings() {
        let config = Config::from_toml(
            r#"
            mod = "super"
            terminal = "kgx"
            autostart = ["waybar", "mako"]

            [keyboard]
            layout = "us,es"
            options = "grp:alt_shift_toggle"

            [bindings]
            "Mod+Shift+Return" = "spawn foot -e htop"
            "Mod+J" = "none"
            "XF86AudioRaiseVolume" = "spawn wpctl set-volume @DEFAULT_SINK@ 5%+"
            "#,
            &no_env,
        )
        .unwrap();
        let logo = mods(false, false, false, true);
        let logo_shift = mods(false, false, true, true);
        assert_eq!(config.mod_key, ModKey::Super);
        assert_eq!(config.terminal.as_deref(), Some("kgx"));
        assert_eq!(config.terminal_command().as_deref(), Some("kgx"));
        assert_eq!(config.autostart, ["waybar", "mako"]);
        assert_eq!(config.keyboard.layout, "us,es");
        assert_eq!(config.keyboard.repeat_rate, 25);
        assert_eq!(
            config.action_for(Keysym::Return, &logo),
            Some(Action::Terminal)
        );
        assert_eq!(
            config.action_for(Keysym::Return, &logo_shift),
            Some(Action::Spawn("foot -e htop".into()))
        );
        assert_eq!(config.action_for(Keysym::j, &logo), None);
        assert_eq!(
            config.action_for(Keysym::Tab, &logo),
            Some(Action::CycleFocus)
        );
        assert!(matches!(
            config.action_for(Keysym::XF86_AudioRaiseVolume, &ModifiersState::default()),
            Some(Action::Spawn(_))
        ));
    }

    #[test]
    fn user_binding_replaces_default() {
        let config =
            Config::from_toml(r#"bindings = { "Mod+M" = "fullscreen" }"#, &no_env).unwrap();
        let alt = mods(false, true, false, false);
        assert_eq!(
            config.action_for(Keysym::m, &alt),
            Some(Action::ToggleFullscreen)
        );
    }

    #[test]
    fn environment_overrides_the_file() {
        let env = |name: &str| (name == "PILA_MOD").then(|| "ctrl".to_string());
        let config = Config::from_toml(r#"mod = "super""#, &env).unwrap();
        assert_eq!(config.mod_key, ModKey::Ctrl);
        // Default bindings follow the effective Mod key.
        let ctrl = mods(true, false, false, false);
        assert_eq!(
            config.action_for(Keysym::Return, &ctrl),
            Some(Action::Terminal)
        );
    }

    #[test]
    fn outputs_are_parsed_and_scales_rounded() {
        let config = Config::from_toml(
            r#"
            [[output]]
            name = "eDP-1"
            scale = 1.33
            [[output]]
            name = "DP-1"
            mode = "2560x1440@59.95"
            position = [1440, 0]
            "#,
            &no_env,
        )
        .unwrap();
        let edp = config.output("eDP-1", None).unwrap();
        assert_eq!(edp.scale, Some(160.0 / 120.0));
        assert_eq!(edp.mode, None);
        let dp = config.output("DP-1", None).unwrap();
        assert_eq!(dp.position, Some((1440, 0)));
        assert_eq!(
            dp.mode,
            Some(ModeSpec {
                width: 2560,
                height: 1440,
                refresh_hz: Some(59.95)
            })
        );
        assert!(config.output("HDMI-A-1", None).is_none());
        assert_eq!(dp.transform, None);
        assert_eq!(dp.enabled, None);
        assert_eq!(dp.adaptive_sync, None);
    }

    #[test]
    fn an_output_placed_out_of_range_is_an_error() {
        let config = |position: &str| {
            Config::from_toml(
                &format!("[[output]]\nname = \"DP-1\"\nposition = {position}\n"),
                &no_env,
            )
        };
        let edge = format!("[{MAX_OUTPUT_POSITION}, -{MAX_OUTPUT_POSITION}]");
        assert!(config(&edge).is_ok());
        let err = config(&format!("[{}, 0]", i32::MAX - 10)).unwrap_err();
        assert!(err.contains("DP-1") && err.contains("position"), "{err}");
        assert!(config(&format!("[0, -{}]", MAX_OUTPUT_POSITION + 1)).is_err());
    }

    #[test]
    fn outputs_can_be_disabled_and_overridden() {
        let config = Config::from_toml(
            "[[output]]\nname = \"DP-1\"\nenabled = false\nadaptive-sync = true\nscale = 2",
            &no_env,
        )
        .unwrap();
        let dp = config.output("DP-1", None).unwrap();
        assert_eq!(dp.enabled, Some(false));
        assert_eq!(dp.adaptive_sync, Some(true));
        let over = OutputConfig {
            enabled: Some(true),
            position: Some((10, 20)),
            ..OutputConfig::named("DP-1")
        };
        let effective = dp.overridden_by(&over);
        assert_eq!(effective.enabled, Some(true));
        assert_eq!(effective.position, Some((10, 20)));
        assert_eq!(effective.scale, Some(2.0));
        assert_eq!(effective.adaptive_sync, Some(true));
    }

    #[test]
    fn outputs_can_be_rotated() {
        let config = Config::from_toml(
            "[[output]]\nname = \"DP-1\"\ntransform = \"90\"\n\
             [[output]]\nname = \"HDMI-A-1\"\ntransform = \"flipped-270\"",
            &no_env,
        )
        .unwrap();
        assert_eq!(
            config.output("DP-1", None).unwrap().transform,
            Some(Transform::_90)
        );
        assert_eq!(
            config.output("HDMI-A-1", None).unwrap().transform,
            Some(Transform::Flipped270)
        );
        let err = Config::from_toml("[[output]]\nname = \"DP-1\"\ntransform = \"45\"", &no_env)
            .unwrap_err();
        assert!(err.contains("unknown transform"));
    }

    #[test]
    fn input_settings_keep_defaults_for_what_is_unset() {
        let config = Config::from_toml("", &no_env).unwrap();
        assert_eq!(config.input.touchpad.tap, Some(true));
        assert_eq!(config.input.touchpad.natural_scroll, None);
        assert_eq!(config.input.mouse, PointerConfig::default());

        let config = Config::from_toml(
            r#"
            [input.touchpad]
            natural-scroll = true
            accel-speed = 0.3
            click-method = "clickfinger"
            disable-while-typing = true
            [input.mouse]
            accel-profile = "flat"
            "#,
            &no_env,
        )
        .unwrap();
        let touchpad = &config.input.touchpad;
        assert_eq!(touchpad.tap, Some(true));
        assert_eq!(touchpad.natural_scroll, Some(true));
        assert_eq!(touchpad.accel_speed, Some(0.3));
        assert_eq!(touchpad.click_method, Some(ClickMethod::Clickfinger));
        assert_eq!(touchpad.disable_while_typing, Some(true));
        assert_eq!(config.input.mouse.accel_profile, Some(AccelProfile::Flat));

        let no_tap = Config::from_toml("[input.touchpad]\ntap = false", &no_env).unwrap();
        assert_eq!(no_tap.input.touchpad.tap, Some(false));
    }

    #[test]
    fn wheel_binding_cooldown_is_0_to_1000_ms() {
        for ms in [0, 1000] {
            let text = format!("[input]\nwheel-binding-cooldown = {ms}");
            let config = Config::from_toml(&text, &no_env).unwrap();
            assert_eq!(config.input.wheel_binding_cooldown, ms);
        }
        let err = Config::from_toml("[input]\nwheel-binding-cooldown = 1001", &no_env).unwrap_err();
        assert!(err.contains("wheel-binding-cooldown"), "{err}");
    }

    #[test]
    fn an_output_named_twice_is_a_warning_once() {
        let config = Config::from_toml(
            "[[output]]\nname = \"DP-1\"\nscale = 1.5\n\
             [[output]]\nname = \"DP-1\"\nmode = \"2560x1440@60\"\n\
             [[output]]\nname = \"DP-1\"\n\
             [[output]]\nname = \"eDP-1\"\n",
            &no_env,
        )
        .unwrap();
        assert_eq!(config.warnings.len(), 1, "{:?}", config.warnings);
        assert!(config.warnings[0].contains("\"DP-1\""));
        // As before: the first section is the one used.
        assert_eq!(config.output("DP-1", None).unwrap().scale, Some(1.5));
        assert!(Config::from_toml("", &no_env).unwrap().warnings.is_empty());
    }

    #[test]
    fn modes_pick_size_then_refresh() {
        let modes = [
            ((2560, 1440), 59_951),
            ((2560, 1440), 143_912),
            ((1920, 1080), 60_000),
            ((2560, 1440), 120_000),
        ];
        let spec = |text| ModeSpec::parse(text).unwrap();
        assert_eq!(spec("2560x1440@120").pick(&modes, None), Some(3));
        assert_eq!(spec("2560x1440@60").pick(&modes, None), Some(0));
        // No refresh: the preferred mode if it fits, else the fastest.
        assert_eq!(spec("2560x1440").pick(&modes, Some(0)), Some(0));
        assert_eq!(spec("2560x1440").pick(&modes, Some(2)), Some(1));
        assert_eq!(spec("1920x1080").pick(&modes, None), Some(2));
        assert_eq!(spec("800x600").pick(&modes, None), None);
    }

    #[test]
    fn screenshots_are_bound_by_default() {
        let config = Config::from_toml("", &no_env).unwrap();
        let alt_shift = mods(false, true, true, false);
        let alt_ctrl = mods(true, true, false, false);
        let spawned = |action: Option<Action>| match action {
            Some(Action::Spawn(command)) => command,
            other => panic!("not a spawn: {other:?}"),
        };
        assert_eq!(
            spawned(config.action_for(Keysym::s, &alt_shift)),
            "grim -g \"$(slurp)\" - | wl-copy"
        );
        assert!(spawned(config.action_for(Keysym::s, &alt_ctrl))
            .contains("grim -o \"$PILA_OUTPUT\" ~/Pictures/screenshot-"));
    }

    #[test]
    fn the_default_tools_are_ones_a_file_can_swap() {
        let spawned = |config: &Config| -> Vec<String> {
            config
                .bindings
                .iter()
                .filter_map(|b| match &b.action {
                    Action::Spawn(command) => Some(command.clone()),
                    _ => None,
                })
                .collect()
        };
        const TOOLS: [&str; 8] = [
            "fuzzel",
            "waybar",
            "wpctl",
            "brightnessctl",
            "playerctl",
            "grim",
            "slurp",
            "wl-copy",
        ];
        let named = |config: &Config| -> Vec<String> {
            let mut named: Vec<String> = spawned(config);
            named.extend(config.launcher.clone());
            named.extend(config.autostart.clone());
            named.retain(|command| TOOLS.iter().any(|tool| command.contains(tool)));
            named
        };
        // By default, every one of them: standard tools, none of Pila's own.
        let defaults = Config::from_toml("", &no_env).unwrap();
        let all = named(&defaults).join("\n");
        for tool in TOOLS {
            assert!(all.contains(tool), "{tool} isn't a default: {all}");
        }
        assert!(!all.contains("pila-"), "{all}");
        // One key of its own: that key's tool goes, the others stay.
        let own = Config::from_toml(
            "[bindings]\n\"Mod+Shift+S\" = \"spawn flameshot gui\"\n",
            &no_env,
        )
        .unwrap();
        let alt_shift = mods(false, true, true, false);
        assert_eq!(
            own.action_for(Keysym::s, &alt_shift),
            Some(Action::Spawn("flameshot gui".into()))
        );
        assert!(spawned(&own).iter().any(|c| c.contains("playerctl")));
        // All of them swapped: none is left, and the file still checks.
        let swapped = Config::from_toml(
            "launcher = \"wofi --show drun\"\nautostart = [\"yambar\"]\ndefault-bindings = false\n",
            &no_env,
        )
        .unwrap();
        assert_eq!(named(&swapped), Vec::<String>::new());
    }

    #[test]
    fn colors_are_premultiplied_and_keep_their_defaults() {
        let config = Config::from_toml(
            "[colors]\nbar = \"#ff0000\"\nsnap-preview = \"#ff000080\"\n",
            &no_env,
        )
        .unwrap();
        assert_eq!(config.colors.bar, [1.0, 0.0, 0.0, 1.0]);
        let alpha = 128.0 / 255.0;
        assert_eq!(config.colors.snap_preview, [alpha, 0.0, 0.0, alpha]);
        // The others are the example's.
        assert_eq!(config.colors.icon, Config::default().colors.icon);
    }

    #[test]
    fn a_bad_color_says_which_and_why() {
        for (text, wanted) in [
            ("[colors]\nbar = \"red\"", "colors.bar"),
            ("[colors]\nbar = \"#ff0000aa\"", "#rrggbb"),
            ("[colors]\nicon = \"#ggg000\"", "colors.icon"),
            ("[colors]\nhover = \"#ñ00000\"", "colors.hover"),
            ("[shadow]\ncolor = \"#12\"", "shadow.color"),
        ] {
            let error = Config::from_toml(text, &no_env).unwrap_err();
            assert!(error.contains(wanted), "{text}: {error}");
        }
    }

    #[test]
    fn shadows_are_on_by_default_and_bounded() {
        let shadow = Config::from_toml("", &no_env).unwrap().shadow;
        assert!(shadow.enabled);
        assert!(Config::from_toml("[shadow]\nsize = 65", &no_env).is_err());
        assert!(Config::from_toml("[shadow]\nsize = -1", &no_env).is_err());
        assert!(Config::from_toml("[shadow]\noffset = [0, 65]", &no_env).is_err());
    }

    #[test]
    fn the_cursor_theme_and_size_are_optional_and_bounded() {
        let cursor = Config::from_toml("", &no_env).unwrap().cursor;
        assert_eq!((cursor.theme, cursor.size), (None, None));
        let cursor = Config::from_toml("[cursor]\ntheme = \"Adwaita\"\nsize = 32", &no_env)
            .unwrap()
            .cursor;
        assert_eq!(cursor.theme.as_deref(), Some("Adwaita"));
        assert_eq!(cursor.size, Some(32));
        for (text, wanted) in [
            ("[cursor]\ntheme = \" \"", "cursor.theme is empty"),
            ("[cursor]\nsize = 4", "cursor.size = 4: must be 8 to 256"),
            ("[cursor]\nsize = 300", "cursor.size = 300"),
        ] {
            let err = Config::from_toml(text, &no_env).unwrap_err();
            assert!(err.contains(wanted), "{text:?}: {err}");
        }
    }

    #[test]
    fn default_bindings_false_leaves_only_the_users() {
        let config = Config::from_toml(
            "default-bindings = false\n[bindings]\n\"Mod+x\" = \"close\"\n\
             [gestures]\n\"swipe-3-up\" = \"maximize\"\n",
            &no_env,
        )
        .unwrap();
        assert_eq!(config.bindings.len(), 1);
        assert_eq!(config.bindings[0].action, Action::CloseFocused);
        assert_eq!(config.gestures.len(), 1);
        let alt = mods(false, true, false, false);
        assert_eq!(config.action_for(Keysym::Return, &alt), None);
        assert_eq!(config.action_for(Keysym::_1, &alt), None);
        // `true` is the default.
        let all = Config::from_toml("default-bindings = true", &no_env).unwrap();
        assert_eq!(
            all.bindings,
            Config::from_toml("", &no_env).unwrap().bindings
        );
    }

    #[test]
    fn shake_to_find_is_on_by_default_and_bounded() {
        let cursor = Config::from_toml("", &no_env).unwrap().cursor;
        assert!(cursor.shake_to_find);
        let config = Config::from_toml(
            "[cursor]\nshake-to-find = false\nshake-magnification = 2.0",
            &no_env,
        )
        .unwrap();
        assert!(!config.cursor.shake_to_find);
        assert_eq!(config.cursor.shake_magnification, 2.0);
        for text in ["shake-magnification = 1.0", "shake-magnification = 6.0"] {
            let error = Config::from_toml(&format!("[cursor]\n{text}"), &no_env).unwrap_err();
            assert!(error.contains("cursor.shake-magnification"), "{error}");
        }
        assert!(Config::from_toml("[cursor]\nshake-to-find = 1", &no_env).is_err());
    }

    #[test]
    fn idle_monitors_off_is_left_to_swayidle_with_a_warning() {
        // Not in the example: no warning for a file without it.
        assert!(Config::from_toml("", &no_env).unwrap().warnings.is_empty());
        let idle = |text: &str| Config::from_toml(&format!("[idle]\n{text}"), &no_env);
        // `false` asked for what Pila does now: nothing to say.
        assert!(idle("monitors-off = false").unwrap().warnings.is_empty());
        for text in [
            "monitors-off = 600",
            "monitors-off = 0",
            "monitors-off = true",
        ] {
            let config = idle(text).unwrap();
            assert_eq!(config.warnings.len(), 1, "{text}: {:?}", config.warnings);
            let warning = &config.warnings[0];
            assert!(
                warning.contains("idle.monitors-off") && warning.contains("swayidle"),
                "{text}: {warning}"
            );
        }
        assert!(idle("monitors-off = \"soon\"").is_err());
        assert!(idle("lock = 300").is_err());
    }

    #[test]
    fn the_wallpaper_is_left_to_swaybg_with_a_warning() {
        let wallpaper = |text: &str| Config::from_toml(&format!("[wallpaper]\n{text}"), &no_env);
        // No image, nothing lost: nothing to say.
        for text in ["", "image = \"\"", "fit = \"fit\""] {
            let config = wallpaper(text).unwrap();
            assert!(config.warnings.is_empty(), "{text}: {:?}", config.warnings);
        }
        // The swaybg line that draws it, quoted for the shell (~ still
        // expands), in swaybg's mode of the same name.
        for (text, line) in [
            (
                "image = \"~/Pictures/a b.jpg\"\nfit = \"center\"",
                "swaybg -i ~/'Pictures/a b.jpg' -m center",
            ),
            (
                "image = \"/srv/it's.png\"",
                r#"swaybg -i '/srv/it'\''s.png' -m fill"#,
            ),
        ] {
            let config = wallpaper(text).unwrap();
            assert_eq!(config.warnings.len(), 1, "{text}: {:?}", config.warnings);
            let warning = &config.warnings[0];
            assert!(warning.contains("[wallpaper]"), "{warning}");
            assert!(warning.contains(line), "{text}: {warning}");
        }
        assert!(wallpaper("picture = \"/a.png\"").is_err());
    }

    #[test]
    fn xwayland_idle_timeout_is_unset_by_default_and_bounded() {
        let config = Config::from_toml("", &no_env).unwrap();
        assert_eq!(config.xwayland.idle_timeout, None);
        assert!(config.xwayland.enabled);
        let config = Config::from_toml("[xwayland]\nidle-timeout = 10", &no_env).unwrap();
        assert_eq!(config.xwayland.idle_timeout, Some(10));
        for text in ["idle-timeout = 0", "idle-timeout = 3601"] {
            let error = Config::from_toml(&format!("[xwayland]\n{text}"), &no_env).unwrap_err();
            assert!(error.contains("xwayland.idle-timeout"), "{error}");
        }
        assert!(Config::from_toml("[xwayland]\nidle-timeout = -1", &no_env).is_err());
    }

    #[test]
    fn the_hot_corner_is_top_left_at_once_by_default_and_bounded() {
        let config = Config::from_toml("", &no_env).unwrap();
        assert_eq!(config.overview.hot_corner_position, Corner::TopLeft);
        assert_eq!(config.overview.hot_corner_pressure, 0);
        assert_eq!(config.overview.hot_corner_delay, 0);
        let config = Config::from_toml(
            "[overview]\nhot-corner-position = \"bottom-left\"\nhot-corner-pressure = 80",
            &no_env,
        )
        .unwrap();
        assert_eq!(config.overview.hot_corner_position, Corner::BottomLeft);
        assert_eq!(config.overview.hot_corner_pressure, 80);
        for (text, key) in [
            ("hot-corner-pressure = 2001", "overview.hot-corner-pressure"),
            ("hot-corner-delay = 5001", "overview.hot-corner-delay"),
        ] {
            let error = Config::from_toml(&format!("[overview]\n{text}"), &no_env).unwrap_err();
            assert!(error.contains(key), "{error}");
        }
        assert!(Config::from_toml("[overview]\nhot-corner-position = \"left\"", &no_env).is_err());
    }

    #[test]
    fn other_gpus_are_off_unless_asked_for() {
        assert!(!Config::from_toml("", &no_env).unwrap().other_gpus);
        let config = Config::from_toml("other-gpus = true", &no_env).unwrap();
        assert!(config.other_gpus);
        assert!(Config::from_toml("other-gpus = 2", &no_env).is_err());
    }

    #[test]
    fn tearing_is_allowed_unless_turned_off() {
        assert!(Config::from_toml("", &no_env).unwrap().tearing);
        assert!(
            !Config::from_toml("tearing = false", &no_env)
                .unwrap()
                .tearing
        );
    }

    #[test]
    fn rendering_late_is_on_unless_turned_off() {
        assert!(Config::from_toml("", &no_env).unwrap().render_late);
        let config = Config::from_toml("render-late = false", &no_env).unwrap();
        assert!(!config.render_late);
        assert!(Config::from_toml("render-late = 0", &no_env).is_err());
    }

    #[test]
    fn direct_scanout_is_on_unless_turned_off() {
        assert!(Config::from_toml("", &no_env).unwrap().direct_scanout);
        let config = Config::from_toml("direct-scanout = false", &no_env).unwrap();
        assert!(!config.direct_scanout);
        assert!(Config::from_toml("direct-scanout = 0", &no_env).is_err());
    }

    #[test]
    fn corners_default_to_gtks_and_are_bounded() {
        // GTK's own windows: 8 px, the top corners only.
        let config = Config::from_toml("", &no_env).unwrap();
        assert_eq!(config.corner_radius, 8);
        assert_eq!(config.rounded_corners, crate::rounded::Corners::Top);
        let config = Config::from_toml("corner-radius = 12", &no_env).unwrap();
        assert_eq!(config.corner_radius, 12);
        let config = Config::from_toml("rounded-corners = \"all\"", &no_env).unwrap();
        assert_eq!(config.rounded_corners, crate::rounded::Corners::All);
        let err = Config::from_toml("rounded-corners = \"bottom\"", &no_env).unwrap_err();
        assert!(err.contains("\"top\" or \"all\""), "{err}");
        assert!(
            Config::from_toml("corner-radius = 0", &no_env)
                .unwrap()
                .corner_radius
                == 0
        );
        assert!(Config::from_toml("corner-radius = 65", &no_env).is_err());
        assert!(Config::from_toml("corner-radius = -1", &no_env).is_err());
    }

    #[test]
    fn gaps_default_to_none_and_are_bounded() {
        assert_eq!(Config::from_toml("", &no_env).unwrap().gaps, 0);
        assert_eq!(Config::from_toml("gaps = 12", &no_env).unwrap().gaps, 12);
        let err = Config::from_toml("gaps = 129", &no_env).unwrap_err();
        assert!(err.contains("gaps = 129: must be 0 to 128"), "{err}");
        assert!(Config::from_toml("gaps = -1", &no_env).is_err());
    }

    #[test]
    fn animations_are_on_by_default_and_configurable() {
        use std::time::Duration;
        use AnimationKind::*;
        let config = Config::from_toml("", &no_env).unwrap();
        assert_eq!(
            config.animations.duration(WindowOpen),
            Some(Duration::from_millis(200))
        );
        assert_eq!(
            config.animations.duration(Workspace),
            Some(Duration::from_millis(250))
        );

        let config = Config::from_toml(
            "[animations]\nspeed = 2\nminimize = false\nlayer = 100\nworkspace = true",
            &no_env,
        )
        .unwrap();
        assert_eq!(config.animations.duration(Minimize), None);
        assert_eq!(
            config.animations.duration(Workspace),
            Some(Duration::from_millis(500))
        );
        assert_eq!(
            config.animations.duration(Layer),
            Some(Duration::from_millis(200))
        );
        assert_eq!(
            config.animations.duration(WindowClose),
            Some(Duration::from_millis(360))
        );

        let off = Config::from_toml("[animations]\nenabled = false", &no_env).unwrap();
        assert!(AnimationKind::ALL
            .iter()
            .all(|kind| off.animations.duration(*kind).is_none()));

        let err = |text| Config::from_toml(text, &no_env).unwrap_err();
        assert!(err("[animations]\nspeed = 0").contains("outside"));
        assert!(err("[animations]\nlayer = 5000").contains("1 to 1000"));
        assert!(err("[animations]\nlayer = 0").contains("false turns it off"));
        assert!(err("[animations]\nfade = 100").contains("unknown field"));
        // The form it used to take.
        assert!(err("[animations]\nlayer = { duration = 100 }").contains("a duration in ms"));
    }

    #[test]
    fn rules_are_read_in_order() {
        use crate::rules::{resolve_layer, resolve_window};
        let config = Config::from_toml(
            r#"
            [[window-rule]]
            app-id = "foot"
            animations = false

            [[layer-rule]]
            namespace = "shell-*"
            blur = true
            "#,
            &no_env,
        )
        .unwrap();
        assert!(!resolve_window(&config.window_rules, "foot", "", "")
            .animates(AnimationKind::WindowOpen));
        assert_eq!(
            resolve_layer(&config.layer_rules, "shell-osd").blur,
            Some(true)
        );
        let err = |text| Config::from_toml(text, &no_env).unwrap_err();
        assert!(err("[[window-rule]]\nclass = \"x\"").contains("unknown field"));
        assert!(err("[[layer-rule]]\nnamespace = 3").contains("invalid type"));
    }

    #[test]
    fn workspaces_have_a_count_and_bindings() {
        use WorkspaceTarget::{Index, Next, Prev};
        let config = Config::from_toml("", &no_env).unwrap();
        // config.example.toml's four.
        assert_eq!(config.workspaces, 4);
        let alt = mods(false, true, false, false);
        let alt_shift = mods(false, true, true, false);
        assert_eq!(
            config.action_for(Keysym::_3, &alt),
            Some(Action::Workspace(Index(2)))
        );
        assert_eq!(
            config.action_for(Keysym::_1, &alt_shift),
            Some(Action::MoveToWorkspace(Index(0)))
        );
        // Only as many digits as there are workspaces.
        assert_eq!(config.action_for(Keysym::_5, &alt), None);
        assert_eq!(config.action_for(Keysym::_5, &alt_shift), None);
        assert_eq!(
            config.action_for(Keysym::Page_Down, &alt),
            Some(Action::Workspace(Next))
        );
        assert_eq!(
            config.action_for(Keysym::Page_Up, &alt_shift),
            Some(Action::MoveToWorkspace(Prev))
        );

        let config = Config::from_toml(
            r#"
            workspaces = 6
            [bindings]
            "Mod+F1" = "workspace 6"
            "Mod+F2" = "move-to-workspace-next"
            "#,
            &no_env,
        )
        .unwrap();
        assert_eq!(config.workspaces, 6);
        assert_eq!(
            config.action_for(Keysym::_6, &alt),
            Some(Action::Workspace(Index(5)))
        );
        assert_eq!(config.action_for(Keysym::_7, &alt), None);
        assert_eq!(
            config.action_for(Keysym::F1, &alt),
            Some(Action::Workspace(Index(5)))
        );
        assert_eq!(
            config.action_for(Keysym::F2, &alt),
            Some(Action::MoveToWorkspace(Next))
        );

        assert!(Config::from_toml("workspaces = 0", &no_env).is_err());
        assert!(Config::from_toml("workspaces = 33", &no_env).is_err());
        let bad = |action: &str| {
            Config::from_toml(&format!("[bindings]\n\"Mod+F1\" = \"{action}\""), &no_env).is_err()
        };
        assert!(bad("workspace 0"));
        assert!(bad("workspace"));
        assert!(bad("workspaces 2"));
        assert!(bad("move-to-workspace-up"));
    }

    #[test]
    fn switch_layout_takes_next_prev_or_a_number_from_1() {
        use LayoutTarget::{Index, Next, Prev};
        let parse = |text| Action::parse(text).unwrap();
        assert_eq!(
            parse("switch-layout next"),
            Some(Action::SwitchLayout(Next))
        );
        assert_eq!(
            parse("switch-layout prev"),
            Some(Action::SwitchLayout(Prev))
        );
        assert_eq!(
            parse("switch-layout 2"),
            Some(Action::SwitchLayout(Index(1)))
        );
        assert!(Action::parse("switch-layout 0").is_err());
        assert!(Action::parse("switch-layout up").is_err());
    }

    #[test]
    fn the_monitors_power_actions_parse() {
        assert_eq!(
            Action::parse("power-off-monitors").unwrap(),
            Some(Action::PowerOffMonitors)
        );
        assert_eq!(
            Action::parse("power-on-monitors").unwrap(),
            Some(Action::PowerOnMonitors)
        );
    }

    #[test]
    fn gestures_have_defaults_and_can_be_changed() {
        let swipe = |fingers, direction| Gesture { fingers, direction };
        let config = Config::from_toml("", &no_env).unwrap();
        assert_eq!(
            config.gesture_action(swipe(3, SwipeDirection::Left)),
            Some(Action::CycleFocus)
        );
        assert!(config.handles_swipes(3));
        assert!(config.handles_swipes(4));
        assert!(!config.handles_swipes(5));

        let config = Config::from_toml(
            r#"
            [gestures]
            "swipe-3-down" = "minimize"
            "swipe-3-up" = "none"
            "swipe-4-left" = "snap-left"
            "#,
            &no_env,
        )
        .unwrap();
        assert_eq!(
            config.gesture_action(swipe(3, SwipeDirection::Down)),
            Some(Action::Minimize)
        );
        assert_eq!(config.gesture_action(swipe(3, SwipeDirection::Up)), None);
        assert_eq!(
            config.gesture_action(swipe(4, SwipeDirection::Left)),
            Some(Action::Snap(Side::Left))
        );
        assert!(config.handles_swipes(4));

        let err = |text| Config::from_toml(text, &no_env).unwrap_err();
        assert!(err(r#"gestures = { "swipe-2-left" = "close" }"#).contains("unknown gesture"));
        assert!(err(r#"gestures = { "pinch-3-in" = "close" }"#).contains("unknown gesture"));
        assert!(err(r#"gestures = { "swipe-3-left" = "fly" }"#).contains("unknown action"));
    }

    #[test]
    fn default_mod_applies_only_when_unset() {
        let config = Config::parse("", &no_env, ModKey::Super).unwrap();
        assert_eq!(config.mod_key, ModKey::Super);
        let logo = mods(false, false, false, true);
        assert_eq!(
            config.action_for(Keysym::Return, &logo),
            Some(Action::Terminal)
        );
        let config = Config::parse(r#"mod = "alt""#, &no_env, ModKey::Super).unwrap();
        assert_eq!(config.mod_key, ModKey::Alt);
        let env = |name: &str| (name == "PILA_MOD").then(|| "ctrl".to_string());
        let config = Config::parse("", &env, ModKey::Super).unwrap();
        assert_eq!(config.mod_key, ModKey::Ctrl);
    }

    #[test]
    fn the_removed_internal_display_key_is_refused_by_name() {
        let error = Config::from_toml("disable-internal-display = true", &no_env).unwrap_err();
        assert!(error.contains("disable-internal-display"), "{error}");
    }

    #[test]
    fn the_most_specific_output_section_wins() {
        let config = Config::from_toml(
            r#"
            [[output]]
            name = "DP-1"
            scale = 1.25
            [[output]]
            name = "GSM 27GL850"
            scale = 1.5
            [[output]]
            name = "GSM 27GL850 ABC"
            scale = 2.0
            "#,
            &no_env,
        )
        .unwrap();
        let monitor = |serial: Option<&str>| Monitor {
            make: "GSM".into(),
            model: "27GL850".into(),
            serial: serial.map(str::to_string),
        };
        let scale = |name: &str, monitor: Option<&Monitor>| {
            config.output(name, monitor).and_then(|c| c.scale)
        };
        assert_eq!(scale("DP-1", Some(&monitor(Some("ABC")))), Some(2.0));
        assert_eq!(scale("DP-2", Some(&monitor(Some("ABC")))), Some(2.0));
        assert_eq!(scale("DP-1", Some(&monitor(Some("XYZ")))), Some(1.5));
        assert_eq!(scale("DP-1", Some(&monitor(None))), Some(1.5));
        assert_eq!(scale("DP-1", None), Some(1.25));
        assert_eq!(scale("DP-2", None), None);
        let other = Monitor {
            make: "DEL".into(),
            model: "U2720Q".into(),
            serial: None,
        };
        assert_eq!(scale("DP-1", Some(&other)), Some(1.25));
    }

    #[test]
    fn example_config_is_valid() {
        if let Err(err) = Config::from_toml(EXAMPLE, &no_env) {
            panic!("config.example.toml: {err}");
        }
    }

    #[test]
    fn the_manual_and_man_page_list_every_default_binding() {
        let manual = include_str!("../docs/MANUAL.md");
        let man = include_str!("../docs/pila.1").replace("\\-", "-");
        let example: ConfigFile = toml::from_str(EXAMPLE).unwrap();
        let defaults = example.bindings.keys().chain(example.gestures.keys());
        for keys in defaults {
            assert!(
                manual.contains(&format!("`{keys}`")),
                "docs/MANUAL.md doesn't list the default {keys:?}"
            );
            assert!(
                man.contains(keys),
                "docs/pila.1 doesn't list the default {keys:?}"
            );
        }
    }

    /// `config` with its bindings and gestures in one order, for comparing
    /// configs whose bindings came in another.
    fn sorted(mut config: Config) -> String {
        config.bindings.sort_by_key(|b| format!("{b:?}"));
        config.gestures.sort_by_key(|g| format!("{g:?}"));
        format!("{config:?}")
    }

    #[test]
    fn every_default_comes_from_the_example() {
        // The code has none of its own: each active setting of the example
        // taken out of it leaves an empty file's config without that
        // setting, an error, rather than falling back to a value in the code.
        // Not the bindings, gestures and environment variables (taking one
        // out takes it away), nor
        // `launcher` and `autostart`, which may be unset (none, nothing), nor
        // the devices' options (unset, libinput's for the device).
        let lines: Vec<&str> = EXAMPLE.lines().collect();
        let mut table = String::new();
        let mut checked = 0;
        for (n, line) in lines.iter().enumerate() {
            if line.starts_with('[') {
                table = line.trim_matches(['[', ']']).to_string();
                continue;
            }
            let Some((key, _)) = line.split_once(" = ") else {
                continue;
            };
            let free = [
                "bindings",
                "gestures",
                "environment",
                "input.touchpad",
                "input.mouse",
            ]
            .contains(&table.as_str())
                || (table.is_empty() && ["launcher", "autostart"].contains(&key));
            if line.starts_with('#') || free {
                continue;
            }
            let mut without = lines.clone();
            without.remove(n);
            let err = Config::parse_with("", &without.join("\n"), &no_env, ModKey::Alt)
                .err()
                .unwrap_or_else(|| {
                    panic!(
                        "config.example.toml:{}: without {line:?}, the code has a value of its own",
                        n + 1
                    )
                });
            let name = if table.is_empty() {
                key.to_string()
            } else {
                format!("{table}.{key}")
            };
            assert!(
                err.contains(&format!("{name} isn't set")) || err.contains(&format!("[{table}]")),
                "config.example.toml:{}: {err}",
                n + 1
            );
            checked += 1;
        }
        // At least as many as config.example.toml had when this was written:
        // the lines are still found.
        assert!(checked >= 38, "only {checked} defaults found");
    }

    #[test]
    fn the_example_as_ones_own_file_changes_nothing() {
        // Pila writes it on the first run: its bindings then come twice,
        // once as the defaults and once as the file's own, to the same end.
        let empty = Config::from_toml("", &no_env).unwrap();
        let example = Config::from_toml(EXAMPLE, &no_env).unwrap();
        assert_eq!(sorted(example), sorted(empty));
    }

    #[test]
    fn a_file_with_only_some_settings_keeps_the_examples_others() {
        let config = Config::from_toml(
            "workspaces = 6\n[shadow]\nsize = 10\n[input.touchpad]\nnatural-scroll = true\n[bindings]\n\"Mod+X\" = \"close\"\n",
            &no_env,
        )
        .unwrap();
        let example = Config::default();
        assert_eq!(config.workspaces, 6);
        assert_eq!(config.shadow.size, 10);
        assert_eq!(config.shadow.offset, example.shadow.offset);
        assert_eq!(config.input.touchpad.natural_scroll, Some(true));
        assert_eq!(config.input.touchpad.tap, example.input.touchpad.tap);
        let alt = mods(false, true, false, false);
        assert_eq!(
            config.action_for(Keysym::x, &alt),
            Some(Action::CloseFocused)
        );
        assert_eq!(
            config.action_for(Keysym::Return, &alt),
            Some(Action::Terminal)
        );
        // `true` is the example's duration.
        let on = Config::from_toml("[animations]\nwindow-open = true", &no_env).unwrap();
        assert_eq!(on.animations, example.animations);
        // default-bindings = false leaves out the example's bindings.
        let own = Config::from_toml(
            "default-bindings = false\n[bindings]\n\"Mod+X\" = \"close\"\n",
            &no_env,
        )
        .unwrap();
        assert_eq!(own.bindings.len(), 1);
        assert!(own.gestures.is_empty());
    }

    #[test]
    fn the_examples_come_before_the_value_they_would_replace() {
        // In each table, a commented-out `key = ...` comes before the active
        // line of that key, never after it.
        let key_of = |line: &str| -> Option<String> {
            let (key, _) = line.split_once('=')?;
            let key = key.trim();
            let plain = |c: char| c.is_ascii_alphanumeric() || "_-+\"".contains(c);
            (!key.is_empty() && key.chars().all(plain)).then(|| key.to_string())
        };
        let mut table = String::from("the top level");
        let mut active = std::collections::HashSet::new();
        for (number, line) in EXAMPLE.lines().enumerate() {
            if line.starts_with('[') {
                table = line.to_string();
                continue;
            }
            if let Some(example) = line.strip_prefix("# ") {
                if let Some(key) = key_of(example) {
                    assert!(
                        !active.contains(&(table.clone(), key.clone())),
                        "line {}: the example of {key} in {table} comes after its active line",
                        number + 1
                    );
                }
            } else if let Some(key) = key_of(line) {
                active.insert((table.clone(), key));
            }
        }
    }

    #[test]
    fn example_lines_uncomment_into_their_tables() {
        // A tester took the `#` off a line of the bindings' list, which then
        // sat above [bindings] and so landed in [input.mouse].
        let example = EXAMPLE;
        let uncomment = |line: &str| {
            assert!(example.contains(line), "{line:?} not in the example");
            example.replacen(line, line.trim_start_matches("# "), 1)
        };
        let text = uncomment(r#"# "Mod+Shift+Return" = "spawn kitty -e htop""#);
        let config = Config::from_toml(&text, &no_env)
            .unwrap_or_else(|err| panic!("an example binding uncommented: {err}"));
        assert!(config
            .bindings
            .iter()
            .any(|b| b.action == Action::Spawn("kitty -e htop".into())));
        let text = uncomment(r#"# "swipe-5-left" = "snap-left""#);
        if let Err(err) = Config::from_toml(&text, &no_env) {
            panic!("an example gesture uncommented: {err}");
        }
        let text = uncomment(r#"# LANGUAGE = "zh_CN:zh_Hans:zh"    # e.g."#);
        let config = Config::from_toml(&text, &no_env)
            .unwrap_or_else(|err| panic!("an example variable uncommented: {err}"));
        assert!(config
            .environment
            .contains(&("LANGUAGE".to_string(), Some("zh_CN:zh_Hans:zh".to_string()))));
    }

    #[test]
    fn a_toml_error_starts_with_what_is_wrong() {
        // Notifications show the first lines: they must say what's wrong,
        // not a source snippet.
        let err = Config::from_toml(
            "[input.mouse]\n# natural-scroll = false\n  \"Mod+Return\" = \"terminal\"\n",
            &no_env,
        )
        .unwrap_err();
        let first = err.lines().next().unwrap();
        assert!(first.contains("line 3"), "{err}");
        assert!(first.contains("unknown field `Mod+Return`"), "{err}");
        assert!(err.contains("[bindings]"), "{err}");
        let err = Config::from_toml("[keyboard]\nrepeat-rate = \"x\"\n", &no_env).unwrap_err();
        assert!(!err.contains("[bindings]"), "{err}");
    }

    #[test]
    fn a_bad_action_names_its_binding_as_written() {
        // It used to name the parsed key combo, in Rust's debug form.
        let err = Config::from_toml("[bindings]\n\"Mod+X\" = \"nope\"\n", &no_env).unwrap_err();
        assert!(
            err.starts_with("binding \"Mod+X\": unknown action"),
            "{err}"
        );
        let err =
            Config::from_toml("[gestures]\n\"swipe-3-up\" = \"nope\"\n", &no_env).unwrap_err();
        assert!(
            err.starts_with("gesture \"swipe-3-up\": unknown action"),
            "{err}"
        );
    }

    #[test]
    fn example_rules_are_valid() {
        // The commented-out rules at the end of config.example.toml, with
        // their `# ` taken off.
        let example = EXAMPLE;
        let start = example
            .find("# [[window-rule]]")
            .expect("the example has rules");
        let rules: String = example[start..]
            .lines()
            .map(|line| {
                line.strip_prefix("# ")
                    .unwrap_or(line.trim_start_matches('#'))
            })
            .collect::<Vec<_>>()
            .join("\n");
        let config = match Config::from_toml(&rules, &no_env) {
            Ok(config) => config,
            Err(err) => panic!("config.example.toml's rules: {err}"),
        };
        assert!(config.window_rules.len() >= 3);
        assert!(config.layer_rules.len() >= 2);
        assert!(config.permission_rules.len() >= 2);
    }

    #[test]
    fn permissions_config_and_rules_are_parsed_and_validated() {
        let defaults = Config::default();
        assert!(!defaults.permissions.enforce);
        assert!(defaults.permission_rules.is_empty());

        let config = Config::from_toml(
            r#"
            [permissions]
            enforce = true

            [[permission]]
            executable = "/usr/bin/grim"
            access = "allow"

            [[permission]]
            executable = "/usr/bin/wf-*"
            access = "ask"

            [[permission]]
            executable = "*"
            access = "deny"
            "#,
            &no_env,
        )
        .unwrap();
        assert!(config.permissions.enforce);
        assert_eq!(config.permission_rules.len(), 3);
        assert_eq!(config.permission_rules[0].executable, "/usr/bin/grim");
        assert_eq!(config.permission_rules[0].access, PermissionAccess::Allow);
        assert_eq!(config.permission_rules[1].access, PermissionAccess::Ask);
        assert_eq!(config.permission_rules[2].access, PermissionAccess::Deny);

        let err = |text| Config::from_toml(text, &no_env).unwrap_err();
        assert!(err("[[permission]]\naccess = \"allow\"").contains("no executable"));
        assert!(
            err("[[permission]]\nexecutable = \"  \"\naccess = \"allow\"")
                .contains("no executable")
        );
        assert!(err("[[permission]]\nexecutable = \"/usr/bin/grim\"").contains("no access"));
        assert!(
            err("[[permission]]\nexecutable = \"/usr/bin/grim\"\naccess = \"maybe\"")
                .contains("unknown variant")
        );
    }

    /// xkbcommon panics on a name with a NUL in it (TOML's "\u0000"), which
    /// crashed `--check-config`, the start and a reload: the file is refused
    /// instead (found from the `app_shortcuts` fuzz target's crash in a
    /// keysym's name).
    #[test]
    fn a_keymap_name_with_a_nul_is_refused() {
        let no_env = |_: &str| None;
        for key in ["layout", "variant", "model", "options"] {
            let text = format!("[keyboard]\n{key} = \"us\\u0000\"\n");
            let err = Config::from_toml(&text, &no_env).unwrap_err();
            assert!(err.contains(&format!("keyboard.{key}")), "{err}");
        }
    }

    #[test]
    fn keymaps_xkbcommon_doesnt_know_are_found() {
        let kb = |layout: &str| KeyboardConfig {
            layout: layout.into(),
            ..Config::default().keyboard
        };
        assert_eq!(check_keymap(&kb("us")), Ok(()));
        let err = check_keymap(&kb("en")).unwrap_err();
        assert!(err.contains(r#"layout "en""#), "{err}");
    }

    #[test]
    fn keymaps_are_described_by_what_is_set() {
        let kb = KeyboardConfig {
            layout: "en".into(),
            variant: "altgr-intl".into(),
            ..Config::default().keyboard
        };
        assert_eq!(
            keymap_description(&kb),
            r#"layout "en", variant "altgr-intl""#
        );
        let kb = KeyboardConfig {
            layout: "us,es".into(),
            options: Some("grp:alt_shift_toggle".into()),
            ..Config::default().keyboard
        };
        assert_eq!(
            keymap_description(&kb),
            r#"layout "us,es", options "grp:alt_shift_toggle""#
        );
    }

    #[test]
    fn mistakes_are_reported() {
        let err = |text| Config::from_toml(text, &no_env).unwrap_err();
        assert!(err(r#"mod = "hyper""#).contains("unknown modifier"));
        assert!(err(r#"bindings = { "Mod+Nope" = "quit" }"#).contains("unknown key"));
        assert!(err(r#"bindings = { "Mod+Q" = "explode" }"#).contains("unknown action"));
        assert!(err(r#"terminl = "foot""#).contains("unknown field"));
        assert!(err("[[output]]\nname = \"DP-1\"\nscale = 0").contains("outside"));
        assert!(err("[[output]]\nname = \"DP-1\"\nmode = \"big\"").contains("bad mode"));
        assert!(err("[[output]]\nscale = 2").contains("no name"));
        assert!(err("[input.mouse]\naccel-speed = 2.0").contains("outside"));
        assert!(err("[input.touchpad]\nclick-method = \"tap\"").contains("unknown variant"));
        assert!(err("[input.touchpad]\nnatural = true").contains("unknown field"));
    }
}
