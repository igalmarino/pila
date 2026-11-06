//! Configuration: `~/.config/pila/config.toml` (or `$XDG_CONFIG_HOME/pila`,
//! or `--config PATH`), then `PILA_*` environment overrides. See
//! `config.example.toml` for every option.
//!
//! Default bindings (Mod is Super on a TTY and Alt nested, unless configured;
//! see `mod`/`PILA_MOD`):
//!
//! | Binding               | Action                         |
//! |-----------------------|--------------------------------|
//! | Mod+Return            | Open terminal                  |
//! | Mod+D                 | Run launcher (if configured)   |
//! | Mod+Tab / Mod+J       | Switch to previous window (hold Mod and repeat: older ones) |
//! | Mod+Shift+Tab         | Same, the other way round      |
//! | Mod+M                 | Toggle maximize                |
//! | Mod+N                 | Minimize (Mod+Tab brings it back) |
//! | Mod+F                 | Toggle fullscreen              |
//! | Mod+Left / Mod+Right  | Snap to the left / right half (again: restore) |
//! | Mod+Up / Mod+Down     | Maximize / restore             |
//! | Mod+Shift+arrows      | Move the window                |
//! | Mod+Ctrl+arrows       | Resize (Right/Down grow, Left/Up shrink) |
//! | Mod+C                 | Center the window              |
//! | Mod+Shift+S           | Screenshot of a region (slurp) to the clipboard |
//! | Mod+Ctrl+S            | Screenshot of the screen under the pointer to ~/Pictures |
//! | Mod+1..N              | Go to workspace 1..N (N = `workspaces`, at most 9) |
//! | Mod+Shift+1..N        | Move the window to workspace 1..N |
//! | Mod+Page_Down / Mod+Page_Up | Next / previous workspace |
//! | Mod+Shift+Page_Down / Page_Up | Move the window to the next / previous workspace |
//! | Mod+Shift+Q           | Close focused window           |
//! | Mod+Shift+E           | Quit pila                      |
//! | Mod+Left drag         | Move window                    |
//! | Mod+Right drag        | Resize window (nearest corner) |
//! | Ctrl+Alt+F1..F12      | Switch VT (on a TTY)           |
//!
//! Default touchpad gestures (`[gestures]`, on a TTY; `gestures.rs`):
//!
//! | Gesture               | Action                         |
//! |-----------------------|--------------------------------|
//! | 3 fingers left / right | Switch windows, like Mod+Tab / Mod+Shift+Tab |
//! | 3 fingers up          | Toggle maximize                |
//! | 3 fingers down        | Restore (undo a snap or maximize) |
//! | 4 fingers left / right | Next / previous workspace     |
//!
//! `[bindings]` in the config adds to or replaces the keyboard bindings above
//! (`"Mod+Shift+Return" = "spawn kitty -e htop"`, `"Mod+J" = "none"`); the
//! modifiers of a binding must match the pressed ones exactly. The VT switch
//! comes from the keymap (`XF86_Switch_VT_N` keysyms), so it's handled in the
//! keyboard filter and can't be rebound; the mouse bindings aren't
//! configurable yet.
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

use crate::snap::KEYBOARD_STEP;

/// The default bindings, as config strings (so they and the user's go
/// through the same parser).
const DEFAULT_BINDINGS: &[(&str, &str)] = &[
    ("Mod+Return", "terminal"),
    ("Mod+D", "launcher"),
    ("Mod+Tab", "cycle-focus"),
    ("Mod+J", "cycle-focus"),
    ("Mod+Shift+Tab", "cycle-focus-back"),
    ("Mod+M", "maximize"),
    ("Mod+N", "minimize"),
    ("Mod+F", "fullscreen"),
    ("Mod+Left", "snap-left"),
    ("Mod+Right", "snap-right"),
    ("Mod+Up", "maximize"),
    ("Mod+Down", "restore"),
    ("Mod+Shift+Left", "move-left"),
    ("Mod+Shift+Right", "move-right"),
    ("Mod+Shift+Up", "move-up"),
    ("Mod+Shift+Down", "move-down"),
    ("Mod+Ctrl+Right", "grow-width"),
    ("Mod+Ctrl+Left", "shrink-width"),
    ("Mod+Ctrl+Down", "grow-height"),
    ("Mod+Ctrl+Up", "shrink-height"),
    ("Mod+C", "center"),
    ("Mod+Shift+S", "spawn grim -g \"$(slurp)\" - | wl-copy"),
    (
        "Mod+Ctrl+S",
        "spawn mkdir -p ~/Pictures && grim -o \"$PILA_OUTPUT\" \
         ~/Pictures/screenshot-$(date +%Y%m%d-%H%M%S).png",
    ),
    ("Mod+Page_Down", "workspace-next"),
    ("Mod+Page_Up", "workspace-prev"),
    ("Mod+Shift+Page_Down", "move-to-workspace-next"),
    ("Mod+Shift+Page_Up", "move-to-workspace-prev"),
    ("Mod+Shift+Q", "close"),
    ("Mod+Shift+E", "quit"),
];

/// The default touchpad gestures (`gestures.rs`), as config strings.
const DEFAULT_GESTURES: &[(&str, &str)] = &[
    ("swipe-3-left", "cycle-focus"),
    ("swipe-3-right", "cycle-focus-back"),
    ("swipe-3-up", "maximize"),
    ("swipe-3-down", "restore"),
    ("swipe-4-left", "workspace-next"),
    ("swipe-4-right", "workspace-prev"),
];

/// The largest `corner-radius` (logical pixels). A window's corners are
/// further limited by its size and the title bar's height (`rounded.rs`).
pub const MAX_CORNER_RADIUS: u32 = 64;

/// How many workspaces there are unless `workspaces` says otherwise, and
/// the most it may say.
pub const DEFAULT_WORKSPACES: usize = 4;
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

/// A key combination, e.g. `Mod+Shift+Q`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyCombo {
    pub mods: Mods,
    /// The unshifted (latin) keysym, lowercase for letters.
    pub sym: Keysym,
}

impl KeyCombo {
    fn parse(text: &str, mod_key: ModKey) -> Result<Self, String> {
        let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let key = parts.pop().filter(|k| !k.is_empty());
        let key = key.ok_or_else(|| format!("{text:?} has no key"))?;
        let mut mods = Mods::default();
        for part in parts {
            match part.to_ascii_lowercase().as_str() {
                "mod" => mods.add(mod_key),
                "shift" => mods.shift = true,
                "ctrl" | "control" => mods.ctrl = true,
                "alt" => mods.alt = true,
                "super" | "logo" | "win" => mods.logo = true,
                _ => return Err(format!("unknown modifier {part:?} in {text:?}")),
            }
        }
        // Case-insensitive lookup returns the lowercase keysym for letters,
        // matching the unshifted sym the keyboard filter compares against.
        let sym = xkb::keysym_from_name(key, xkb::KEYSYM_CASE_INSENSITIVE);
        if sym.raw() == 0 {
            return Err(format!("unknown key {key:?} in {text:?}"));
        }
        Ok(KeyCombo { mods, sym })
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
    /// Snap to the left/right half of the output (`snap.rs`).
    SnapLeft,
    SnapRight,
    /// Undo a snap or a maximize.
    Restore,
    /// Move by (x, y) / resize by (w, h) logical pixels.
    MoveWindow(i32, i32),
    ResizeWindow(i32, i32),
    Center,
    ToggleMaximize,
    ToggleFullscreen,
    Quit,
    /// Go to a workspace, or move the focused window there (`workspaces.rs`).
    Workspace(WorkspaceTarget),
    MoveToWorkspace(WorkspaceTarget),
    /// Switch to virtual terminal N (Ctrl+Alt+F<N> on a TTY; not configurable).
    ChangeVt(i32),
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
    /// `None` for `"none"`, which removes a binding.
    fn parse(text: &str) -> Result<Option<Self>, String> {
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
        Ok(Some(match text {
            "none" => return Ok(None),
            "terminal" => Action::Terminal,
            "launcher" => Action::Launcher,
            "close" => Action::CloseFocused,
            "cycle-focus" => Action::CycleFocus,
            "cycle-focus-back" => Action::CycleFocusBack,
            "minimize" => Action::Minimize,
            "snap-left" => Action::SnapLeft,
            "snap-right" => Action::SnapRight,
            "restore" => Action::Restore,
            "move-left" => Action::MoveWindow(-KEYBOARD_STEP, 0),
            "move-right" => Action::MoveWindow(KEYBOARD_STEP, 0),
            "move-up" => Action::MoveWindow(0, -KEYBOARD_STEP),
            "move-down" => Action::MoveWindow(0, KEYBOARD_STEP),
            "grow-width" => Action::ResizeWindow(KEYBOARD_STEP, 0),
            "shrink-width" => Action::ResizeWindow(-KEYBOARD_STEP, 0),
            "grow-height" => Action::ResizeWindow(0, KEYBOARD_STEP),
            "shrink-height" => Action::ResizeWindow(0, -KEYBOARD_STEP),
            "center" => Action::Center,
            "maximize" => Action::ToggleMaximize,
            "fullscreen" => Action::ToggleFullscreen,
            "quit" => Action::Quit,
            other => {
                return Err(format!(
                    "unknown action {other:?} (spawn <command>, terminal, launcher, close, \
                     cycle-focus, cycle-focus-back, minimize, maximize, fullscreen, snap-left, \
                     snap-right, restore, move-left/right/up/down, grow-width, shrink-width, \
                     grow-height, shrink-height, center, workspace <n|next|prev>, \
                     move-to-workspace <n|next|prev>, quit or none)"
                ))
            }
        }))
    }
}

/// `[keyboard]`: passed to xkbcommon, plus key repeat.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct KeyboardConfig {
    pub layout: String,
    pub variant: String,
    pub model: String,
    pub options: Option<String>,
    /// Milliseconds before a held key repeats.
    pub repeat_delay: i32,
    /// Repeats per second.
    pub repeat_rate: i32,
}

impl Default for KeyboardConfig {
    fn default() -> Self {
        // Empty strings: xkbcommon's defaults (and `XKB_DEFAULT_*`).
        Self {
            layout: String::new(),
            variant: String::new(),
            model: String::new(),
            options: None,
            repeat_delay: 200,
            repeat_rate: 25,
        }
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
    /// Tap to click (touchpads; pila's default is on).
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
}

impl AnimationKind {
    const ALL: [AnimationKind; 6] = [
        AnimationKind::WindowOpen,
        AnimationKind::WindowClose,
        AnimationKind::Minimize,
        AnimationKind::Maximize,
        AnimationKind::Layer,
        AnimationKind::Workspace,
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
        }
    }

    /// Its duration in milliseconds unless configured.
    fn default_duration(self) -> u32 {
        match self {
            AnimationKind::WindowOpen => 200,
            AnimationKind::WindowClose => 180,
            AnimationKind::Minimize => 200,
            AnimationKind::Maximize => 220,
            AnimationKind::Layer => 150,
            AnimationKind::Workspace => 250,
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
    durations: [u32; 6],
    /// Multiplies every duration (2: twice as slow).
    speed: f64,
}

impl Default for AnimationsConfig {
    fn default() -> Self {
        Self {
            durations: AnimationKind::ALL.map(AnimationKind::default_duration),
            speed: 1.0,
        }
    }
}

impl AnimationsConfig {
    /// How long `kind` lasts, or `None` if it's off.
    pub fn duration(&self, kind: AnimationKind) -> Option<std::time::Duration> {
        let index = AnimationKind::ALL.iter().position(|k| *k == kind)?;
        let ms = f64::from(self.durations[index]) * self.speed;
        (ms >= 1.0).then(|| std::time::Duration::from_secs_f64(ms / 1000.0))
    }
}

/// One animation's entry in `[animations]`, as written: its duration in
/// milliseconds, or `false` (off) / `true` (the default duration).
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
}

impl AnimationsFile {
    fn validate(self) -> Result<AnimationsConfig, String> {
        let speed = self.speed.unwrap_or(1.0);
        if !ANIMATION_SPEEDS.contains(&speed) {
            return Err(format!(
                "[animations] speed {speed} is outside {}..{}",
                ANIMATION_SPEEDS.start(),
                ANIMATION_SPEEDS.end()
            ));
        }
        let all_on = self.enabled.unwrap_or(true);
        let entries = [
            self.window_open,
            self.window_close,
            self.minimize,
            self.maximize,
            self.layer,
            self.workspace,
        ];
        let mut durations = [0; 6];
        for ((kind, entry), duration) in AnimationKind::ALL.iter().zip(entries).zip(&mut durations)
        {
            let ms = match entry {
                None | Some(AnimationFile::On(true)) => kind.default_duration(),
                Some(AnimationFile::On(false)) => 0,
                Some(AnimationFile::Duration(ms @ 1..=MAX_ANIMATION_MS)) => ms,
                Some(AnimationFile::Duration(ms)) => {
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
        Ok(AnimationsConfig { durations, speed })
    }
}

/// `[input]`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InputConfig {
    pub touchpad: PointerConfig,
    pub mouse: PointerConfig,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            // Tap-to-click was always on; keep it unless the file says no.
            touchpad: PointerConfig {
                tap: Some(true),
                ..Default::default()
            },
            mouse: PointerConfig::default(),
        }
    }
}

/// `[[output]]`: settings for one monitor, matched by connector name
/// (`eDP-1`, `DP-1`, `HDMI-A-1`, ...; `winit` is the nested window) or by the
/// monitor's make and model, with or without its serial (`LEN P27h-30`).
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

/// `transform` in `[[output]]`: wl_output's names.
fn parse_transform(text: &str) -> Result<Transform, String> {
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

/// The file's layout, before validation.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct ConfigFile {
    #[serde(rename = "mod")]
    mod_key: Option<String>,
    terminal: Option<String>,
    launcher: Option<String>,
    autostart: Vec<String>,
    keyboard: KeyboardConfig,
    bindings: BTreeMap<String, String>,
    gestures: BTreeMap<String, String>,
    output: Vec<OutputFile>,
    input: InputFile,
    workspaces: Option<usize>,
    disable_internal_display: Option<bool>,
    corner_radius: Option<u32>,
    animations: AnimationsFile,
    #[serde(rename = "window-rule")]
    window_rules: Vec<crate::rules::WindowRuleFile>,
    #[serde(rename = "layer-rule")]
    layer_rules: Vec<crate::rules::LayerRuleFile>,
}

/// `[input]` as written: unset sections keep `InputConfig`'s defaults.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct InputFile {
    touchpad: Option<PointerConfig>,
    mouse: Option<PointerConfig>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub mod_key: ModKey,
    /// The Mod used when neither the file nor `PILA_MOD` sets one (depends on
    /// the backend; kept so a reload parses the same way).
    pub default_mod: ModKey,
    /// `None`: the first of `TERMINALS` that is installed (`terminal_command`).
    pub terminal: Option<String>,
    pub launcher: Option<String>,
    /// Commands run at startup (when no `-c` is given).
    pub autostart: Vec<String>,
    pub keyboard: KeyboardConfig,
    pub bindings: Vec<(KeyCombo, Action)>,
    /// Touchpad swipes pila handles (the others reach the clients).
    pub gestures: Vec<(Gesture, Action)>,
    pub outputs: Vec<OutputConfig>,
    pub input: InputConfig,
    /// How many workspaces there are (`workspaces.rs`).
    pub workspaces: usize,
    /// When the built-in screen goes off for an external one (`udev.rs`).
    pub internal_display: InternalDisplay,
    /// Window corner radius in logical pixels; 0 is square (`rounded.rs`).
    pub corner_radius: u32,
    /// Which animations play and how long they last (`animation.rs`).
    pub animations: AnimationsConfig,
    /// `[[window-rule]]` and `[[layer-rule]]`, in order (`rules.rs`).
    pub window_rules: Vec<crate::rules::WindowRule>,
    pub layer_rules: Vec<crate::rules::LayerRule>,
}

/// `disable-internal-display`: whether a laptop's built-in screen (eDP,
/// LVDS, DSI) is turned off while an external monitor is on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InternalDisplay {
    #[default]
    Never,
    Always,
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
fn in_path(program: &str) -> bool {
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
    fn default() -> Self {
        Self::from_toml("", &|_| None).expect("the defaults are valid")
    }
}

impl Config {
    /// The config file pila reads without `--config`.
    pub fn default_path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
        Some(base.join("pila/config.toml"))
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

    /// `read` at startup: an invalid file is reported and ignored, since a
    /// typo shouldn't leave you without a desktop.
    pub fn load(path: Option<&Path>, default_mod: ModKey) -> Self {
        Self::read(path, default_mod).unwrap_or_else(|err| {
            warn!(%err, "invalid config; using defaults");
            Self::parse("", &env_override, default_mod).unwrap_or_default()
        })
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
        let file: ConfigFile = toml::from_str(text).map_err(|err| err.to_string())?;

        let mod_key = match env("PILA_MOD").or(file.mod_key) {
            Some(name) => ModKey::parse(&name)?,
            None => default_mod,
        };
        let terminal = env("PILA_TERMINAL").or(file.terminal);
        let launcher = env("PILA_LAUNCHER").or(file.launcher);

        let internal_display = match file.disable_internal_display {
            Some(true) => InternalDisplay::Always,
            Some(false) | None => InternalDisplay::Never,
        };
        let workspaces = match file.workspaces {
            None => DEFAULT_WORKSPACES,
            Some(n @ 1..=MAX_WORKSPACES) => n,
            Some(n) => return Err(format!("workspaces = {n}: must be 1 to {MAX_WORKSPACES}")),
        };
        let corner_radius = match file.corner_radius {
            None => 0,
            Some(r @ 0..=MAX_CORNER_RADIUS) => r,
            Some(r) => {
                return Err(format!(
                    "corner-radius = {r}: must be 0 to {MAX_CORNER_RADIUS}"
                ))
            }
        };

        let animations = file.animations.validate()?;
        let window_rules = file
            .window_rules
            .into_iter()
            .enumerate()
            .map(|(i, rule)| rule.validate(i + 1, workspaces))
            .collect::<Result<Vec<_>, _>>()?;
        let layer_rules = file
            .layer_rules
            .into_iter()
            .map(crate::rules::LayerRuleFile::validate)
            .collect();

        // Defaults first, then the user's: a user binding replaces a default
        // one for the same combo, and "none" removes it. Mod+N and
        // Mod+Shift+N only exist for the workspaces there are (up to 9), so
        // the other digits still reach the apps.
        let mut bindings: Vec<(KeyCombo, Action)> = Vec::new();
        let workspace_bindings: Vec<(String, String)> = (1..=workspaces.min(9))
            .flat_map(|n| {
                [
                    (format!("Mod+{n}"), format!("workspace {n}")),
                    (format!("Mod+Shift+{n}"), format!("move-to-workspace {n}")),
                ]
            })
            .collect();
        let defaults = DEFAULT_BINDINGS.iter().map(|&(k, a)| (k, a)).chain(
            workspace_bindings
                .iter()
                .map(|(k, a)| (k.as_str(), a.as_str())),
        );
        let user = file.bindings.iter().map(|(k, a)| (k.as_str(), a.as_str()));
        for (combo, action) in defaults.chain(user) {
            let combo = KeyCombo::parse(combo, mod_key)?;
            let action =
                Action::parse(action).map_err(|err| format!("binding {combo:?}: {err}"))?;
            bindings.retain(|(c, _)| *c != combo);
            if let Some(action) = action {
                bindings.push((combo, action));
            }
        }

        // Same for gestures.
        let mut gestures: Vec<(Gesture, Action)> = Vec::new();
        let defaults = DEFAULT_GESTURES.iter().map(|&(g, a)| (g, a));
        let user = file.gestures.iter().map(|(g, a)| (g.as_str(), a.as_str()));
        for (gesture, action) in defaults.chain(user) {
            let gesture = Gesture::parse(gesture)?;
            let action =
                Action::parse(action).map_err(|err| format!("gesture {gesture:?}: {err}"))?;
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

        let defaults = InputConfig::default();
        let input = InputConfig {
            touchpad: match file.input.touchpad {
                // An unset `tap` keeps pila's default (on).
                Some(touchpad) => PointerConfig {
                    tap: touchpad.tap.or(defaults.touchpad.tap),
                    ..touchpad
                }
                .validate("touchpad")?,
                None => defaults.touchpad,
            },
            mouse: match file.input.mouse {
                Some(mouse) => mouse.validate("mouse")?,
                None => defaults.mouse,
            },
        };

        Ok(Self {
            mod_key,
            default_mod,
            terminal,
            launcher,
            autostart: file.autostart,
            keyboard: file.keyboard,
            bindings,
            gestures,
            outputs,
            input,
            workspaces,
            internal_display,
            corner_radius,
            animations,
            window_rules,
            layer_rules,
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
        let combo = KeyCombo {
            mods: mods.into(),
            sym,
        };
        let action = self
            .bindings
            .iter()
            .find(|(c, _)| *c == combo)
            .map(|(_, action)| action.clone())?;
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
    fn launcher_binding_needs_a_launcher() {
        let alt = mods(false, true, false, false);
        let none = Config::from_toml("", &no_env).unwrap();
        assert_eq!(none.action_for(Keysym::d, &alt), None);
        let some = Config::from_toml(r#"launcher = "fuzzel""#, &no_env).unwrap();
        assert_eq!(some.action_for(Keysym::d, &alt), Some(Action::Launcher));
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
        assert!(spawned(config.action_for(Keysym::s, &alt_shift)).contains("wl-copy"));
        assert!(spawned(config.action_for(Keysym::s, &alt_ctrl)).contains("$PILA_OUTPUT"));
    }

    #[test]
    fn corner_radius_defaults_to_square_and_is_bounded() {
        assert_eq!(Config::from_toml("", &no_env).unwrap().corner_radius, 0);
        let config = Config::from_toml("corner-radius = 12", &no_env).unwrap();
        assert_eq!(config.corner_radius, 12);
        assert!(Config::from_toml("corner-radius = 65", &no_env).is_err());
        assert!(Config::from_toml("corner-radius = -1", &no_env).is_err());
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
            namespace = "noctalia-*"
            blur = true
            "#,
            &no_env,
        )
        .unwrap();
        assert!(!resolve_window(&config.window_rules, "foot", "", "")
            .animates(AnimationKind::WindowOpen));
        assert_eq!(
            resolve_layer(&config.layer_rules, "noctalia-osd").blur,
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
        assert_eq!(config.workspaces, DEFAULT_WORKSPACES);
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
            Some(Action::SnapLeft)
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
    fn internal_display_parses() {
        let parse = |text: &str| Config::from_toml(text, &no_env).map(|c| c.internal_display);
        assert_eq!(parse(""), Ok(InternalDisplay::Never));
        assert_eq!(
            parse("disable-internal-display = true"),
            Ok(InternalDisplay::Always)
        );
        assert_eq!(
            parse("disable-internal-display = false"),
            Ok(InternalDisplay::Never)
        );
        // The words it used to take.
        assert!(parse(r#"disable-internal-display = "always""#).is_err());
    }

    #[test]
    fn the_most_specific_output_section_wins() {
        let config = Config::from_toml(
            r#"
            [[output]]
            name = "DP-1"
            scale = 1.25
            [[output]]
            name = "LEN P27h-30"
            scale = 1.5
            [[output]]
            name = "LEN P27h-30 ABC"
            scale = 2.0
            "#,
            &no_env,
        )
        .unwrap();
        let monitor = |serial: Option<&str>| Monitor {
            make: "LEN".into(),
            model: "P27h-30".into(),
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
        if let Err(err) = Config::from_toml(include_str!("../config.example.toml"), &no_env) {
            panic!("config.example.toml: {err}");
        }
    }

    #[test]
    fn example_rules_are_valid() {
        // The commented-out rules at the end of config.example.toml, with
        // their `# ` taken off.
        let example = include_str!("../config.example.toml");
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
    }

    #[test]
    fn keymaps_xkbcommon_doesnt_know_are_found() {
        let kb = |layout: &str| KeyboardConfig {
            layout: layout.into(),
            ..KeyboardConfig::default()
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
            ..KeyboardConfig::default()
        };
        assert_eq!(
            keymap_description(&kb),
            r#"layout "en", variant "altgr-intl""#
        );
        let kb = KeyboardConfig {
            layout: "us,es".into(),
            options: Some("grp:alt_shift_toggle".into()),
            ..KeyboardConfig::default()
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
