//! Shake to find the pointer (`shake_cursor.rs`): the cursor grows while
//! it's shaken and shrinks back after, drawn bigger around its hotspot.

mod common;

use common::*;
use serde_json::Value;
use std::time::Duration;

const ON: &str = "[cursor]\nshake-to-find = true\n";
/// Animations off, but the cursor stays big four times as long (1.6 s), so
/// a test sees it before it shrinks.
const LONG_HOLD: &str = "[animations]\nenabled = false\nspeed = 4\n";
const WHITE: [u8; 4] = [255, 255, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];

/// Pila with `config`, once Xwayland is up: starting it holds the event
/// loop back for a moment, which could take the time a test has to see the
/// cursor big.
fn start(config: &str) -> TestPila {
    let mut pila = TestPila::with_config(config);
    pila.wait_log("Xwayland ready");
    pila
}

fn magnification(state: &Value) -> f64 {
    state["pointer"]["magnification"].as_f64().unwrap()
}

#[test]
fn shaking_the_pointer_makes_the_cursor_bigger_for_a_moment() {
    let mut pila = start(&format!("{ON}{LONG_HOLD}"));
    pila.move_to(640.0, 400.0);
    assert_eq!(magnification(&pila.state()), 1.0);
    pila.shake_pointer(640.0, 400.0);
    pila.wait("magnified 3 times", |s| magnification(s) == 3.0);
    // Where the pointer is doesn't change, only how big it's drawn.
    let state = pila.state();
    assert_eq!(state["pointer"]["x"], 640.0);
    assert_eq!(state["pointer"]["y"], 400.0);
    pila.wait("back to its size", |s| magnification(s) == 1.0);
}

#[test]
fn moving_without_shaking_leaves_the_cursor_alone() {
    let mut pila = start(&format!("{ON}{LONG_HOLD}"));
    // Each way of moving apart: a shake is told by the last second of motion.
    let apart = Duration::from_millis(1100);
    let at_its_size = |s: &Value| magnification(s) == 1.0;
    // Across the screen and back, once.
    for i in 0..=40 {
        pila.move_to(100.0 + 25.0 * i as f64, 100.0 + 15.0 * i as f64);
    }
    for i in 0..=40 {
        pila.move_to(1100.0 - 25.0 * i as f64, 700.0 - 15.0 * i as f64);
    }
    pila.stays("at its size after a line", apart, at_its_size);
    // Round, once (round and round fast enough is a shake too).
    for i in 0..=40 {
        let angle = i as f64 / 40.0 * std::f64::consts::TAU;
        pila.move_to(640.0 + 200.0 * angle.cos(), 400.0 + 200.0 * angle.sin());
    }
    pila.stays("at its size after a circle", apart, at_its_size);
    // A hand's tremor: back and forth, 10 px.
    for i in 0..=60 {
        pila.move_to(640.0 + 10.0 * (i % 2) as f64, 400.0);
    }
    pila.stays("at its size after a tremor", apart, at_its_size);
}

#[test]
fn it_grows_while_shaken_and_shrinks_back_gradually() {
    // Growing all the way takes 2 s of shaking at speed 4, more than the shake.
    let mut pila = start(&format!("{ON}[animations]\nenabled = true\nspeed = 4\n"));
    pila.move_to(640.0, 400.0);
    pila.shake_pointer(640.0, 400.0);
    let grown = magnification(&pila.state());
    assert!(grown > 1.0 && grown < 3.0, "{grown}");
    // Not growing any more with the pointer still.
    pila.stays(
        "as big as the shake made it",
        Duration::from_millis(300),
        |s| magnification(s) == grown,
    );
    pila.wait("shrinking", |s| magnification(s) < grown);
    pila.wait("back to its size", |s| magnification(s) == 1.0);
}

#[test]
fn how_much_it_grows_is_set_and_it_can_be_turned_off() {
    let mut pila = start(&format!("{LONG_HOLD}[cursor]\nshake-magnification = 2.0\n"));
    pila.move_to(640.0, 400.0);
    pila.shake_pointer(640.0, 400.0);
    pila.wait("magnified twice", |s| magnification(s) == 2.0);
    // Turned off while it's big: back at once, not shrinking after the hold.
    pila.write_config(&format!("{LONG_HOLD}[cursor]\nshake-to-find = false\n"));
    pila.wait("back to its size", |s| magnification(s) == 1.0);
    assert!(!pila.log().contains("cursor shaking stopped"));
    pila.shake_pointer(640.0, 400.0);
    pila.stays("at its size", Duration::from_millis(300), |s| {
        magnification(s) == 1.0
    });
}

#[test]
fn a_magnified_named_cursor_is_drawn_bigger_around_its_hotspot() {
    let mut pila = start(&format!("{ON}{LONG_HOLD}"));
    pila.move_to(640.0, 400.0);
    pila.settle();
    // Headless, as nested, a named cursor at its own size is the host's to
    // draw: nothing of it here.
    let background = pila.pixel(641.0, 440.0);
    assert_ne!(background, BLACK);
    pila.shake_pointer(640.0, 400.0);
    pila.wait("magnified", |s| magnification(s) == 3.0);
    // The built-in arrow (no theme in the tests), 11x17 px, three times:
    // its tip at the pointer, its left edge 40 px down, its white inside.
    pila.wait_pixel(641.0, 440.0, BLACK);
    assert_eq!(pila.pixel(650.0, 431.0), WHITE);
    assert_eq!(pila.pixel(639.0, 420.0), background);
    // Gone again once it's back to its size.
    pila.wait_pixel(641.0, 440.0, background);
}

#[test]
fn a_client_cursor_surface_is_drawn_bigger_too() {
    let mut pila = start(&format!("{ON}{LONG_HOLD}"));
    let client = pila.client();
    client.window("a").size(500, 400).create();
    pila.wait_focus("a");
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    let (px, py) = ((x + 300) as f64, (y + 150) as f64);
    pila.move_to(px, py);
    client.wait("pointer enter", |s| s.pointer_name.as_deref() == Some("a"));
    let serial = client.lock().pointer_enter_serial;
    let cursor = {
        let compositor = client.lock().compositor.clone().unwrap();
        let surface = compositor.create_surface(&client.qh, ());
        let shm = client.lock().shm.clone().unwrap();
        common::client::paint(&shm, &client.qh, &surface, WHITE, (16, 16));
        surface
    };
    let pointer = client.lock().pointer.clone().expect("a pointer");
    pointer.set_cursor(serial, Some(&cursor), 0, 0);
    client.flush();
    pila.wait("a surface cursor", |s| s["pointer"]["cursor"] == "surface");
    pila.wait_pixel(px + 8.0, py + 8.0, WHITE);
    let window = pila.pixel(px + 30.0, py + 30.0);
    assert_ne!(window, WHITE);
    // Within the window, so the cursor stays the client's.
    pila.shake_pointer(px, py);
    pila.wait("magnified", |s| magnification(s) == 3.0);
    // 16 px, three times: 48 px from the hotspot.
    pila.wait_pixel(px + 30.0, py + 30.0, WHITE);
    assert_eq!(pila.pixel(px + 47.0, py + 47.0), WHITE);
    assert_eq!(pila.pixel(px + 50.0, py + 50.0), window);
    pila.wait_pixel(px + 30.0, py + 30.0, window);
}
