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

/// With the menu open, a right click elsewhere is still a right click: on
/// another title bar it opens that window's menu, in an app it reaches the
/// app (its own menu, or one it asks pila for).
#[test]
fn a_right_click_elsewhere_closes_the_window_menu_and_counts() {
    let (mut pila, client, _a, (ax, ay, ..)) = one_window();
    let other = pila.client();
    other.window("b").size(300, 200).create();
    let state = pila.wait_focus("b");
    let (bx, by, ..) = rect(&window_in(&state, "b").unwrap()["geometry"]);
    pila.move_to(bx as f64 + 10.0, by as f64 - 14.0);
    pila.click("right");
    pila.wait("b's menu", |s| menu(s)["window"] == "b");
    // a's title bar, left of b (cascaded).
    pila.move_to(ax as f64 + 10.0, ay as f64 - 14.0);
    pila.click("right");
    pila.wait("a's menu", |s| menu(s)["window"] == "a");
    // Inside a, away from the menu: the app gets the press.
    client.clear_events();
    pila.move_to(ax as f64 + 350.0, ay as f64 + 250.0);
    pila.click("right");
    pila.wait("closed", |s| menu(s).is_null());
    client.wait("the app's right press", |s| {
        s.events.iter().any(|e| {
            matches!(
                e,
                Ev::Button {
                    button: 0x111,
                    pressed: true,
                    ..
                }
            )
        })
    });
}

/// GTK asks for the menu when it handles the press, often after the release
/// arrived (a quick click): the serial of that press, from that client,
/// still opens it. A press of someone else, or an old one, doesn't.
#[test]
fn an_app_can_open_its_window_menu_after_a_quick_click() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client
        .window("a")
        .size(400, 300)
        .decorations(common::client::Decorations::Client)
        .create();
    let state = pila.wait_focus("a");
    let (x, y, ..) = rect(&window_in(&state, "a").unwrap()["geometry"]);
    pila.move_to(x as f64 + 50.0, y as f64 + 10.0);
    pila.click("right");
    client.wait("the release", |s| {
        s.events
            .iter()
            .any(|e| matches!(e, Ev::Button { pressed: false, .. }))
    });
    let press = client
        .events()
        .iter()
        .find_map(|e| match e {
            Ev::Button {
                pressed: true,
                serial,
                ..
            } => Some(*serial),
            _ => None,
        })
        .expect("the press");
    let seat = client.seat();
    client.request(a, |t| t.show_window_menu(&seat, press, 50, 10));
    let state = pila.wait("the menu", |s| !menu(s).is_null());
    assert_eq!(rect(&menu(&state)["geometry"]).0, x + 50);
    pila.key("Escape");
    pila.wait("closed", |s| menu(s).is_null());
    // A later click makes that press old.
    pila.click("left");
    client.roundtrip();
    client.request(a, |t| t.show_window_menu(&seat, press, 50, 10));
    client.roundtrip();
    pila.stays("no menu", std::time::Duration::from_millis(100), |s| {
        menu(s).is_null()
    });
}

/// The release of the right click that opened the menu, a few pixels away
/// (a hand on a touchpad), lands on its first item: that's not a choice.
#[test]
fn the_release_of_the_opening_click_runs_nothing() {
    let (mut pila, _client, _a, (x, y, ..)) = one_window();
    pila.move_to(x as f64 + 60.0, y as f64 - 14.0);
    pila.press("right");
    pila.wait("the menu", |s| !menu(s).is_null());
    pila.move_to(x as f64 + 66.0, y as f64 - 6.0);
    pila.wait("over its first item", |s| menu(s)["hover"] == 0);
    pila.release("right");
    let state = pila.settle();
    assert!(!menu(&state).is_null(), "the menu closed");
    assert_eq!(window_in(&state, "a").unwrap()["minimized"], false);
    // A real click on it still runs it.
    pila.click("left");
    pila.wait("minimized", |s| {
        window_in(s, "a").unwrap()["minimized"] == true
    });
}

/// A real pointer sits between pixels: the menu opens at its position rounded
/// to a pixel, and the press that opened it must not count as one outside it
/// (it reopened the menu in a loop, which never showed).
#[test]
fn a_menu_opened_between_pixels_stays_open() {
    let (mut pila, _client, _a, (x, y, ..)) = one_window();
    pila.move_to(x as f64 + 60.6, y as f64 - 13.6);
    pila.click("right");
    let state = pila.wait("the menu", |s| !menu(s).is_null());
    let opened = menu(&state)["geometry"].clone();
    let state = pila.settle();
    assert_eq!(menu(&state)["geometry"], opened, "the menu was reopened");
    let (mx, my, ..) = rect(&opened);
    assert!((mx as f64) <= x as f64 + 60.6 && (my as f64) <= y as f64 - 13.6);
}
