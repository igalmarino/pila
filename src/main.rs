//! pila: a stacking Wayland compositor built on Smithay.
//!
//! Structure is based on Smithay's `smallvil` example (MIT licensed,
//! https://github.com/Smithay/smithay), extended with window placement,
//! focus management, keybindings, modifier-drag move/resize, and
//! maximize/fullscreen.
//!
//! Runs on a TTY (DRM/KMS + libinput, `udev.rs`), as a login session or by
//! hand, or nested as a window inside another Wayland or X11 session
//! (`winit.rs`).

#![allow(irrefutable_let_patterns)]

mod backend;
mod bell;
mod blur;
mod capture;
mod config;
mod config_reload;
mod cursor;
mod decoration;
mod ext_workspace;
mod focus;
mod foreign_toplevel;
mod frame;
mod gamma_control;
mod gestures;
mod grabs;
mod handlers;
mod idle;
mod input;
mod layers;
mod minimize;
mod output_management;
mod output_power;
mod pointer_constraints;
mod render;
mod rounded;
mod scale;
mod screencopy;
mod session_lock;
mod snap;
mod state;
mod switcher;
mod text;
mod toplevel_management;
mod udev;
mod winit;
mod workspaces;
mod xwayland;

use std::io::IsTerminal;

use smithay::reexports::{calloop::EventLoop, wayland_server::Display};

use crate::config::{Config, ModKey};
pub use crate::state::Pila;

const USAGE: &str = "\
Usage: pila [-c|--command <CMD>] [--config <FILE>] [--backend winit|udev]

Options:
  -c, --command <CMD>   Shell command to run at startup, instead of the
                        config's autostart (default: the terminal)
  --config <FILE>       Config file (default: $XDG_CONFIG_HOME/pila/config.toml
                        or ~/.config/pila/config.toml; see config.example.toml)
  --backend <BACKEND>   winit: a window in another Wayland/X11 session;
                        udev: DRM/KMS on a TTY. Default: winit if
                        WAYLAND_DISPLAY or DISPLAY is set, else udev
  -V, --version         Show the version
  -h, --help            Show this help

Environment (overrides the config file):
  PILA_MOD       Modifier for bindings: super, alt, ctrl (default: super
                 on a TTY, alt nested)
  PILA_TERMINAL  Terminal for Mod+Return and startup (default: the first
                 installed of kitty, foot, alacritty, ghostty, wezterm,
                 konsole, gnome-terminal, xterm)
  PILA_LAUNCHER  Command for Mod+D (unset: binding disabled)
  XCURSOR_THEME  Cursor theme on a TTY (default: default)
  XCURSOR_SIZE   Cursor size on a TTY (default: 24)
  XKB_DEFAULT_LAYOUT, _VARIANT, _OPTIONS
                 Keymap when the config's [keyboard] leaves it empty
  RUST_LOG       Log filter (default: info)

The config file is reloaded when it changes (all but autostart).

Programs pila starts get XDG_CURRENT_DESKTOP=pila, and commands run by
bindings PILA_OUTPUT, the name of the output under the pointer. On a TTY, pila
exports WAYLAND_DISPLAY and DISPLAY to the D-Bus session bus (and to systemd
--user when that's the bus), for the screen-sharing portals: run it as a login
session (scripts/install) or under dbus-run-session (scripts/tty), with
resources/xdg/xdg-desktop-portal/pila-portals.conf and
resources/xdg/xdg-desktop-portal-wlr/pila installed in /etc/xdg or ~/.config.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendKind {
    Winit,
    Udev,
}

/// The crate version, plus `git describe` when built from a checkout.
fn version() -> String {
    let version = env!("CARGO_PKG_VERSION");
    match env!("PILA_GIT_VERSION") {
        "" => version.to_string(),
        git => format!("{version} ({git})"),
    }
}

/// Send panics to the log too: in a login session stderr is the log file or
/// the journal, but the default message is unstructured and has no
/// timestamp. The default hook still runs (it prints the backtrace when
/// RUST_BACKTRACE is set).
fn log_panics() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("(no message)");
        tracing::error!(%location, version = version(), "pila crashed: {message}");
        default_hook(info);
    }));
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    // stderr, so `pila 2> file` captures the log (stdout is the TTY or the
    // nested session's terminal, and clients inherit it). Colors only on a
    // terminal: a log file or the journal (a login session) gets plain text.
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .init();
    log_panics();

    let mut startup_command = None;
    let mut config_path = None;
    let mut backend = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-c" | "--command" => match args.next() {
                Some(cmd) => startup_command = Some(cmd),
                None => {
                    eprintln!("{arg} needs a command\n\n{USAGE}");
                    std::process::exit(2);
                }
            },
            "--config" => match args.next() {
                Some(path) => config_path = Some(std::path::PathBuf::from(path)),
                None => {
                    eprintln!("--config needs a file\n\n{USAGE}");
                    std::process::exit(2);
                }
            },
            "--backend" => match args.next().as_deref() {
                Some("winit") => backend = Some(BackendKind::Winit),
                Some("udev") => backend = Some(BackendKind::Udev),
                _ => {
                    eprintln!("--backend needs winit or udev\n\n{USAGE}");
                    std::process::exit(2);
                }
            },
            "-V" | "--version" => {
                println!("pila {}", version());
                return Ok(());
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => {
                eprintln!("unknown argument: {other}\n\n{USAGE}");
                std::process::exit(2);
            }
        }
    }

    // Slow (system font scan), so it runs in the background; title bars get
    // their text once it's done.
    text::init();

    // Nested if there's a host session to be a window in.
    let backend = backend.unwrap_or_else(|| {
        let host = ["WAYLAND_DISPLAY", "DISPLAY"]
            .iter()
            .any(|var| std::env::var_os(var).is_some_and(|v| !v.is_empty()));
        if host {
            BackendKind::Winit
        } else {
            BackendKind::Udev
        }
    });

    // Nested, the host desktop (e.g. GNOME) grabs Super.
    let default_mod = match backend {
        BackendKind::Winit => ModKey::Alt,
        BackendKind::Udev => ModKey::Super,
    };
    let config_path = Config::path(config_path.as_deref());
    let config = Config::load(config_path.as_deref(), default_mod);
    tracing::info!(
        mod_key = ?config.mod_key,
        terminal = ?config.terminal_command(),
        launcher = ?config.launcher,
        autostart = ?config.autostart,
        bindings = config.bindings.len(),
        ?backend,
        version = version(),
        "starting pila"
    );

    let mut event_loop: EventLoop<'static, Pila> = EventLoop::try_new()?;
    let display: Display<Pila> = Display::new()?;
    let mut state = Pila::new(&mut event_loop, display, config);
    state.config_path = config_path;
    state.watch_config();

    match backend {
        BackendKind::Winit => winit::init_winit(&mut event_loop, &mut state)?,
        BackendKind::Udev => udev::init_udev(&mut event_loop, &mut state)?,
    }

    // `-c`, else the config's autostart, else a terminal. They run once
    // Xwayland is up, so they can be X11 apps.
    let startup = match startup_command {
        Some(command) => vec![command],
        None if !state.config.autostart.is_empty() => state.config.autostart.clone(),
        None => state.config.terminal_command().into_iter().collect(),
    };
    state.start_xwayland(startup);

    // Ctrl+C or `kill` quit like Mod+Shift+E, so the cleanup below runs.
    let signals = calloop::signals::Signals::new(&[
        calloop::signals::Signal::SIGINT,
        calloop::signals::Signal::SIGTERM,
    ])?;
    event_loop
        .handle()
        .insert_source(signals, |event, _, state: &mut Pila| {
            tracing::info!(signal = ?event.signal(), "quitting");
            state.loop_signal.stop();
        })?;

    // Once per iteration, after any redraws (idle callbacks run last): the
    // post-frame housekeeping, then a flush, which events queued outside a
    // redraw (timers, deferred callbacks) need too.
    event_loop.run(None, &mut state, |state| {
        state.after_frames();
        let _ = state.display_handle.flush_clients();
    })?;

    state.stop_xwayland();
    // Leave the monitors as found: no night-light ramps for the next
    // compositor on this seat.
    if let Some(backend) = &mut state.backend {
        backend.reset_gamma();
    }

    Ok(())
}
