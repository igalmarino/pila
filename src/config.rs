//! Configuration: `~/.config/pila/config.toml` (or `$XDG_CONFIG_HOME/pila`,
//! or `--config PATH`), then `PILA_*` environment overrides. See
//! `config.example.toml` for every option.
//!
//! Default bindings (Mod is Alt unless configured, see `mod`/`PILA_MOD`):
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
//! | Mod+Shift+Q           | Close focused window           |
//! | Mod+Shift+E           | Quit pila                      |
//! | Mod+Left drag         | Move window                    |
//! | Mod+Right drag        | Resize window (nearest corner) |
//! | Ctrl+Alt+F1..F12      | Switch VT (on a TTY)           |
//!
//! `[bindings]` in the config adds to or replaces the keyboard bindings above
//! (`"Mod+Shift+Return" = "spawn foot -e htop"`, `"Mod+J" = "none"`); the
//! modifiers of a binding must match the pressed ones exactly. The VT switch
//! comes from the keymap (`XF86_Switch_VT_N` keysyms), so it's handled in the
//! keyboard filter and can't be rebound; the mouse bindings aren't
//! configurable yet.
//!
//! Alt is the default because a host desktop such as GNOME grabs Super while
//! pila runs nested. Use `mod = "super"` on bare metal.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use smithay::input::keyboard::{xkb, Keysym, ModifiersState};
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
    ("Mod+Shift+Q", "close"),
    ("Mod+Shift+E", "quit"),
];

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
    /// Switch to virtual terminal N (Ctrl+Alt+F<N> on a TTY; not configurable).
    ChangeVt(i32),
}

impl Action {
    /// `None` for `"none"`, which removes a binding.
    fn parse(text: &str) -> Result<Option<Self>, String> {
        let text = text.trim();
        if let Some(command) = text.strip_prefix("spawn ") {
            return Ok(Some(Action::Spawn(command.trim().to_string())));
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
                     grow-height, shrink-height, center, quit or none)"
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

/// `[[output]]`: settings for one monitor, matched by connector name
/// (`eDP-1`, `DP-1`, `HDMI-A-1`, ...; `winit` is the nested window).
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
        Ok(OutputConfig {
            name: self.name,
            mode,
            scale,
            position: self.position.map(|[x, y]| (x, y)),
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
    output: Vec<OutputFile>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub mod_key: ModKey,
    pub terminal: String,
    pub launcher: Option<String>,
    /// Commands run at startup (when no `-c` is given).
    pub autostart: Vec<String>,
    pub keyboard: KeyboardConfig,
    pub bindings: Vec<(KeyCombo, Action)>,
    pub outputs: Vec<OutputConfig>,
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

    /// Load `path` (or the default path), apply `PILA_*` overrides. A missing
    /// file means defaults; an invalid one is reported and ignored, since a
    /// typo shouldn't leave you without a desktop.
    pub fn load(path: Option<&Path>) -> Self {
        let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        let path = path.map(Path::to_path_buf).or_else(Self::default_path);
        let text = match &path {
            Some(path) => match std::fs::read_to_string(path) {
                Ok(text) => {
                    info!(path = %path.display(), "loading config");
                    text
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(err) => {
                    warn!(path = %path.display(), %err, "can't read config; using defaults");
                    String::new()
                }
            },
            None => String::new(),
        };
        match Self::from_toml(&text, &env) {
            Ok(config) => config,
            Err(err) => {
                warn!(%err, "invalid config; using defaults");
                Self::from_toml("", &env).unwrap_or_default()
            }
        }
    }

    /// Parse a config file's contents; `env` looks up `PILA_*` overrides.
    pub fn from_toml(text: &str, env: &dyn Fn(&str) -> Option<String>) -> Result<Self, String> {
        let file: ConfigFile = toml::from_str(text).map_err(|err| err.to_string())?;

        let mod_key = match env("PILA_MOD").or(file.mod_key) {
            Some(name) => ModKey::parse(&name)?,
            None => ModKey::Alt,
        };
        let terminal = env("PILA_TERMINAL")
            .or(file.terminal)
            .unwrap_or_else(|| "foot".to_string());
        let launcher = env("PILA_LAUNCHER").or(file.launcher);

        // Defaults first, then the user's: a user binding replaces a default
        // one for the same combo, and "none" removes it.
        let mut bindings: Vec<(KeyCombo, Action)> = Vec::new();
        let defaults = DEFAULT_BINDINGS.iter().map(|&(k, a)| (k, a));
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

        let outputs = file
            .output
            .into_iter()
            .map(OutputFile::validate)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            mod_key,
            terminal,
            launcher,
            autostart: file.autostart,
            keyboard: file.keyboard,
            bindings,
            outputs,
        })
    }

    /// The `[[output]]` settings for the output called `name`, if any.
    pub fn output(&self, name: &str) -> Option<&OutputConfig> {
        self.outputs.iter().find(|o| o.name == name)
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
        assert_eq!(config.terminal, "foot");
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
        assert_eq!(config.terminal, "kgx");
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
        let edp = config.output("eDP-1").unwrap();
        assert_eq!(edp.scale, Some(160.0 / 120.0));
        assert_eq!(edp.mode, None);
        let dp = config.output("DP-1").unwrap();
        assert_eq!(dp.position, Some((1440, 0)));
        assert_eq!(
            dp.mode,
            Some(ModeSpec {
                width: 2560,
                height: 1440,
                refresh_hz: Some(59.95)
            })
        );
        assert!(config.output("HDMI-A-1").is_none());
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
    fn mistakes_are_reported() {
        let err = |text| Config::from_toml(text, &no_env).unwrap_err();
        assert!(err(r#"mod = "hyper""#).contains("unknown modifier"));
        assert!(err(r#"bindings = { "Mod+Nope" = "quit" }"#).contains("unknown key"));
        assert!(err(r#"bindings = { "Mod+Q" = "explode" }"#).contains("unknown action"));
        assert!(err(r#"terminl = "foot""#).contains("unknown field"));
        assert!(err("[[output]]\nname = \"DP-1\"\nscale = 0").contains("outside"));
        assert!(err("[[output]]\nname = \"DP-1\"\nmode = \"big\"").contains("bad mode"));
        assert!(err("[[output]]\nscale = 2").contains("no name"));
    }
}
