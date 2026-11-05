//! Key bindings: every default action, exact modifiers, locks ignored,
//! keys kept from the client, unbinding, the Mod setting, spawned commands'
//! environment.

mod common;

use common::client::states;
use common::*;

// Evdev key codes, as clients get them.
const KEY_Q: u32 = 16;
const KEY_J: u32 = 36;
const KEY_A: u32 = 30;
const KEY_5: u32 = 6;
const KEY_F5: u32 = 63;

const BAR: i64 = 28;

/// A pila with one focused 400x300 window "a"; returns its geometry.
fn one_window(
    config: &str,
) -> (
    TestPila,
    Client,
    common::client::Window,
    (i64, i64, i64, i64),
) {
    let mut pila = TestPila::with_config(config);
    let client = pila.client();
    let window = client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let geometry = rect(&pila.window("a")["geometry"]);
    (pila, client, window, geometry)
}

fn geometry_of(pila: &mut TestPila, app_id: &str) -> (i64, i64, i64, i64) {
    rect(&pila.window(app_id)["geometry"])
}

#[test]
fn mod_shift_q_closes_and_the_client_never_sees_q() {
    let (mut pila, client, window, _) = one_window("");
    pila.combo("Super+Shift+q");
    client.wait("close request", |s| s.windows[window.0].close_requested);
    client.roundtrip();
    assert!(
        !client.keys().iter().any(|(k, _)| *k == KEY_Q),
        "q reached the client: {:?}",
        client.keys()
    );
}

#[test]
fn modifiers_must_match_exactly() {
    let (mut pila, client, window, _) = one_window("");
    pila.combo("Super+Ctrl+Shift+q");
    // Not a binding: the key goes to the client.
    client.wait("q to reach the client", |s| {
        s.events.iter().any(|e| {
            *e == common::client::Ev::Key {
                key: KEY_Q,
                pressed: false,
            }
        })
    });
    assert!(!client.with_window(window, |w| w.close_requested));
}

#[test]
fn caps_lock_and_num_lock_dont_stop_bindings() {
    let (mut pila, client, window, _) = one_window("");
    pila.key("Caps_Lock");
    pila.key("Num_Lock");
    pila.combo("Super+Shift+q");
    client.wait("close request", |s| s.windows[window.0].close_requested);
}

#[test]
fn plain_keys_reach_the_focused_client() {
    let (mut pila, client, _, _) = one_window("");
    pila.key("a");
    client.wait("a to reach the client", |s| {
        s.events.iter().any(|e| {
            *e == common::client::Ev::Key {
                key: KEY_A,
                pressed: false,
            }
        })
    });
    assert_eq!(client.keys(), vec![(KEY_A, true), (KEY_A, false)]);
}

#[test]
fn a_binding_set_to_none_is_removed() {
    let (mut pila, client, _, _) = one_window("[bindings]\n\"Mod+J\" = \"none\"\n");
    pila.combo("Super+j");
    client.wait("j to reach the client", |s| {
        s.events.iter().any(|e| {
            *e == common::client::Ev::Key {
                key: KEY_J,
                pressed: false,
            }
        })
    });
}

#[test]
fn a_key_without_mod_can_be_bound() {
    let (mut pila, client, window, _) = one_window("[bindings]\n\"F5\" = \"close\"\n");
    pila.key("F5");
    client.wait("close request", |s| s.windows[window.0].close_requested);
    client.roundtrip();
    assert!(!client.keys().iter().any(|(k, _)| *k == KEY_F5));
}

#[test]
fn mod_can_be_alt() {
    let (mut pila, client, window, _) = one_window("mod = \"alt\"\n");
    pila.combo("Super+Shift+q");
    client.wait("q to reach the client", |s| {
        s.events.iter().any(|e| {
            *e == common::client::Ev::Key {
                key: KEY_Q,
                pressed: false,
            }
        })
    });
    assert!(!client.with_window(window, |w| w.close_requested));
    pila.combo("Alt+Shift+q");
    client.wait("close request", |s| s.windows[window.0].close_requested);
}

#[test]
fn pila_mod_overrides_the_config() {
    let mut pila = TestPila::start_with(Options {
        config: "mod = \"alt\"\n".into(),
        env: vec![("PILA_MOD".into(), "ctrl".into())],
        ..Options::default()
    });
    let client = pila.client();
    let window = client.window("a").create();
    pila.wait_focus("a");
    pila.combo("Ctrl+Shift+q");
    client.wait("close request", |s| s.windows[window.0].close_requested);
}

#[test]
fn mod_m_and_mod_up_maximize_and_toggle_back() {
    let (mut pila, client, window, before) = one_window("");
    pila.combo("Super+m");
    pila.wait("maximized", |s| {
        let w = window_in(s, "a").unwrap();
        w["maximized"] == true && w["geometry"]["w"] == 1280
    });
    assert_eq!(geometry_of(&mut pila, "a"), (0, BAR, 1280, 800 - BAR));
    client.wait_configure(window, "maximized", |c| c.has(states::MAXIMIZED));
    pila.combo("Super+m");
    pila.wait("restored", |s| {
        window_in(s, "a").unwrap()["geometry"]["w"] == 400
    });
    assert_eq!(geometry_of(&mut pila, "a"), before);
    pila.combo("Super+Up");
    pila.wait("maximized again", |s| {
        window_in(s, "a").unwrap()["geometry"]["w"] == 1280
    });
    pila.combo("Super+Down");
    pila.wait("restored again", |s| {
        window_in(s, "a").unwrap()["geometry"]["w"] == 400
    });
    assert_eq!(geometry_of(&mut pila, "a"), before);
}

#[test]
fn mod_f_toggles_fullscreen_over_the_whole_output() {
    let (mut pila, client, window, before) = one_window("");
    pila.combo("Super+f");
    pila.wait("fullscreen", |s| {
        let w = window_in(s, "a").unwrap();
        w["fullscreen"] == true && w["geometry"]["w"] == 1280
    });
    // No title bar: the whole output.
    assert_eq!(geometry_of(&mut pila, "a"), (0, 0, 1280, 800));
    assert_eq!(pila.window("a")["title_bar"], false);
    client.wait_configure(window, "fullscreen", |c| c.has(states::FULLSCREEN));
    pila.combo("Super+f");
    pila.wait("back", |s| {
        window_in(s, "a").unwrap()["geometry"]["w"] == 400
    });
    assert_eq!(geometry_of(&mut pila, "a"), before);
    assert_eq!(pila.window("a")["title_bar"], true);
}

#[test]
fn mod_n_minimizes() {
    let (mut pila, _client, _, _) = one_window("");
    pila.combo("Super+n");
    let state = pila.wait("minimized", |s| {
        window_in(s, "a").unwrap()["minimized"] == true
    });
    assert_eq!(window_in(&state, "a").unwrap()["mapped"], false);
    assert!(state["focused"].is_null());
}

#[test]
fn mod_left_and_right_snap_to_halves_and_again_restores() {
    let (mut pila, client, window, before) = one_window("");
    pila.combo("Super+Left");
    pila.wait("snapped left", |s| {
        let w = window_in(s, "a").unwrap();
        w["snapped"] == "left" && w["geometry"]["w"] == 640
    });
    assert_eq!(geometry_of(&mut pila, "a"), (0, BAR, 640, 800 - BAR));
    let c = client.wait_configure(window, "tiled", |c| c.has(states::TILED_LEFT));
    assert!(c.has(states::TILED_TOP) && c.has(states::TILED_BOTTOM));
    pila.combo("Super+Right");
    pila.wait("snapped right", |s| {
        let w = window_in(s, "a").unwrap();
        w["snapped"] == "right" && w["geometry"]["x"] == 640
    });
    assert_eq!(geometry_of(&mut pila, "a"), (640, BAR, 640, 800 - BAR));
    // The same side again restores the size from before the first snap.
    pila.combo("Super+Right");
    pila.wait("unsnapped", |s| {
        window_in(s, "a").unwrap()["snapped"].is_null()
    });
    pila.wait("its old size", |s| {
        window_in(s, "a").unwrap()["geometry"]["w"] == 400
    });
    assert_eq!(geometry_of(&mut pila, "a"), before);
    client.wait_configure(window, "untiled", |c| !c.has(states::TILED_RIGHT));
}

#[test]
fn mod_down_undoes_a_snap() {
    let (mut pila, _client, _, before) = one_window("");
    pila.combo("Super+Left");
    pila.wait("snapped", |s| {
        window_in(s, "a").unwrap()["geometry"]["w"] == 640
    });
    pila.combo("Super+Down");
    pila.wait("restored", |s| {
        window_in(s, "a").unwrap()["geometry"]["w"] == 400
    });
    assert_eq!(geometry_of(&mut pila, "a"), before);
}

#[test]
fn mod_shift_arrows_move_by_40() {
    let (mut pila, _client, _, (x, y, w, h)) = one_window("");
    for (keys, dx, dy) in [
        ("Super+Shift+Left", -40, 0),
        ("Super+Shift+Up", -40, -40),
        ("Super+Shift+Right", 0, -40),
        ("Super+Shift+Down", 0, 0),
    ] {
        pila.combo(keys);
        pila.wait(keys, |s| {
            rect(&window_in(s, "a").unwrap()["geometry"]) == (x + dx, y + dy, w, h)
        });
    }
}

#[test]
fn mod_ctrl_arrows_resize_by_40() {
    let (mut pila, _client, _, (x, y, w, h)) = one_window("");
    for (keys, dw, dh) in [
        ("Super+Ctrl+Right", 40, 0),
        ("Super+Ctrl+Down", 40, 40),
        ("Super+Ctrl+Left", 0, 40),
        ("Super+Ctrl+Up", 0, 0),
    ] {
        pila.combo(keys);
        pila.wait(keys, |s| {
            rect(&window_in(s, "a").unwrap()["geometry"]) == (x, y, w + dw, h + dh)
        });
    }
}

#[test]
fn keyboard_resizes_stop_at_40() {
    let (mut pila, _client, _, (x, y, _, _)) = one_window("");
    for _ in 0..12 {
        pila.combo("Super+Ctrl+Left");
    }
    pila.wait("40 wide", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == (x, y, 40, 300)
    });
}

#[test]
fn moving_or_resizing_a_filled_window_does_nothing() {
    let (mut pila, _client, _, _) = one_window("");
    pila.combo("Super+m");
    pila.wait("maximized", |s| {
        window_in(s, "a").unwrap()["geometry"]["w"] == 1280
    });
    pila.combo("Super+Shift+Left");
    pila.combo("Super+Ctrl+Left");
    pila.combo("Super+c");
    pila.stays(
        "still maximized",
        std::time::Duration::from_millis(200),
        |s| rect(&window_in(s, "a").unwrap()["geometry"]) == (0, BAR, 1280, 800 - BAR),
    );
}

#[test]
fn mod_c_centers() {
    let (mut pila, _client, _, before) = one_window("");
    pila.combo("Super+Shift+Left");
    pila.combo("Super+Shift+Up");
    pila.wait("moved", |s| {
        window_in(s, "a").unwrap()["geometry"]["x"] == before.0 - 40
    });
    pila.combo("Super+c");
    pila.wait("centered", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == before
    });
}

#[test]
fn mod_return_runs_the_terminal() {
    let mut pila = TestPila::with_config("terminal = \"echo terminal > $XDG_RUNTIME_DIR/out\"\n");
    pila.combo("Super+Return");
    assert_eq!(pila.wait_file("out"), "terminal\n");
}

#[test]
fn pila_terminal_overrides_the_config() {
    let mut pila = TestPila::start_with(Options {
        config: "terminal = \"echo config > $XDG_RUNTIME_DIR/out\"\n".into(),
        env: vec![(
            "PILA_TERMINAL".into(),
            "echo env > $XDG_RUNTIME_DIR/out".into(),
        )],
        ..Options::default()
    });
    pila.combo("Super+Return");
    assert_eq!(pila.wait_file("out"), "env\n");
}

#[test]
fn mod_d_runs_the_launcher_and_pila_launcher_overrides_it() {
    let mut pila = TestPila::with_config("launcher = \"echo launcher > $XDG_RUNTIME_DIR/out\"\n");
    pila.combo("Super+d");
    assert_eq!(pila.wait_file("out"), "launcher\n");

    let mut pila = TestPila::start_with(Options {
        config: "launcher = \"echo config > $XDG_RUNTIME_DIR/out\"\n".into(),
        env: vec![(
            "PILA_LAUNCHER".into(),
            "echo env > $XDG_RUNTIME_DIR/out".into(),
        )],
        ..Options::default()
    });
    pila.combo("Super+d");
    assert_eq!(pila.wait_file("out"), "env\n");
}

#[test]
fn mod_d_without_a_launcher_does_nothing() {
    let (mut pila, client, _, _) = one_window("");
    pila.combo("Super+d");
    client.roundtrip();
    let state = pila.settle();
    assert_eq!(state["children"], 0);
    assert_eq!(state["focused"], "a");
}

#[test]
fn spawned_commands_get_the_session_environment() {
    let mut pila = TestPila::start_with(Options {
        config: "[bindings]\n\"Mod+x\" = \"spawn env > $XDG_RUNTIME_DIR/env\"\n".into(),
        outputs: Some("A:1280x800,B:1280x800".into()),
        ..Options::default()
    });
    pila.move_to(1500.0, 100.0);
    pila.combo("Super+x");
    let env = pila.wait_file("env");
    let var = |name: &str| {
        env.lines()
            .find_map(|l| l.strip_prefix(&format!("{name}=")))
            .map(str::to_string)
    };
    assert_eq!(
        var("WAYLAND_DISPLAY").as_deref(),
        Some(pila.display.as_str())
    );
    assert_eq!(var("XDG_CURRENT_DESKTOP").as_deref(), Some("pila"));
    assert_eq!(var("XDG_SESSION_TYPE").as_deref(), Some("wayland"));
    assert_eq!(var("PILA_OUTPUT").as_deref(), Some("B"));
    // Xwayland's display, once it's up.
    let state = pila.state();
    if let Some(display) = state["xwayland"].as_u64() {
        assert_eq!(var("DISPLAY"), Some(format!(":{display}")));
    }
}

#[test]
fn digits_past_the_workspaces_reach_the_client() {
    let (mut pila, client, _, _) = one_window("workspaces = 4\n");
    pila.combo("Super+5");
    client.wait("5 to reach the client", |s| {
        s.events.iter().any(|e| {
            *e == common::client::Ev::Key {
                key: KEY_5,
                pressed: false,
            }
        })
    });
    assert_eq!(pila.state()["workspace"], 1);
}

#[test]
fn bindings_ignore_unfocused_state_without_a_window() {
    // No window: window actions do nothing, and pila keeps running.
    let mut pila = TestPila::start();
    for keys in [
        "Super+Shift+q",
        "Super+m",
        "Super+n",
        "Super+f",
        "Super+Left",
        "Super+Down",
        "Super+Shift+Left",
        "Super+Ctrl+Right",
        "Super+c",
        "Super+Tab",
    ] {
        pila.combo(keys);
    }
    pila.settle();
    assert!(pila.alive());
}
