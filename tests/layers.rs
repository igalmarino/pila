//! Layer shell: placement and sizing, exclusive zones and what they do to
//! windows, stacking with windows, keyboard interactivity, outputs.

mod common;

use common::protocols::{LayerSpec, Surf};
use common::*;
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::Layer,
    zwlr_layer_surface_v1::{Anchor, KeyboardInteractivity},
};

const BAR: i64 = 28;
const RED: [u8; 4] = [255, 255, 0, 0];
const GREEN: [u8; 4] = [255, 0, 255, 0];
const BLUE: [u8; 4] = [255, 0, 0, 255];
const YELLOW: [u8; 4] = [255, 255, 255, 0];

fn rgba(argb: [u8; 4]) -> [u8; 4] {
    [argb[1], argb[2], argb[3], argb[0]]
}

fn layer_in<'a>(state: &'a Value, namespace: &str) -> Option<&'a Value> {
    state["layers"]
        .as_array()?
        .iter()
        .find(|l| l["namespace"] == namespace)
}

fn top_bar(height: u32) -> LayerSpec {
    LayerSpec {
        layer: Layer::Top,
        anchor: Anchor::Top | Anchor::Left | Anchor::Right,
        size: (0, height),
        exclusive_zone: height as i32,
        ..LayerSpec::default()
    }
}

fn wait_layer(pila: &mut TestPila, namespace: &str) -> Value {
    // Its size comes with its first buffer.
    let state = pila.wait(&format!("layer {namespace}"), |s| {
        layer_in(s, namespace).is_some_and(|l| l["geometry"]["w"].as_i64().unwrap_or(0) > 0)
    });
    layer_in(&state, namespace).unwrap().clone()
}

#[test]
fn a_bar_is_stretched_and_takes_its_exclusive_zone() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let (bar, _) = client.layer_surface("bar", &top_bar(30), BLUE);
    assert_eq!(client.wait_surface_configured(bar), (0, 0, 1280, 30));
    let l = wait_layer(&mut pila, "bar");
    assert_eq!(rect(&l["geometry"]), (0, 0, 1280, 30));
    assert_eq!(l["layer"], "top");
    assert_eq!(l["exclusive_zone"], 30);
    let state = pila.wait("the usable area", |s| {
        rect(&s["outputs"][0]["usable"]) == (0, 30, 1280, 770)
    });
    assert_eq!(state["outputs"][0]["name"], "HEADLESS-1");
}

#[test]
fn windows_open_and_maximize_inside_the_usable_area() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.layer_surface("bar", &top_bar(30), BLUE);
    pila.wait("the usable area", |s| {
        rect(&s["outputs"][0]["usable"]).1 == 30
    });
    client.window("a").size(400, 300).create();
    let w = pila.wait_window("a");
    // 30 + (770 - 300 - 28) / 2 + 28.
    assert_eq!(rect(&w["geometry"]), (440, 30 + 221 + BAR, 400, 300));
    pila.combo("Super+m");
    pila.wait("maximized below the bar", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == (0, 30 + BAR, 1280, 770 - BAR)
    });
    // Fullscreen takes the whole output anyway.
    pila.combo("Super+f");
    pila.wait("fullscreen", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == (0, 0, 1280, 800)
    });
}

#[test]
fn maximized_windows_refit_when_the_zone_changes_or_goes() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let (bar, layer) = client.layer_surface("bar", &top_bar(30), BLUE);
    client.wait_surface_configured(bar);
    client.window("a").create();
    pila.wait_focus("a");
    pila.combo("Super+m");
    pila.wait("maximized", |s| {
        window_in(s, "a").unwrap()["geometry"]["y"] == 30 + BAR
    });
    layer.set_size(0, 60);
    layer.set_exclusive_zone(60);
    client.wl_surface_of(bar).commit();
    client.flush();
    pila.wait("refit to the taller bar", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == (0, 60 + BAR, 1280, 740 - BAR)
    });
    layer.destroy();
    client.wl_surface_of(bar).destroy();
    client.flush();
    pila.wait("refit to the whole output", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == (0, BAR, 1280, 800 - BAR)
    });
}

#[test]
fn layers_stack_around_windows() {
    let mut pila = TestPila::start();
    let client = pila.client();
    // Bottom: the whole output; Top and Overlay: overlapping squares.
    let everywhere = Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right;
    let (bottom, _) = client.layer_surface(
        "bottom",
        &LayerSpec {
            layer: Layer::Bottom,
            anchor: everywhere,
            size: (0, 0),
            ..LayerSpec::default()
        },
        GREEN,
    );
    client.wait_surface_configured(bottom);
    client.window("a").size(400, 300).color(RED).create();
    let w = pila.wait_window("a");
    let (x, y, ..) = rect(&w["geometry"]);
    let square = |layer| LayerSpec {
        layer,
        anchor: Anchor::Top | Anchor::Left,
        size: (100, 100),
        margin: (y as i32, 0, 0, x as i32),
        ..LayerSpec::default()
    };
    let (top, _) = client.layer_surface("top", &square(Layer::Top), BLUE);
    client.wait_surface_configured(top);
    let mut overlay_spec = square(Layer::Overlay);
    overlay_spec.margin.0 += 50;
    overlay_spec.margin.3 += 50;
    let (overlay, _) = client.layer_surface("overlay", &overlay_spec, YELLOW);
    client.wait_surface_configured(overlay);
    pila.settle();
    // Bottom shows around the window; the window over Bottom; Top over the
    // window; Overlay over Top.
    assert_eq!(pila.pixel(5.0, 795.0), rgba(GREEN));
    assert_eq!(pila.pixel((x + 300) as f64, (y + 200) as f64), rgba(RED));
    assert_eq!(pila.pixel((x + 10) as f64, (y + 10) as f64), rgba(BLUE));
    assert_eq!(pila.pixel((x + 90) as f64, (y + 90) as f64), rgba(YELLOW));
    // A fullscreen window covers Top but not Overlay.
    pila.combo("Super+f");
    pila.wait("fullscreen", |s| {
        window_in(s, "a").unwrap()["geometry"]["h"] == 800
    });
    pila.settle();
    assert_eq!(pila.pixel((x + 10) as f64, (y + 10) as f64), rgba(RED));
    assert_eq!(pila.pixel((x + 90) as f64, (y + 90) as f64), rgba(YELLOW));
}

#[test]
fn a_fullscreen_window_only_covers_the_top_layer_of_its_own_output() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1280x800".into()),
        ..Options::default()
    });
    let client = pila.client();
    pila.move_to(1500.0, 300.0);
    client.window("b").size(400, 300).color(RED).create();
    let w = pila.wait_window("b");
    assert_eq!(w["output"], "B");
    let (x, y, ..) = rect(&w["geometry"]);
    let (panel, _) = client.layer_surface(
        "panel",
        &LayerSpec {
            layer: Layer::Top,
            output: Some("B".into()),
            anchor: Anchor::Top | Anchor::Left,
            size: (100, 100),
            margin: (y as i32, 0, 0, x as i32 - 1280),
            ..LayerSpec::default()
        },
        BLUE,
    );
    client.wait_surface_configured(panel);
    // Once the client has drawn it, after that configure.
    pila.wait_pixel((x + 10) as f64, (y + 10) as f64, rgba(BLUE));

    pila.move_to(300.0, 300.0);
    client.window("a").size(400, 300).color(GREEN).create();
    pila.wait_focus("a");
    pila.combo("Super+f");
    pila.wait("fullscreen", |s| {
        window_in(s, "a").unwrap()["geometry"]["h"] == 800
    });
    pila.settle();
    assert_eq!(pila.pixel((x + 10) as f64, (y + 10) as f64), rgba(BLUE));
}

#[test]
fn an_exclusive_keyboard_layer_keeps_the_keyboard() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let (launcher, layer) = client.layer_surface(
        "launcher",
        &LayerSpec {
            layer: Layer::Overlay,
            keyboard: KeyboardInteractivity::Exclusive,
            ..LayerSpec::default()
        },
        BLUE,
    );
    client.wait_surface_configured(launcher);
    pila.wait("the launcher has the keyboard", |s| {
        s["keyboard_focus"] == "layer:launcher"
    });
    client.wait("the client sees it", |s| {
        s.keyboard_name.as_deref() == Some("launcher")
    });
    // A new window is raised but doesn't take the keyboard.
    client.window("b").create();
    let state = pila.wait_window("b");
    let _ = state;
    let state = pila.settle();
    assert_eq!(state["keyboard_focus"], "layer:launcher");
    assert_eq!(stacking(&state), vec!["a", "b"]);
    // Keys go to the launcher.
    pila.key("a");
    client.wait("the key", |s| {
        s.events.iter().any(|e| {
            *e == common::client::Ev::Key {
                key: 30,
                pressed: false,
            }
        })
    });
    // Gone: the keyboard goes back to a window.
    layer.destroy();
    client.wl_surface_of(launcher).destroy();
    client.flush();
    pila.wait("a window has the keyboard", |s| s["focused"].is_string());
}

#[test]
fn an_on_demand_layer_gets_the_keyboard_when_clicked() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let (panel, _) = client.layer_surface(
        "panel",
        &LayerSpec {
            layer: Layer::Top,
            anchor: Anchor::Bottom,
            size: (300, 40),
            keyboard: KeyboardInteractivity::OnDemand,
            ..LayerSpec::default()
        },
        BLUE,
    );
    client.wait_surface_configured(panel);
    let l = wait_layer(&mut pila, "panel");
    assert_eq!(rect(&l["geometry"]), (490, 760, 300, 40));
    // It took the keyboard when it showed up; the window takes it back.
    pila.wait("the panel has the keyboard", |s| {
        s["keyboard_focus"] == "layer:panel"
    });
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    pila.click_at((x + 10) as f64, (y + 10) as f64);
    pila.wait_focus("a");
    pila.click_at(600.0, 780.0);
    pila.wait("the panel has the keyboard", |s| {
        s["keyboard_focus"] == "layer:panel"
    });
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    pila.click_at((x + 10) as f64, (y + 10) as f64);
    pila.wait_focus("a");
}

#[test]
fn clicking_a_background_layer_clears_focus() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let (wallpaper, _) = client.layer_surface(
        "wallpaper",
        &LayerSpec {
            layer: Layer::Background,
            anchor: Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right,
            size: (0, 0),
            ..LayerSpec::default()
        },
        GREEN,
    );
    client.wait_surface_configured(wallpaper);
    client.window("a").create();
    pila.wait_focus("a");
    pila.click_at(5.0, 795.0);
    pila.wait("no focus", |s| s["focused"].is_null());
}

#[test]
fn a_layer_goes_on_the_output_it_asks_for() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1024x768".into()),
        ..Options::default()
    });
    let client = pila.client();
    let mut spec = top_bar(25);
    spec.output = Some("B".into());
    let (bar, _) = client.layer_surface("bar", &spec, BLUE);
    assert_eq!(client.wait_surface_configured(bar), (0, 0, 1024, 25));
    let l = wait_layer(&mut pila, "bar");
    assert_eq!(l["output"], "B");
    assert_eq!(rect(&l["geometry"]), (1280, 0, 1024, 25));
    let state = pila.settle();
    assert_eq!(rect(&state["outputs"][0]["usable"]), (0, 0, 1280, 800));
    assert_eq!(rect(&state["outputs"][1]["usable"]), (1280, 25, 1024, 743));
}

/// A layer surface that names no output goes on the one under the pointer,
/// and enters it: Noctalia maps such a surface to learn which monitor is the
/// current one, and opens its panels there.
#[test]
fn a_layer_without_an_output_goes_under_the_pointer() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1024x768".into()),
        ..Options::default()
    });
    let client = pila.client();
    pila.move_to(1500.5, 300.25);
    let (probe, _) = client.layer_surface("probe", &LayerSpec::default(), BLUE);
    client.wait_surface_configured(probe);
    assert_eq!(wait_layer(&mut pila, "probe")["output"], "B");
    client.wait("the probe entered B", |s| {
        s.surfaces
            .iter()
            .any(|d| d.name == "probe" && d.outputs == ["B"])
    });

    pila.move_to(100.5, 300.25);
    let (second, _) = client.layer_surface("second", &LayerSpec::default(), BLUE);
    client.wait_surface_configured(second);
    assert_eq!(wait_layer(&mut pila, "second")["output"], "A");
    client.wait("the second probe entered A", |s| {
        s.surfaces
            .iter()
            .any(|d| d.name == "second" && d.outputs == ["A"])
    });
}

/// A layer surface can change layers (layer-shell v2's set_layer): Noctalia
/// moves its bar between top and overlay.
#[test]
fn a_layer_surface_moves_to_another_layer() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let (bar, layer) = client.layer_surface("bar", &top_bar(25), BLUE);
    client.wait_surface_configured(bar);
    assert_eq!(wait_layer(&mut pila, "bar")["layer"], "top");
    layer.set_layer(Layer::Overlay);
    client.surface_data(bar, |d| d.surface.commit());
    client.flush();
    pila.wait("the bar on the overlay layer", |s| {
        layer_in(s, "bar").is_some_and(|l| l["layer"] == "overlay")
    });
    layer.set_layer(Layer::Top);
    client.surface_data(bar, |d| d.surface.commit());
    client.flush();
    pila.wait("the bar back on top", |s| {
        layer_in(s, "bar").is_some_and(|l| l["layer"] == "top")
    });
}

#[test]
fn margins_and_anchors_place_a_layer() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let (corner, _) = client.layer_surface(
        "corner",
        &LayerSpec {
            anchor: Anchor::Bottom | Anchor::Right,
            size: (100, 50),
            margin: (0, 20, 10, 0),
            ..LayerSpec::default()
        },
        BLUE,
    );
    client.wait_surface_configured(corner);
    let l = wait_layer(&mut pila, "corner");
    assert_eq!(
        rect(&l["geometry"]),
        (1280 - 20 - 100, 800 - 10 - 50, 100, 50)
    );
}

#[test]
fn a_layer_rule_matches_by_namespace() {
    let mut pila = TestPila::with_config(
        "[animations]\nenabled = true\n\n[[layer-rule]]\nnamespace = \"quiet-*\"\nanimations = false\n",
    );
    let client = pila.client();
    let (quiet, _) = client.layer_surface("quiet-launcher", &LayerSpec::default(), BLUE);
    client.wait_surface_configured(quiet);
    pila.wait_log("off by rule what=\"layer-in\" id=\"quiet-launcher\"");
    let (loud, _) = client.layer_surface("launcher", &LayerSpec::default(), BLUE);
    client.wait_surface_configured(loud);
    pila.wait_log("starts what=\"layer-in\" id=\"launcher\"");
}

#[test]
fn layer_surfaces_on_a_removed_output_are_closed() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1024x768".into()),
        ..Options::default()
    });
    let client = pila.client();
    let mut spec = top_bar(25);
    spec.output = Some("B".into());
    let (bar, _): (Surf, _) = client.layer_surface("bar", &spec, BLUE);
    client.wait_surface_configured(bar);
    wait_layer(&mut pila, "bar");
    pila.request(json!({ "cmd": "output_remove", "name": "B" }));
    client.wait_surface_closed(bar);
    pila.wait("no layer", |s| layer_in(s, "bar").is_none());
}

/// Qt hides a layer shell window by destroying its role object, then
/// committing the bare surface with no buffer (DMS's launcher does it each
/// time it closes): that must not cost the client its connection.
#[test]
fn a_surface_committed_after_its_layer_surface_is_destroyed_keeps_the_client() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let spec = LayerSpec {
        layer: Layer::Overlay,
        anchor: Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right,
        keyboard: KeyboardInteractivity::Exclusive,
        exclusive_zone: -1,
        ..LayerSpec::default()
    };
    let (launcher, layer) = client.layer_surface("launcher", &spec, BLUE);
    client.wait_surface_configured(launcher);
    wait_layer(&mut pila, "launcher");
    let surface = client.wl_surface_of(launcher);
    layer.destroy();
    surface.attach(None, 0, 0);
    surface.commit();
    client.roundtrip();
    pila.wait("no layer", |s| layer_in(s, "launcher").is_none());
    assert_eq!(client.error(), None);
}

/// A layer surface hidden with a null commit goes back to its state right
/// after creation; the configure that maps it again answers the client's new
/// initial commit, not the hiding one (wlr-layer-shell; what niri does).
#[test]
fn a_layer_surface_hidden_and_shown_again_maps_again() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let spec = top_bar(30);
    let (bar, layer) = client.layer_surface("bar", &spec, BLUE);
    client.wait_surface_configured(bar);
    wait_layer(&mut pila, "bar");
    let surface = client.wl_surface_of(bar);
    client.surface_data(bar, |s| s.auto_draw = false);
    surface.attach(None, 0, 0);
    surface.commit();
    client.roundtrip();
    client.roundtrip();
    let configures = client.surface_data(bar, |s| s.configures.len());
    assert_eq!(configures, 1, "a configure answered the hiding commit");

    // Shown again: its state anew, then a new initial commit.
    client.surface_data(bar, |s| s.auto_draw = true);
    layer.set_size(spec.size.0, spec.size.1);
    layer.set_anchor(spec.anchor);
    layer.set_exclusive_zone(spec.exclusive_zone);
    surface.commit();
    client.flush();
    client.wait("the configure answering it", |s| {
        s.surfaces[bar.0].configures.len() == 2
    });
    wait_layer(&mut pila, "bar");
    assert_eq!(client.error(), None);
}

/// A Top or Overlay surface that takes the keyboard on demand gets it when it
/// shows up (a launcher, a welcome dialog), as on niri and COSMIC, and gives it
/// back when it goes; one below the windows doesn't take it.
#[test]
fn an_on_demand_layer_gets_the_keyboard_when_it_maps() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let on_demand = |layer| LayerSpec {
        layer,
        anchor: Anchor::Bottom,
        size: (300, 40),
        keyboard: KeyboardInteractivity::OnDemand,
        ..LayerSpec::default()
    };
    let (dock, _) = client.layer_surface("dock", &on_demand(Layer::Bottom), GREEN);
    client.wait_surface_configured(dock);
    wait_layer(&mut pila, "dock");
    pila.stays(
        "the window keeps the keyboard",
        std::time::Duration::from_millis(100),
        |s| s["keyboard_focus"] == "a",
    );
    let (launcher, layer) = client.layer_surface("launcher", &on_demand(Layer::Overlay), BLUE);
    client.wait_surface_configured(launcher);
    pila.wait("the launcher has the keyboard", |s| {
        s["keyboard_focus"] == "layer:launcher"
    });
    layer.destroy();
    client.wl_surface_of(launcher).destroy();
    client.flush();
    pila.wait("the window has it back", |s| s["keyboard_focus"] == "a");
}
