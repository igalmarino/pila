//! Windows kept above the others or on all workspaces, moving windows and
//! focus between outputs, snapping to quarters, showing the desktop.

mod common;

use common::client::{states, Decorations};
use common::protocols::LayerSpec;
use common::*;
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::Layer, zwlr_layer_surface_v1::Anchor,
};

const BAR: i64 = 28;
const SLOW: &str = "[animations]\nenabled = true\nspeed = 4\n";

fn top(state: &Value) -> String {
    stacking(state).last().cloned().unwrap_or_default()
}

fn two_outputs(config: &str) -> TestPila {
    TestPila::start_with(Options {
        config: config.into(),
        outputs: Some("A:1280x800,B:1280x800".into()),
        ..Options::default()
    })
}

/// `pin` kept above the others by its binding, then `other` opened and focused.
fn pinned_and_other(pila: &mut TestPila, client: &Client) {
    client.window("pin").size(300, 200).create();
    pila.wait_focus("pin");
    pila.combo("Super+t");
    pila.wait("marked", |s| {
        window_in(s, "pin").unwrap()["always_on_top"] == true
    });
    client.window("other").size(600, 400).create();
    pila.wait_focus("other");
}

#[test]
fn a_window_on_top_stays_above_one_that_opens_and_takes_the_focus() {
    let mut pila = TestPila::start();
    let client = pila.client();
    pinned_and_other(&mut pila, &client);
    assert_eq!(top(&pila.state()), "pin");
    assert_eq!(pila.state()["focused"], "other");
}

#[test]
fn closing_a_window_focuses_the_one_used_before_not_the_one_on_top() {
    let mut pila = TestPila::start();
    let client = pila.client();
    pinned_and_other(&mut pila, &client);
    let b = client.window("b").size(500, 300).create();
    pila.wait_focus("b");
    client.destroy(b);
    pila.wait("b gone", |s| window_in(s, "b").is_none());
    pila.wait_focus("other");
    assert_eq!(top(&pila.state()), "pin");
}

#[test]
fn a_launcher_going_away_gives_the_keyboard_back_to_the_window_used_before() {
    use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::KeyboardInteractivity;
    let mut pila = TestPila::start();
    let client = pila.client();
    pinned_and_other(&mut pila, &client);
    let (launcher, layer) = client.layer_surface(
        "launcher",
        &LayerSpec {
            layer: Layer::Overlay,
            keyboard: KeyboardInteractivity::Exclusive,
            ..LayerSpec::default()
        },
        [255, 0, 0, 255],
    );
    client.wait_surface_configured(launcher);
    pila.wait("the launcher has the keyboard", |s| {
        s["keyboard_focus"] == "layer:launcher"
    });
    layer.destroy();
    client.wl_surface_of(launcher).destroy();
    client.flush();
    pila.wait("a window has the keyboard", |s| s["focused"].is_string());
    assert_eq!(pila.state()["focused"], "other");
}

#[test]
fn unlocking_gives_the_keyboard_back_to_the_window_used_before() {
    let mut pila = TestPila::start();
    let client = pila.client();
    pinned_and_other(&mut pila, &client);
    let locker = pila.client();
    let lock = locker.lock_session();
    let surf = locker.lock_surface(&lock, "HEADLESS-1", [255, 10, 20, 30]);
    locker.wait_surface_configured(surf);
    locker.wait_logged("lock", "Locked");
    pila.wait("locked", |s| {
        s["locked"] == true && s["lock_pending"] == false
    });
    lock.unlock_and_destroy();
    locker.flush();
    pila.wait("unlocked", |s| s["locked"] == false);
    pila.wait("a window has the keyboard", |s| s["focused"].is_string());
    assert_eq!(pila.state()["focused"], "other");
}

#[test]
fn a_window_on_top_stays_above_whatever_moves_the_others() {
    let mut pila = TestPila::start();
    let client = pila.client();
    pinned_and_other(&mut pila, &client);
    let check = |pila: &mut TestPila, what: &str| {
        pila.settle();
        assert_eq!(top(&pila.state()), "pin", "after {what}");
    };

    let (x, y, ..) = rect(&pila.window("other")["geometry"]);
    pila.click_at((x + 5) as f64, (y + 395) as f64);
    check(&mut pila, "a click on the other");

    pila.combo("Super+Shift+Right");
    check(&mut pila, "a keyboard move");
    pila.combo("Super+Ctrl+Right");
    check(&mut pila, "a keyboard resize");
    pila.combo("Super+c");
    check(&mut pila, "centering");

    let (x, y, ..) = rect(&pila.window("other")["geometry"]);
    pila.move_to((x + 5) as f64, (y - BAR / 2) as f64);
    pila.press("left");
    pila.wait("a move grab", |s| s["pointer"]["grab"] == "move");
    pila.move_by(30.0, 20.0);
    check(&mut pila, "a drag");
    pila.release("left");

    pila.combo("Super+Left");
    pila.wait("snapped", |s| {
        window_in(s, "other").unwrap()["snapped"] == "left"
    });
    check(&mut pila, "a snap");
    pila.combo("Super+Left");
    check(&mut pila, "an unsnap");

    pila.combo("Super+m");
    pila.wait("maximized", |s| {
        window_in(s, "other").unwrap()["maximized"] == true
    });
    check(&mut pila, "a maximize");
    pila.combo("Super+m");
    pila.wait("back", |s| {
        window_in(s, "other").unwrap()["maximized"] == false
    });
    check(&mut pila, "a restore");

    pila.combo("Super+n");
    pila.wait("minimized", |s| {
        window_in(s, "other").unwrap()["minimized"] == true
    });
    pila.combo("Super+Tab");
    check(&mut pila, "Mod+Tab");

    pila.combo("Super+2");
    pila.wait("workspace 2", |s| s["workspace"] == 2);
    pila.combo("Super+1");
    pila.wait("workspace 1", |s| s["workspace"] == 1);
    check(&mut pila, "coming back from another workspace");
}

#[test]
fn a_window_on_top_can_stop_being_so() {
    let mut pila = TestPila::start();
    let client = pila.client();
    pinned_and_other(&mut pila, &client);
    pila.combo("Super+Tab");
    pila.wait_focus("pin");
    pila.combo("Super+t");
    pila.wait("unmarked", |s| {
        window_in(s, "pin").unwrap()["always_on_top"] == false
    });
    pila.combo("Super+Tab");
    pila.wait_focus("other");
    assert_eq!(top(&pila.state()), "other");
}

#[test]
fn the_last_focused_of_two_windows_on_top_is_above() {
    let mut pila = TestPila::start();
    let client = pila.client();
    for name in ["one", "two"] {
        client.window(name).size(300, 200).create();
        pila.wait_focus(name);
        pila.combo("Super+t");
        pila.wait("marked", |s| {
            window_in(s, name).unwrap()["always_on_top"] == true
        });
    }
    client.window("other").size(600, 400).create();
    pila.wait_focus("other");
    assert_eq!(stacking(&pila.state()), ["other", "one", "two"]);
    // Back two steps by recent use: other, two, one.
    pila.key_press("Super");
    pila.key("Tab");
    pila.key("Tab");
    pila.key_release("Super");
    pila.wait_focus("one");
    assert_eq!(stacking(&pila.state()), ["other", "two", "one"]);
}

#[test]
fn a_dialog_of_a_window_on_top_comes_with_it_and_one_of_another_stays_below() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let pin = client.window("pin").size(300, 200).create();
    pila.wait_focus("pin");
    pila.combo("Super+t");
    pila.wait("marked", |s| {
        window_in(s, "pin").unwrap()["always_on_top"] == true
    });
    let parent = client.toplevel(pin);
    client
        .window("dialog")
        .size(100, 100)
        .before_commit(|t| t.set_parent(Some(&parent)))
        .create();
    pila.wait_focus("dialog");
    assert_eq!(
        window_in(&pila.state(), "dialog").unwrap()["always_on_top"],
        true
    );

    let other = client.window("other").size(600, 400).create();
    pila.wait_focus("other");
    assert_eq!(stacking(&pila.state()), ["other", "pin", "dialog"]);

    let parent = client.toplevel(other);
    client
        .window("own")
        .size(100, 100)
        .before_commit(|t| t.set_parent(Some(&parent)))
        .create();
    pila.wait_focus("own");
    assert_eq!(stacking(&pila.state()), ["other", "own", "pin", "dialog"]);
}

#[test]
fn a_click_on_a_window_on_top_goes_to_it() {
    let mut pila = TestPila::with_config(
        "[[window-rule]]\napp-id = \"pin\"\nalways-on-top = true\nsize = [200, 200]\n\
         position = [100, 100]\ntitle-bar = false\n\
         [[window-rule]]\napp-id = \"other\"\nsize = [600, 400]\nposition = [50, 50]\n\
         title-bar = false\n",
    );
    let client = pila.client();
    client.window("pin").create();
    pila.wait_focus("pin");
    client.window("other").create();
    pila.wait_focus("other");
    pila.click_at(150.0, 150.0);
    pila.wait_focus("pin");
    pila.click_at(500.0, 300.0);
    pila.wait_focus("other");
    assert_eq!(top(&pila.state()), "pin");
}

#[test]
fn a_window_on_top_doesnt_bring_panels_back_over_a_fullscreen_one() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let (panel, _) = client.layer_surface(
        "panel",
        &LayerSpec {
            layer: Layer::Top,
            anchor: Anchor::Top | Anchor::Left | Anchor::Right,
            size: (0, 30),
            ..LayerSpec::default()
        },
        [255, 0, 255, 0],
    );
    client.wait_surface_configured(panel);
    client.window("pin").size(300, 200).create();
    pila.wait_focus("pin");
    pila.combo("Super+t");
    pila.wait("marked", |s| {
        window_in(s, "pin").unwrap()["always_on_top"] == true
    });
    client.window("other").color([255, 255, 0, 0]).create();
    pila.wait_focus("other");
    pila.combo("Super+f");
    pila.wait("fullscreen", |s| {
        window_in(s, "other").unwrap()["fullscreen"] == true
    });
    pila.settle();
    assert_eq!(top(&pila.state()), "pin");
    // The panel's place shows the fullscreen window, not the panel.
    pila.wait_pixel(1200.0, 10.0, [255, 0, 0, 255]);
}

#[test]
fn a_rule_keeps_a_window_on_top_from_the_start_and_a_reload_leaves_it() {
    let config = "[[window-rule]]\napp-id = \"pin\"\nalways-on-top = true\n";
    let mut pila = TestPila::with_config(config);
    let client = pila.client();
    client.window("pin").create();
    pila.wait_focus("pin");
    client.window("other").create();
    pila.wait_focus("other");
    assert_eq!(top(&pila.state()), "pin");
    // Applied when it opens, like the open-* properties.
    pila.write_config("workspaces = 5\n");
    pila.wait_log("config reloaded");
    assert_eq!(
        window_in(&pila.state(), "pin").unwrap()["always_on_top"],
        true
    );
}

#[test]
fn a_window_on_all_workspaces_goes_along_above_what_comes_in() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("two").create();
    pila.wait_focus("two");
    pila.combo("Super+Shift+2");
    pila.wait("moved", |s| window_in(s, "two").unwrap()["workspace"] == 2);
    client.window("pin").size(300, 200).create();
    pila.wait_focus("pin");
    pila.combo("Super+p");
    pila.wait("marked", |s| {
        window_in(s, "pin").unwrap()["all_workspaces"] == true
    });
    let before = rect(&pila.window("pin")["geometry"]);

    pila.combo("Super+2");
    let state = pila.wait("workspace 2", |s| s["workspace"] == 2);
    assert_eq!(stacking(&state), ["two", "pin"]);
    assert_eq!(window_in(&state, "pin").unwrap()["mapped"], true);
    assert_eq!(window_in(&state, "pin").unwrap()["workspace"], 2);
    assert_eq!(rect(&window_in(&state, "pin").unwrap()["geometry"]), before);
    // The focus goes to the workspace's own window.
    assert_eq!(state["focused"], "two");
    assert_eq!(window_in(&state, "two").unwrap()["mapped"], true);

    pila.combo("Super+3");
    let state = pila.wait("workspace 3", |s| s["workspace"] == 3);
    assert_eq!(stacking(&state), ["pin"]);
    assert_eq!(state["focused"], "pin", "it's all there is");
}

#[test]
fn a_rule_puts_a_window_on_all_workspaces_whatever_else_it_says() {
    let mut pila = TestPila::with_config(
        "[[window-rule]]\napp-id = \"pin\"\nall-workspaces = true\nopen-on-workspace = 3\n",
    );
    let client = pila.client();
    client.window("pin").create();
    pila.wait_focus("pin");
    pila.combo("Super+2");
    let state = pila.wait("workspace 2", |s| s["workspace"] == 2);
    assert_eq!(stacking(&state), ["pin"]);
}

#[test]
fn sending_a_window_on_all_workspaces_to_one_leaves_it_there() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("pin").create();
    pila.wait_focus("pin");
    pila.combo("Super+p");
    pila.wait("marked", |s| {
        window_in(s, "pin").unwrap()["all_workspaces"] == true
    });
    pila.combo("Super+Shift+3");
    let state = pila.wait("sent", |s| window_in(s, "pin").unwrap()["mapped"] == false);
    let pin = window_in(&state, "pin").unwrap();
    assert_eq!(pin["all_workspaces"], false);
    assert_eq!(pin["workspace"], 3);
}

#[test]
fn a_minimized_window_on_all_workspaces_comes_back_where_you_are() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("pin").create();
    pila.wait_focus("pin");
    pila.combo("Super+p");
    pila.wait("marked", |s| {
        window_in(s, "pin").unwrap()["all_workspaces"] == true
    });
    pila.combo("Super+n");
    pila.wait("minimized", |s| {
        window_in(s, "pin").unwrap()["minimized"] == true
    });
    pila.combo("Super+2");
    pila.wait("workspace 2", |s| s["workspace"] == 2);
    pila.combo("Super+Tab");
    let state = pila.wait_focus("pin");
    assert_eq!(state["workspace"], 2);
    assert_eq!(window_in(&state, "pin").unwrap()["mapped"], true);
}

#[test]
fn a_dialog_opened_from_a_window_on_all_workspaces_goes_along() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let pin = client.window("pin").size(300, 200).create();
    pila.wait_focus("pin");
    pila.combo("Super+p");
    pila.wait("marked", |s| {
        window_in(s, "pin").unwrap()["all_workspaces"] == true
    });
    let parent = client.toplevel(pin);
    client
        .window("dialog")
        .size(100, 100)
        .before_commit(|t| t.set_parent(Some(&parent)))
        .create();
    pila.wait_focus("dialog");
    pila.combo("Super+2");
    let state = pila.wait("workspace 2", |s| s["workspace"] == 2);
    assert_eq!(stacking(&state), ["pin", "dialog"]);
}

#[test]
fn fewer_workspaces_leave_a_window_on_all_of_them_alone() {
    let mut pila = TestPila::with_config("workspaces = 5\n");
    let client = pila.client();
    client.window("pin").create();
    pila.wait_focus("pin");
    pila.combo("Super+p");
    pila.wait("marked", |s| {
        window_in(s, "pin").unwrap()["all_workspaces"] == true
    });
    pila.combo("Super+5");
    pila.wait("workspace 5", |s| s["workspace"] == 5);
    pila.write_config("workspaces = 2\n");
    let state = pila.wait("workspace 2", |s| s["workspace"] == 2);
    assert_eq!(stacking(&state), ["pin"]);
}

#[test]
fn a_window_on_all_workspaces_stays_put_during_a_slide() {
    let mut pila = TestPila::with_config(&format!(
        "{SLOW}[colors]\nbackground = \"#ffffff\"\n\
         [[window-rule]]\napp-id = \"pin\"\nsize = [200, 200]\nposition = [540, 300]\n\
         title-bar = false\nall-workspaces = true\n\
         [[window-rule]]\napp-id = \"gone\"\nsize = [200, 200]\nposition = [100, 300]\n\
         title-bar = false\n",
    ));
    let client = pila.client();
    client.window("gone").color([255, 0, 0, 255]).create();
    pila.wait_focus("gone");
    client.window("pin").color([255, 255, 0, 0]).create();
    pila.wait_focus("pin");
    pila.settle();
    pila.combo("Super+2");
    pila.wait_log("starts what=\"workspace\"");
    let started = std::time::Instant::now();
    let mut samples = 0;
    while pila.state()["animations"]["running"] == true {
        assert!(started.elapsed() < TIMEOUT, "it never ended");
        assert_eq!(pila.pixel(640.0, 400.0), [255, 0, 0, 255], "it moved");
        samples += 1;
    }
    assert!(samples > 0, "no slide seen");
    pila.settle();
    assert_eq!(pila.pixel(640.0, 400.0), [255, 0, 0, 255]);
    assert_eq!(
        pila.pixel(200.0, 400.0),
        [255, 255, 255, 255],
        "gone slid away"
    );
}

#[test]
fn a_window_moves_to_the_next_output_and_back_around() {
    let mut pila = two_outputs("");
    let client = pila.client();
    client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let (x, y, w, h) = rect(&pila.window("a")["geometry"]);
    pila.combo("Super+Shift+o");
    let state = pila.wait("on B", |s| window_in(s, "a").unwrap()["output"] == "B");
    assert_eq!(
        rect(&window_in(&state, "a").unwrap()["geometry"]),
        (x + 1280, y, w, h),
        "the same place on the other output"
    );
    assert_eq!(state["focused"], "a");
    pila.combo("Super+Shift+o");
    let state = pila.wait("on A", |s| window_in(s, "a").unwrap()["output"] == "A");
    assert_eq!(
        rect(&window_in(&state, "a").unwrap()["geometry"]),
        (x, y, w, h)
    );
}

#[test]
fn a_bound_action_moves_a_window_to_the_previous_output() {
    let mut pila = two_outputs(
        "[bindings]\n\"Mod+Y\" = \"move-to-output prev\"\n\"Mod+U\" = \"focus-output-prev\"\n",
    );
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    pila.combo("Super+y");
    pila.wait("on B", |s| window_in(s, "a").unwrap()["output"] == "B");
    pila.combo("Super+u");
    pila.wait("the pointer on B", |s| {
        s["pointer"]["x"].as_f64().unwrap() > 1280.0
    });
}

#[test]
fn a_window_too_big_for_the_next_output_stays_on_it() {
    let mut pila = TestPila::start_with(Options {
        config: "[[window-rule]]\napp-id = \"a\"\nposition = [600, 300]\nsize = [600, 400]\n"
            .into(),
        outputs: Some("A:1280x800,B:800x600".into()),
        ..Options::default()
    });
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    pila.combo("Super+Shift+o");
    let state = pila.wait("on B", |s| window_in(s, "a").unwrap()["output"] == "B");
    let (x, y, w, h) = rect(&window_in(&state, "a").unwrap()["geometry"]);
    assert!(x >= 1280 && x + w <= 1280 + 800, "{x} {w}");
    assert!(y >= BAR && y + h <= 600, "{y} {h}");
}

#[test]
fn a_filled_window_fills_the_next_output_and_comes_back_there() {
    for (combo, field) in [("Super+m", "maximized"), ("Super+f", "fullscreen")] {
        let mut pila = two_outputs("");
        let client = pila.client();
        client.window("a").size(400, 300).create();
        pila.wait_focus("a");
        let (x, y, w, h) = rect(&pila.window("a")["geometry"]);
        pila.combo(combo);
        pila.wait("filled", |s| window_in(s, "a").unwrap()[field] == true);
        pila.combo("Super+Shift+o");
        let state = pila.wait("on B", |s| {
            let a = window_in(s, "a").unwrap();
            a["output"] == "B" && a["geometry"]["w"] == 1280
        });
        assert_eq!(window_in(&state, "a").unwrap()[field], true);
        pila.combo(combo);
        let state = pila.wait("back", |s| {
            let a = window_in(s, "a").unwrap();
            a[field] == false && a["geometry"]["w"] == w
        });
        assert_eq!(
            rect(&window_in(&state, "a").unwrap()["geometry"]),
            (x + 1280, y, w, h),
            "{field}"
        );
    }
}

#[test]
fn a_snapped_window_snaps_to_the_same_side_of_the_next_output() {
    let mut pila = two_outputs("");
    let client = pila.client();
    client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let (x, y, w, h) = rect(&pila.window("a")["geometry"]);
    pila.combo("Super+Right");
    pila.wait("snapped", |s| {
        window_in(s, "a").unwrap()["snapped"] == "right"
    });
    pila.combo("Super+Shift+o");
    let state = pila.wait("on B", |s| {
        let a = window_in(s, "a").unwrap();
        a["output"] == "B" && a["geometry"]["x"] == 1280 + 640
    });
    assert_eq!(window_in(&state, "a").unwrap()["snapped"], "right");
    pila.combo("Super+Right");
    pila.wait("unsnapped", |s| {
        let a = window_in(s, "a").unwrap();
        a["snapped"].is_null() && rect(&a["geometry"]) == (x + 1280, y, w, h)
    });
}

#[test]
fn a_dialog_goes_to_the_next_output_with_its_window() {
    let mut pila = two_outputs("");
    let client = pila.client();
    let parent = client.window("parent").size(400, 300).create();
    pila.wait_focus("parent");
    let toplevel = client.toplevel(parent);
    client
        .window("dialog")
        .size(100, 100)
        .before_commit(|t| t.set_parent(Some(&toplevel)))
        .create();
    pila.wait_focus("dialog");
    pila.combo("Super+Tab");
    let state = pila.state();
    let before = rect(&window_in(&state, "dialog").unwrap()["geometry"]);
    pila.combo("Super+Shift+o");
    let state = pila.wait("on B", |s| {
        window_in(s, "dialog").unwrap()["output"] == "B"
            && window_in(s, "parent").unwrap()["output"] == "B"
    });
    let after = rect(&window_in(&state, "dialog").unwrap()["geometry"]);
    assert_eq!(after.0, before.0 + 1280);
    assert_eq!(after.1, before.1);
}

#[test]
fn outputs_of_other_scales_take_a_moved_window() {
    let mut pila = two_outputs("[[output]]\nname = \"B\"\nscale = 2\n");
    let client = pila.client();
    client.window("a").size(300, 200).create();
    pila.wait_focus("a");
    pila.combo("Super+Shift+o");
    let state = pila.wait("on B", |s| window_in(s, "a").unwrap()["output"] == "B");
    let (x, _, w, _) = rect(&window_in(&state, "a").unwrap()["geometry"]);
    assert!(x >= 1280 && x + w <= 1280 + 640, "{x} {w}");
}

#[test]
fn with_one_output_nothing_moves() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let before = pila.state();
    pila.combo("Super+Shift+o");
    pila.combo("Super+o");
    pila.settle();
    let after = pila.state();
    assert_eq!(after["focused"], "a");
    assert_eq!(after["pointer"]["x"], before["pointer"]["x"]);
    assert_eq!(
        rect(&pila.window("a")["geometry"]),
        rect(&window_in(&before, "a").unwrap()["geometry"])
    );
}

#[test]
fn focusing_an_output_moves_the_pointer_and_takes_its_top_window() {
    let mut pila = two_outputs("");
    let client = pila.client();
    client.window("a").size(300, 200).create();
    pila.wait_focus("a");
    pila.combo("Super+Shift+o");
    pila.wait("on B", |s| window_in(s, "a").unwrap()["output"] == "B");
    client.window("b").size(300, 200).create();
    pila.wait_focus("b");
    assert_eq!(pila.window("b")["output"], "A");

    pila.combo("Super+o");
    let state = pila.wait_focus("a");
    assert_eq!(state["pointer"]["x"].as_f64().unwrap(), 1280.0 + 640.0);
    assert_eq!(state["pointer"]["y"].as_f64().unwrap(), 400.0);

    pila.combo("Super+o");
    let state = pila.wait_focus("b");
    assert_eq!(state["pointer"]["x"].as_f64().unwrap(), 640.0);

    // An output without windows: nothing focused.
    pila.combo("Super+Shift+o");
    pila.wait("b on B", |s| window_in(s, "b").unwrap()["output"] == "B");
    let on_a = pila.state()["pointer"]["x"].as_f64().unwrap() < 1280.0;
    assert!(on_a);
    pila.combo("Super+o");
    pila.wait("pointer on B", |s| {
        s["pointer"]["x"].as_f64().unwrap() > 1280.0
    });
    pila.combo("Super+o");
    pila.wait("nothing focused", |s| s["focused"].is_null());
}

#[test]
fn the_quarters_take_a_quarter_of_the_output() {
    let bindings = "[bindings]\n\"Mod+1\" = \"snap-top-left\"\n\"Mod+2\" = \"snap-top-right\"\n\
                    \"Mod+3\" = \"snap-bottom-left\"\n\"Mod+4\" = \"snap-bottom-right\"\n";
    let mut pila = TestPila::with_config(bindings);
    let client = pila.client();
    let window = client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let before = rect(&pila.window("a")["geometry"]);
    let (t, b, l, r) = (
        states::TILED_TOP,
        states::TILED_BOTTOM,
        states::TILED_LEFT,
        states::TILED_RIGHT,
    );
    for (combo, side, geometry, has, hasnt) in [
        (
            "Super+1",
            "top-left",
            (0, BAR, 640, 400 - BAR),
            [t, l],
            [b, r],
        ),
        (
            "Super+2",
            "top-right",
            (640, BAR, 640, 400 - BAR),
            [t, r],
            [b, l],
        ),
        (
            "Super+3",
            "bottom-left",
            (0, 400 + BAR, 640, 400 - BAR),
            [b, l],
            [t, r],
        ),
        (
            "Super+4",
            "bottom-right",
            (640, 400 + BAR, 640, 400 - BAR),
            [b, r],
            [t, l],
        ),
    ] {
        pila.combo(combo);
        let state = pila.wait(side, |s| window_in(s, "a").unwrap()["snapped"] == side);
        client.wait_configure(window, side, |c| {
            has.iter().all(|s| c.has(*s)) && !hasnt.iter().any(|s| c.has(*s))
        });
        let _ = state;
        let state = pila.wait("its size", |s| {
            rect(&window_in(s, "a").unwrap()["geometry"]) == geometry
        });
        assert_eq!(window_in(&state, "a").unwrap()["snapped"], side);
    }
    // Again: restored to where it was before the first.
    pila.combo("Super+4");
    pila.wait("restored", |s| {
        let a = window_in(s, "a").unwrap();
        a["snapped"].is_null() && rect(&a["geometry"]) == before
    });
}

#[test]
fn a_quarter_without_a_title_bar_takes_all_of_it() {
    let mut pila = TestPila::with_config(
        "[[window-rule]]\napp-id = \"a\"\ntitle-bar = false\n[bindings]\n\"Mod+Y\" = \"snap-bottom-right\"\n",
    );
    let client = pila.client();
    client
        .window("a")
        .decorations(Decorations::Server)
        .size(400, 300)
        .create();
    pila.wait_focus("a");
    pila.combo("Super+y");
    pila.wait("its size", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == (640, 400, 640, 400)
    });
}

#[test]
fn dragging_to_a_corner_snaps_to_a_quarter_with_a_preview() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    pila.move_to((x + 100) as f64, (y + 100) as f64);
    pila.key_press("Super");
    pila.press("left");
    pila.wait("a move grab", |s| s["pointer"]["grab"] == "move");
    pila.move_to(0.0, 799.0);
    let state = pila.wait("a preview", |s| !s["snap_preview"].is_null());
    assert_eq!(rect(&state["snap_preview"]), (0, 400, 640, 400));
    pila.release("left");
    pila.key_release("Super");
    let state = pila.wait("snapped", |s| {
        window_in(s, "a").unwrap()["snapped"] == "bottom-left"
    });
    assert!(state["snap_preview"].is_null());
    pila.wait("its size", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == (0, 400 + BAR, 640, 400 - BAR)
    });
}

#[test]
fn showing_the_desktop_minimizes_the_windows_and_again_brings_them_back() {
    let mut pila = TestPila::start();
    let client = pila.client();
    for name in ["a", "b", "c"] {
        client.window(name).size(300, 200).create();
        pila.wait_focus(name);
    }
    pila.combo("Super+Tab");
    pila.wait_focus("b");
    let order = stacking(&pila.state());
    pila.combo("Super+h");
    let state = pila.wait("nothing shown", |s| stacking(s).is_empty());
    assert!(state["focused"].is_null());
    for name in ["a", "b", "c"] {
        assert_eq!(window_in(&state, name).unwrap()["minimized"], true);
    }
    pila.combo("Super+h");
    let state = pila.wait_focus("b");
    assert_eq!(stacking(&state), order);
}

#[test]
fn showing_the_desktop_leaves_what_was_minimized_already() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    client.window("b").create();
    pila.wait_focus("b");
    pila.combo("Super+n");
    pila.wait("b minimized", |s| {
        window_in(s, "b").unwrap()["minimized"] == true
    });
    pila.combo("Super+h");
    pila.wait("a minimized", |s| {
        window_in(s, "a").unwrap()["minimized"] == true
    });
    pila.combo("Super+h");
    let state = pila.wait_focus("a");
    assert_eq!(window_in(&state, "b").unwrap()["minimized"], true);
}

#[test]
fn showing_the_desktop_takes_only_this_workspace() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    pila.combo("Super+Shift+2");
    pila.wait("moved", |s| window_in(s, "a").unwrap()["workspace"] == 2);
    client.window("b").create();
    pila.wait_focus("b");
    pila.combo("Super+h");
    let state = pila.wait("b minimized", |s| {
        window_in(s, "b").unwrap()["minimized"] == true
    });
    assert_eq!(window_in(&state, "a").unwrap()["minimized"], false);
    // On another workspace the same key starts over there.
    pila.combo("Super+2");
    pila.wait_focus("a");
    pila.combo("Super+h");
    let state = pila.wait("a minimized", |s| {
        window_in(s, "a").unwrap()["minimized"] == true
    });
    assert_eq!(window_in(&state, "b").unwrap()["minimized"], true);
}

#[test]
fn a_new_window_ends_a_shown_desktop() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    pila.combo("Super+h");
    pila.wait("a minimized", |s| {
        window_in(s, "a").unwrap()["minimized"] == true
    });
    client.window("b").create();
    pila.wait_focus("b");
    // Not a way back: it minimizes what's there.
    pila.combo("Super+h");
    let state = pila.wait("b minimized", |s| {
        window_in(s, "b").unwrap()["minimized"] == true
    });
    assert_eq!(window_in(&state, "a").unwrap()["minimized"], true);
}

#[test]
fn bringing_one_window_back_ends_a_shown_desktop() {
    let mut pila = TestPila::start();
    let client = pila.client();
    for name in ["a", "b"] {
        client.window(name).create();
        pila.wait_focus(name);
    }
    pila.combo("Super+h");
    pila.wait("hidden", |s| stacking(s).is_empty());
    pila.combo("Super+Tab");
    pila.key("Super");
    let state = pila.wait("one back", |s| stacking(s).len() == 1);
    let back = stacking(&state)[0].clone();
    pila.combo("Super+h");
    let state = pila.wait("hidden again", |s| stacking(s).is_empty());
    assert_eq!(window_in(&state, &back).unwrap()["minimized"], true);
}

#[test]
fn a_dialog_goes_and_comes_back_with_a_shown_desktop() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let parent = client.window("parent").size(400, 300).create();
    pila.wait_focus("parent");
    let toplevel = client.toplevel(parent);
    client
        .window("dialog")
        .size(100, 100)
        .before_commit(|t| t.set_parent(Some(&toplevel)))
        .create();
    pila.wait_focus("dialog");
    pila.combo("Super+h");
    pila.wait("hidden", |s| stacking(s).is_empty());
    pila.combo("Super+h");
    let state = pila.wait("back", |s| stacking(s).len() == 2);
    assert_eq!(stacking(&state), ["parent", "dialog"]);
}
