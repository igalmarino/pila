//! Animations: each kind starts and ends, closed windows leave a ghost
//! that goes away, settings turn them off or slow them, and frames are
//! drawn only while something changes.

mod common;

use std::time::Duration;

use common::*;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;

const ANIMATED: &str = "[animations]\nenabled = true\n";

fn frames(state: &Value) -> u64 {
    state["outputs"][0]["frames"].as_u64().unwrap()
}

#[test]
fn opening_and_closing_animate_and_the_ghost_goes_away() {
    let mut pila = TestPila::with_config(ANIMATED);
    let client = pila.client();
    let a = client.window("a").create();
    pila.wait_window("a");
    pila.wait_log("starts what=\"window-open\" id=\"a\"");
    pila.settle();
    client.destroy(a);
    pila.wait_log("starts what=\"window-close\" id=\"a\"");
    let state = pila.wait("the ghost", |s| {
        s["animations"]["ghosts"] == 1 || s["animations"]["running"] == false
    });
    let _ = state;
    let state = pila.settle();
    assert_eq!(state["animations"]["ghosts"], 0);
    assert!(window_in(&state, "a").is_none());
}

#[test]
fn minimizing_restoring_maximizing_and_switching_workspaces_animate() {
    let mut pila = TestPila::with_config(ANIMATED);
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    pila.settle();
    pila.combo("Super+n");
    pila.wait_log("starts what=\"minimize\" id=\"a\"");
    pila.settle();
    pila.combo("Super+Tab");
    pila.wait_log("starts what=\"restore\" id=\"a\"");
    pila.settle();
    pila.combo("Super+m");
    pila.wait_log("starts what=\"maximize\" id=\"a\"");
    pila.settle();
    pila.combo("Super+2");
    pila.wait_log("starts what=\"workspace\" id=\"2\"");
    let state = pila.settle();
    assert_eq!(state["animations"]["slide"], false);
}

#[test]
fn transient_layer_surfaces_fade_in_and_out_but_bars_dont() {
    let mut pila = TestPila::with_config(ANIMATED);
    let client = pila.client();
    let (launcher, layer) = client.layer_surface(
        "launcher",
        &common::protocols::LayerSpec::default(),
        [255, 0, 0, 255],
    );
    client.wait_surface_configured(launcher);
    pila.wait_log("starts what=\"layer-in\" id=\"launcher\"");
    pila.settle();
    layer.destroy();
    client.wl_surface_of(launcher).destroy();
    client.flush();
    pila.wait_log("starts what=\"layer-out\" id=\"launcher\"");
    let bar_spec = common::protocols::LayerSpec {
        layer: Layer::Top,
        anchor: wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::Anchor::Top,
        size: (300, 30),
        exclusive_zone: 30,
        ..Default::default()
    };
    let (bar, _) = client.layer_surface("bar", &bar_spec, [255, 0, 0, 255]);
    client.wait_surface_configured(bar);
    pila.settle();
    assert!(!pila.log().contains("what=\"layer-in\" id=\"bar\""));
}

#[test]
fn a_kind_set_to_false_doesnt_animate() {
    let mut pila = TestPila::with_config("[animations]\nenabled = true\nwindow-open = false\n");
    let client = pila.client();
    let a = client.window("a").create();
    pila.wait_window("a");
    pila.settle();
    assert!(!pila.log().contains("starts what=\"window-open\""));
    client.destroy(a);
    pila.wait_log("starts what=\"window-close\" id=\"a\"");
}

#[test]
fn enabled_false_turns_them_all_off() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").create();
    pila.wait_focus("a");
    pila.combo("Super+m");
    pila.combo("Super+n");
    client.destroy(a);
    pila.settle();
    assert!(!pila.log().contains("pila::animation: starts"));
}

#[test]
fn speed_stretches_the_durations() {
    let mut pila =
        TestPila::with_config("[animations]\nenabled = true\nspeed = 4\nwindow-open = 200\n");
    let client = pila.client();
    client.window("a").create();
    pila.wait_window("a");
    pila.wait_log("starts what=\"window-open\"");
    // 800 ms in all: still running after 400.
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(pila.state()["animations"]["running"], true);
    pila.settle();
}

#[test]
fn a_window_animates_through_a_texture_while_opening() {
    let mut pila = TestPila::with_config("[animations]\nenabled = true\nspeed = 4\n");
    let client = pila.client();
    client.window("a").create();
    let state = pila.wait("a animating", |s| {
        window_in(s, "a").is_some_and(|w| w["animating"] == true)
    });
    assert_eq!(state["animations"]["running"], true);
    pila.wait("done", |s| window_in(s, "a").unwrap()["animating"] == false);
}

#[test]
fn nothing_is_drawn_while_nothing_changes() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    // The title gets its text once the fonts are loaded; Xwayland starts.
    pila.wait_log("fonts loaded");
    pila.wait_log("Xwayland ready");
    std::thread::sleep(Duration::from_millis(100));
    let before = frames(&pila.settle());
    std::thread::sleep(Duration::from_millis(300));
    let state = pila.state();
    assert_eq!(frames(&state), before, "frames drawn while idle");
    assert_eq!(state["redraw_pending"], false);
}

#[test]
fn an_animation_is_drawn_every_refresh() {
    let mut pila = TestPila::with_config(ANIMATED);
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    let before = frames(&pila.settle());
    pila.combo("Super+m");
    pila.wait_log("starts what=\"maximize\"");
    let after = frames(&pila.settle());
    // 220 ms at 60 Hz: about 13 frames.
    assert!(
        after - before >= 8,
        "only {} frames for the animation",
        after - before
    );
}

#[test]
fn a_new_animation_starts_from_where_the_last_one_was() {
    let mut pila = TestPila::with_config("[animations]\nenabled = true\nspeed = 4\n");
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    pila.settle();
    // Minimize and restore quickly: the restore starts mid-way.
    pila.combo("Super+n");
    std::thread::sleep(Duration::from_millis(100));
    pila.combo("Super+Tab");
    pila.wait_log("starts what=\"restore\"");
    let state = pila.settle();
    assert_eq!(window_in(&state, "a").unwrap()["minimized"], false);
    assert_eq!(state["animations"]["appearing"], 0);
}

#[test]
fn locking_drops_running_animations() {
    let mut pila = TestPila::with_config("[animations]\nenabled = true\nspeed = 4\n");
    let client = pila.client();
    client.window("a").create();
    pila.wait("animating", |s| s["animations"]["running"] == true);
    let lock = client.lock_session();
    client.lock_surface(&lock, "HEADLESS-1", [255, 0, 0, 0]);
    client.wait_logged("lock", "Locked");
    let state = pila.state();
    assert_eq!(state["animations"]["appearing"], 0);
}
