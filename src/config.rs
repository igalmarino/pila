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
//! | Mod+Tab / Mod+J       | Focus and raise next window    |
//! | Mod+M                 | Toggle maximize                |
//! | Mod+F                 | Toggle fullscreen              |
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

/// The default bindings, as config strings (so they and the user's go
/// through the same parser).
const DEFAULT_BINDINGS: &[(&str, &str)] = &[
    ("Mod+Return", "terminal"),
    ("Mod+D", "launcher"),
    ("Mod+Tab", "cycle-focus"),
    ("Mod+J", "cycle-focus"),
    ("Mod+M", "maximize"),
    ("Mod+F", "fullscreen"),
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
    CycleFocus,
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
            "maximize" => Action::ToggleMaximize,
            "fullscreen" => Action::ToggleFullscreen,
            "quit" => Action::Quit,
            other => {
                return Err(format!(
                    "unknown action {other:?} (spawn <command>, terminal, launcher, close, \
                     cycle-focus, maximize, fullscreen, quit or none)"
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

        Ok(Self {
            mod_key,
            terminal,
            launcher,
            autostart: file.autostart,
            keyboard: file.keyboard,
            bindings,
        })
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
    fn mistakes_are_reported() {
        let err = |text| Config::from_toml(text, &no_env).unwrap_err();
        assert!(err(r#"mod = "hyper""#).contains("unknown modifier"));
        assert!(err(r#"bindings = { "Mod+Nope" = "quit" }"#).contains("unknown key"));
        assert!(err(r#"bindings = { "Mod+Q" = "explode" }"#).contains("unknown action"));
        assert!(err(r#"terminl = "foot""#).contains("unknown field"));
    }
}
