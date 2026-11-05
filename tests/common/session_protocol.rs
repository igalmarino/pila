//! Client bindings of xdg-session-management-v1, like the server's
//! (`src/session_protocol.rs`), and gone with them.

#![allow(
    dead_code,
    non_camel_case_types,
    unused_unsafe,
    unused_variables,
    non_upper_case_globals,
    non_snake_case,
    unused_imports,
    clippy::all
)]

use wayland_client::{self, protocol::*};
use wayland_protocols::xdg::shell::client::*;

pub mod __interfaces {
    use wayland_client::{backend as wayland_backend, protocol::__interfaces::*};
    use wayland_protocols::xdg::shell::client::__interfaces::*;
    wayland_scanner::generate_interfaces!("resources/protocols/xdg-session-management-v1.xml");
}
use self::__interfaces::*;

wayland_scanner::generate_client_code!("resources/protocols/xdg-session-management-v1.xml");
