//! Maximize and fullscreen: client requests, restore geometry, windows that
//! open filled, fullscreen on a chosen output, refits.

mod common;

use common::client::states;
use common::*;
use std::time::Duration;

const BAR: i64 = 28;

fn geometry(pila: &mut TestPila, app_id: &str) -> (i64, i64, i64, i64) {
    rect(&pila.window(app_id)["geometry"])
}

#[test]
fn a_client_can_maximize_and_unmaximize_itself() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let before = geometry(&mut pila, "a");
    client.request(a, |t| t.set_maximized());
    client.wait_configure(a, "maximized", |c| {
        c.has(states::MAXIMIZED) && c.size == (1280, 772)
    });
    pila.wait("maximized", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == (0, BAR, 1280, 800 - BAR)
    });
    client.request(a, |t| t.unset_maximized());
    client.wait_configure(a, "restored", |c| {
        !c.has(states::MAXIMIZED) && c.size == (400, 300)
    });
    pila.wait("restored", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == before
    });
}

#[test]
fn a_client_can_go_fullscreen_and_back() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let before = geometry(&mut pila, "a");
    client.request(a, |t| t.set_fullscreen(None));
    client.wait_configure(a, "fullscreen", |c| {
        c.has(states::FULLSCREEN) && c.size == (1280, 800)
    });
    pila.wait("fullscreen", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == (0, 0, 1280, 800)
    });
    client.request(a, |t| t.unset_fullscreen());
    pila.wait("back", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == before
    });
}

#[test]
fn fullscreen_over_maximized_comes_back_maximized() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let before = geometry(&mut pila, "a");
    pila.combo("Super+m");
    pila.wait("maximized", |s| {
        window_in(s, "a").unwrap()["geometry"]["w"] == 1280
    });
    pila.combo("Super+f");
    pila.wait("fullscreen", |s| {
        window_in(s, "a").unwrap()["geometry"]["h"] == 800
    });
    let c = client.last_configure(a);
    assert!(c.has(states::FULLSCREEN));
    pila.combo("Super+f");
    pila.wait("maximized again", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == (0, BAR, 1280, 800 - BAR)
    });
    pila.combo("Super+m");
    pila.wait("its own size", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == before
    });
}

#[test]
fn a_window_that_opens_maximized_is_restored_to_two_thirds_centered() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").size(400, 300).maximized().create();
    let w = pila.wait_window("a");
    assert_eq!(w["maximized"], true);
    // Its first configure is already maximized.
    assert!(client.configures(a)[0].has(states::MAXIMIZED));
    pila.wait("maximized size", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == (0, BAR, 1280, 800 - BAR)
    });
    pila.combo("Super+m");
    // 2/3 of 1280x800 is 853x533, centered with its bar:
    // ((1280 - 853) / 2, (800 - 533 - 28) / 2 + 28).
    pila.wait("two thirds", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == (213, 119 + BAR, 853, 533)
    });
}

#[test]
fn a_window_that_opens_fullscreen_fills_the_output() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").fullscreen().create();
    pila.wait_window("a");
    pila.wait("fullscreen", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == (0, 0, 1280, 800)
    });
}

#[test]
fn fullscreen_on_a_named_output() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1024x768".into()),
        ..Options::default()
    });
    let client = pila.client();
    let a = client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let output_b = client
        .lock()
        .outputs
        .iter()
        .find(|(_, name)| name == "B")
        .map(|(o, _)| o.clone())
        .expect("output B");
    client.request(a, |t| t.set_fullscreen(Some(&output_b)));
    let state = pila.wait("fullscreen on B", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == (1280, 0, 1024, 768)
    });
    assert_eq!(window_in(&state, "a").unwrap()["output"], "B");
}

#[test]
fn maximized_windows_refit_when_the_output_changes_mode() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").create();
    pila.wait_focus("a");
    pila.combo("Super+m");
    pila.wait("maximized", |s| {
        window_in(s, "a").unwrap()["geometry"]["w"] == 1280
    });
    pila.write_config("[[output]]\nname = \"HEADLESS-1\"\nmode = \"1024x768\"\n");
    client.wait_configure(a, "the new size", |c| c.size == (1024, 768 - 28));
    pila.wait("refit", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == (0, BAR, 1024, 768 - BAR)
    });
}

#[test]
fn a_maximized_window_keeps_its_restore_geometry_across_workspaces() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let before = geometry(&mut pila, "a");
    pila.combo("Super+m");
    pila.wait("maximized", |s| {
        window_in(s, "a").unwrap()["geometry"]["w"] == 1280
    });
    pila.combo("Super+Shift+2");
    pila.wait("on 2", |s| window_in(s, "a").unwrap()["workspace"] == 2);
    pila.combo("Super+2");
    pila.wait_focus("a");
    pila.combo("Super+m");
    pila.wait("restored", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == before
    });
}

#[test]
fn maximizing_a_snapped_window_restores_to_before_the_snap() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let before = geometry(&mut pila, "a");
    pila.combo("Super+Left");
    pila.wait("snapped", |s| {
        window_in(s, "a").unwrap()["geometry"]["w"] == 640
    });
    pila.combo("Super+m");
    let state = pila.wait("maximized", |s| {
        window_in(s, "a").unwrap()["geometry"]["w"] == 1280
    });
    assert!(window_in(&state, "a").unwrap()["snapped"].is_null());
    pila.combo("Super+m");
    pila.wait("restored", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == before
    });
}

#[test]
fn toplevel_bounds_are_the_usable_area() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").create();
    pila.wait_window("a");
    client.wait("bounds", |s| s.windows[a.0].bounds.is_some());
    let bounds = client.with_window(a, |w| w.bounds).unwrap();
    assert!(bounds.0 <= 1280 && bounds.1 <= 800, "{bounds:?}");
}

/// A click on a snapped or maximized window's title bar, without moving,
/// leaves it where it is: only a drag brings it back to its old size.
#[test]
fn a_click_on_the_title_bar_of_a_filled_window_doesnt_restore_it() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).create();
    pila.wait_focus("a");

    pila.combo("Super+Left");
    let snapped = pila.wait("snapped", |s| {
        let a = window_in(s, "a").unwrap();
        a["snapped"] == "left" && a["geometry"]["w"] == 640
    });
    let (x, y, w, h) = rect(&window_in(&snapped, "a").unwrap()["geometry"]);
    pila.click_at((x + w / 2) as f64, (y - BAR / 2) as f64);
    pila.stays(
        "snapped after a click on its title bar",
        Duration::from_millis(300),
        |s| {
            let a = window_in(s, "a").unwrap();
            a["snapped"] == "left" && rect(&a["geometry"]) == (x, y, w, h)
        },
    );

    pila.combo("Super+m");
    let maximized = pila.wait("maximized", |s| {
        let a = window_in(s, "a").unwrap();
        a["maximized"] == true && a["geometry"]["w"] == 1280
    });
    let (x, y, w, h) = rect(&window_in(&maximized, "a").unwrap()["geometry"]);
    pila.click_at((x + w / 2) as f64, (y - BAR / 2) as f64);
    pila.stays(
        "maximized after a click on its title bar",
        Duration::from_millis(300),
        |s| {
            let a = window_in(s, "a").unwrap();
            a["maximized"] == true && rect(&a["geometry"]) == (x, y, w, h)
        },
    );

    // Mod + a click anywhere in it: the same.
    pila.key_press("Super_L");
    pila.click_at((x + w / 2) as f64, (y + h / 2) as f64);
    pila.key_release("Super_L");
    pila.stays(
        "maximized after a Mod+click in it",
        Duration::from_millis(300),
        |s| {
            let a = window_in(s, "a").unwrap();
            a["maximized"] == true && rect(&a["geometry"]) == (x, y, w, h)
        },
    );
}
