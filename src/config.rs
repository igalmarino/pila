//! Runtime configuration and keybindings.
//!
//! Bindings (Mod is Alt by default, see `PILA_MOD`):
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
//! The VT switch comes from the keymap (`XF86_Switch_VT_N` keysyms), so it's
//! handled in the keyboard filter rather than `action_for`.
//!
//! Alt is the default because a host desktop such as GNOME grabs Super
//! while pila runs nested. Use `PILA_MOD=super` on bare metal.

use smithay::input::keyboard::{Keysym, ModifiersState};

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
}

#[derive(Debug, Clone)]
pub struct Config {
    pub mod_key: ModKey,
    pub terminal: String,
    pub launcher: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Spawn(String),
    CloseFocused,
    CycleFocus,
    ToggleMaximize,
    ToggleFullscreen,
    Quit,
    /// Switch to virtual terminal N (Ctrl+Alt+F<N> on a TTY; not configurable).
    ChangeVt(i32),
}

impl Config {
    pub fn from_env() -> Self {
        let mod_key = match std::env::var("PILA_MOD")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "super" | "logo" | "win" => ModKey::Super,
            "ctrl" | "control" => ModKey::Ctrl,
            _ => ModKey::Alt,
        };

        let terminal = std::env::var("PILA_TERMINAL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "foot".to_string());

        let launcher = std::env::var("PILA_LAUNCHER")
            .ok()
            .filter(|s| !s.is_empty());

        Self {
            mod_key,
            terminal,
            launcher,
        }
    }

    /// Map a key press to an action. Only called while the modifier is held.
    ///
    /// `sym` is the unshifted (latin) keysym, so Shift is checked via `mods`.
    pub fn action_for(&self, sym: Keysym, mods: &ModifiersState) -> Option<Action> {
        let action = match (sym, mods.shift) {
            (Keysym::Return, false) => Action::Spawn(self.terminal.clone()),
            (Keysym::d, false) => Action::Spawn(self.launcher.clone()?),
            (Keysym::Tab | Keysym::j, false) => Action::CycleFocus,
            (Keysym::m, false) => Action::ToggleMaximize,
            (Keysym::f, false) => Action::ToggleFullscreen,
            (Keysym::q, true) => Action::CloseFocused,
            (Keysym::e, true) => Action::Quit,
            _ => return None,
        };
        Some(action)
    }
}
