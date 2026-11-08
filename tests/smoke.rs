//! The harness itself: a headless pila starts, reports its state, maps a
//! client's window, takes injected input and quits.

mod common;

use common::*;

#[test]
fn headless_pila_starts_with_one_output() {
    let mut pila = TestPila::start();
    let state = pila.state();
    let outputs = state["outputs"].as_array().unwrap();
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0]["name"], "HEADLESS-1");
    assert_eq!(rect(&outputs[0]["geometry"]), (0, 0, 1280, 800));
    assert!(state["windows"].as_array().unwrap().is_empty());
    assert_eq!(state["workspace"], 1);
}

#[test]
fn a_window_maps_focused_and_is_drawn() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client
        .window("smoke")
        .size(400, 300)
        .color([255, 0, 0, 255])
        .create();
    let w = pila.wait_window("smoke");
    pila.wait_focus("smoke");
    let (x, y, width, height) = rect(&w["geometry"]);
    assert_eq!((width, height), (400, 300));
    let configure =
        client.wait_configure(window, "activated", |c| c.has(client::states::ACTIVATED));
    assert!(configure.has(client::states::ACTIVATED));
    pila.settle();
    assert_eq!(
        pila.pixel((x + width / 2) as f64, (y + height / 2) as f64),
        [0, 0, 255, 255]
    );
}

#[test]
fn bindings_run_from_injected_keys() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("smoke").create();
    pila.wait_focus("smoke");
    pila.combo("Super+Shift+q");
    client.wait("the window to be asked to close", |s| {
        s.windows[window.0].close_requested
    });
}

#[test]
fn quit_binding_exits() {
    let mut pila = TestPila::start();
    pila.combo("Super+Shift+e");
    assert!(pila.wait_exit().success());
}

/// A client that pila disconnects for a protocol error is named in pila's
/// log, with its process id: often the only trace of why a shell went away.
#[test]
fn a_client_killed_by_a_protocol_error_is_logged() {
    use std::os::fd::AsFd;
    use wayland_protocols_wlr::gamma_control::v1::client::zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1;

    let mut pila = TestPila::start();
    let client = pila.client();
    let manager: ZwlrGammaControlManagerV1 = client.bind("zwlr_gamma_control_manager_v1", 1);
    let control =
        manager.get_gamma_control(&client.output("HEADLESS-1"), &client.qh, "gamma".into());
    client.flush();
    client.wait_logged("gamma", "GammaSize");
    // A ramp of the wrong size is a protocol error (invalid_gamma).
    let short = tempfile_with(&[0u8; 10]);
    client.expect_error();
    control.set_gamma(short.as_fd());
    client.flush();
    client.wait_error();
    pila.wait_log(&format!(
        "client disconnected by a protocol error pid={}",
        std::process::id()
    ));
}

/// A file holding `data`, already unlinked.
fn tempfile_with(data: &[u8]) -> std::fs::File {
    use std::io::{Seek, Write};
    let path = std::env::temp_dir().join(format!("pila-ramp-{}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .unwrap();
    std::fs::remove_file(&path).unwrap();
    file.write_all(data).unwrap();
    file.rewind().unwrap();
    file
}
