//! Reloading the config file when it changes.
//!
//! pila watches the config file's directory with inotify (editors often
//! replace the file instead of writing it, which a watch on the file itself
//! would lose) and reloads shortly after a change to the file, so a burst of
//! writes is one reload. A symlinked config's target is watched in its own
//! directory, followed when the link is pointed elsewhere; a queue that
//! overflowed reloads too, as the config's change may be among the events
//! lost. An invalid file is reported (log and a desktop notification) and
//! the running config stays.
//!
//! What applies live: bindings, Mod, terminal and launcher (read when used),
//! the keyboard (keymap and repeat), `[input]` on connected devices, and
//! `[[output]]` (mode, scale and position on a TTY; the nested output's
//! scale). `autostart` only runs at startup.

use std::{
    ffi::OsString,
    mem::MaybeUninit,
    os::{
        fd::{AsFd, BorrowedFd},
        unix::ffi::OsStrExt,
    },
    path::PathBuf,
    time::Duration,
};

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
        let fd = match inotify::init(inotify::CreateFlags::CLOEXEC | inotify::CreateFlags::NONBLOCK)
        {
            Ok(fd) => fd,
            Err(err) => {
                warn!(?err, "can't watch the config file");
                return;
            }
        };
        let dir_wd = match inotify::add_watch(&fd, dir, watch_flags()) {
            Ok(wd) => wd,
            Err(err) => {
                info!(dir = %dir.display(), ?err, "not watching the config (no such directory?)");
                return;
            }
        };
        let mut watch = ConfigWatch {
            path: path.clone(),
            file: file.to_owned(),
            dir_wd,
            target: None,
        };
        watch.follow_target(fd.as_fd());
        let source = Generic::new(fd, Interest::READ, Mode::Level);
        let inserted = self.loop_handle.insert_source(source, move |_, fd, state| {
            Ok(on_ready(
                fd.as_fd(),
                &mut watch,
                state,
                Pila::schedule_config_reload,
            ))
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

/// What the watch on the config follows: the file in its directory and, for
/// a symlink, the file it points to, in that one's directory.
struct ConfigWatch {
    path: PathBuf,
    /// The config's file name, in the directory watched as `dir_wd`.
    file: OsString,
    dir_wd: i32,
    /// A symlinked config's target: its directory, that one's watch and the
    /// target's file name.
    target: Option<(PathBuf, i32, OsString)>,
}

/// The changes that can touch the config: written, moved or linked in
/// (editors and dotfiles managers replace it), deleted.
fn watch_flags() -> inotify::WatchFlags {
    inotify::WatchFlags::CLOSE_WRITE
        | inotify::WatchFlags::MOVED_TO
        | inotify::WatchFlags::CREATE
        | inotify::WatchFlags::DELETE
}

impl ConfigWatch {
    /// Watch the directory of the file a symlinked config points to now
    /// (dotfiles managers keep it elsewhere, and point the link elsewhere
    /// again on each change: stow, home-manager), and stop watching the one
    /// it pointed to before.
    fn follow_target(&mut self, fd: BorrowedFd<'_>) {
        let is_link = self.path.is_symlink();
        let target = is_link
            .then(|| std::fs::canonicalize(&self.path).ok())
            .flatten();
        let parts = target
            .as_deref()
            .and_then(|t| Some((t.parent()?.to_owned(), t.file_name()?.to_owned())));
        if is_link && parts.is_none() {
            info!(path = %self.path.display(), "the config is a broken symlink");
        }
        let old = self.target.take();
        let new = parts.and_then(|(dir, name)| {
            if let Some((_, wd, _)) = old.as_ref().filter(|(old_dir, ..)| *old_dir == dir) {
                return Some((dir, *wd, name));
            }
            match inotify::add_watch(fd, &dir, watch_flags()) {
                Ok(wd) => {
                    info!(dir = %dir.display(), "watching the config's target");
                    Some((dir, wd, name))
                }
                Err(err) => {
                    info!(dir = %dir.display(), ?err, "not watching the config's target");
                    None
                }
            }
        });
        if let Some((_, old_wd, _)) = old {
            let still_used =
                old_wd == self.dir_wd || new.as_ref().is_some_and(|(_, wd, _)| *wd == old_wd);
            if !still_used {
                let _ = inotify::remove_watch(fd, old_wd);
            }
        }
        self.target = new;
    }
}

/// Read the inotify events waiting on `fd` and schedule a reload with
/// `reload` if one touches the config. A read that fails for good (not a
/// wait or an interruption) would wake the loop again and again, the fd
/// staying readable: the watch stops then, said once.
fn on_ready<D>(
    fd: BorrowedFd<'_>,
    watch: &mut ConfigWatch,
    data: &mut D,
    reload: impl FnOnce(&mut D),
) -> PostAction {
    let events = read_events(fd, watch);
    if events.changed {
        reload(data);
    }
    match events.failed {
        Some(err) => {
            warn!(
                ?err,
                "reading config file events failed; no longer watching the config"
            );
            PostAction::Remove
        }
        None => PostAction::Continue,
    }
}

/// What the events read at once said.
struct Events {
    /// One touched the config (or some were lost: it may have).
    changed: bool,
    /// The read failed for good.
    failed: Option<Errno>,
}

/// Read the inotify events waiting on `fd`: whether one touches the config,
/// following a symlinked config to its new target when the link changes.
fn read_events(fd: BorrowedFd<'_>, watch: &mut ConfigWatch) -> Events {
    let mut buf = [MaybeUninit::<u8>::uninit(); 4096];
    let mut reader = inotify::Reader::new(fd, &mut buf);
    let mut changed = false;
    let mut link_changed = false;
    let mut failed = None;
    loop {
        match reader.next() {
            Ok(event) => {
                if event.events().contains(inotify::ReadFlags::QUEUE_OVERFLOW) {
                    // Events were lost, the config's among them maybe.
                    info!("config file events overflowed; reloading");
                    changed = true;
                    link_changed = true;
                    continue;
                }
                let Some(name) = event.file_name() else {
                    continue;
                };
                let name = name.to_bytes();
                if event.wd() == watch.dir_wd && name == watch.file.as_bytes() {
                    changed = true;
                    link_changed = true;
                } else if watch
                    .target
                    .as_ref()
                    .is_some_and(|(_, wd, target)| event.wd() == *wd && name == target.as_bytes())
                {
                    changed = true;
                }
            }
            Err(Errno::WOULDBLOCK | Errno::INTR) => break,
            Err(err) => {
                failed = Some(err);
                break;
            }
        }
    }
    if link_changed && failed.is_none() {
        watch.follow_target(fd);
    }
    Events { changed, failed }
}

#[cfg(test)]
mod tests {
    use std::{
        os::{fd::AsFd, unix::net::UnixStream},
        time::Duration,
    };

    use smithay::reexports::calloop::{generic::Generic, EventLoop, Interest, Mode};

    use super::{on_ready, ConfigWatch};

    /// A read that fails for good stops the watch at once: in `Mode::Level`
    /// the fd stays readable, and the loop would spin on it, logging each
    /// time. A socket whose other end is closed reads as end of file, which
    /// an inotify fd never does (`EINVAL`).
    #[test]
    fn a_read_that_fails_for_good_stops_the_watch() {
        let (ours, theirs) = UnixStream::pair().unwrap();
        ours.set_nonblocking(true).unwrap();
        drop(theirs);
        let mut event_loop: EventLoop<(usize, usize)> = EventLoop::try_new().unwrap();
        let mut watch = ConfigWatch {
            path: "/nonexistent/config.toml".into(),
            file: "config.toml".into(),
            dir_wd: 1,
            target: None,
        };
        let source = Generic::new(
            std::os::fd::OwnedFd::from(ours),
            Interest::READ,
            Mode::Level,
        );
        event_loop
            .handle()
            .insert_source(source, move |_, fd, (calls, reloads)| {
                *calls += 1;
                Ok(on_ready(fd.as_fd(), &mut watch, reloads, |r| *r += 1))
            })
            .unwrap();
        let mut counts = (0, 0);
        for _ in 0..5 {
            event_loop
                .dispatch(Some(Duration::ZERO), &mut counts)
                .unwrap();
        }
        assert_eq!(counts, (1, 0), "(reads, reloads)");
    }
}
