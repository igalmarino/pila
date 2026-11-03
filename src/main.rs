//! pila: a stacking Wayland compositor built on Smithay.
//!
//! Structure is based on Smithay's `smallvil` example (MIT licensed,
//! https://github.com/Smithay/smithay), extended with window placement,
//! focus management, keybindings, modifier-drag move/resize, and
//! maximize/fullscreen.
//!
//! Currently runs nested (winit backend) as a window inside another
//! Wayland session.

#![allow(irrefutable_let_patterns)]

mod backend;
mod config;
mod cursor;
mod decoration;
mod focus;
mod foreign_toplevel;
mod frame;
mod grabs;
mod handlers;
mod idle;
mod input;
mod layers;
mod minimize;
mod pointer_constraints;
mod render;
mod scale;
mod session_lock;
mod snap;
mod state;
mod switcher;
mod text;
mod udev;
mod winit;
mod xwayland;

use smithay::reexports::{calloop::EventLoop, wayland_server::Display};

use crate::config::Config;
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
  -h, --help            Show this help

Environment (overrides the config file):
  PILA_MOD       Modifier for bindings: alt (default), super, ctrl
  PILA_TERMINAL  Terminal for Mod+Return and startup (default: foot)
  PILA_LAUNCHER  Command for Mod+D (unset: binding disabled)
  XCURSOR_THEME  Cursor theme on a TTY (default: default)
  XCURSOR_SIZE   Cursor size on a TTY (default: 24)
  RUST_LOG       Log filter (default: info)";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendKind {
    Winit,
    Udev,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    // stderr, so `pila 2> file` captures the log (stdout is the TTY or the
    // nested session's terminal, and clients inherit it).
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .init();

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

    let config = Config::load(config_path.as_deref());
    tracing::info!(
        mod_key = ?config.mod_key,
        terminal = config.terminal,
        launcher = ?config.launcher,
        autostart = ?config.autostart,
        bindings = config.bindings.len(),
        ?backend,
        "starting pila"
    );

    let mut event_loop: EventLoop<'static, Pila> = EventLoop::try_new()?;
    let display: Display<Pila> = Display::new()?;
    let mut state = Pila::new(&mut event_loop, display, config);

    match backend {
        BackendKind::Winit => winit::init_winit(&mut event_loop, &mut state)?,
        BackendKind::Udev => udev::init_udev(&mut event_loop, &mut state)?,
    }

    // `-c`, else the config's autostart, else a terminal. They run once
    // Xwayland is up, so they can be X11 apps.
    let startup = match startup_command {
        Some(command) => vec![command],
        None if !state.config.autostart.is_empty() => state.config.autostart.clone(),
        None => vec![state.config.terminal.clone()],
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

    // Events queued outside a redraw (timers, deferred callbacks) must reach
    // clients too, so flush after every loop iteration.
    event_loop.run(None, &mut state, |state| {
        let _ = state.display_handle.flush_clients();
    })?;

    state.stop_xwayland();

    Ok(())
}
