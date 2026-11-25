//! The quit dialog (`confirm-quit`, Mod+Shift+E by default): it asks before
//! Pila quits, takes every key and click while shown, and `quit` itself
//! never asks.

mod common;

use common::client::Ev;
use common::*;

const RED: [u8; 4] = [255, 255, 0, 0];

fn wait_shown(pila: &mut TestPila) -> Value {
    pila.wait("the quit dialog shown with its text", |s| {
        s["quit_dialog"]["shown"]["text"] == true
    })
}

fn center(value: &Value) -> (f64, f64) {
    let (x, y, w, h) = rect(value);
    (x as f64 + w as f64 / 2.0, y as f64 + h as f64 / 2.0)
}

#[test]
fn the_binding_asks_and_escape_keeps_pila_running() {
    let mut pila = TestPila::with_config(
        "[[window-rule]]\napp-id = \"app\"\ntitle-bar = false\n\
         size = [400, 300]\nposition = [40, 40]\n",
    );
    let client = pila.client();
    client.window("app").color(RED).create();
    pila.wait_focus("app");
    pila.settle();

    pila.combo("Super+Shift+e");
    let state = wait_shown(&mut pila);
    assert_eq!(state["quit_dialog"]["shown"]["output"], "HEADLESS-1");

    // Modal: keys and clicks off its buttons never reach the focused window,
    // and do nothing.
    client.clear_events();
    pila.key("a");
    pila.key("q");
    pila.click_at(100.0, 100.0);
    pila.settle();
    client.roundtrip();
    assert!(
        client.events().is_empty(),
        "input reached the window under the dialog: {:?}",
        client.events()
    );
    assert!(pila.alive());
    assert!(!pila.state()["quit_dialog"]["shown"].is_null());

    pila.key("Escape");
    pila.wait("the dialog closed", |s| s["quit_dialog"]["shown"].is_null());
    assert!(pila.alive());
    // Keys go to the window again.
    client.clear_events();
    pila.key("a");
    client.wait("a to reach the window", |s| {
        s.events
            .iter()
            .any(|e| matches!(e, Ev::Key { key: 30, .. }))
    });
}

#[test]
fn enter_quits() {
    let mut pila = TestPila::start();
    pila.combo("Super+Shift+e");
    wait_shown(&mut pila);
    pila.key("Return");
    assert!(pila.wait_exit().success());
}

#[test]
fn the_binding_pressed_again_quits() {
    let mut pila = TestPila::start();
    pila.combo("Super+Shift+e");
    wait_shown(&mut pila);
    pila.combo("Super+Shift+e");
    assert!(pila.wait_exit().success());
}

#[test]
fn the_buttons_cancel_and_quit() {
    let mut pila = TestPila::start();
    pila.combo("Super+Shift+e");
    let state = wait_shown(&mut pila);
    let (x, y) = center(&state["quit_dialog"]["shown"]["cancel"]);
    pila.click_at(x, y);
    pila.wait("Cancel closed it", |s| s["quit_dialog"]["shown"].is_null());
    assert!(pila.alive());

    // A second `confirm-quit` while shown leaves it as it is.
    pila.combo("Super+Shift+e");
    wait_shown(&mut pila);
    assert!(pila.msg(&["action", "confirm-quit"]).status.success());
    assert!(pila.alive());
    let state = pila.state();
    let (x, y) = center(&state["quit_dialog"]["shown"]["quit"]);
    pila.click_at(x, y);
    assert!(pila.wait_exit().success());
}

#[test]
fn quit_never_asks() {
    let mut pila = TestPila::start();
    assert!(pila.msg(&["action", "quit"]).status.success());
    assert!(pila.wait_exit().success());
}

#[test]
fn locking_closes_it_and_nothing_opens_it_while_locked() {
    let mut pila = TestPila::start();
    assert!(pila.msg(&["action", "confirm-quit"]).status.success());
    wait_shown(&mut pila);

    let locker = pila.client();
    let lock = locker.lock_session();
    let surface = locker.lock_surface(&lock, "HEADLESS-1", [255, 10, 20, 30]);
    locker.wait_surface_configured(surface);
    locker.wait_logged("lock", "Locked");
    pila.wait("locked and the dialog closed", |s| {
        s["locked"] == true && s["quit_dialog"]["shown"].is_null()
    });

    // `pila msg` is refused while locked. (The binding, in the debug build
    // the tests run, quits at once: Quit's escape from a nested session.)
    assert!(!pila.msg(&["action", "confirm-quit"]).status.success());
    pila.settle();
    assert!(pila.state()["quit_dialog"]["shown"].is_null());
    assert!(pila.alive());
}

#[test]
fn the_cheatsheet_names_its_binding() {
    let mut pila = TestPila::start();
    pila.combo("Super+Shift+slash");
    let state = pila.wait("the cheatsheet shown", |s| {
        !s["shortcuts_view"]["rows"].is_null()
    });
    let rows = state["shortcuts_view"]["rows"].as_array().unwrap();
    assert!(rows.contains(&json!(["Super+Shift+E", "Quit"])), "{rows:?}");
}
