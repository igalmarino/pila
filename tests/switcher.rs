//! Mod+Tab window switching and minimizing.

mod common;

use common::*;

/// Windows `names` opened in order (the last one focused).
fn windows(names: &[&str]) -> (TestPila, Client, Vec<common::client::Window>) {
    let mut pila = TestPila::start();
    let client = pila.client();
    let mut handles = Vec::new();
    for name in names {
        handles.push(client.window(name).create());
        pila.wait_focus(name);
    }
    (pila, client, handles)
}

fn history(state: &Value) -> Vec<String> {
    state["focus_history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn mod_tab_switches_to_the_previous_window_and_ends_on_release() {
    let (mut pila, _client, _) = windows(&["a", "b"]);
    pila.key_press("Super");
    pila.key("Tab");
    let state = pila.wait_focus("a");
    assert!(state["switch"].is_object(), "a switch in progress");
    // The history waits for the switch to end.
    assert_eq!(history(&state), vec!["b", "a"]);
    assert_eq!(stacking(&state), vec!["b", "a"]);
    pila.key_release("Super");
    let state = pila.wait("the switch to end", |s| s["switch"].is_null());
    assert_eq!(history(&state), vec!["a", "b"]);
    assert_eq!(state["focused"], "a");
}

#[test]
fn holding_mod_and_tapping_tab_goes_further_back() {
    let (mut pila, _client, _) = windows(&["a", "b", "c"]);
    pila.key_press("Super");
    pila.key("Tab");
    pila.wait_focus("b");
    pila.key("Tab");
    pila.wait_focus("a");
    pila.key("Tab");
    // Wraps around to the start.
    pila.wait_focus("c");
    pila.key("Tab");
    pila.wait_focus("b");
    pila.key_release("Super");
    let state = pila.wait("the switch to end", |s| s["switch"].is_null());
    assert_eq!(history(&state), vec!["b", "c", "a"]);
}

#[test]
fn mod_shift_tab_goes_the_other_way() {
    let (mut pila, _client, _) = windows(&["a", "b", "c"]);
    pila.key_press("Super");
    pila.key_press("Shift");
    pila.key("Tab");
    pila.wait_focus("a");
    pila.key_release("Shift");
    pila.key_release("Super");
    let state = pila.wait("the switch to end", |s| s["switch"].is_null());
    assert_eq!(history(&state), vec!["a", "c", "b"]);
}

#[test]
fn mod_j_switches_too() {
    let (mut pila, _client, _) = windows(&["a", "b"]);
    pila.combo("Super+j");
    let state = pila.wait_focus("a");
    assert!(state["switch"].is_null());
    assert_eq!(history(&state), vec!["a", "b"]);
}

#[test]
fn a_click_ends_the_switch() {
    let (mut pila, _client, _) = windows(&["a", "b"]);
    pila.key_press("Super");
    pila.key("Tab");
    pila.wait_focus("a");
    pila.click("middle");
    let state = pila.wait("the switch to end", |s| s["switch"].is_null());
    assert_eq!(history(&state)[0], "a");
    pila.key_release("Super");
}

#[test]
fn with_one_window_mod_tab_does_nothing() {
    let (mut pila, _client, _) = windows(&["a"]);
    pila.key_press("Super");
    pila.key("Tab");
    let state = pila.settle();
    assert!(state["switch"].is_null());
    assert_eq!(state["focused"], "a");
    pila.key_release("Super");
}

#[test]
fn minimizing_focuses_the_most_recent_window_left() {
    let (mut pila, _client, _) = windows(&["a", "b", "c"]);
    // History c, b, a: minimizing c focuses b.
    pila.combo("Super+n");
    let state = pila.wait_focus("b");
    let c = window_in(&state, "c").unwrap();
    assert_eq!(c["minimized"], true);
    assert_eq!(c["mapped"], false);
    assert_eq!(c["stack"], Value::Null);
}

#[test]
fn mod_tab_shows_a_minimized_window_while_selected_and_keeps_it_when_ending_there() {
    let (mut pila, _client, _) = windows(&["a", "b"]);
    let before = rect(&pila.window("b")["geometry"]);
    pila.combo("Super+n");
    pila.wait_focus("a");
    pila.key_press("Super");
    pila.key("Tab");
    let state = pila.wait_focus("b");
    let b = window_in(&state, "b").unwrap();
    assert_eq!(b["minimized"], false);
    assert_eq!(rect(&b["geometry"]), before);
    // Moving on hides it again.
    pila.key("Tab");
    let state = pila.wait_focus("a");
    assert_eq!(window_in(&state, "b").unwrap()["minimized"], true);
    // Back to it, and the switch ends there: it stays.
    pila.key("Tab");
    pila.wait_focus("b");
    pila.key_release("Super");
    let state = pila.wait("the switch to end", |s| s["switch"].is_null());
    let b = window_in(&state, "b").unwrap();
    assert_eq!(b["minimized"], false);
    assert_eq!(b["mapped"], true);
}

#[test]
fn a_lone_minimized_window_comes_back_with_mod_tab() {
    let (mut pila, _client, _) = windows(&["a"]);
    pila.combo("Super+n");
    pila.wait("minimized", |s| s["focused"].is_null());
    pila.combo("Super+Tab");
    let state = pila.wait_focus("a");
    assert_eq!(window_in(&state, "a").unwrap()["minimized"], false);
}

#[test]
fn a_client_can_minimize_itself() {
    let (mut pila, client, handles) = windows(&["a", "b"]);
    client.request(handles[1], |t| t.set_minimized());
    let state = pila.wait_focus("a");
    assert_eq!(window_in(&state, "b").unwrap()["minimized"], true);
}

#[test]
fn a_minimized_window_whose_client_dies_is_forgotten() {
    let mut pila = TestPila::start();
    let other = pila.client();
    other.window("other").create();
    pila.wait_focus("other");
    let mut client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    pila.combo("Super+n");
    pila.wait_focus("other");
    client.disconnect();
    pila.wait("a gone", |s| window_in(s, "a").is_none());
    pila.combo("Super+Tab");
    let state = pila.settle();
    assert_eq!(state["focused"], "other");
}
