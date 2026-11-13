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
    // Its frame for the configure that activates it, in before counting.
    client.roundtrip();
    // Xwayland starts (the fonts are loaded already: `TestPila::start`).
    let started = std::time::Instant::now();
    while !pila.log().contains("Xwayland ready") && !pila.log().contains("X11 apps won't run") {
        assert!(
            started.elapsed() < TIMEOUT,
            "Xwayland never started nor failed"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(100));
    let before = frames(&pila.settle());
    std::thread::sleep(Duration::from_millis(300));
    let state = pila.state();
    assert_eq!(frames(&state), before, "frames drawn while idle");
    assert_eq!(state["redraw_pending"], false);
}

#[test]
fn an_animation_keeps_drawing_until_it_ends() {
    let mut pila = TestPila::with_config("[animations]\nenabled = true\nspeed = 4\n");
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    let before = frames(&pila.settle());
    pila.combo("Super+m");
    pila.wait_log("starts what=\"maximize\"");
    let after = frames(&pila.settle());
    // 880 ms at 60 Hz: about 50 frames, or 14 on a starved CPU with software
    // rendering. Without its own redraws an animation gets one or two. Whether
    // it hits every refresh is for scripts/check, on real outputs.
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

/// How many of the textures slides draw into are kept, unused.
fn slide_textures(state: &Value) -> u64 {
    state["animations"]["slide_textures"].as_u64().unwrap()
}

/// A slide's textures are as large as its output: made as it starts, they
/// made its first frame late on bare metal. They're there before any slide,
/// and come back when it ends.
#[test]
fn a_slides_textures_are_ready_before_it_and_kept_after() {
    let mut pila = TestPila::with_config("[animations]\nenabled = true\nspeed = 4\n");
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    let state = pila.wait("textures made ahead", |s| slide_textures(s) == 2);
    assert_eq!(state["animations"]["slide"], false);

    pila.combo("Super+2");
    pila.wait_log("starts what=\"workspace\"");
    assert_eq!(
        slide_textures(&pila.state()),
        0,
        "the slide didn't take them"
    );
    pila.settle();
    assert_eq!(slide_textures(&pila.state()), 2, "they didn't come back");
    assert!(
        !pila.log().contains("slide: a new texture"),
        "the slide made a texture of its own"
    );

    // A switch during a slide draws its leaving picture from the two in use:
    // a third, kept too, and no more however many follow.
    for keys in ["Super+1", "Super+2", "Super+1", "Super+2"] {
        pila.combo(keys);
    }
    pila.settle();
    assert_eq!(slide_textures(&pila.state()), 3);
    assert_eq!(pila.log().matches("slide: a new texture").count(), 1);
}

#[test]
fn slide_textures_follow_the_outputs_and_the_setting() {
    let mut pila = TestPila::with_config("[animations]\nenabled = true\n");
    pila.wait("textures made ahead", |s| slide_textures(s) == 2);
    // Another output, of another size: its own two.
    pila.request(json!({ "cmd": "output_add", "spec": "NEW:800x600@75" }));
    pila.wait("the new output's", |s| slide_textures(s) == 4);
    pila.request(json!({ "cmd": "output_remove", "name": "NEW" }));
    pila.wait("the gone output's dropped", |s| slide_textures(s) == 2);
    // Not animated, a switch takes none.
    pila.write_config("[animations]\nenabled = true\nworkspace = false\n");
    pila.wait("none kept", |s| slide_textures(s) == 0);
    pila.write_config("workspaces = 1\n[animations]\nenabled = true\n");
    pila.settle();
    assert_eq!(slide_textures(&pila.state()), 0);
}
