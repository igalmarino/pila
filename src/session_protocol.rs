//! Server bindings of xdg-session-management-v1, generated the way
//! wayland-protocols' `wayland_protocol!` does. wayland-protocols 0.32.13 ships
//! the XML (copied to `resources/protocols/`) but no module for it; its next
//! release has `xdg::session_management`: delete this file and the XML then, and
//! the `wayland-scanner` dependency.

#![allow(
    dead_code,
    non_camel_case_types,
    unused_unsafe,
    unused_variables,
    non_upper_case_globals,
    non_snake_case,
    unused_imports,
    missing_docs,
    clippy::all
)]

use smithay::reexports::{
    wayland_protocols::xdg::shell::server::*,
    wayland_server::{self, protocol::*},
};

pub mod __interfaces {
    use smithay::reexports::{
        wayland_protocols::xdg::shell::server::__interfaces::*,
        wayland_server::{backend as wayland_backend, protocol::__interfaces::*},
    };
    wayland_scanner::generate_interfaces!("resources/protocols/xdg-session-management-v1.xml");
}
use self::__interfaces::*;

wayland_scanner::generate_server_code!("resources/protocols/xdg-session-management-v1.xml");
