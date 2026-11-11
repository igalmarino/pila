//! pila: a stacking Wayland compositor built on Smithay. The library is
//! everything but the command line (`main.rs`): the compositor, and what the
//! fuzz targets (`fuzz/`) reach.
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

pub mod animation;
pub mod backend;
pub mod bell;
pub mod blur;
pub mod capture;
pub mod commit_pacing;
pub mod config;
pub mod config_reload;
pub mod cursor;
pub mod decoration;
pub mod edid;
pub mod ext_workspace;
pub mod focus;
pub mod foreign_toplevel;
pub mod frame;
pub mod frame_stats;
pub mod gamma_control;
pub mod gestures;
pub mod grabs;
pub mod handlers;
pub mod headless;
pub mod idle;
pub mod input;
pub mod introspect;
pub mod ipc;
pub mod ipc_client;
pub mod keyboard_layout;
pub mod layers;
pub mod minimize;
pub mod output_management;
pub mod output_power;
pub mod pointer_constraints;
pub mod redraw;
pub mod render;
pub mod rounded;
pub mod rules;
pub mod scale;
pub mod screencopy;
pub mod session_lock;
pub mod session_management;
pub mod session_protocol;
pub mod shake_cursor;
pub mod snap;
pub mod stacking;
pub mod state;
pub mod surface_limits;
pub mod switcher;
pub mod sync;
pub mod test_control;
pub mod text;
pub mod toplevel_management;
pub mod transients;
pub mod udev;
pub mod urgency;
pub mod virtual_keyboard;
pub mod window_menu;
pub mod winit;
pub mod workspace_slide;
pub mod workspaces;
pub mod xwayland;

pub use crate::state::Pila;

/// The crate version, plus `git describe` when built from a checkout.
pub fn version() -> String {
    let version = env!("CARGO_PKG_VERSION");
    match env!("PILA_GIT_VERSION") {
        "" => version.to_string(),
        git => format!("{version} ({git})"),
    }
}
