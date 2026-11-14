//! Workspaces: switching, moving windows, keyboard bindings, the count.

mod common;

use common::client::Ev;
use common::*;

fn visible(state: &Value) -> Vec<String> {
    stacking(state)
}

#[test]
fn switching_parks_the_windows_and_brings_them_back() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    let before = rect(&pila.window("a")["geometry"]);
    pila.combo("Super+2");
    let state = pila.wait("workspace 2", |s| s["workspace"] == 2);
    let a = window_in(&state, "a").unwrap();
    assert_eq!(a["mapped"], false);
    assert_eq!(a["workspace"], 1);
    assert!(state["focused"].is_null());
    assert!(visible(&state).is_empty());
    pila.combo("Super+1");
    let state = pila.wait_focus("a");
    assert_eq!(state["workspace"], 1);
    assert_eq!(rect(&window_in(&state, "a").unwrap()["geometry"]), before);
}

#[test]
fn a_window_opens_on_the_active_workspace() {
    let mut pila = TestPila::start();
    pila.combo("Super+3");
    pila.wait("workspace 3", |s| s["workspace"] == 3);
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    assert_eq!(pila.window("a")["workspace"], 3);
    pila.combo("Super+1");
    let state = pila.wait("workspace 1", |s| s["workspace"] == 1);
    assert_eq!(window_in(&state, "a").unwrap()["mapped"], false);
}

#[test]
fn mod_shift_digit_moves_the_focused_window_and_stays() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    client.window("b").create();
    pila.wait_focus("b");
    pila.combo("Super+Shift+2");
    let state = pila.wait_focus("a");
    assert_eq!(state["workspace"], 1);
    let b = window_in(&state, "b").unwrap();
    assert_eq!(b["workspace"], 2);
    assert_eq!(b["mapped"], false);
    pila.combo("Super+2");
    let state = pila.wait_focus("b");
    assert_eq!(visible(&state), vec!["b"]);
}

#[test]
fn page_keys_step_without_wrapping() {
    let mut pila = TestPila::with_config("workspaces = 3\n");
    pila.combo("Super+Page_Up");
    pila.stays("still on 1", std::time::Duration::from_millis(100), |s| {
        s["workspace"] == 1
    });
    pila.combo("Super+Page_Down");
    pila.wait("2", |s| s["workspace"] == 2);
    pila.combo("Super+Page_Down");
    pila.wait("3", |s| s["workspace"] == 3);
    pila.combo("Super+Page_Down");
    pila.stays("still on 3", std::time::Duration::from_millis(100), |s| {
        s["workspace"] == 3
    });
    pila.combo("Super+Page_Up");
    pila.wait("2", |s| s["workspace"] == 2);
}

#[test]
fn mod_shift_page_keys_move_the_window() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    pila.combo("Super+Shift+Page_Down");
    pila.wait("a on 2", |s| window_in(s, "a").unwrap()["workspace"] == 2);
    pila.combo("Super+Page_Down");
    pila.wait_focus("a");
    pila.combo("Super+Shift+Page_Up");
    pila.wait("a on 1", |s| window_in(s, "a").unwrap()["workspace"] == 1);
    assert_eq!(pila.state()["workspace"], 2);
}

/// Mod with a mouse wheel notch: down is the next workspace, up the
/// previous; the window under the pointer never sees those notches.
#[test]
fn mod_wheel_steps_through_workspaces_and_the_window_doesnt_see_it() {
    let mut pila = TestPila::with_config("workspaces = 3\n");
    let client = pila.client();
    client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    pila.move_to((x + 100) as f64, (y + 100) as f64);
    client.wait("pointer enter", |s| s.pointer_name.as_deref() == Some("a"));

    pila.key_press("Super_L");
    pila.scroll("wheel", 0.0, 1.0);
    pila.wait("2", |s| s["workspace"] == 2);
    pila.scroll("wheel", 0.0, 1.0);
    pila.wait("3", |s| s["workspace"] == 3);
    pila.scroll("wheel", 0.0, 1.0);
    pila.stays("still on 3", std::time::Duration::from_millis(100), |s| {
        s["workspace"] == 3
    });
    pila.scroll("wheel", 0.0, -2.0);
    pila.wait("1, two notches up", |s| s["workspace"] == 1);
    pila.key_release("Super_L");
    client.roundtrip();
    let scrolled = |e: &Ev| matches!(e, Ev::Axis { .. } | Ev::AxisV120 { .. });
    assert!(
        !client.events().iter().any(scrolled),
        "{:?}",
        client.events()
    );

    // Without Mod the wheel scrolls the window, and the workspace stays.
    pila.scroll("wheel", 0.0, 1.0);
    client.wait("a wheel notch", |s| s.events.iter().any(scrolled));
    assert_eq!(pila.state()["workspace"], 1);
}

/// A high-resolution wheel sends parts of a notch: a whole one switches
/// once; turning back starts over.
#[test]
fn a_high_resolution_wheel_switches_once_a_notch() {
    let mut pila = TestPila::start();
    pila.key_press("Super_L");
    pila.scroll("wheel", 0.0, 0.5);
    pila.stays(
        "half a notch: still on 1",
        std::time::Duration::from_millis(100),
        |s| s["workspace"] == 1,
    );
    pila.scroll("wheel", 0.0, 0.5);
    pila.wait("a whole notch: 2", |s| s["workspace"] == 2);
    pila.scroll("wheel", 0.0, 0.75);
    pila.scroll("wheel", 0.0, -0.5);
    pila.scroll("wheel", 0.0, 0.5);
    pila.stays(
        "the way back started over: still on 2",
        std::time::Duration::from_millis(100),
        |s| s["workspace"] == 2,
    );
    pila.key_release("Super_L");
}

#[test]
fn mod_shift_wheel_moves_the_focused_window() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    pila.key_press("Super_L");
    pila.key_press("Shift_L");
    pila.scroll("wheel", 0.0, 1.0);
    pila.wait("a on 2", |s| window_in(s, "a").unwrap()["workspace"] == 2);
    pila.key_release("Shift_L");
    pila.key_release("Super_L");
    assert_eq!(pila.state()["workspace"], 1);
}

#[test]
fn the_most_recent_window_of_a_workspace_gets_focus_back() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    client.window("b").create();
    pila.wait_focus("b");
    pila.click_at(445.0, 270.0);
    pila.wait_focus("a");
    pila.combo("Super+2");
    pila.wait("2", |s| s["workspace"] == 2);
    pila.combo("Super+1");
    let state = pila.wait_focus("a");
    // Stacking order is kept too.
    assert_eq!(visible(&state), vec!["b", "a"]);
}

#[test]
fn a_window_being_dragged_comes_along() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    pila.move_to(500.0, 300.0);
    pila.key_press("Super");
    pila.press("left");
    pila.wait("a move grab", |s| s["pointer"]["grab"] == "move");
    pila.key("2");
    let state = pila.wait("workspace 2", |s| s["workspace"] == 2);
    let a = window_in(&state, "a").unwrap();
    assert_eq!(a["mapped"], true);
    assert_eq!(a["workspace"], 2);
    pila.release("left");
    pila.key_release("Super");
    pila.combo("Super+1");
    let state = pila.wait("workspace 1", |s| s["workspace"] == 1);
    assert_eq!(window_in(&state, "a").unwrap()["mapped"], false);
}

#[test]
fn a_dragged_window_cant_be_sent_away() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    pila.move_to(500.0, 300.0);
    pila.key_press("Super");
    pila.press("left");
    pila.wait("a move grab", |s| s["pointer"]["grab"] == "move");
    pila.key_press("Shift");
    pila.key("2");
    pila.key_release("Shift");
    pila.stays("a stays", std::time::Duration::from_millis(100), |s| {
        window_in(s, "a").unwrap()["workspace"] == 1
    });
    pila.release("left");
    pila.key_release("Super");
}

#[test]
fn fewer_workspaces_after_a_reload_bring_windows_to_the_last_one() {
    let mut pila = TestPila::with_config("workspaces = 4\n");
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    pila.combo("Super+Shift+4");
    pila.wait("a on 4", |s| window_in(s, "a").unwrap()["workspace"] == 4);
    pila.combo("Super+3");
    pila.wait("3", |s| s["workspace"] == 3);
    pila.write_config("workspaces = 2\n");
    let state = pila.wait("2 workspaces", |s| s["workspaces"] == 2);
    // The active one is gone: the last one left is shown.
    let state = if state["workspace"] == 2 {
        state
    } else {
        pila.wait("workspace 2", |s| s["workspace"] == 2)
    };
    let a = window_in(&state, "a").unwrap();
    assert_eq!(a["workspace"], 2);
}

#[test]
fn a_minimized_window_keeps_its_workspace() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    pila.combo("Super+n");
    pila.wait("minimized", |s| {
        window_in(s, "a").unwrap()["minimized"] == true
    });
    pila.combo("Super+2");
    pila.wait("2", |s| s["workspace"] == 2);
    // Mod+Tab lists only the active workspace.
    pila.combo("Super+Tab");
    let state = pila.settle();
    assert!(state["focused"].is_null());
    assert_eq!(window_in(&state, "a").unwrap()["workspace"], 1);
}
