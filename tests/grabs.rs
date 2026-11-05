//! Pointer grabs: Mod+drag move and resize, the invisible resize border,
//! client-requested move/resize, snapping by dragging to an output's edge,
//! dragging filled and snapped windows.

mod common;

use common::client::{states, Decorations};
use common::*;
use wayland_protocols::xdg::shell::client::xdg_toplevel::ResizeEdge;

const BAR: i64 = 28;

fn one_window(
    decorations: Decorations,
) -> (
    TestPila,
    Client,
    common::client::Window,
    (i64, i64, i64, i64),
) {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client
        .window("a")
        .size(400, 300)
        .decorations(decorations)
        .create();
    pila.wait_focus("a");
    let geometry = rect(&pila.window("a")["geometry"]);
    (pila, client, window, geometry)
}

fn geometry(pila: &mut TestPila) -> (i64, i64, i64, i64) {
    rect(&pila.window("a")["geometry"])
}

#[test]
fn mod_left_drag_moves_and_the_client_never_sees_the_click() {
    let (mut pila, client, _, (x, y, w, h)) = one_window(Decorations::Server);
    pila.move_to((x + 100) as f64, (y + 100) as f64);
    pila.key_press("Super");
    pila.press("left");
    let state = pila.wait("a move or resize grab", |s| {
        matches!(s["pointer"]["grab"].as_str(), Some("move" | "resize"))
    });
    assert_eq!(state["pointer"]["cursor"], "grabbing");
    pila.move_by(-30.0, 20.0);
    pila.move_by(-30.0, 20.0);
    pila.wait("moved", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == (x - 60, y + 40, w, h)
    });
    pila.release("left");
    pila.key_release("Super");
    let state = pila.wait("the grab ends", |s| s["pointer"]["grabbed"] == false);
    assert_ne!(state["pointer"]["cursor"], "grabbing");
    client.roundtrip();
    assert!(client.buttons().is_empty(), "{:?}", client.buttons());
}

#[test]
fn mod_right_drag_resizes_from_the_nearest_corner() {
    let (mut pila, client, window, (x, y, w, h)) = one_window(Decorations::Server);
    // Bottom-right quarter: the bottom-right corner moves.
    pila.move_to((x + w - 50) as f64, (y + h - 50) as f64);
    pila.key_press("Super");
    pila.press("right");
    pila.wait("a move or resize grab", |s| {
        matches!(s["pointer"]["grab"].as_str(), Some("move" | "resize"))
    });
    pila.move_by(40.0, 30.0);
    let c = client.wait_configure(window, "resizing", |c| c.size == (440, 330));
    assert!(c.has(states::RESIZING));
    pila.wait("resized", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == (x, y, w + 40, h + 30)
    });
    pila.release("right");
    pila.key_release("Super");
    client.wait_configure(window, "resize over", |c| !c.has(states::RESIZING));
    assert!(client.buttons().is_empty());
}

#[test]
fn resizing_from_the_top_left_moves_the_window_once_it_commits() {
    let (mut pila, client, window, (x, y, w, h)) = one_window(Decorations::Server);
    pila.move_to((x + 50) as f64, (y + 50) as f64);
    pila.key_press("Super");
    pila.press("right");
    pila.wait("a move or resize grab", |s| {
        matches!(s["pointer"]["grab"].as_str(), Some("move" | "resize"))
    });
    pila.move_by(-40.0, -30.0);
    client.wait_configure(window, "bigger", |c| c.size == (440, 330));
    // The bottom-right corner stays put.
    pila.wait("resized up and left", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == (x - 40, y - 30, w + 40, h + 30)
    });
    pila.release("right");
    pila.key_release("Super");
}

#[test]
fn a_resize_respects_the_minimum_size() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client
        .window("a")
        .size(400, 300)
        .min_size(300, 250)
        .create();
    pila.wait_focus("a");
    let (x, y, w, h) = geometry(&mut pila);
    pila.move_to((x + w - 10) as f64, (y + h - 10) as f64);
    pila.key_press("Super");
    pila.press("right");
    pila.wait("a move or resize grab", |s| {
        matches!(s["pointer"]["grab"].as_str(), Some("move" | "resize"))
    });
    pila.move_by(-300.0, -200.0);
    client.wait_configure(window, "the minimum", |c| c.size == (300, 250));
    pila.release("right");
    pila.key_release("Super");
}

#[test]
fn the_invisible_border_resizes() {
    let (mut pila, client, window, (x, y, w, h)) = one_window(Decorations::Server);
    // Just left of the window, halfway down: the left edge.
    pila.move_to((x - 3) as f64, (y + h / 2) as f64);
    let state = pila.wait("a resize cursor", |s| s["pointer"]["cursor"] == "w-resize");
    assert_eq!(state["pointer"]["focus"], Value::Null);
    pila.press("left");
    pila.wait("a move or resize grab", |s| {
        matches!(s["pointer"]["grab"].as_str(), Some("move" | "resize"))
    });
    pila.move_by(-50.0, 0.0);
    client.wait_configure(window, "wider", |c| c.size == (450, 300));
    pila.wait("resized leftwards", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == (x - 50, y, w + 50, h)
    });
    pila.release("left");
}

#[test]
fn the_border_reaches_around_the_title_bar() {
    let (mut pila, _client, _, (x, y, _, _)) = one_window(Decorations::Server);
    // Above the title bar: the top edge (near the left corner, diagonal).
    pila.move_to((x + 10) as f64, (y - BAR - 3) as f64);
    pila.wait("a diagonal cursor", |s| {
        s["pointer"]["cursor"] == "nw-resize"
    });
    pila.move_to((x + 200) as f64, (y - BAR - 3) as f64);
    pila.wait("a top cursor", |s| s["pointer"]["cursor"] == "n-resize");
}

#[test]
fn a_client_side_decorated_window_has_no_border() {
    let (mut pila, _client, _, (x, y, _, h)) = one_window(Decorations::Client);
    pila.move_to((x - 3) as f64, (y + h / 2) as f64);
    pila.stays(
        "no resize cursor",
        std::time::Duration::from_millis(100),
        |s| s["pointer"]["cursor"] != "w-resize",
    );
}

#[test]
fn a_client_can_start_a_move_while_its_button_is_held() {
    let (mut pila, client, window, (x, y, w, h)) = one_window(Decorations::Client);
    pila.move_to((x + 100) as f64, (y + 10) as f64);
    pila.press("left");
    client.wait("the press", |s| !s.events.is_empty() && s.last_serial != 0);
    client.wait("the press", |s| {
        s.events
            .iter()
            .any(|e| matches!(e, common::client::Ev::Button { pressed: true, .. }))
    });
    let serial = client.last_serial();
    let seat = client.seat();
    client.request(window, |t| t._move(&seat, serial));
    pila.wait("a move grab", |s| s["pointer"]["grab"] == "move");
    pila.move_by(25.0, 35.0);
    pila.wait("moved", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == (x + 25, y + 35, w, h)
    });
    pila.release("left");
}

#[test]
fn a_client_cant_start_a_move_without_a_held_button() {
    let (mut pila, client, window, _) = one_window(Decorations::Client);
    let (x, y, _, _) = geometry(&mut pila);
    pila.click_at((x + 100) as f64, (y + 10) as f64);
    client.wait("the release", |s| {
        s.events
            .iter()
            .any(|e| matches!(e, common::client::Ev::Button { pressed: false, .. }))
    });
    let serial = client.last_serial();
    let seat = client.seat();
    client.request(window, |t| t._move(&seat, serial));
    client.roundtrip();
    pila.stays("no grab", std::time::Duration::from_millis(100), |s| {
        s["pointer"]["grab"] != "move"
    });
}

#[test]
fn a_client_can_start_a_resize() {
    let (mut pila, client, window, (x, y, w, h)) = one_window(Decorations::Client);
    pila.move_to((x + w - 5) as f64, (y + h - 5) as f64);
    pila.press("left");
    client.wait("the press", |s| {
        s.events
            .iter()
            .any(|e| matches!(e, common::client::Ev::Button { pressed: true, .. }))
    });
    let serial = client.last_serial();
    let seat = client.seat();
    client.request(window, |t| t.resize(&seat, serial, ResizeEdge::BottomRight));
    pila.wait("a resize grab", |s| s["pointer"]["grab"] == "resize");
    pila.move_by(20.0, 10.0);
    client.wait_configure(window, "bigger", |c| c.size == (420, 310));
    pila.release("left");
}

#[test]
fn dragging_to_the_left_edge_snaps_with_a_preview() {
    let (mut pila, client, window, (x, y, _, _)) = one_window(Decorations::Server);
    pila.move_to((x + 100) as f64, (y + 100) as f64);
    pila.key_press("Super");
    pila.press("left");
    pila.wait("a move or resize grab", |s| {
        matches!(s["pointer"]["grab"].as_str(), Some("move" | "resize"))
    });
    pila.move_to(0.0, 400.0);
    let state = pila.wait("a preview", |s| !s["snap_preview"].is_null());
    // The left half, title bar included.
    assert_eq!(rect(&state["snap_preview"]), (0, 0, 640, 800));
    pila.release("left");
    pila.key_release("Super");
    let state = pila.wait("snapped", |s| {
        let a = window_in(s, "a").unwrap();
        a["snapped"] == "left" && a["geometry"]["w"] == 640
    });
    assert!(state["snap_preview"].is_null());
    assert_eq!(geometry(&mut pila), (0, BAR, 640, 800 - BAR));
    client.wait_configure(window, "tiled", |c| c.has(states::TILED_LEFT));
}

#[test]
fn dragging_to_the_top_edge_maximizes() {
    let (mut pila, _client, _, (x, y, _, _)) = one_window(Decorations::Server);
    pila.move_to((x + 100) as f64, (y + 100) as f64);
    pila.key_press("Super");
    pila.press("left");
    pila.wait("a move or resize grab", |s| {
        matches!(s["pointer"]["grab"].as_str(), Some("move" | "resize"))
    });
    pila.move_to(600.0, 0.0);
    let state = pila.wait("a preview", |s| !s["snap_preview"].is_null());
    assert_eq!(rect(&state["snap_preview"]), (0, 0, 1280, 800));
    pila.release("left");
    pila.key_release("Super");
    pila.wait("maximized", |s| {
        let a = window_in(s, "a").unwrap();
        a["maximized"] == true && a["geometry"]["w"] == 1280
    });
}

#[test]
fn moving_away_from_the_edge_drops_the_preview() {
    let (mut pila, _client, _, (x, y, _, _)) = one_window(Decorations::Server);
    pila.move_to((x + 100) as f64, (y + 100) as f64);
    pila.key_press("Super");
    pila.press("left");
    pila.wait("a move or resize grab", |s| {
        matches!(s["pointer"]["grab"].as_str(), Some("move" | "resize"))
    });
    pila.move_to(0.0, 400.0);
    pila.wait("a preview", |s| !s["snap_preview"].is_null());
    pila.move_to(300.0, 400.0);
    pila.wait("no preview", |s| s["snap_preview"].is_null());
    pila.release("left");
    pila.key_release("Super");
    let state = pila.settle();
    assert!(window_in(&state, "a").unwrap()["snapped"].is_null());
}

#[test]
fn dragging_a_maximized_window_restores_it_under_the_pointer() {
    let (mut pila, client, window, (_, _, w, h)) = one_window(Decorations::Server);
    pila.combo("Super+m");
    pila.wait("maximized", |s| {
        window_in(s, "a").unwrap()["geometry"]["w"] == 1280
    });
    pila.move_to(640.0, (BAR / 2) as f64);
    pila.press("left");
    pila.wait("a move or resize grab", |s| {
        matches!(s["pointer"]["grab"].as_str(), Some("move" | "resize"))
    });
    pila.move_by(0.0, 100.0);
    let state = pila.wait("restored", |s| {
        let a = window_in(s, "a").unwrap();
        a["maximized"] == false && a["geometry"]["w"] == w
    });
    let (x, y, ..) = rect(&window_in(&state, "a").unwrap()["geometry"]);
    // Centered under the pointer, which is on its title bar.
    assert_eq!(x, 640 - w / 2);
    assert_eq!(y, BAR / 2 + 100 + BAR / 2);
    assert_eq!(rect(&window_in(&state, "a").unwrap()["geometry"]).3, h);
    client.wait_configure(window, "not maximized", |c| !c.has(states::MAXIMIZED));
    pila.release("left");
}

#[test]
fn dragging_a_snapped_window_restores_its_size() {
    let (mut pila, _client, _, (_, _, w, _)) = one_window(Decorations::Server);
    pila.combo("Super+Left");
    pila.wait("snapped", |s| {
        window_in(s, "a").unwrap()["geometry"]["w"] == 640
    });
    pila.move_to(320.0, (BAR / 2) as f64);
    pila.press("left");
    pila.wait("a move or resize grab", |s| {
        matches!(s["pointer"]["grab"].as_str(), Some("move" | "resize"))
    });
    pila.move_by(50.0, 100.0);
    pila.wait("unsnapped", |s| {
        let a = window_in(s, "a").unwrap();
        a["snapped"].is_null() && a["geometry"]["w"] == w
    });
    pila.release("left");
}

#[test]
fn resizing_a_snapped_window_forgets_the_snap() {
    let (mut pila, _client, _, _) = one_window(Decorations::Server);
    pila.combo("Super+Left");
    pila.wait("snapped", |s| {
        window_in(s, "a").unwrap()["geometry"]["w"] == 640
    });
    pila.combo("Super+Ctrl+Left");
    let state = pila.wait("narrower", |s| {
        window_in(s, "a").unwrap()["geometry"]["w"] == 600
    });
    assert!(window_in(&state, "a").unwrap()["snapped"].is_null());
}
