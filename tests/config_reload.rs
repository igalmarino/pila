//! Config reloads: each section applied while running, invalid files kept
//! out, autostart not run again.

mod common;

use common::*;

const BAR: i64 = 28;

#[test]
fn a_new_binding_works_after_a_reload() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").create();
    pila.wait_focus("a");
    pila.write_config("[bindings]\n\"Mod+x\" = \"close\"\n");
    pila.wait_log("config reloaded");
    pila.combo("Super+x");
    client.wait("close request", |s| s.windows[a.0].close_requested);
}

#[test]
fn an_invalid_file_keeps_the_running_config() {
    let mut pila = TestPila::with_config("[bindings]\n\"Mod+x\" = \"close\"\n");
    let client = pila.client();
    let a = client.window("a").create();
    pila.wait_focus("a");
    pila.write_config("[bindings]\n\"Mod+x\" = \"no-such-action\"\n");
    pila.wait_log("config not reloaded; keeping the running one");
    pila.combo("Super+x");
    client.wait("close request", |s| s.windows[a.0].close_requested);
}

#[test]
fn an_invalid_file_at_startup_is_notified() {
    // pila still starts, on the defaults, and says so on screen as well as in
    // the log: `workspaces` would be 2 if the file had been used.
    let mut pila = TestPila::with_config("workspaces = 2\n[[window-rule]]\nno-such-field = 1\n");
    assert_ne!(pila.state()["workspaces"], 2);
    pila.wait_log("summary=\"Invalid config, using the defaults\"");
    assert!(pila.log().contains("no-such-field"));
}

#[test]
fn a_syntax_error_keeps_the_running_config() {
    let mut pila = TestPila::with_config("workspaces = 2\n");
    pila.write_config("workspaces = = 6\n");
    pila.wait_log("config not reloaded");
    assert_eq!(pila.state()["workspaces"], 2);
}

#[test]
fn the_keyboard_layout_changes_on_reload() {
    // The key that types y on a US keyboard types z on a German one.
    let mut pila = TestPila::with_config("[bindings]\n\"Mod+z\" = \"close\"\n");
    let client = pila.client();
    let a = client.window("a").create();
    pila.wait_focus("a");
    // evdev KEY_Y (21) is xkb keycode 29.
    pila.request(json!({ "cmd": "key", "key": "Super", "state": "press" }));
    pila.request(json!({ "cmd": "key", "key": "code:29", "state": "tap" }));
    pila.request(json!({ "cmd": "key", "key": "Super", "state": "release" }));
    client.roundtrip();
    assert!(!client.with_window(a, |w| w.close_requested));
    pila.write_config("[bindings]\n\"Mod+z\" = \"close\"\n\n[keyboard]\nlayout = \"de\"\n");
    pila.wait_log("config reloaded");
    pila.request(json!({ "cmd": "key", "key": "Super", "state": "press" }));
    pila.request(json!({ "cmd": "key", "key": "code:29", "state": "tap" }));
    pila.request(json!({ "cmd": "key", "key": "Super", "state": "release" }));
    client.wait("close request", |s| s.windows[a.0].close_requested);
}

#[test]
fn an_invalid_keymap_keeps_the_current_one() {
    let mut pila = TestPila::start();
    pila.write_config("[keyboard]\nlayout = \"no-such-layout\"\n");
    pila.wait_log("invalid keyboard config; keeping the current keymap");
    let client = pila.client();
    let a = client.window("a").create();
    pila.wait_focus("a");
    pila.combo("Super+Shift+q");
    client.wait("close request", |s| s.windows[a.0].close_requested);
}

#[test]
fn the_mod_key_changes_on_reload() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").create();
    pila.wait_focus("a");
    pila.write_config("mod = \"alt\"\n");
    pila.wait_log("config reloaded");
    pila.combo("Alt+Shift+q");
    client.wait("close request", |s| s.windows[a.0].close_requested);
}

#[test]
fn output_scale_position_and_transform_apply_on_reload() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1280x800".into()),
        ..Options::default()
    });
    pila.write_config(
        "[[output]]\nname = \"A\"\nscale = 2\n\n\
         [[output]]\nname = \"B\"\nposition = [0, 400]\ntransform = \"90\"\n",
    );
    let state = pila.wait("the new layout", |s| s["outputs"][0]["scale"] == 2.0);
    let outputs = state["outputs"].as_array().unwrap();
    assert_eq!(rect(&outputs[0]["geometry"]), (0, 0, 640, 400));
    // Rotated a quarter turn: logical width and height swap.
    assert_eq!(rect(&outputs[1]["geometry"]), (0, 400, 800, 1280));
    assert_eq!(outputs[1]["transform"], "_90");
}

#[test]
fn an_output_disabled_by_the_config_moves_its_windows() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1280x800".into()),
        ..Options::default()
    });
    pila.move_to(1900.0, 400.0);
    let client = pila.client();
    client.window("a").create();
    let w = pila.wait_window("a");
    assert_eq!(w["output"], "B");
    pila.write_config("[[output]]\nname = \"B\"\nenabled = false\n");
    let state = pila.wait("B off", |s| s["outputs"][1]["enabled"] == false);
    assert!(state["outputs"][1]["geometry"].is_null());
    let state = pila.wait("a moved onto A", |s| {
        window_in(s, "a").unwrap()["output"] == "A"
    });
    let (x, y, w, h) = rect(&window_in(&state, "a").unwrap()["geometry"]);
    assert!(
        x >= 0 && y >= BAR && x + w <= 1280 && y + h <= 800,
        "{x},{y} {w}x{h}"
    );
    // And back.
    pila.write_config("");
    pila.wait("B on", |s| s["outputs"][1]["enabled"] == true);
}

#[test]
fn every_output_off_leaves_one_on() {
    let mut pila = TestPila::start();
    pila.write_config("[[output]]\nname = \"HEADLESS-1\"\nenabled = false\n");
    pila.wait_log("config reloaded");
    let state = pila.settle();
    assert_eq!(state["outputs"][0]["enabled"], true);
}

#[test]
fn autostart_runs_once_not_on_reload() {
    let mut pila =
        TestPila::with_config("autostart = [\"echo started >> $XDG_RUNTIME_DIR/autostart\"]\n");
    assert_eq!(pila.wait_file("autostart"), "started\n");
    pila.write_config("autostart = [\"echo again >> $XDG_RUNTIME_DIR/autostart\"]\n");
    pila.wait_log("config reloaded");
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(pila.wait_file("autostart"), "started\n");
}

#[test]
fn a_command_line_command_replaces_autostart() {
    let pila = TestPila::start_with(Options {
        config: "autostart = [\"echo autostart > $XDG_RUNTIME_DIR/ran\"]\n".into(),
        command: Some("echo command > $XDG_RUNTIME_DIR/ran".into()),
        ..Options::default()
    });
    assert_eq!(pila.wait_file("ran"), "command\n");
}

#[test]
fn a_reload_with_an_unchanged_file_keeps_windows_as_they_are() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    let before = pila.window("a");
    pila.write_config("");
    pila.wait_log("config reloaded");
    let after = pila.settle();
    assert_eq!(
        window_in(&after, "a").unwrap()["geometry"],
        before["geometry"]
    );
    assert_eq!(after["focused"], "a");
}
