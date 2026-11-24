//! The overview (Mod+W): the workspace's windows spread out, picked with a
//! click or the arrows and Enter.

mod common;

use common::*;
use std::time::Duration;

/// Windows `names` opened in order (the last one focused).
fn windows(pila: &mut TestPila, names: &[&str]) -> (Client, Vec<common::client::Window>) {
    let client = pila.client();
    let mut handles = Vec::new();
    for name in names {
        handles.push(client.window(name).create());
        pila.wait_focus(name);
    }
    (client, handles)
}

/// `pila msg <args>` as a program in this session would run it.
fn msg(pila: &TestPila, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_pila"))
        .arg("msg")
        .args(args)
        .env_remove("PILA_SOCKET")
        .env("WAYLAND_DISPLAY", &pila.display)
        .env("XDG_RUNTIME_DIR", &pila.dir)
        .output()
        .expect("run pila msg")
}

/// `pila msg action <words>`, which must succeed.
#[track_caller]
fn action(pila: &TestPila, words: &[&str]) {
    let mut all = vec!["action"];
    all.extend_from_slice(words);
    let out = msg(pila, &all);
    assert!(out.status.success(), "{words:?}: {:?}", out);
}

fn open(pila: &mut TestPila) -> Value {
    pila.combo("Super+w");
    pila.wait("the overview", |s| s["overview"]["open"] == true)
}

/// The place of `app_id` in the overview: (x, y, w, h).
fn slot(state: &Value, app_id: &str) -> (i64, i64, i64, i64) {
    let slot = slots(state)
        .into_iter()
        .find(|s| s["app_id"] == app_id)
        .unwrap_or_else(|| panic!("no place for {app_id}: {}", state["overview"]));
    rect(&slot["rect"])
}

fn slots(state: &Value) -> Vec<Value> {
    state["overview"]["slots"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn center((x, y, w, h): (i64, i64, i64, i64)) -> (f64, f64) {
    ((x + w / 2) as f64, (y + h / 2) as f64)
}

/// The app got no key but Super (a modifier reaches it, as ever).
fn no_key_but_super(client: &Client) -> bool {
    const KEY_LEFTMETA: u32 = 125;
    client.keys().iter().all(|(key, _)| *key == KEY_LEFTMETA)
}

fn overlap(a: (i64, i64, i64, i64), b: (i64, i64, i64, i64)) -> bool {
    a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
}

#[test]
fn mod_w_spreads_the_windows_out_and_escape_closes_it() {
    let mut pila = TestPila::start();
    let (client, _) = windows(&mut pila, &["a", "b", "c"]);
    let state = open(&mut pila);
    let output = rect(&state["outputs"][0]["geometry"]);
    let places: Vec<_> = ["a", "b", "c"].iter().map(|a| slot(&state, a)).collect();
    for (i, place) in places.iter().enumerate() {
        assert!(
            place.0 >= output.0
                && place.1 >= output.1
                && place.0 + place.2 <= output.0 + output.2
                && place.1 + place.3 <= output.1 + output.3,
            "{place:?} out of {output:?}"
        );
        for other in &places[i + 1..] {
            assert!(!overlap(*place, *other), "{place:?} over {other:?}");
        }
    }
    assert_eq!(state["overview"]["selected"], "c", "the focused one");
    assert_eq!(state["overview"]["shown"], 1.0, "at once, animations off");
    // No client gets the pointer while it's open.
    let (x, y) = center(slot(&state, "a"));
    pila.move_to(x, y);
    pila.wait("no pointer focus", |s| s["pointer"]["focus"].is_null());
    pila.key("Escape");
    let state = pila.wait("closed", |s| s["overview"]["open"] == false);
    assert_eq!(state["focused"], "c", "the focus as it was");
    client.roundtrip();
    assert!(
        client.keys().iter().all(|k| k.0 != 1),
        "Escape reached no app: {:?}",
        client.keys()
    );
}

#[test]
fn a_click_on_a_window_picks_it_and_reaches_no_app() {
    let mut pila = TestPila::start();
    let (client, _) = windows(&mut pila, &["a", "b", "c"]);
    let state = open(&mut pila);
    let (x, y) = center(slot(&state, "a"));
    pila.move_to(x, y);
    pila.wait("a selected under the pointer", |s| {
        s["overview"]["selected"] == "a"
    });
    client.roundtrip();
    let buttons = client.buttons().len();
    pila.click("left");
    let state = pila.wait("closed on a", |s| {
        s["overview"]["open"] == false && s["focused"] == "a"
    });
    assert_eq!(stacking(&state).last().unwrap(), "a", "raised");
    client.roundtrip();
    assert_eq!(client.buttons().len(), buttons, "{:?}", client.buttons());
}

#[test]
fn a_click_beside_the_windows_closes_it() {
    let mut pila = TestPila::start();
    let (client, _) = windows(&mut pila, &["a", "b"]);
    open(&mut pila);
    pila.move_to(2.0, 400.0);
    client.roundtrip();
    let buttons = client.buttons().len();
    pila.click("left");
    let state = pila.wait("closed", |s| s["overview"]["open"] == false);
    assert_eq!(state["focused"], "b");
    client.roundtrip();
    assert_eq!(client.buttons().len(), buttons);
}

#[test]
fn arrows_and_enter_pick_a_window() {
    let mut pila = TestPila::start();
    let (client, _) = windows(&mut pila, &["a", "b", "c"]);
    let state = open(&mut pila);
    // From c, the arrow toward a's place.
    let (cx, _) = center(slot(&state, "c"));
    let (ax, _) = center(slot(&state, "a"));
    let toward_a = if ax < cx { "Left" } else { "Right" };
    // Each step leaves c; the one that reaches a is the last.
    for _ in 0..3 {
        if pila.state()["overview"]["selected"] == "a" {
            break;
        }
        pila.key(toward_a);
    }
    pila.wait("a selected", |s| s["overview"]["selected"] == "a");
    pila.key("Return");
    pila.wait("closed on a", |s| {
        s["overview"]["open"] == false && s["focused"] == "a"
    });
    client.roundtrip();
    assert!(no_key_but_super(&client), "{:?}", client.keys());
}

/// Each arrow picks a window that way, or stays where there's none.
#[test]
fn each_arrow_picks_a_window_its_way() {
    let mut pila = TestPila::start();
    let (_client, _) = windows(&mut pila, &["a", "b", "c", "d"]);
    let state = open(&mut pila);
    let centers: Vec<(String, (f64, f64))> = slots(&state)
        .iter()
        .map(|s| {
            let app = s["app_id"].as_str().unwrap().to_string();
            (app, center(rect(&s["rect"])))
        })
        .collect();
    let at = |app: &str| centers.iter().find(|(a, _)| a == app).unwrap().1;
    // Its way: further along the axis by more than a few pixels.
    let ahead = |from: (f64, f64), to: (f64, f64), key: &str| match key {
        "Right" => to.0 > from.0 + 4.0,
        "Left" => to.0 < from.0 - 4.0,
        "Down" => to.1 > from.1 + 4.0,
        _ => to.1 < from.1 - 4.0,
    };
    let mut moved = [false; 4];
    // Twice round, so each arrow is tried from more than one window.
    for _ in 0..2 {
        for (i, key) in ["Right", "Down", "Left", "Up"].into_iter().enumerate() {
            let state = pila.state();
            let from = state["overview"]["selected"].as_str().unwrap().to_string();
            let any_that_way = centers.iter().any(|(_, c)| ahead(at(&from), *c, key));
            pila.key(key);
            let state = pila.settle();
            let to = state["overview"]["selected"].as_str().unwrap().to_string();
            if any_that_way {
                assert!(
                    ahead(at(&from), at(&to), key),
                    "{key} from {from} picked {to}: {centers:?}"
                );
                moved[i] = true;
            } else {
                assert_eq!(to, from, "{key} from {from}, with nothing that way");
            }
        }
    }
    assert!(
        moved.iter().all(|m| *m),
        "every arrow moved once: {moved:?}"
    );
}

#[test]
fn a_key_that_isnt_a_binding_reaches_no_window_and_another_binding_closes_it_and_runs() {
    let mut pila = TestPila::start();
    let (client, _) = windows(&mut pila, &["a", "b"]);
    open(&mut pila);
    pila.key("x");
    pila.settle();
    client.roundtrip();
    assert!(no_key_but_super(&client), "{:?}", client.keys());
    assert_eq!(pila.state()["overview"]["open"], true);
    // Mod+M maximizes the focused window, once the overview is closed.
    pila.combo("Super+m");
    let state = pila.wait("closed and b maximized", |s| {
        s["overview"]["open"] == false && window_in(s, "b").unwrap()["maximized"] == true
    });
    assert_eq!(state["focused"], "b");
}

#[test]
fn mod_w_again_closes_it_and_pila_msg_toggles_it() {
    let mut pila = TestPila::start();
    let _client = windows(&mut pila, &["a"]);
    open(&mut pila);
    pila.combo("Super+w");
    pila.wait("closed", |s| s["overview"]["open"] == false);
    action(&pila, &["overview", "open"]);
    pila.wait("open", |s| s["overview"]["open"] == true);
    action(&pila, &["overview"]);
    pila.wait("closed", |s| s["overview"]["open"] == false);
}

#[test]
fn each_output_shows_its_own_windows() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1024x768".into()),
        ..Options::default()
    });
    let (_client, _) = windows(&mut pila, &["a", "b"]);
    // b to the second output.
    pila.combo("Super+Shift+o");
    pila.wait("b on B", |s| window_in(s, "b").unwrap()["output"] == "B");
    let state = open(&mut pila);
    let output_of = |app: &str| {
        slots(&state)
            .into_iter()
            .find(|s| s["app_id"] == app)
            .unwrap()["output"]
            .clone()
    };
    assert_eq!(output_of("a"), "A");
    assert_eq!(output_of("b"), "B");
    let b = slot(&state, "b");
    assert!(b.0 >= 1280, "on B: {b:?}");
}

#[test]
fn a_minimized_window_has_a_place_and_picking_it_brings_it_back() {
    let mut pila = TestPila::start();
    let _client = windows(&mut pila, &["a", "b"]);
    pila.combo("Super+n");
    pila.wait("b minimized", |s| {
        window_in(s, "b").unwrap()["minimized"] == true
    });
    let state = open(&mut pila);
    let (x, y) = center(slot(&state, "b"));
    pila.click_at(x, y);
    let state = pila.wait("b back and focused", |s| {
        s["overview"]["open"] == false && s["focused"] == "b"
    });
    assert_eq!(window_in(&state, "b").unwrap()["minimized"], false);
}

#[test]
fn a_window_opening_or_closing_meanwhile_takes_or_leaves_its_place() {
    let mut pila = TestPila::start();
    let (client, handles) = windows(&mut pila, &["a", "b"]);
    open(&mut pila);
    client.window("c").create();
    let state = pila.wait("c's place", |s| slots(s).iter().any(|s| s["app_id"] == "c"));
    assert_eq!(state["overview"]["open"], true);
    client.destroy(handles[0]);
    let state = pila.wait("a's place gone", |s| {
        !slots(s).iter().any(|s| s["app_id"] == "a")
    });
    let places: Vec<_> = slots(&state).iter().map(|s| rect(&s["rect"])).collect();
    assert_eq!(places.len(), 2);
    assert!(!overlap(places[0], places[1]));
}

#[test]
fn the_windows_are_live_in_it() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").color([255, 200, 30, 30]).create();
    pila.wait_focus("a");
    let state = open(&mut pila);
    let (x, y) = center(slot(&state, "a"));
    pila.wait_pixel_is(x, y, "a's picture", |px| px[0] > 150 && px[2] < 80);
    client.with_window(a, |w| w.color = [255, 30, 30, 200]);
    client.draw(a, None);
    pila.wait_pixel_is(x, y, "a's new picture", |px| px[2] > 150 && px[0] < 80);
}

#[test]
fn going_to_another_workspace_shows_its_windows_in_it() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    pila.combo("Super+2");
    pila.wait("workspace 2", |s| s["workspace"] == 2);
    client.window("b").create();
    pila.wait_focus("b");
    open(&mut pila);
    pila.combo("Super+1");
    let state = pila.wait("a's place on workspace 1", |s| {
        s["workspace"] == 1 && slots(s).iter().any(|s| s["app_id"] == "a")
    });
    assert_eq!(state["overview"]["open"], true);
    assert!(!slots(&state).iter().any(|s| s["app_id"] == "b"));
}

#[test]
fn locking_closes_it_and_it_doesnt_open_while_locked() {
    let mut pila = TestPila::start();
    let _client = windows(&mut pila, &["a"]);
    open(&mut pila);
    let locker = pila.client();
    let lock = locker.lock_session();
    locker.lock_surface(&lock, "HEADLESS-1", [255, 0, 0, 0]);
    pila.wait("locked, overview closed", |s| {
        s["locked"] == true && s["overview"]["open"] == false
    });
    // `pila msg` refuses it while locked.
    let out = msg(&pila, &["action", "overview", "open"]);
    assert!(!out.status.success());
    pila.combo("Super+w");
    pila.stays("closed while locked", Duration::from_millis(300), |s| {
        s["overview"]["open"] == false
    });
}

#[test]
fn with_animations_the_windows_glide_to_their_places_and_back() {
    let mut pila = TestPila::with_config("[animations]\noverview = 1000\n");
    let (client, _) = windows(&mut pila, &["a", "b"]);
    pila.combo("Super+w");
    let state = pila.wait("on its way", |s| {
        s["overview"]["open"] == true && s["overview"]["shown"].as_f64().unwrap() < 1.0
    });
    assert!(
        state["animations"]["running"] == true,
        "{}",
        state["animations"]
    );
    pila.wait("open", |s| s["overview"]["shown"] == 1.0);
    pila.key("Escape");
    // Closed to input at once, gliding back meanwhile.
    let state = pila.wait("on its way back", |s| {
        s["overview"]["open"] == false && s["overview"]["shown"].as_f64().unwrap() > 0.0
    });
    assert_eq!(state["focused"], "b");
    let (x, y, w, h) = rect(&window_in(&state, "b").unwrap()["geometry"]);
    pila.wait("closed", |s| s["overview"]["shown"] == 0.0);
    // The windows are back where they were, and get clicks again.
    pila.click_at((x + w / 2) as f64, (y + h / 2) as f64);
    pila.settle();
    client.roundtrip();
    assert!(!client.buttons().is_empty());
}

#[test]
fn opening_it_again_on_its_way_back_turns_it_around() {
    let mut pila = TestPila::with_config("[animations]\noverview = 1000\n");
    let _client = windows(&mut pila, &["a"]);
    open(&mut pila);
    pila.wait("open", |s| s["overview"]["shown"] == 1.0);
    pila.key("Escape");
    pila.wait("on its way back", |s| {
        s["overview"]["open"] == false && s["overview"]["shown"].as_f64().unwrap() < 1.0
    });
    pila.combo("Super+w");
    let state = pila.wait("open again", |s| s["overview"]["open"] == true);
    assert!(
        state["overview"]["shown"].as_f64().unwrap() > 0.0,
        "from where it was"
    );
    pila.wait("all the way", |s| s["overview"]["shown"] == 1.0);
}

fn strip(state: &Value) -> Vec<Value> {
    state["overview"]["strip"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The accent color, `[colors] snap-preview` made opaque.
const ACCENT: [u8; 4] = [68, 119, 204, 255];

fn near(a: [u8; 4], b: [u8; 4]) -> bool {
    (0..3).all(|i| (a[i] as i32 - b[i] as i32).abs() <= 3)
}

#[test]
fn the_strip_has_a_thumbnail_per_workspace_above_the_windows_the_active_one_marked() {
    let mut pila = TestPila::start();
    let _client = windows(&mut pila, &["a", "b"]);
    let state = open(&mut pila);
    let thumbs = strip(&state);
    assert_eq!(thumbs.len() as u64, state["workspaces"].as_u64().unwrap());
    let active: Vec<_> = thumbs.iter().filter(|t| t["active"] == true).collect();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0]["workspace"], 1);
    let lowest = thumbs.iter().map(|t| {
        let (_, y, _, h) = rect(&t["rect"]);
        y + h
    });
    let lowest = lowest.max().unwrap();
    for app in ["a", "b"] {
        assert!(slot(&state, app).1 > lowest, "{app} under the strip");
    }
    // The active one's border is in the accent color.
    let (x, y, _, h) = rect(&active[0]["rect"]);
    pila.wait_pixel_is(
        (x - 2) as f64,
        (y + h / 2) as f64,
        "the accent border",
        |px| near(px, ACCENT),
    );
}

#[test]
fn one_workspace_has_no_strip() {
    let mut pila = TestPila::with_config("workspaces = 1\n");
    let _client = windows(&mut pila, &["a"]);
    let state = open(&mut pila);
    assert!(strip(&state).is_empty(), "{}", state["overview"]);
}

#[test]
fn a_click_on_a_thumbnail_goes_to_that_workspace_and_stays_open() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    pila.combo("Super+2");
    pila.wait("workspace 2", |s| s["workspace"] == 2);
    client.window("b").create();
    pila.wait_focus("b");
    pila.combo("Super+1");
    pila.wait_focus("a");
    let state = open(&mut pila);
    let second = strip(&state)
        .into_iter()
        .find(|t| t["workspace"] == 2)
        .unwrap();
    let (x, y) = center(rect(&second["rect"]));
    client.roundtrip();
    let buttons = client.buttons().len();
    pila.click_at(x, y);
    let state = pila.wait("on workspace 2, still open", |s| {
        s["workspace"] == 2 && slots(s).iter().any(|s| s["app_id"] == "b")
    });
    assert_eq!(state["overview"]["open"], true);
    assert!(!slots(&state).iter().any(|s| s["app_id"] == "a"));
    let active: Vec<_> = strip(&state)
        .into_iter()
        .filter(|t| t["active"] == true)
        .collect();
    assert_eq!(active[0]["workspace"], 2);
    client.roundtrip();
    assert_eq!(client.buttons().len(), buttons);
}

#[test]
fn a_thumbnail_shows_its_workspaces_windows_live() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    pila.combo("Super+2");
    pila.wait("workspace 2", |s| s["workspace"] == 2);
    let b = client.window("b").color([255, 200, 30, 30]).create();
    let state = pila.wait_focus("b");
    let (bx, by, bw, bh) = rect(&window_in(&state, "b").unwrap()["geometry"]);
    pila.combo("Super+1");
    pila.wait_focus("a");
    let state = open(&mut pila);
    let output = rect(&state["outputs"][0]["geometry"]);
    let second = strip(&state)
        .into_iter()
        .find(|t| t["workspace"] == 2)
        .unwrap();
    let (tx, ty, tw, _) = rect(&second["rect"]);
    let fit = tw as f64 / output.2 as f64;
    let (x, y) = (
        tx as f64 + (bx - output.0 + bw / 2) as f64 * fit,
        ty as f64 + (by - output.1 + bh / 2) as f64 * fit,
    );
    pila.wait_pixel_is(x, y, "b in workspace 2's thumbnail", |px| {
        px[0] > 150 && px[2] < 80
    });
    // A new picture on another workspace shows there too.
    client.with_window(b, |w| w.color = [255, 30, 30, 200]);
    client.draw(b, None);
    pila.wait_pixel_is(x, y, "b's new picture", |px| px[2] > 150 && px[0] < 80);
}

/// Drag what's at `from` to `to` with the left button, in steps.
fn drag(pila: &mut TestPila, from: (f64, f64), to: (f64, f64)) {
    pila.move_to(from.0, from.1);
    pila.press("left");
    for step in 1..=5 {
        let t = step as f64 / 5.0;
        pila.move_to(from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
    }
}

fn thumb_center(state: &Value, workspace: u64) -> (f64, f64) {
    let thumb = strip(state)
        .into_iter()
        .find(|t| t["workspace"] == workspace)
        .unwrap();
    center(rect(&thumb["rect"]))
}

#[test]
fn dragging_a_window_onto_a_thumbnail_moves_it_to_that_workspace() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").color([255, 200, 30, 30]).create();
    pila.wait_focus("a");
    client.window("b").create();
    pila.wait_focus("b");
    let state = open(&mut pila);
    let (to_x, to_y) = thumb_center(&state, 3);
    client.roundtrip();
    let buttons = client.buttons().len();
    drag(&mut pila, center(slot(&state, "a")), (to_x, to_y));
    let state = pila.wait("a dragged", |s| s["overview"]["dragging"] == "a");
    // Shrunk to aim at a thumbnail (at most 240 across), under the pointer.
    let (dx, dy, dw, dh) = rect(&state["overview"]["dragged"]);
    assert!(dw <= 240 && dh <= 240, "{dw}x{dh}");
    assert!(
        (dx as f64) <= to_x
            && to_x <= (dx + dw) as f64
            && (dy as f64) <= to_y
            && to_y <= (dy + dh) as f64,
        "{:?} not under the pointer",
        (dx, dy, dw, dh)
    );
    // Drawn under the pointer, over the strip.
    pila.wait_pixel_is(to_x, to_y, "a under the pointer", |px| {
        px[0] > 150 && px[2] < 80
    });
    assert_eq!(state["workspace"], 1);
    pila.release("left");
    let state = pila.wait("a on workspace 3", |s| {
        window_in(s, "a").unwrap()["workspace"] == 3
    });
    assert_eq!(state["overview"]["open"], true, "still open");
    assert_eq!(state["workspace"], 1);
    assert!(!slots(&state).iter().any(|s| s["app_id"] == "a"));
    assert_eq!(state["focused"], "b");
    client.roundtrip();
    assert_eq!(client.buttons().len(), buttons, "{:?}", client.buttons());
}

#[test]
fn a_window_dropped_anywhere_else_goes_back_to_its_place() {
    let mut pila = TestPila::start();
    let _client = windows(&mut pila, &["a", "b"]);
    let state = open(&mut pila);
    let place = slot(&state, "a");
    let (x, y) = center(place);
    drag(&mut pila, (x, y), (x + 60.0, y + 40.0));
    pila.wait("a dragged", |s| s["overview"]["dragging"] == "a");
    pila.release("left");
    let state = pila.wait("dropped", |s| s["overview"]["dragging"].is_null());
    assert_eq!(state["overview"]["open"], true, "not picked: still open");
    assert_eq!(slot(&state, "a"), place);
    assert_eq!(window_in(&state, "a").unwrap()["workspace"], 1);
    assert_eq!(state["focused"], "b");
}

#[test]
fn a_minimized_window_dragged_to_a_workspace_stays_minimized_there() {
    let mut pila = TestPila::start();
    let _client = windows(&mut pila, &["a", "b"]);
    pila.combo("Super+n");
    pila.wait("b minimized", |s| {
        window_in(s, "b").unwrap()["minimized"] == true
    });
    let state = open(&mut pila);
    let to = thumb_center(&state, 2);
    drag(&mut pila, center(slot(&state, "b")), to);
    pila.wait("b dragged", |s| s["overview"]["dragging"] == "b");
    pila.release("left");
    let state = pila.wait("b on workspace 2", |s| {
        window_in(s, "b").unwrap()["workspace"] == 2
    });
    assert_eq!(window_in(&state, "b").unwrap()["minimized"], true);
    // Picked there later, it comes back on workspace 2.
    pila.key("Escape");
    pila.combo("Super+2");
    let state = pila.wait("workspace 2", |s| s["workspace"] == 2);
    assert_eq!(window_in(&state, "b").unwrap()["minimized"], true);
}

#[test]
fn the_hot_corner_opens_it_and_again_closes_it_once_each_time() {
    let mut pila = TestPila::start();
    let _client = windows(&mut pila, &["a"]);
    pila.move_to(100.0, 100.0);
    pila.move_to(0.0, 0.0);
    pila.wait("opened", |s| s["overview"]["open"] == true);
    // Still in the corner: nothing more.
    pila.move_to(0.5, 0.5);
    pila.stays("still open", Duration::from_millis(200), |s| {
        s["overview"]["open"] == true
    });
    pila.move_to(60.0, 60.0);
    pila.move_to(0.0, 0.0);
    pila.wait("closed", |s| s["overview"]["open"] == false);
}

#[test]
fn the_hot_corner_can_be_turned_off() {
    let mut pila = TestPila::with_config("[overview]\nhot-corner = false\n");
    let _client = windows(&mut pila, &["a"]);
    pila.move_to(100.0, 100.0);
    pila.move_to(0.0, 0.0);
    pila.stays("closed", Duration::from_millis(300), |s| {
        s["overview"]["open"] == false
    });
}

#[test]
fn the_hot_corner_does_nothing_over_a_fullscreen_window_or_with_a_button_held() {
    let mut pila = TestPila::start();
    let _client = windows(&mut pila, &["a"]);
    pila.combo("Super+f");
    pila.wait("fullscreen", |s| {
        window_in(s, "a").unwrap()["fullscreen"] == true
    });
    pila.move_to(300.0, 300.0);
    pila.move_to(0.0, 0.0);
    pila.stays(
        "closed: a fullscreen window",
        Duration::from_millis(300),
        |s| s["overview"]["open"] == false,
    );
    pila.combo("Super+f");
    let state = pila.wait("back", |s| {
        window_in(s, "a").unwrap()["fullscreen"] == false
    });
    let (x, y) = center(rect(&window_in(&state, "a").unwrap()["geometry"]));
    pila.move_to(x, y);
    pila.press("left");
    pila.move_to(0.0, 0.0);
    pila.release("left");
    pila.stays(
        "closed: a button was held",
        Duration::from_millis(300),
        |s| s["overview"]["open"] == false,
    );
}

#[test]
fn only_a_corner_of_the_whole_layout_is_hot() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1024x768".into()),
        ..Options::default()
    });
    let _client = windows(&mut pila, &["a"]);
    // B's top-left touches A: the pointer goes through it.
    pila.move_to(1300.0, 100.0);
    pila.move_to(1280.0, 0.0);
    pila.stays("closed", Duration::from_millis(300), |s| {
        s["overview"]["open"] == false
    });
    pila.move_to(0.0, 0.0);
    pila.wait("A's corner opens it", |s| s["overview"]["open"] == true);
}

#[test]
fn the_hot_corner_can_be_another_corner() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1024x768".into()),
        config: "[overview]\nhot-corner-position = \"top-right\"\n".into(),
        ..Options::default()
    });
    let _client = windows(&mut pila, &["a"]);
    pila.move_to(100.0, 100.0);
    pila.move_to(0.0, 0.0);
    pila.stays("closed: top-left", Duration::from_millis(300), |s| {
        s["overview"]["open"] == false
    });
    // A's top-right touches B: the pointer goes through it.
    pila.move_to(1200.0, 100.0);
    pila.move_to(1279.0, 0.0);
    pila.stays("closed: A's top-right", Duration::from_millis(300), |s| {
        s["overview"]["open"] == false
    });
    pila.move_to(2000.0, 100.0);
    pila.move_to(2303.0, 0.0);
    pila.wait("B's top-right opens it", |s| s["overview"]["open"] == true);
}

#[test]
fn with_pressure_the_pointer_must_be_pushed_past_the_corner() {
    let mut pila = TestPila::with_config("[overview]\nhot-corner-pressure = 100\n");
    let _client = windows(&mut pila, &["a"]);
    pila.move_to(100.0, 100.0);
    pila.move_to(0.0, 0.0);
    pila.stays("closed: reached", Duration::from_millis(200), |s| {
        s["overview"]["open"] == false
    });
    pila.move_by(-30.0, -30.0);
    pila.stays("closed: pushed 60", Duration::from_millis(200), |s| {
        s["overview"]["open"] == false
    });
    pila.move_by(-30.0, -10.0);
    pila.wait("pushed 100", |s| s["overview"]["open"] == true);
    pila.key("Escape");
    pila.wait("closed", |s| s["overview"]["open"] == false);
    // Leaving starts over.
    pila.move_to(60.0, 60.0);
    pila.move_to(0.0, 0.0);
    pila.move_by(-40.0, -40.0);
    pila.move_to(60.0, 60.0);
    pila.move_to(0.0, 0.0);
    pila.move_by(-40.0, -40.0);
    pila.stays(
        "closed: pushed 80 each time",
        Duration::from_millis(200),
        |s| s["overview"]["open"] == false,
    );
}

#[test]
fn with_a_delay_the_pointer_must_stay_in_the_corner() {
    let mut pila = TestPila::with_config("[overview]\nhot-corner-delay = 400\n");
    let _client = windows(&mut pila, &["a"]);
    pila.move_to(100.0, 100.0);
    // Through the corner and out again before the delay: nothing.
    pila.move_to(0.0, 0.0);
    pila.move_to(60.0, 60.0);
    pila.stays("closed: left", Duration::from_millis(700), |s| {
        s["overview"]["open"] == false
    });
    pila.move_to(0.0, 0.0);
    pila.stays("closed: not yet", Duration::from_millis(150), |s| {
        s["overview"]["open"] == false
    });
    pila.wait("opened after the delay", |s| s["overview"]["open"] == true);
    // Once per visit.
    pila.stays("still open", Duration::from_millis(600), |s| {
        s["overview"]["open"] == true
    });
}

fn swipe(pila: &mut TestPila, fingers: u32, steps: &[(f64, f64)]) {
    pila.gesture(json!({ "kind": "swipe", "phase": "begin", "fingers": fingers }));
    for (dx, dy) in steps {
        pila.gesture(json!({ "kind": "swipe", "phase": "update", "dx": dx, "dy": dy }));
    }
    pila.gesture(json!({ "kind": "swipe", "phase": "end" }));
}

#[test]
fn a_four_finger_swipe_up_opens_it_and_down_closes_it() {
    let mut pila = TestPila::start();
    let _client = windows(&mut pila, &["a"]);
    swipe(&mut pila, 4, &[(0.0, -40.0), (0.0, -40.0)]);
    pila.wait("opened", |s| s["overview"]["open"] == true);
    // Up again: it stays open.
    swipe(&mut pila, 4, &[(0.0, -40.0), (0.0, -40.0)]);
    pila.stays("still open", Duration::from_millis(200), |s| {
        s["overview"]["open"] == true
    });
    swipe(&mut pila, 4, &[(0.0, 40.0), (0.0, 40.0)]);
    pila.wait("closed", |s| s["overview"]["open"] == false);
}

/// A window held in it (pressed, or dragged) that closes meanwhile is let
/// go: nothing is dragged, and the release picks nothing and moves nothing.
#[test]
fn a_window_closing_while_held_in_it_is_let_go() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").create();
    pila.wait_focus("a");
    client.window("b").create();
    pila.wait_focus("b");
    let state = open(&mut pila);
    let (to_x, to_y) = thumb_center(&state, 3);
    drag(&mut pila, center(slot(&state, "a")), (to_x, to_y));
    pila.wait("a dragged", |s| s["overview"]["dragging"] == "a");
    client.destroy(a);
    client.flush();
    let state = pila.wait("a gone and let go", |s| {
        window_in(s, "a").is_none() && s["overview"]["dragging"].is_null()
    });
    assert_eq!(state["overview"]["open"], true);
    pila.release("left");
    pila.settle();
    let state = pila.state();
    assert_eq!(state["overview"]["open"], true, "still open");
    assert_eq!(state["workspace"], 1);
    assert!(pila.alive());
}
