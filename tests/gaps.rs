//! `gaps`: room around maximized windows, and around and between snapped
//! ones; fullscreen windows still take the whole output.

mod common;

use common::protocols::LayerSpec;
use common::*;
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::Layer, zwlr_layer_surface_v1::Anchor,
};

const BAR: i64 = 28;
/// Red as the test client draws it (ARGB) and as a pixel reads (RGBA).
const RED: [u8; 4] = [255, 255, 0, 0];
const RED_PIXEL: [u8; 4] = [255, 0, 0, 255];

fn geometry(state: &Value, app_id: &str) -> (i64, i64, i64, i64) {
    rect(&window_in(state, app_id).unwrap()["geometry"])
}

fn one_window(config: &str) -> (TestPila, Client) {
    let mut pila = TestPila::with_config(config);
    let client = pila.client();
    client.window("a").color(RED).create();
    pila.wait_focus("a");
    (pila, client)
}

#[test]
fn a_maximized_window_keeps_the_gap_from_the_edges() {
    let (mut pila, _client) = one_window("gaps = 10\n");
    pila.combo("Super+m");
    pila.wait("maximized inside the gap", |s| {
        geometry(s, "a") == (10, 10 + BAR, 1260, 780 - BAR)
    });
    // Fullscreen takes the whole output still.
    pila.combo("Super+f");
    pila.wait("fullscreen", |s| geometry(s, "a") == (0, 0, 1280, 800));
}

#[test]
fn snapped_windows_keep_the_gap_around_and_between_them() {
    let (mut pila, client) = one_window("gaps = 10\n");
    pila.combo("Super+Left");
    pila.wait("the left half", |s| {
        geometry(s, "a") == (10, 10 + BAR, 625, 780 - BAR)
    });
    client.window("b").color(RED).create();
    pila.wait_focus("b");
    pila.combo("Super+Right");
    pila.wait("the right half, a gap from the left one", |s| {
        geometry(s, "b") == (645, 10 + BAR, 625, 780 - BAR)
    });
    // A quarter: the top half of the right half.
    pila.combo("Super+Up");
    pila.wait("the top right quarter", |s| {
        geometry(s, "b") == (645, 10 + BAR, 625, 385 - BAR)
    });
    pila.combo("Super+Down");
    pila.combo("Super+Down");
    pila.wait("the bottom right quarter", |s| {
        geometry(s, "b") == (645, 405 + BAR, 625, 385 - BAR)
    });
}

#[test]
fn the_gap_is_kept_from_a_bar() {
    let mut pila = TestPila::with_config("gaps = 10\n");
    let client = pila.client();
    let (bar, _) = client.layer_surface(
        "bar",
        &LayerSpec {
            layer: Layer::Top,
            anchor: Anchor::Top | Anchor::Left | Anchor::Right,
            size: (0, 30),
            exclusive_zone: 30,
            ..LayerSpec::default()
        },
        [255, 0, 0, 255],
    );
    client.wait_surface_configured(bar);
    client.window("a").create();
    pila.wait_focus("a");
    pila.combo("Super+m");
    pila.wait("maximized below the bar, inside the gap", |s| {
        geometry(s, "a") == (10, 40 + BAR, 1260, 750 - BAR)
    });
}

#[test]
fn a_window_that_opens_maximized_keeps_the_gap() {
    let mut pila = TestPila::with_config(
        "gaps = 10\n[[window-rule]]\napp-id = \"a\"\nopen-maximized = true\n",
    );
    let client = pila.client();
    client.window("a").create();
    pila.wait("open maximized inside the gap", |s| {
        window_in(s, "a").is_some_and(|w| rect(&w["geometry"]) == (10, 10 + BAR, 1260, 780 - BAR))
    });
}

#[test]
fn a_saved_gap_refits_maximized_and_snapped_windows() {
    let (mut pila, client) = one_window("");
    pila.combo("Super+m");
    pila.wait("maximized", |s| {
        geometry(s, "a") == (0, BAR, 1280, 800 - BAR)
    });
    client.window("b").create();
    pila.wait_focus("b");
    pila.combo("Super+Left");
    pila.wait("snapped", |s| geometry(s, "b") == (0, BAR, 640, 800 - BAR));
    pila.write_config("gaps = 20\n");
    pila.wait("both refit to the gap", |s| {
        geometry(s, "a") == (20, 20 + BAR, 1240, 760 - BAR)
            && geometry(s, "b") == (20, 20 + BAR, 610, 760 - BAR)
    });
    let state = pila.state();
    assert_eq!(stacking(&state), ["a", "b"], "the refit raised nothing");
}

/// With a gap a maximized window doesn't touch the edges: it keeps its shadow
/// and rounded corners, as a floating window does.
#[test]
fn with_a_gap_a_maximized_window_keeps_its_shadow_and_corners() {
    let config = "gaps = 20\ncorner-radius = 16\nrounded-corners = \"all\"\n\
                  [shadow]\nsize = 24\ncolor = \"#000000ff\"\ninactive-color = \"#00000080\"\n\
                  [colors]\nbackground = \"#ffffff\"\n\
                  [[window-rule]]\napp-id = \"a\"\ntitle-bar = false\n";
    let (mut pila, _client) = one_window(config);
    pila.combo("Super+m");
    pila.wait("maximized inside the gap", |s| {
        geometry(s, "a") == (20, 20, 1240, 760)
    });
    pila.settle();
    assert_eq!(pila.pixel(640.0, 400.0), RED_PIXEL);
    let [shadow, ..] = pila.pixel(640.0, 785.0);
    assert!(shadow < 200, "a shadow in the gap below it: {shadow}");
    assert_ne!(pila.pixel(21.0, 21.0), RED_PIXEL, "its corner is rounded");
}

/// The shadow of a maximized window with a gap narrower than it reaches the
/// next output: that output is redrawn as it comes and goes (the tests'
/// redraw check fails an output that changes unasked).
#[test]
fn a_maximized_windows_shadow_on_the_next_output_is_redrawn() {
    let mut pila = TestPila::start_with(Options {
        config: "gaps = 4\n\
                 [shadow]\nsize = 24\ncolor = \"#000000ff\"\ninactive-color = \"#000000ff\"\n\
                 [colors]\nbackground = \"#ffffff\"\n\
                 [[window-rule]]\napp-id = \"a\"\ntitle-bar = false\n"
            .into(),
        outputs: Some("A:1280x800,B:1280x800".into()),
        ..Options::default()
    });
    let client = pila.client();
    let a = client.window("a").color(RED).create();
    pila.wait_focus("a");
    pila.combo("Super+m");
    pila.wait("maximized inside the gap", |s| {
        geometry(s, "a") == (4, 4, 1272, 792)
    });
    pila.settle();
    let [shadow, ..] = pila.pixel(1281.0, 400.0);
    assert!(shadow < 200, "its shadow on B: {shadow}");
    // Gone, its shadow goes from B too.
    client.set_auto_draw(a, false);
    client.unmap(a);
    pila.wait("unmapped", |s| window_in(s, "a").is_none());
    pila.settle();
    assert_eq!(
        pila.pixel(1281.0, 400.0),
        [255, 255, 255, 255],
        "gone from B"
    );
}

/// Without a gap nothing changes: square, no shadow.
#[test]
fn without_a_gap_a_maximized_window_is_square() {
    let config = "corner-radius = 16\nrounded-corners = \"all\"\n\
                  [[window-rule]]\napp-id = \"a\"\ntitle-bar = false\n";
    let (mut pila, _client) = one_window(config);
    pila.combo("Super+m");
    pila.wait("maximized", |s| geometry(s, "a") == (0, 0, 1280, 800));
    pila.settle();
    assert_eq!(pila.pixel(1.0, 1.0), RED_PIXEL, "a square corner");
}
