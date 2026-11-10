//! Touchpad gestures: pila's swipes (`[gestures]`) and what reaches the
//! clients through pointer-gestures.

mod common;

use common::*;
use wayland_client::protocol::wl_pointer::WlPointer;
use wayland_protocols::wp::{
    keyboard_shortcuts_inhibit::zv1::client::zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1,
    pointer_gestures::zv1::client::zwp_pointer_gestures_v1::ZwpPointerGesturesV1,
};

fn swipe(pila: &mut TestPila, fingers: u32, steps: &[(f64, f64)]) {
    pila.gesture(json!({ "kind": "swipe", "phase": "begin", "fingers": fingers }));
    for (dx, dy) in steps {
        pila.gesture(json!({ "kind": "swipe", "phase": "update", "dx": dx, "dy": dy }));
    }
    pila.gesture(json!({ "kind": "swipe", "phase": "end" }));
}

fn pointer(client: &Client) -> WlPointer {
    client.lock().pointer.clone().expect("a pointer")
}

/// Two windows (b focused) and a client listening to gestures on them.
fn setup() -> (TestPila, Client) {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    client.window("b").size(400, 300).create();
    pila.wait_focus("b");
    let (x, y, ..) = rect(&pila.window("b")["geometry"]);
    pila.move_to((x + 100) as f64, (y + 100) as f64);
    client.wait("pointer on b", |s| s.pointer_name.as_deref() == Some("b"));
    let gestures: ZwpPointerGesturesV1 = client.bind("zwp_pointer_gestures_v1", 3);
    let p = pointer(&client);
    gestures.get_swipe_gesture(&p, &client.qh, "swipe".into());
    gestures.get_pinch_gesture(&p, &client.qh, "pinch".into());
    gestures.get_hold_gesture(&p, &client.qh, "hold".into());
    client.flush();
    client.roundtrip();
    (pila, client)
}

#[test]
fn a_three_finger_swipe_left_switches_windows_and_stays_with_pila() {
    let (mut pila, client) = setup();
    swipe(&mut pila, 3, &[(-30.0, 0.0), (-40.0, 0.0)]);
    let state = pila.wait_focus("a");
    assert!(
        state["switch"].is_null(),
        "the switch ends when the fingers lift"
    );
    client.roundtrip();
    assert!(
        client.logged("swipe").is_empty(),
        "{:?}",
        client.logged("swipe")
    );
}

#[test]
fn a_long_swipe_steps_through_more_windows() {
    let mut pila = TestPila::start();
    let client = pila.client();
    for name in ["a", "b", "c"] {
        client.window(name).create();
        pila.wait_focus(name);
    }
    // 60 to trigger, then 140 per further step.
    swipe(&mut pila, 3, &[(-70.0, 0.0), (-150.0, 0.0)]);
    pila.wait_focus("a");
}

#[test]
fn three_fingers_up_maximize_and_down_restore() {
    let (mut pila, _client) = setup();
    swipe(&mut pila, 3, &[(0.0, -80.0)]);
    pila.wait("maximized", |s| {
        window_in(s, "b").unwrap()["maximized"] == true
    });
    swipe(&mut pila, 3, &[(0.0, 80.0)]);
    pila.wait("restored", |s| {
        window_in(s, "b").unwrap()["maximized"] == false
    });
}

#[test]
fn four_fingers_change_workspace() {
    let (mut pila, _client) = setup();
    swipe(&mut pila, 4, &[(-80.0, 0.0)]);
    pila.wait("workspace 2", |s| s["workspace"] == 2);
    swipe(&mut pila, 4, &[(80.0, 0.0)]);
    pila.wait("workspace 1", |s| s["workspace"] == 1);
}

#[test]
fn a_short_swipe_does_nothing() {
    let (mut pila, _client) = setup();
    swipe(&mut pila, 3, &[(-30.0, 0.0)]);
    pila.stays("still b", std::time::Duration::from_millis(100), |s| {
        s["focused"] == "b"
    });
}

#[test]
fn unbound_swipes_pinches_and_holds_reach_the_client() {
    let (mut pila, client) = setup();
    swipe(&mut pila, 5, &[(-80.0, 0.0)]);
    client.wait_logged("swipe", "Begin");
    assert!(client.has_logged("swipe", "fingers: 5"));
    client.wait_logged("swipe", "End");
    pila.gesture(json!({ "kind": "pinch", "phase": "begin", "fingers": 2 }));
    pila.gesture(json!({ "kind": "pinch", "phase": "update", "scale": 1.5, "rotation": 10.0 }));
    pila.gesture(json!({ "kind": "pinch", "phase": "end" }));
    client.wait_logged("pinch", "End");
    assert!(client.has_logged("pinch", "scale: 1.5"));
    pila.gesture(json!({ "kind": "hold", "phase": "begin", "fingers": 3 }));
    pila.gesture(json!({ "kind": "hold", "phase": "end", "cancelled": true }));
    client.wait_logged("hold", "End");
    assert_eq!(pila.state()["focused"], "b");
}

#[test]
fn a_shortcuts_inhibitor_gets_the_swipe() {
    let (mut pila, client) = setup();
    let manager: ZwpKeyboardShortcutsInhibitManagerV1 =
        client.bind("zwp_keyboard_shortcuts_inhibit_manager_v1", 1);
    let _inhibitor = manager.inhibit_shortcuts(
        &client.surface(common::client::Window(1)),
        &client.seat(),
        &client.qh,
        "inhibitor".into(),
    );
    client.flush();
    client.wait_logged("inhibitor", "Active");
    swipe(&mut pila, 3, &[(-80.0, 0.0)]);
    client.wait_logged("swipe", "fingers: 3");
    assert_eq!(pila.state()["focused"], "b");
}

#[test]
fn gestures_do_nothing_while_locked() {
    let (mut pila, client) = setup();
    let lock = client.lock_session();
    client.lock_surface(&lock, "HEADLESS-1", [255, 0, 0, 0]);
    client.wait_logged("lock", "Locked");
    swipe(&mut pila, 4, &[(-80.0, 0.0)]);
    pila.stays(
        "still workspace 1",
        std::time::Duration::from_millis(100),
        |s| s["workspace"] == 1,
    );
}

/// A swipe begun before the screen locked runs nothing once it's locked
/// (here one that needs no focused window, which a lock takes away).
#[test]
fn a_swipe_that_crosses_a_lock_does_nothing() {
    let mut pila = TestPila::with_config("[gestures]\n\"swipe-3-up\" = \"show-desktop\"\n");
    let client = pila.client();
    client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    pila.gesture(json!({ "kind": "swipe", "phase": "begin", "fingers": 3 }));
    pila.gesture(json!({ "kind": "swipe", "phase": "update", "dx": 0.0, "dy": -10.0 }));
    let lock = client.lock_session();
    client.lock_surface(&lock, "HEADLESS-1", [255, 0, 0, 0]);
    client.wait_logged("lock", "Locked");
    pila.gesture(json!({ "kind": "swipe", "phase": "update", "dx": 0.0, "dy": -80.0 }));
    pila.gesture(json!({ "kind": "swipe", "phase": "end" }));
    pila.stays(
        "a still shown",
        std::time::Duration::from_millis(100),
        |s| window_in(s, "a").unwrap()["minimized"] == false,
    );
}

#[test]
fn a_gesture_config_rebinds_swipes() {
    let mut pila = TestPila::with_config("[gestures]\n\"swipe-3-down\" = \"minimize\"\n");
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    swipe(&mut pila, 3, &[(0.0, 80.0)]);
    pila.wait("minimized", |s| {
        window_in(s, "a").unwrap()["minimized"] == true
    });
}
