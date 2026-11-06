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

use std::{
    ffi::OsString, mem::MaybeUninit, os::unix::ffi::OsStrExt, process::Command, sync::Once,
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

/// How long after the last change to reload.
const SETTLE: Duration = Duration::from_millis(250);

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
                self.apply_config(config);
                info!("config reloaded");
                self.notify("Config reloaded", "");
            }
            Err(err) => {
                warn!(%err, "config not reloaded; keeping the running one");
                self.notify("Config not reloaded", &err);
            }
        }
    }

    fn apply_config(&mut self, config: Config) {
        let old = std::mem::replace(&mut self.config, config);
        if old.keyboard != self.config.keyboard {
            self.apply_keyboard_config();
        }
        if old.input != self.config.input {
            if let Some(backend) = &mut self.backend {
                backend.apply_input_config(&self.config.input);
            }
        }
        if old.workspaces != self.config.workspaces {
            self.apply_workspace_count();
        }
        if old.window_rules != self.config.window_rules
            || old.layer_rules != self.config.layer_rules
        {
            self.refresh_rule_props();
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
        // `disable-internal-display` decides which outputs are on, too. A
        // display tool's applies pin every head's on/off state; a changed
        // rule takes over the built-in screens from them.
        let rule_changed = old.internal_display != self.config.internal_display;
        if rule_changed {
            for (name, over) in &mut self.outputs.overrides {
                if crate::udev::is_internal(name) {
                    over.enabled = None;
                }
            }
        }
        if outputs_changed || rule_changed {
            self.apply_outputs();
        }
        self.queue_redraw();
    }

    /// Re-apply the outputs' settings (`[[output]]` with the runtime
    /// overrides on top) and fit everything to the result. Whether the
    /// backend could do all of it.
    pub(crate) fn apply_outputs(&mut self) -> bool {
        let applied = crate::backend::apply_output_config(self);
        for output in self.space.outputs().cloned().collect::<Vec<_>>() {
            layer_map_for_output(&output).arrange();
        }
        // Also refits maximized and fullscreen windows.
        self.fixup_windows();
        self.configure_lock_surfaces();
        self.refresh_output_heads();
        self.queue_redraw();
        applied
    }

    fn apply_keyboard_config(&mut self) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let kb = self.config.keyboard.clone();
        if let Err(err) = keyboard.set_xkb_config(self, crate::state::xkb_config(&kb)) {
            warn!(
                ?err,
                ?kb,
                "invalid keyboard config; keeping the current keymap"
            );
            self.notify(
                "Keyboard config not applied",
                &format!(
                    "{}: xkbcommon doesn't know it; keeping the current keymap.",
                    crate::config::keymap_description(&kb)
                ),
            );
        }
        keyboard.change_repeat_info(kb.repeat_rate, kb.repeat_delay);
    }

    /// Nested: only the scale of the `winit` output applies.
    pub(crate) fn apply_nested_scale(&mut self) {
        let file = self.config.output("winit", None).and_then(|c| c.scale);
        let scale = self
            .outputs
            .overrides
            .get("winit")
            .and_then(|c| c.scale)
            .or(file)
            .unwrap_or(1.0);
        for output in self.space.outputs() {
            output.change_current_state(None, None, Some(Scale::Fractional(scale)), None);
        }
    }

    /// A desktop notification (through `notify-send`, if there's a
    /// notification daemon such as mako); also in the log. Without
    /// `notify-send` they silently don't show, so the log says why, once.
    pub(crate) fn notify(&mut self, summary: &str, body: &str) {
        info!(summary, body, "notification");
        let mut cmd = Command::new("notify-send");
        cmd.args(["--app-name=pila", "--expire-time=3000", summary]);
        if !body.is_empty() {
            cmd.arg(body);
        }
        self.session_env(&mut cmd);
        match cmd.spawn() {
            Ok(child) => self.adopt_child(child),
            Err(err) => {
                static WARNED: Once = Once::new();
                WARNED.call_once(|| {
                    warn!(
                        ?err,
                        "can't run notify-send: no desktop notifications (package libnotify)"
                    )
                });
            }
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
