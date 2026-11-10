//! pila: a stacking Wayland compositor built on Smithay.
//!
//! Structure is based on Smithay's `smallvil` example (MIT licensed,
//! https://github.com/Smithay/smithay), extended with window placement,
//! focus management, keybindings, modifier-drag move/resize, and
//! maximize/fullscreen.
//!
//! Runs on a TTY (DRM/KMS + libinput, `udev/`), as a login session or by
//! hand, or nested as a window inside another Wayland or X11 session
//! (`winit.rs`).

#![allow(irrefutable_let_patterns)]

mod animation;
mod backend;
mod bell;
mod blur;
mod capture;
mod commit_pacing;
mod config;
mod config_reload;
mod cursor;
mod decoration;
mod edid;
mod ext_workspace;
mod focus;
mod foreign_toplevel;
mod frame;
mod frame_stats;
mod gamma_control;
mod gestures;
mod grabs;
mod handlers;
mod headless;
mod idle;
mod input;
mod introspect;
mod ipc;
mod ipc_client;
mod keyboard_layout;
mod layers;
mod minimize;
mod output_management;
mod output_power;
mod pointer_constraints;
mod redraw;
mod render;
mod rounded;
mod rules;
mod scale;
mod screencopy;
mod session_lock;
mod session_management;
mod session_protocol;
mod snap;
mod stacking;
mod state;
mod surface_limits;
mod switcher;
mod test_control;
mod text;
mod toplevel_management;
mod transients;
mod udev;
mod urgency;
mod virtual_keyboard;
mod window_menu;
mod winit;
mod workspace_slide;
mod workspaces;
mod xwayland;

use std::io::IsTerminal;

use smithay::reexports::{calloop::EventLoop, wayland_server::Display};

use crate::config::{Config, ModKey};
pub use crate::state::Pila;

const USAGE: &str = "\
Usage: pila [-c|--command <CMD>] [--config <FILE>] [--backend winit|udev|headless]
            [--headless-outputs <OUTPUTS>]
       pila --check-config [--config <FILE>]
       pila msg [--json] <request>    (pila msg --help)

Options:
  -c, --command <CMD>   Shell command to run at startup, instead of the
                        config's autostart (default: the terminal)
  --config <FILE>       Config file (default: $XDG_CONFIG_HOME/pila/config.toml
                        or ~/.config/pila/config.toml; see config.example.toml)
  --backend <BACKEND>   winit: a window in another Wayland/X11 session;
                        udev: DRM/KMS on a TTY; headless: virtual outputs
                        and a control socket, for automated tests
                        ($XDG_RUNTIME_DIR/<WAYLAND_DISPLAY>.control). Default:
                        winit if WAYLAND_DISPLAY or DISPLAY is set, else udev
  --headless-outputs <OUTPUTS>
                        The headless backend's outputs,
                        NAME:WxH[@HZ][=MAKE/MODEL[/SERIAL]] separated by commas
                        (default: HEADLESS-1:1280x800@60)
  --check-config        Check the config file (as Pila would read it, the
                        keymap included) and exit: 0 if it's valid, 1 if not
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
  RUST_LOG       Log filter (default: info); pila::frames=debug logs frame
                 times and missed refreshes per output

The config file is reloaded when it changes (all but autostart).

Programs Pila starts get XDG_CURRENT_DESKTOP=pila and PILA_SOCKET (pila msg),
and commands run by bindings PILA_OUTPUT, the name of the output under the
pointer. On a TTY, Pila
exports WAYLAND_DISPLAY, DISPLAY and PILA_SOCKET to the D-Bus session bus (and to
systemd --user when that's the bus), for the screen-sharing portals and shells
started as services: run it as a login
session (scripts/install) or under dbus-run-session (scripts/tty), with
resources/xdg/xdg-desktop-portal/pila-portals.conf and
resources/xdg/xdg-desktop-portal-wlr/pila installed in /etc/xdg or ~/.config.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendKind {
    Winit,
    Udev,
    Headless,
}

/// `--check-config`: read the config as pila would and say whether it's
/// valid. Returns the exit status.
fn run_config_check(path: Option<&std::path::Path>) -> i32 {
    let path = Config::path(path);
    let shown = path
        .as_deref()
        .map_or("(no config path)".to_string(), |p| p.display().to_string());
    let exists = path.as_deref().is_some_and(std::path::Path::exists);
    // The Mod default only matters for running; any will do.
    let config = match Config::read(path.as_deref(), ModKey::Super) {
        Ok(config) => config,
        Err(err) => {
            println!("{shown}: invalid\n{err}");
            return 1;
        }
    };
    if let Err(err) = config::check_keymap(&config.keyboard) {
        println!("{shown}: invalid\n{err}");
        return 1;
    }
    if exists {
        println!("{shown}: OK");
    } else {
        println!("{shown}: no such file; pila uses the defaults");
    }
    0
}

/// The crate version, plus `git describe` when built from a checkout.
fn version() -> String {
    let version = env!("CARGO_PKG_VERSION");
    match env!("PILA_GIT_VERSION") {
        "" => version.to_string(),
        git => format!("{version} ({git})"),
    }
}

/// Send panics to the log too, with their backtrace: in a login session
/// stderr is the log file or the journal, but the default message is
/// unstructured, has no timestamp and no backtrace without RUST_BACKTRACE,
/// which a session doesn't set (a tester's first report should say where it
/// happened). The default hook still runs.
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
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!(%location, version = version(), "pila crashed: {message}\n{backtrace}");
        default_hook(info);
    }));
}

/// Why `name` can't be loaded at runtime, or `None` if it can. Smithay loads
/// EGL by itself when a backend starts and panics if it can't, so that's
/// checked first, to say what's wrong and leave cleanly.
fn missing_library(name: &str) -> Option<String> {
    let path = std::ffi::CString::new(name).ok()?;
    // SAFETY: `path` is a valid C string; a library opened is closed right
    // away (a library Smithay opens later is loaded again), and dlerror's
    // message is copied before anything else can change it.
    unsafe {
        let library = libc::dlopen(path.as_ptr(), libc::RTLD_LAZY);
        if !library.is_null() {
            libc::dlclose(library);
            return None;
        }
        let reason = libc::dlerror();
        let reason = if reason.is_null() {
            "no reason given".into()
        } else {
            std::ffi::CStr::from_ptr(reason).to_string_lossy()
        };
        Some(format!(
            "can't load {name}, which pila needs at runtime: {reason}. Install the package that \
             has it (libglvnd or Mesa, depending on the distro); if it's outside the standard \
             library paths, as on NixOS, add its directory to LD_LIBRARY_PATH"
        ))
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Talks to a running pila: nothing below applies.
    let all_args: Vec<String> = std::env::args().skip(1).collect();
    if all_args.first().is_some_and(|arg| arg == "msg") {
        std::process::exit(ipc_client::run(&all_args[1..]));
    }
    let mut startup_command = None;
    let mut check_config = false;
    let mut config_path = None;
    let mut backend = None;
    let mut headless_outputs = None;
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
                Some("headless") => backend = Some(BackendKind::Headless),
                _ => {
                    eprintln!("--backend needs winit, udev or headless\n\n{USAGE}");
                    std::process::exit(2);
                }
            },
            "--headless-outputs" => match args.next().map(|s| headless::parse_output_specs(&s)) {
                Some(Ok(specs)) if !specs.is_empty() => headless_outputs = Some(specs),
                Some(Err(err)) => {
                    eprintln!("--headless-outputs: {err}\n\n{USAGE}");
                    std::process::exit(2);
                }
                _ => {
                    eprintln!("--headless-outputs needs outputs\n\n{USAGE}");
                    std::process::exit(2);
                }
            },
            "--check-config" => check_config = true,
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

    // A check says only what's wrong, unless RUST_LOG asks for more.
    let default_level = if check_config { "warn" } else { "info" };
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_level));
    // stderr, so `pila 2> file` captures the log (stdout is the TTY or the
    // nested session's terminal, and clients inherit it). Colors only on a
    // terminal: a log file or the journal (a login session) gets plain text.
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .init();
    log_panics();

    if check_config {
        std::process::exit(run_config_check(config_path.as_deref()));
    }
    state::raise_file_limit();

    // Ctrl+C or `kill` quit like Mod+Shift+E, so the cleanup below runs;
    // SIGCHLD reaps the programs pila started. Set up before any thread
    // exists: the source blocks these signals in this thread, threads
    // inherit that, and a thread that didn't block them could take one
    // (process-directed) instead of the event loop.
    let mut event_loop: EventLoop<'static, Pila> = EventLoop::try_new()?;
    let signals = calloop::signals::Signals::new(&[
        calloop::signals::Signal::SIGINT,
        calloop::signals::Signal::SIGTERM,
        calloop::signals::Signal::SIGCHLD,
    ])?;
    event_loop
        .handle()
        .insert_source(signals, |event, _, state: &mut Pila| {
            if event.signal() == calloop::signals::Signal::SIGCHLD {
                state.reap_children();
            } else {
                tracing::info!(signal = ?event.signal(), "quitting");
                state.loop_signal.stop();
            }
        })?;

    // Slow (system font scan), so it runs in the background; title bars get
    // their text once it's done, with a redraw it asks for.
    let (fonts_ready, fonts_source) = calloop::ping::make_ping()?;
    event_loop
        .handle()
        .insert_source(fonts_source, |_, _, state: &mut Pila| state.queue_redraw())?;
    text::init(move || fonts_ready.ping());

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
        BackendKind::Udev | BackendKind::Headless => ModKey::Super,
    };
    let config_path = Config::path(config_path.as_deref());
    let (config, config_error) = Config::load(config_path.as_deref(), default_mod);
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

    if let Some(error) = missing_library("libEGL.so.1") {
        return Err(error.into());
    }

    let display: Display<Pila> = Display::new()?;
    let mut state = Pila::new(&mut event_loop, display, config);
    state.config_path = config_path;
    state.watch_config();
    if let Some(err) = config_error {
        state::notify_after_startup(
            &state.loop_handle,
            "Invalid config, using the defaults",
            err,
        );
    }
    // Nested, a pila on the side mustn't touch the login session's file.
    if backend != BackendKind::Winit {
        state.session_management.load_and_persist();
    }

    ipc::init(&mut state);

    match backend {
        BackendKind::Winit => winit::init_winit(&mut event_loop, &mut state)?,
        BackendKind::Udev => udev::init_udev(&mut event_loop, &mut state)?,
        BackendKind::Headless => {
            let specs = headless_outputs.unwrap_or_else(|| {
                headless::parse_output_specs(headless::DEFAULT_OUTPUTS).expect("valid")
            });
            headless::init_headless(&mut state, &specs)?;
            test_control::init(&mut state)?;
        }
    }

    // `-c`, else the config's autostart, else a terminal. They run once
    // Xwayland is up, so they can be X11 apps.
    let startup = match startup_command {
        Some(command) => vec![command],
        None if !state.config.autostart.is_empty() => state.config.autostart.clone(),
        // Tests start their own clients.
        None if backend == BackendKind::Headless => Vec::new(),
        None => state.config.terminal_command().into_iter().collect(),
    };
    state.start_xwayland(startup);

    // Once per iteration, after any redraws (idle callbacks run last): the
    // post-frame housekeeping, then a flush, which events queued outside a
    // redraw (timers, deferred callbacks) need too.
    event_loop.run(None, &mut state, |state| {
        state.after_frames();
        let _ = state.display_handle.flush_clients();
    })?;

    ipc::close(&state);
    state.report_frames();
    state.finish_sessions();
    state.stop_xwayland();
    // Leave the monitors as found: no night-light ramps for the next
    // compositor on this seat.
    if let Some(backend) = &mut state.backend {
        backend.reset_gamma();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{missing_library, USAGE};

    #[test]
    fn the_man_page_has_every_option_and_variable_of_the_usage() {
        let man = include_str!("../docs/pila.1").replace("\\-", "-");
        // `--flag`s and `-x`s, and the variables (all capitals, at least
        // one underscore, as PILA_MOD and XKB_DEFAULT_LAYOUT are written).
        // pila's own flags are those above the environment (below, "systemd
        // --user" is another program's).
        let options = USAGE.split("Environment").next().unwrap();
        let words = USAGE.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        for word in words {
            let flag = word.starts_with('-')
                && word.len() > 1
                && !word.ends_with('-')
                && options.contains(word);
            let variable = word.contains('_')
                && word.chars().all(|c| c.is_ascii_uppercase() || c == '_')
                && !word.starts_with('_');
            if (flag || variable) && !man.contains(word) {
                panic!("docs/pila.1 doesn't mention {word} from USAGE");
            }
        }
    }

    #[test]
    fn a_library_that_is_there_is_not_missing() {
        assert_eq!(missing_library("libc.so.6"), None);
    }

    #[test]
    fn a_missing_library_is_named_and_says_what_to_do() {
        let error = missing_library("libpila-no-such-library.so.1").expect("it's missing");
        assert!(error.contains("libpila-no-such-library.so.1"), "{error}");
        assert!(error.contains("LD_LIBRARY_PATH"), "{error}");
    }
}
