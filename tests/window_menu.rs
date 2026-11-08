//! The window menu: opened by a right click on pila's title bar or by an app
//! (`xdg_toplevel.show_window_menu`), run with the pointer or the keyboard.

mod common;

use common::client::Ev;
use common::*;

/// The open menu (`window_menu` in the state), or Null.
fn menu(state: &Value) -> &Value {
    &state["window_menu"]
}

/// The middle of row `index` of the open menu.
fn row(state: &Value, index: i64) -> (f64, f64) {
    let (x, y, w, _) = rect(&menu(state)["geometry"]);
    ((x + w / 2) as f64, (y + 4 + index * 28 + 14) as f64)
}

/// Window `a` (400x300), focused; its geometry.
fn one_window() -> (
    TestPila,
    Client,
    common::client::Window,
    (i64, i64, i64, i64),
) {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").size(400, 300).create();
    let state = pila.wait_focus("a");
    let geometry = rect(&window_in(&state, "a").unwrap()["geometry"]);
    (pila, client, a, geometry)
}

/// Right-click `a`'s title bar; the state with the menu open.
fn open_from_title_bar(pila: &mut TestPila, (x, y, ..): (i64, i64, i64, i64)) -> Value {
    pila.move_to(x as f64 + 60.0, y as f64 - 14.0);
    pila.click("right");
    pila.wait("the menu", |s| !menu(s).is_null())
}

#[test]
fn right_clicking_a_title_bar_opens_the_window_menu() {
    let (mut pila, _client, _a, geometry) = one_window();
    let state = open_from_title_bar(&mut pila, geometry);
    assert_eq!(menu(&state)["window"], "a");
    assert_eq!(
        menu(&state)["items"],
        json!([
            "Minimize",
            "Maximize",
            "Always on top",
            "On all workspaces",
            "Close"
        ])
    );
    let (mx, my) = row(&state, 0);
    pila.move_to(mx, my);
    pila.wait("Minimize under the pointer", |s| menu(s)["hover"] == 0);
    pila.click("left");
    let state = pila.wait("minimized", |s| {
        window_in(s, "a").unwrap()["minimized"] == true
    });
    assert!(menu(&state).is_null());
}

#[test]
fn the_window_menu_runs_from_the_keyboard() {
    let (mut pila, client, _a, geometry) = one_window();
    open_from_title_bar(&mut pila, geometry);
    client.clear_events();
    pila.key("Escape");
    pila.wait("closed", |s| menu(s).is_null());
    client.roundtrip();
    assert!(
        !client.events().iter().any(|e| matches!(e, Ev::Key { .. })),
        "the app saw the menu's key"
    );
    open_from_title_bar(&mut pila, geometry);
    pila.key("Down");
    pila.key("Down");
    pila.wait("Maximize picked", |s| menu(s)["hover"] == 1);
    pila.key("Return");
    pila.wait("maximized", |s| {
        window_in(s, "a").unwrap()["maximized"] == true
    });
}

#[test]
fn a_click_outside_the_window_menu_closes_it_and_reaches_nothing() {
    let (mut pila, _client, _a, geometry) = one_window();
    let other = pila.client();
    other.window("b").size(300, 200).create();
    let state = pila.wait_focus("b");
    let (bx, by, ..) = rect(&window_in(&state, "b").unwrap()["geometry"]);
    pila.click_at(geometry.0 as f64 + 10.0, geometry.1 as f64 + 10.0);
    pila.wait_focus("a");
    open_from_title_bar(&mut pila, geometry);
    other.clear_events();
    // b's far corner, outside the menu.
    pila.click_at(bx as f64 + 290.0, by as f64 + 190.0);
    let state = pila.wait("closed", |s| menu(s).is_null());
    assert_eq!(state["focused"], "a");
    other.roundtrip();
    assert!(
        !other
            .events()
            .iter()
            .any(|e| matches!(e, Ev::Button { .. })),
        "the click went through"
    );
}

#[test]
fn an_app_can_open_its_window_menu() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client
        .window("a")
        .size(400, 300)
        .decorations(common::client::Decorations::Client)
        .create();
    let state = pila.wait_focus("a");
    let (x, y, ..) = rect(&window_in(&state, "a").unwrap()["geometry"]);
    // A right press on its own title bar; it asks with that press's serial.
    pila.move_to(x as f64 + 50.0, y as f64 + 10.0);
    pila.press("right");
    client.wait("the press", |s| {
        s.events
            .iter()
            .any(|e| matches!(e, Ev::Button { pressed: true, .. }))
    });
    let serial = client.last_serial();
    let seat = client.seat();
    client.request(a, |t| t.show_window_menu(&seat, serial, 50, 10));
    let state = pila.wait("the menu", |s| !menu(s).is_null());
    assert_eq!(rect(&menu(&state)["geometry"]).0, x + 50);
    assert_eq!(rect(&menu(&state)["geometry"]).1, y + 10);
    // Releasing where it opened runs nothing.
    pila.release("right");
    let (mx, my) = row(&state, 2);
    pila.move_to(mx, my);
    pila.click("left");
    pila.wait("on top", |s| {
        window_in(s, "a").unwrap()["always_on_top"] == true
    });
    // Its mark shows the next time.
    pila.move_to(x as f64 + 50.0, y as f64 + 10.0);
    pila.press("right");
    client.wait("another press", |s| {
        s.events
            .iter()
            .filter(|e| matches!(e, Ev::Button { pressed: true, .. }))
            .count()
            == 2
    });
    let serial = client.last_serial();
    client.request(a, |t| t.show_window_menu(&seat, serial, 50, 10));
    let state = pila.wait("the menu again", |s| !menu(s).is_null());
    assert_eq!(
        menu(&state)["checked"],
        json!([false, false, true, false, false])
    );
}

#[test]
fn a_window_menu_needs_a_real_press() {
    let (mut pila, client, a, _) = one_window();
    let seat = client.seat();
    client.request(a, |t| t.show_window_menu(&seat, 12345, 10, 10));
    client.roundtrip();
    pila.stays("no menu", std::time::Duration::from_millis(100), |s| {
        menu(s).is_null()
    });
}
