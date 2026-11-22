//! The shortcuts cheatsheet (`shortcuts_view.rs`): shown on the first run
//! (the welcome screen) and by `show-shortcuts` (Mod+?); hidden by any key
//! but a modifier, which goes nowhere else unless it's a binding, or by a
//! click, which goes nowhere else on the panel; never while locked.

mod common;

use common::*;

const KEY_ESC: u32 = 1;
const KEY_A: u32 = 30;
const BTN_LEFT: u32 = 0x110;

fn shown(state: &serde_json::Value) -> bool {
    state["shortcuts_view"]["shown"] == true
}

/// A window "a" that has the keyboard, under a cheatsheet shown by Mod+?.
fn shown_over_a_window(config: &str) -> (TestPila, Client) {
    let mut pila = TestPila::with_config(config);
    let client = pila.client();
    client.window("a").size(1280, 800).create();
    pila.wait_focus("a");
    pila.settle();
    client.roundtrip();
    pila.combo("Super+Shift+slash");
    pila.wait("shown", shown);
    (pila, client)
}

#[test]
fn the_first_run_welcomes_with_the_shortcuts() {
    let mut pila = TestPila::start_with(Options {
        default_config_path: true,
        ..Options::default()
    });
    pila.wait_log("created the config from the example");
    let state = pila.wait("welcome", shown);
    let view = &state["shortcuts_view"];
    assert_eq!(view["heading"], "Welcome to Pila");
    assert_eq!(view["output"], "HEADLESS-1");
    // Centered on the output, 1280x800.
    let (x, y, w, h) = rect(&view["rect"]);
    assert_eq!((x, 1280 - x - w), (x, x));
    assert!((y - (800 - y - h)).abs() <= 1, "{view}");
    let rows = view["rows"].as_array().unwrap();
    assert!(
        rows.contains(&serde_json::json!(["Super+Return", "Open a terminal"])),
        "{view}"
    );
    assert!(
        rows.contains(&serde_json::json!(["Super+Shift+/", "All the shortcuts"])),
        "{view}"
    );
    assert!(
        view["notes"][0].as_str().unwrap().starts_with("Settings: "),
        "{view}"
    );

    // Drawn, with its content: the panel isn't the desktop, the title's
    // line, centered, has text in it, and the first row's keys are keycaps
    // in the title bars' color ([colors] bar).
    let desktop = pila.pixel(5.0, 5.0);
    let panel_y = (y + 20) as f64;
    pila.wait_pixel_is((x + w - 5) as f64, panel_y, "the panel", |p| p != desktop);
    let panel = pila.pixel((x + w - 5) as f64, panel_y);
    let title_y = (y + 54) as f64;
    let middle = x + w / 2;
    let text = (middle - 110..middle + 110).any(|tx| pila.pixel(tx as f64, title_y) != panel);
    assert!(text, "no title drawn");
    assert_eq!(
        pila.pixel((x + 93) as f64, (y + 131) as f64),
        [0x2f, 0x31, 0x36, 255]
    );

    // Escape hides it, and what was under it shows again.
    pila.key("Escape");
    pila.wait("hidden", |s| !shown(s));
    pila.wait_pixel((x + w - 5) as f64, panel_y, desktop);
}

#[test]
fn a_config_of_ones_own_starts_without_it_and_mod_question_mark_toggles_it() {
    let mut pila = TestPila::start_with(Options {
        default_config_path: true,
        config: "# mine\n".into(),
        ..Options::default()
    });
    pila.wait_log("watching the config for changes");
    pila.stays("not shown", std::time::Duration::from_millis(300), |s| {
        !shown(s)
    });
    pila.combo("Super+Shift+slash");
    let state = pila.wait("shown", shown);
    assert_eq!(state["shortcuts_view"]["heading"], "Shortcuts");
    // Only the welcome names the settings file.
    assert_eq!(
        state["shortcuts_view"]["notes"].as_array().unwrap().len(),
        0
    );
    pila.combo("Super+Shift+slash");
    pila.wait("hidden", |s| !shown(s));
}

#[test]
fn a_key_that_isnt_a_binding_hides_it_and_reaches_no_window() {
    let (mut pila, client) = shown_over_a_window("");
    // A modifier alone doesn't hide it.
    pila.key("Shift_L");
    pila.stays("shown", std::time::Duration::from_millis(200), shown);
    pila.key("Escape");
    pila.wait("hidden", |s| !shown(s));
    pila.settle();
    client.roundtrip();
    assert!(
        !client.keys().iter().any(|(k, _)| *k == KEY_ESC),
        "{:?}",
        client.keys()
    );
    // Hidden, keys reach the window again.
    pila.key("a");
    client.wait("the key", |s| {
        s.events.iter().any(|e| {
            *e == common::client::Ev::Key {
                key: KEY_A,
                pressed: false,
            }
        })
    });
}

#[test]
fn a_binding_hides_it_and_still_runs() {
    let (mut pila, _client) = shown_over_a_window(
        "[bindings]\n\"Mod+Return\" = \"spawn echo ran > \\\"$XDG_RUNTIME_DIR/out\\\"\"\n",
    );
    pila.combo("Super+Return");
    pila.wait("hidden", |s| !shown(s));
    assert_eq!(pila.wait_file("out"), "ran\n");
}

#[test]
fn the_rows_follow_the_bindings() {
    let (mut pila, _client) = shown_over_a_window(
        "[bindings]\n\"Mod+Return\" = \"none\"\n\"Mod+Shift+F12\" = \"quit\"\n",
    );
    let state = pila.state();
    let rows = state["shortcuts_view"]["rows"].as_array().unwrap().clone();
    assert!(!rows.iter().any(|r| r[1] == "Open a terminal"), "{rows:?}");
    // Named keys (F12) before letters (E): the one named.
    assert!(
        rows.contains(&serde_json::json!(["Super+Shift+F12", "Quit"])),
        "{rows:?}"
    );
    assert_eq!(
        state["shortcuts_view"]["sections"],
        serde_json::json!(["Windows", "Workspaces", "Mouse", "Pila"])
    );
}

#[test]
fn a_click_on_it_hides_it_and_reaches_no_window() {
    let (mut pila, client) = shown_over_a_window("");
    let state = pila.state();
    let (x, y, ..) = rect(&state["shortcuts_view"]["rect"]);
    pila.click_at((x + 20) as f64, (y + 20) as f64);
    pila.wait("hidden", |s| !shown(s));
    pila.settle();
    client.roundtrip();
    assert!(client.buttons().is_empty(), "{:?}", client.buttons());
}

#[test]
fn a_click_beside_it_hides_it_and_reaches_the_window() {
    let (mut pila, client) = shown_over_a_window("");
    // Left of the panel, below the title bar.
    pila.click_at(100.0, 400.0);
    pila.wait("hidden", |s| !shown(s));
    client.wait("the click", |s| {
        s.events.iter().any(|e| {
            matches!(e, common::client::Ev::Button { button, pressed: false, .. } if *button == BTN_LEFT)
        })
    });
}

#[test]
fn locking_hides_it_and_it_doesnt_show_while_locked() {
    let (mut pila, _client) = shown_over_a_window("");
    let locker = pila.client();
    let lock = locker.lock_session();
    locker.lock_surface(&lock, "HEADLESS-1", [255, 10, 20, 30]);
    locker.wait_logged("lock", "Locked");
    pila.wait("locked and hidden", |s| s["locked"] == true && !shown(s));
    pila.combo("Super+Shift+slash");
    pila.stays("not shown", std::time::Duration::from_millis(300), |s| {
        !shown(s)
    });
}

/// The panel has the windows' corners: by default the top two rounded, as
/// GTK's; all four with `rounded-corners = "all"`; square with no radius.
#[test]
fn the_panel_has_the_windows_corners() {
    for (config, top_cut, bottom_cut) in [
        ("corner-radius = 8\n", true, false),
        ("corner-radius = 8\nrounded-corners = \"all\"\n", true, true),
        ("", false, false),
    ] {
        let mut pila = TestPila::with_config(config);
        let desktop = pila.pixel(5.0, 5.0);
        pila.combo("Super+Shift+slash");
        let state = pila.wait("shown", shown);
        let (x, y, w, h) = rect(&state["shortcuts_view"]["rect"]);
        // The panel, drawn: its content came.
        pila.wait_pixel_is((x + w - 5) as f64, (y + 28) as f64, "drawn", |p| {
            p != desktop
        });
        // The desktop, dimmed under it.
        let desktop = pila.pixel(5.0, 5.0);
        // The panel's very corners, its 1 px border just outside.
        let top = pila.pixel((x - 1) as f64, (y - 1) as f64);
        let bottom = pila.pixel((x - 1) as f64, (y + h) as f64);
        assert_eq!(top == desktop, top_cut, "{config:?}: top corner {top:?}");
        assert_eq!(
            bottom == desktop,
            bottom_cut,
            "{config:?}: bottom corner {bottom:?}"
        );
    }
}

/// On a monitor smaller than the panel (1280x800 at scale 2: 640x400
/// logical), the cheatsheet fits inside it, shrunk as a whole (the welcome
/// shrinks the same way: `src/shortcuts_view.rs`' `fit` test).
#[test]
fn the_panels_fit_a_small_monitor() {
    let mut pila = TestPila::with_config("[[output]]\nname = \"HEADLESS-1\"\nscale = 2\n");
    pila.combo("Super+Shift+slash");
    let state = pila.wait("shown", shown);
    let (x, y, w, h) = rect(&state["shortcuts_view"]["rect"]);
    assert!(
        x >= 0 && y >= 0 && x + w <= 640 && y + h <= 400,
        "the cheatsheet at {x},{y} {w}x{h} on 640x400"
    );
    // Shrunk as a whole, not cropped: the right column's first keycap
    // (laid out at 448 + 3, 72 + 22 + 11 in its 860 wide layout) is where
    // the shrinking puts it, in the keycaps' fill ([colors] bar).
    let f = w as f64 / 860.0;
    let (cx, cy) = (x as f64 + 451.0 * f, y as f64 + 105.0 * f);
    pila.wait_pixel_is(cx, cy, "the right column's keycap", |p| {
        p == [0x2f, 0x31, 0x36, 255]
    });
    pila.key("Escape");
    pila.wait("hidden", |s| !shown(s));
}

/// A reload that changes the corners while the panel is shown reshapes it at
/// once.
#[test]
fn a_reload_reshapes_the_panel_shown() {
    let mut pila = TestPila::with_config("corner-radius = 8\n");
    pila.combo("Super+Shift+slash");
    let state = pila.wait("shown", shown);
    let (x, y, ..) = rect(&state["shortcuts_view"]["rect"]);
    let desktop = pila.pixel(5.0, 5.0);
    let corner = ((x - 1) as f64, (y - 1) as f64);
    pila.wait_pixel(corner.0, corner.1, desktop);
    // Square now: the border is at the very corner.
    pila.write_config("corner-radius = 0\n");
    pila.wait_log("config reloaded");
    pila.wait_pixel_is(corner.0, corner.1, "the square corner's border", |p| {
        p == [0x3f, 0x42, 0x49, 255]
    });
}

/// With no fonts (CI's system has none) there's no text to show: the welcome
/// isn't left open with nothing drawn, taking keys and clicks, and Mod+?
/// shows nothing; keys reach the window.
#[test]
fn without_fonts_no_panel_takes_the_keys() {
    let mut pila = TestPila::start_with(Options {
        default_config_path: true,
        env: vec![("PILA_HEADLESS_NO_FONTS".into(), "1".into())],
        ..Options::default()
    });
    pila.wait_log("created the config from the example");
    pila.wait("no panel", |s| !shown(s));
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    pila.combo("Super+Shift+slash");
    pila.stays("no panel", std::time::Duration::from_millis(300), |s| {
        !shown(s)
    });
    pila.settle();
    client.roundtrip();
    pila.key("a");
    client.wait("the key", |s| {
        s.events.iter().any(|e| {
            *e == common::client::Ev::Key {
                key: KEY_A,
                pressed: false,
            }
        })
    });
}

/// While it's shown every output is dimmed, the panel's and the others, as
/// under the share picker; hidden, they're as they were.
#[test]
fn it_dims_every_output_while_shown() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:800x600,B:800x600".into()),
        config: "[colors]\nbackground = \"#ffffff\"\n".into(),
        ..Options::default()
    });
    pila.move_to(400.0, 300.0);
    pila.settle();
    const WHITE: [u8; 4] = [255, 255, 255, 255];
    assert_eq!(pila.pixel(5.0, 5.0), WHITE);
    pila.combo("Super+Shift+slash");
    let state = pila.wait("shown", shown);
    assert_eq!(state["shortcuts_view"]["output"], "A");
    // White under 45 % black.
    let dimmed = |p: [u8; 4]| p[..3].iter().all(|&c| (138..=142).contains(&c));
    pila.wait_pixel_is(5.0, 5.0, "A dimmed", dimmed);
    let (bx, by) = (800.0 + 5.0, 5.0);
    pila.wait_pixel_is(bx, by, "B dimmed", dimmed);
    pila.key("Escape");
    pila.wait("hidden", |s| !shown(s));
    pila.wait_pixel(5.0, 5.0, WHITE);
    pila.wait_pixel(bx, by, WHITE);
}
