//! Window and layer rules: matching (app id, title, tag, wildcards, all
//! matchers), merging, every window property, re-resolving on reload.

mod common;

use common::client::Decorations;
use common::*;
use wayland_protocols::xdg::toplevel_tag::v1::client::xdg_toplevel_tag_manager_v1::XdgToplevelTagManagerV1;

const BAR: i64 = 28;
/// pila's background where nothing is drawn.
const BACKGROUND: [u8; 4] = [31, 33, 38, 255];

fn open(pila: &mut TestPila, client: &Client, app_id: &str, title: &str) -> Value {
    client.window(app_id).title(title).size(400, 300).create();
    pila.wait_window(app_id)
}

#[test]
fn a_rule_sizes_and_places_a_window() {
    let mut pila = TestPila::with_config(
        "[[window-rule]]\napp-id = \"sized\"\nsize = [640, 400]\nposition = [50, 60]\n",
    );
    let client = pila.client();
    let a = client.window("sized").size(400, 300).create();
    let w = pila.wait_window("sized");
    // Its first configure carries the size.
    assert_eq!(client.configures(a)[0].size, (640, 400));
    // The position is the top-left of the title bar.
    assert_eq!(rect(&w["geometry"]), (50, 60 + BAR, 640, 400));
    pila.wait_log("placed by rule x=50 y=60");
}

#[test]
fn a_rule_opens_a_window_maximized_or_fullscreen() {
    let mut pila = TestPila::with_config(
        "[[window-rule]]\napp-id = \"max\"\nopen-maximized = true\n\n\
         [[window-rule]]\napp-id = \"full\"\nopen-fullscreen = true\n",
    );
    let client = pila.client();
    client.window("max").create();
    pila.wait("max maximized", |s| {
        window_in(s, "max").is_some_and(|w| {
            w["maximized"] == true && rect(&w["geometry"]) == (0, BAR, 1280, 800 - BAR)
        })
    });
    client.window("full").create();
    pila.wait("full fullscreen", |s| {
        window_in(s, "full")
            .is_some_and(|w| w["fullscreen"] == true && rect(&w["geometry"]) == (0, 0, 1280, 800))
    });
}

#[test]
fn a_rule_opens_a_window_on_another_workspace_unfocused() {
    let mut pila =
        TestPila::with_config("[[window-rule]]\napp-id = \"elsewhere\"\nopen-on-workspace = 3\n");
    let client = pila.client();
    client.window("here").create();
    pila.wait_focus("here");
    client.window("elsewhere").create();
    let state = pila.wait("elsewhere parked", |s| {
        window_in(s, "elsewhere").is_some_and(|w| w["workspace"] == 3)
    });
    assert_eq!(state["workspace"], 1);
    assert_eq!(state["focused"], "here");
    assert_eq!(window_in(&state, "elsewhere").unwrap()["mapped"], false);
    pila.combo("Super+3");
    pila.wait_focus("elsewhere");
}

#[test]
fn title_bar_false_draws_no_bar_on_a_server_side_window() {
    let mut pila = TestPila::with_config("[[window-rule]]\napp-id = \"bare\"\ntitle-bar = false\n");
    let client = pila.client();
    let a = client.window("bare").size(400, 300).create();
    let w = pila.wait_window("bare");
    assert_eq!(w["ssd"], true);
    assert_eq!(w["title_bar"], false);
    // Centered without room for a bar.
    assert_eq!(rect(&w["geometry"]), (440, 250, 400, 300));
    client.wait("decoration mode", |s| s.windows[a.0].server_side.is_some());
    assert_eq!(client.with_window(a, |w| w.server_side), Some(true));
}

#[test]
fn opacity_blends_the_window_with_whats_behind() {
    let mut pila = TestPila::with_config(
        "[[window-rule]]\napp-id = \"half\"\nopacity = 0.5\ntitle-bar = false\n",
    );
    let client = pila.client();
    client
        .window("half")
        .size(400, 300)
        .color([255, 255, 0, 0])
        .decorations(Decorations::Client)
        .create();
    let w = pila.wait_window("half");
    assert_eq!(w["opacity"], 0.5);
    pila.settle();
    let (x, y, ..) = rect(&w["geometry"]);
    let px = pila.pixel((x + 200) as f64, (y + 150) as f64);
    let expected = [
        (255 + BACKGROUND[0] as i32) / 2,
        BACKGROUND[1] as i32 / 2,
        BACKGROUND[2] as i32 / 2,
    ];
    for c in 0..3 {
        assert!(
            (px[c] as i32 - expected[c]).abs() <= 3,
            "{px:?} is not about {expected:?}"
        );
    }
}

#[test]
fn corner_radius_cuts_the_corners() {
    let mut pila = TestPila::with_config(
        "[[window-rule]]\napp-id = \"round\"\ncorner-radius = 24\ntitle-bar = false\n",
    );
    let client = pila.client();
    client
        .window("round")
        .size(400, 300)
        .color([255, 255, 0, 0])
        .decorations(Decorations::Client)
        .create();
    let w = pila.wait_window("round");
    assert_eq!(w["corner_radius"], 24);
    pila.settle();
    let (x, y, width, height) = rect(&w["geometry"]);
    for (cx, cy) in [
        (x + 1, y + 1),
        (x + width - 2, y + 1),
        (x + 1, y + height - 2),
        (x + width - 2, y + height - 2),
    ] {
        assert_eq!(
            pila.pixel(cx as f64, cy as f64),
            BACKGROUND,
            "corner at {cx},{cy}"
        );
    }
    // The middle of an edge is the window.
    assert_eq!(
        pila.pixel((x + 200) as f64, (y + 1) as f64),
        [255, 0, 0, 255]
    );
}

#[test]
fn the_global_corner_radius_applies_and_a_rule_overrides_it() {
    let mut pila = TestPila::with_config(
        "corner-radius = 20\n\n[[window-rule]]\napp-id = \"square\"\ncorner-radius = 0\n",
    );
    let client = pila.client();
    for app_id in ["round", "square"] {
        client
            .window(app_id)
            .size(400, 300)
            .color([255, 255, 0, 0])
            .decorations(Decorations::Client)
            .create();
        pila.wait_window(app_id);
    }
    pila.settle();
    let (x, y, ..) = rect(&pila.window("round")["geometry"]);
    let (sx, sy, ..) = rect(&pila.window("square")["geometry"]);
    // "square" is on top (cascaded), its corner is its own.
    assert_eq!(
        pila.pixel((sx + 1) as f64, (sy + 1) as f64),
        [255, 0, 0, 255]
    );
    assert_eq!(pila.pixel((x + 1) as f64, (y + 1) as f64), BACKGROUND);
}

#[test]
fn all_matchers_must_match_and_wildcards_work() {
    let mut pila = TestPila::with_config(
        "[[window-rule]]\napp-id = \"term*\"\ntitle = \"*htop*\"\nsize = [300, 200]\n",
    );
    let client = pila.client();
    let w = open(&mut pila, &client, "terminal", "running htop here");
    assert_eq!(rect(&w["geometry"]).2, 300);
    let w = open(&mut pila, &client, "terminal2", "a shell");
    assert_eq!(rect(&w["geometry"]).2, 400);
    let w = open(&mut pila, &client, "editor", "htop");
    assert_eq!(rect(&w["geometry"]).2, 400);
}

#[test]
fn a_rule_without_matchers_matches_everything() {
    let mut pila = TestPila::with_config("[[window-rule]]\nopacity = 0.8\n");
    let client = pila.client();
    let w = open(&mut pila, &client, "any", "thing");
    assert_eq!(w["opacity"], 0.8);
}

#[test]
fn later_rules_win_field_by_field() {
    let mut pila = TestPila::with_config(
        "[[window-rule]]\napp-id = \"a\"\nsize = [300, 200]\nopacity = 0.5\n\n\
         [[window-rule]]\napp-id = \"*\"\nopacity = 0.9\n",
    );
    let client = pila.client();
    let w = open(&mut pila, &client, "a", "a");
    assert_eq!(rect(&w["geometry"]).2, 300);
    assert_eq!(w["opacity"], 0.9);
}

#[test]
fn rules_are_resolved_again_on_reload_but_not_on_title_changes() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").title("one").size(400, 300).create();
    let w = pila.wait_window("a");
    assert_eq!(w["opacity"], Value::Null);
    pila.write_config("[[window-rule]]\napp-id = \"a\"\nopacity = 0.7\ntitle-bar = false\n");
    let state = pila.wait("the rule applied", |s| {
        window_in(s, "a").unwrap()["opacity"] == 0.7
    });
    assert_eq!(window_in(&state, "a").unwrap()["title_bar"], false);
    // A rule matching the title doesn't follow title changes.
    pila.write_config("[[window-rule]]\ntitle = \"two\"\nopacity = 0.3\n");
    pila.wait("the old rule gone", |s| {
        window_in(s, "a").unwrap()["opacity"].is_null()
    });
    client.request(a, |t| t.set_title("two".into()));
    client.surface(a).commit();
    client.flush();
    pila.wait("the new title", |s| {
        window_in(s, "a").unwrap()["title"] == "two"
    });
    pila.stays("no re-match", std::time::Duration::from_millis(100), |s| {
        window_in(s, "a").unwrap()["opacity"].is_null()
    });
}

#[test]
fn a_bar_hidden_by_reload_moves_a_maximized_window_up() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    pila.combo("Super+m");
    pila.wait("maximized", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == (0, BAR, 1280, 800 - BAR)
    });
    pila.write_config("[[window-rule]]\napp-id = \"a\"\ntitle-bar = false\n");
    pila.wait("refit without a bar", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == (0, 0, 1280, 800)
    });
}

#[test]
fn an_animation_rule_turns_one_kind_off() {
    let mut pila = TestPila::with_config(
        "[animations]\nenabled = true\n\n\
         [[window-rule]]\napp-id = \"quiet\"\nanimations = { open = false }\n",
    );
    let client = pila.client();
    let a = client.window("quiet").create();
    pila.wait_window("quiet");
    pila.wait_log("off by rule what=\"window-open\" id=\"quiet\"");
    client.destroy(a);
    pila.wait_log("starts what=\"window-close\" id=\"quiet\"");
    assert!(!pila
        .log()
        .contains("starts what=\"window-open\" id=\"quiet\""));
}

#[test]
fn invalid_rules_are_refused() {
    for config in [
        "[[window-rule]]\nopacity = 2.0\n",
        "[[window-rule]]\nopen-on-workspace = 0\n",
        "[[window-rule]]\nopen-on-workspace = 9\n",
        "[[window-rule]]\ncorner-radius = 100\n",
        "[[window-rule]]\nsize = [0, 100]\n",
        "[[window-rule]]\nfoo = 1\n",
        "[[layer-rule]]\nblur = \"yes\"\n",
    ] {
        let dir = std::env::temp_dir().join(format!("pila-rule-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, config).unwrap();
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_pila"))
            .args(["--check-config", "--config"])
            .arg(&path)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "accepted: {config}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn a_window_rule_can_only_set_shadow_on_or_off() {
    let dir = std::env::temp_dir().join(format!("pila-shadow-rule-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let check = |config: &str| {
        std::fs::write(&path, config).unwrap();
        std::process::Command::new(env!("CARGO_BIN_EXE_pila"))
            .args(["--check-config", "--config"])
            .arg(&path)
            .output()
            .unwrap()
            .status
            .success()
    };
    assert!(check("[[window-rule]]\nshadow = true\n"));
    assert!(check("[[window-rule]]\nshadow = false\n"));
    assert!(!check("[[window-rule]]\nshadow = 1\n"));
}

#[test]
fn a_window_rule_can_only_set_blur_on_or_off() {
    let dir = std::env::temp_dir().join(format!("pila-blur-rule-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let check = |config: &str| {
        std::fs::write(&path, config).unwrap();
        std::process::Command::new(env!("CARGO_BIN_EXE_pila"))
            .args(["--check-config", "--config"])
            .arg(&path)
            .output()
            .unwrap()
            .status
            .code()
    };
    assert_eq!(check("[[window-rule]]\nblur = true\n"), Some(0));
    assert_eq!(check("[[window-rule]]\nblur = false\n"), Some(0));
    assert_eq!(check("[[window-rule]]\nblur = \"yes\"\n"), Some(1));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn opacity_can_only_lower_what_a_window_draws() {
    // A clear window with `opacity = 1`: still clear, not filled in.
    let mut pila = TestPila::with_config(
        "[[window-rule]]\napp-id = \"clear\"\nopacity = 1.0\ntitle-bar = false\n",
    );
    let client = pila.client();
    client
        .window("clear")
        .size(400, 300)
        .color([0, 0, 0, 0])
        .decorations(Decorations::Client)
        .create();
    pila.wait_window("clear");
    let state = pila.settle();
    let (x, y, ..) = rect(&window_in(&state, "clear").unwrap()["geometry"]);
    assert_eq!(
        pila.pixel((x + 200) as f64, (y + 150) as f64),
        [31, 33, 38, 255]
    );
}

#[test]
fn a_rule_matches_the_tag_a_window_set() {
    let mut pila = TestPila::with_config(
        "[[window-rule]]\ntag = \"settings\"\nsize = [640, 400]\n\
         [[window-rule]]\ntag = \"pre*\"\nsize = [300, 200]\n",
    );
    let client = pila.client();
    let manager: XdgToplevelTagManagerV1 = client.bind("xdg_toplevel_tag_manager_v1", 1);
    client
        .window("tagged")
        .before_commit(|t| manager.set_toplevel_tag(t, "settings".to_string()))
        .create();
    let w = pila.wait_window("tagged");
    assert_eq!(w["tag"], "settings");
    assert_eq!(rect(&w["geometry"]).2, 640);
    client
        .window("wildcard")
        .before_commit(|t| manager.set_toplevel_tag(t, "preferences".to_string()))
        .create();
    assert_eq!(rect(&pila.wait_window("wildcard")["geometry"]).2, 300);
}

#[test]
fn a_tag_rule_leaves_a_window_without_that_tag_alone() {
    let mut pila =
        TestPila::with_config("[[window-rule]]\ntag = \"settings\"\nsize = [640, 400]\n");
    let client = pila.client();
    client.window("plain").size(400, 300).create();
    let w = pila.wait_window("plain");
    assert_eq!(w["tag"], "");
    assert_eq!(rect(&w["geometry"]).2, 400);
}
