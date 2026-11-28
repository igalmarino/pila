//! Reloading the config file when it changes.
//!
//! pila watches the config file's directory with inotify (editors often
//! replace the file instead of writing it, which a watch on the file itself
//! would lose) and reloads shortly after a change to the file, so a burst of
//! writes is one reload. An invalid file is reported (log and a desktop
//! notification) and the running config stays.
//!
//! What applies live: bindings, Mod, terminal and launcher (read when used),
//! the keyboard (keymap and repeat), `[input]` on connected devices, and
//! `[[output]]` (mode, scale and position on a TTY; the nested output's
//! scale). `autostart` only runs at startup.

use std::{ffi::OsString, mem::MaybeUninit, os::unix::ffi::OsStrExt, time::Duration};

use smithay::{
    desktop::layer_map_for_output,
    output::Scale,
    reexports::{
        calloop::{
            generic::Generic,
            timer::{TimeoutAction, Timer},
            Interest, Mode, PostAction,
        },
        rustix::{fs::inotify, io::Errno},
    },
};
use tracing::{info, warn};

use crate::{config::Config, Pila};

/// How long after the first change to reload: the changes that come
/// meanwhile go with it.
const SETTLE: Duration = Duration::from_millis(250);
/// How long a notification that all went well stays.
const NOTICE_MS: u32 = 3000;
/// How long a notification of a problem stays: an error can be long.
const ERROR_MS: u32 = 10_000;

impl Pila {
    /// Start watching `config_path` for changes. Without a config directory
    /// there's nothing to watch (it may be created later; restart pila then).
    pub fn watch_config(&mut self) {
        let Some(path) = self.config_path.clone() else {
            return;
        };
        let Some(file) = path.file_name() else {
            return;
        };
        // `--config foo.toml` has an empty parent: the current directory.
        let dir = path
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .unwrap_or(std::path::Path::new("."));
        let mut names = vec![file.to_owned()];
        let fd = match inotify::init(inotify::CreateFlags::CLOEXEC | inotify::CreateFlags::NONBLOCK)
        {
            Ok(fd) => fd,
            Err(err) => {
                warn!(?err, "can't watch the config file");
                return;
            }
        };
        let flags = inotify::WatchFlags::CLOSE_WRITE
            | inotify::WatchFlags::MOVED_TO
            | inotify::WatchFlags::CREATE
            | inotify::WatchFlags::DELETE;
        if let Err(err) = inotify::add_watch(&fd, dir, flags) {
            info!(dir = %dir.display(), ?err, "not watching the config (no such directory?)");
            return;
        }
        // A symlinked config (dotfiles managers) is edited where it points
        // to: watch that file's directory too.
        if path.is_symlink() {
            let target = std::fs::canonicalize(&path).ok();
            let parts = target
                .as_deref()
                .and_then(|t| Some((t.parent()?, t.file_name()?)));
            match parts {
                Some((target_dir, target_file)) => {
                    match inotify::add_watch(&fd, target_dir, flags) {
                        Ok(_) => names.push(target_file.to_owned()),
                        Err(err) => {
                            info!(dir = %target_dir.display(), ?err, "not watching the config's target")
                        }
                    }
                }
                None => info!(path = %path.display(), "the config is a broken symlink"),
            }
        }
        let source = Generic::new(fd, Interest::READ, Mode::Level);
        let inserted = self.loop_handle.insert_source(source, move |_, fd, state| {
            if changed(fd, &names) {
                state.schedule_config_reload();
            }
            Ok(PostAction::Continue)
        });
        match inserted {
            Ok(_) => info!(path = %path.display(), "watching the config for changes"),
            Err(err) => warn!(?err, "can't watch the config file"),
        }
    }

    /// Reload once the file has settled.
    fn schedule_config_reload(&mut self) {
        if self.config_reload_pending {
            return;
        }
        self.config_reload_pending = true;
        let timer = Timer::from_duration(SETTLE);
        let inserted = self.loop_handle.insert_source(timer, |_, _, state| {
            state.config_reload_pending = false;
            state.reload_config();
            TimeoutAction::Drop
        });
        if let Err(err) = inserted {
            self.config_reload_pending = false;
            warn!(?err, "can't schedule the config reload");
        }
    }

    /// Read the config again and apply what changed.
    pub fn reload_config(&mut self) {
        let path = self.config_path.clone();
        match Config::read(path.as_deref(), self.config.default_mod) {
            Ok(config) => {
                let warnings = config.warnings.join("\n");
                self.apply_config(config);
                info!("config reloaded");
                if warnings.is_empty() {
                    self.notify("Config reloaded", "");
                } else {
                    warn!(%warnings, "config warning");
                    self.notify_error("Config reloaded with a warning", &warnings);
                }
            }
            Err(err) => {
                warn!(%err, "config not reloaded; keeping the running one");
                self.notify_error("Config not reloaded", &err);
            }
        }
    }

    fn apply_config(&mut self, config: Config) {
        let old = std::mem::replace(&mut self.config, config);
        if old.keyboard != self.config.keyboard {
            self.apply_keyboard_config();
        }
        // The wheel's cooldown is read when used; the rest is libinput's.
        if old.input.touchpad != self.config.input.touchpad
            || old.input.mouse != self.config.input.mouse
        {
            if let Some(backend) = &mut self.backend {
                backend.apply_input_config(&self.config.input);
            }
        }
        if old.workspaces != self.config.workspaces {
            self.apply_workspace_count();
        }
        if old.gaps != self.config.gaps {
            self.refit_to_usable_area();
        }
        if old.xwayland != self.config.xwayland {
            self.xwayland_config_changed();
        }
        if old.window_rules != self.config.window_rules
            || old.layer_rules != self.config.layer_rules
        {
            self.refresh_rule_props();
        }
        if old.permissions != self.config.permissions
            || old.permission_rules != self.config.permission_rules
        {
            self.permissions_config_changed();
        }
        // Turned on or off, or a new accent for its icon, or another bus.
        if old.permissions.tray != self.config.permissions.tray
            || old.colors.snap_preview != self.config.colors.snap_preview
            || old.environment != self.config.environment
        {
            self.update_tray();
        }
        let outputs_changed = old.outputs != self.config.outputs;
        if outputs_changed {
            // An edited [[output]] section takes over from what a display
            // tool set for that monitor; untouched sections don't.
            let (monitors, config) = (&self.outputs.monitors, &self.config);
            self.outputs.overrides.retain(|name, _| {
                let monitor = monitors.get(name);
                old.output(name, monitor) == config.output(name, monitor)
            });
        }
        if old.other_gpus != self.config.other_gpus {
            crate::backend::apply_other_gpus(self);
        }
        if outputs_changed {
            self.apply_outputs();
        }
        if !self.config.cursor.shake_to_find && self.shake_cursor.magnified() {
            self.reset_shake_cursor();
        }
        // A new theme or size: Pila's cursor at once (the redraw below), and
        // the apps started from now on (`session_env`).
        let cursor_changed = (&old.cursor.theme, old.cursor.size)
            != (&self.config.cursor.theme, self.config.cursor.size);
        if cursor_changed {
            self.cursor_theme = crate::cursor::CursorTheme::load(&self.config.cursor);
        }
        // What Pila starts from now on gets it (`session_env`); on bare
        // metal, the bus and systemd too.
        if cursor_changed || old.environment != self.config.environment {
            self.export_environment();
        }
        // Clients that look like Pila take them up (`pila msg subscribe colors`).
        if old.colors != self.config.colors {
            self.send_colors_event();
            // The tools' too, for their next start (`tool_configs.rs`).
            if self.config.tool_configs && matches!(self.tool_configs_dir.get(), Some(Some(_))) {
                crate::tool_configs::rewrite_in_background(self.config.colors);
            }
        }
        // The shortcuts shown: its keys, colors and corners as they are now.
        self.shortcuts_config_changed();
        // Every panel shown: its text in the colors as they are now.
        self.draw_panels_text();
        self.queue_redraw();
    }

    /// Re-apply the outputs' settings (`[[output]]` with the runtime
    /// overrides on top) and fit everything to the result. Whether the
    /// backend could do all of it.
    pub(crate) fn apply_outputs(&mut self) -> bool {
        let applied = crate::backend::apply_output_config(self);
        self.fit_to_outputs();
        applied
    }

    /// Fit layer surfaces, windows, lock surfaces and the heads clients
    /// see to the outputs' current modes, scales and places: after
    /// `apply_outputs`, or a mode the backend had to change by itself (a
    /// lighter one when an output came back on, `udev::outputs`).
    pub(crate) fn fit_to_outputs(&mut self) {
        for output in self.space.outputs().cloned().collect::<Vec<_>>() {
            layer_map_for_output(&output).arrange();
        }
        // Also refits maximized and fullscreen windows.
        self.fixup_windows();
        self.configure_lock_surfaces();
        self.refresh_output_heads();
        self.queue_redraw();
    }

    pub(crate) fn apply_keyboard_config(&mut self) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        // Replaces a virtual keyboard's keymap too (`virtual_keyboard.rs`).
        self.input.virtual_keymap = None;
        self.input.configured_keymap = None;
        let kb = self.config.keyboard.clone();
        if let Err(err) = keyboard.set_xkb_config(self, crate::state::xkb_config(&kb)) {
            warn!(
                ?err,
                ?kb,
                "invalid keyboard config; keeping the current keymap"
            );
            self.notify_error(
                "Keyboard config not applied",
                &format!(
                    "{}: xkbcommon doesn't know it; keeping the current keymap.",
                    crate::config::keymap_description(&kb)
                ),
            );
        }
        keyboard.change_repeat_info(kb.repeat_rate, kb.repeat_delay);
        // A new keymap starts with every lock off.
        if kb.numlock && crate::state::lock_num_lock(&keyboard) {
            // `set_modifier_state` tells neither the client nor the LEDs.
            keyboard.advertise_modifier_state(self);
            if let Some(backend) = &mut self.backend {
                backend.update_leds(keyboard.led_state());
            }
        }
    }

    /// Nested: only the scale of the `winit` output applies.
    pub(crate) fn apply_nested_scale(&mut self) {
        let scale = self.effective_output_config("winit").scale.unwrap_or(1.0);
        for output in self.space.outputs() {
            output.change_current_state(None, None, Some(Scale::Fractional(scale)), None);
        }
    }

    /// A desktop notification that something went well: a glance is enough.
    fn notify(&mut self, summary: &str, body: &str) {
        self.send_notification(summary, body, NOTICE_MS);
    }

    /// A desktop notification of a problem, with time to read what it says.
    pub(crate) fn notify_error(&mut self, summary: &str, body: &str) {
        self.send_notification(summary, body, ERROR_MS);
    }

    /// A desktop notification, over D-Bus if there's a notification daemon
    /// such as mako (`notifications.rs`: the log says once if there isn't);
    /// also in the log. On the session bus Pila's programs get: an
    /// `[environment]` one first.
    fn send_notification(&mut self, summary: &str, body: &str, milliseconds: u32) {
        info!(summary, body, "notification");
        let bus = self.session_bus_address();
        self.notifier.send(bus, summary, body, milliseconds);
    }

    /// The session bus Pila's programs get, where their notification daemon
    /// and tray are: an `[environment]` one first, else Pila's own; `None`
    /// without one.
    pub(crate) fn session_bus_address(&self) -> Option<String> {
        const BUS: &str = "DBUS_SESSION_BUS_ADDRESS";
        match self
            .config
            .environment
            .iter()
            .find(|(name, _)| *name == BUS)
        {
            Some((_, value)) => value.clone(),
            None => std::env::var(BUS).ok(),
        }
    }
}

/// Whether the inotify events waiting on `fd` touch one of the files `names`.
fn changed(fd: &impl std::os::fd::AsFd, names: &[OsString]) -> bool {
    let mut buf = [MaybeUninit::<u8>::uninit(); 4096];
    let mut reader = inotify::Reader::new(fd, &mut buf);
    let mut changed = false;
    loop {
        match reader.next() {
            Ok(event) => {
                let ours = event
                    .file_name()
                    .is_some_and(|n| names.iter().any(|name| n.to_bytes() == name.as_bytes()));
                changed |= ours;
            }
            Err(Errno::WOULDBLOCK | Errno::INTR) => break,
            Err(err) => {
                warn!(?err, "reading config file events failed");
                break;
            }
        }
    }
    changed
}
