//! What's drawn, by pixels: title bars (focused or not, buttons on hover),
//! rounded corners with a bar and when maximized, the bell flash, the snap
//! preview, blur behind layer surfaces.

mod common;

use common::client::Decorations;
use common::protocols::{LayerSpec, Surf};
use common::*;
use wayland_client::protocol::{wl_compositor::WlCompositor, wl_surface::WlSurface};
use wayland_protocols::ext::background_effect::v1::client::{
    ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1,
    ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1,
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::Layer, zwlr_layer_surface_v1::Anchor,
};

const BAR: i64 = 28;
const BACKGROUND: [u8; 4] = [31, 33, 38, 255];
const BAR_FOCUSED: [u8; 4] = [47, 49, 54, 255];
const BAR_UNFOCUSED: [u8; 4] = [38, 39, 43, 255];
const RED: [u8; 4] = [255, 255, 0, 0];

fn near(a: [u8; 4], b: [u8; 4], tolerance: i32) -> bool {
    (0..3).all(|i| (a[i] as i32 - b[i] as i32).abs() <= tolerance)
}

#[test]
fn the_title_bar_shows_focus() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    pila.settle();
    let px = pila.pixel((x + 4) as f64, (y - BAR / 2) as f64);
    assert!(near(px, BAR_FOCUSED, 2), "{px:?}");
    pila.click_at(5.0, 795.0);
    pila.wait("unfocused", |s| s["focused"].is_null());
    pila.settle();
    let px = pila.pixel((x + 4) as f64, (y - BAR / 2) as f64);
    assert!(near(px, BAR_UNFOCUSED, 2), "{px:?}");
}

#[test]
fn a_client_side_decorated_window_has_nothing_above_it() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client
        .window("a")
        .size(400, 300)
        .decorations(Decorations::Client)
        .create();
    pila.wait_focus("a");
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    pila.settle();
    assert_eq!(pila.pixel((x + 4) as f64, (y - BAR / 2) as f64), BACKGROUND);
}

#[test]
fn hovering_the_close_button_highlights_it() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let (x, y, w, _) = rect(&pila.window("a")["geometry"]);
    // A corner of the close button, away from its icon.
    let (bx, by) = ((x + w - 3) as f64, (y - BAR + 3) as f64);
    pila.settle();
    let before = pila.pixel(bx, by);
    pila.move_to(bx, by);
    pila.settle();
    let after = pila.pixel(bx, by);
    assert!(
        near(after, [196, 43, 28, 255], 3),
        "{before:?} -> {after:?}"
    );
}

#[test]
fn rounded_corners_include_the_bar_and_go_square_when_maximized() {
    let mut pila = TestPila::with_config("corner-radius = 12\n");
    let client = pila.client();
    client.window("a").size(400, 300).color(RED).create();
    pila.wait_focus("a");
    let (x, y, w, h) = rect(&pila.window("a")["geometry"]);
    pila.settle();
    // The bar's top corners and the window's bottom ones are cut.
    assert_eq!(pila.pixel((x + 1) as f64, (y - BAR + 1) as f64), BACKGROUND);
    assert_eq!(
        pila.pixel((x + w - 2) as f64, (y - BAR + 1) as f64),
        BACKGROUND
    );
    assert_eq!(pila.pixel((x + 1) as f64, (y + h - 2) as f64), BACKGROUND);
    // The window's top corners, under the bar, aren't.
    assert_eq!(pila.pixel((x + 1) as f64, (y + 1) as f64), [255, 0, 0, 255]);
    pila.combo("Super+m");
    pila.wait("maximized", |s| {
        window_in(s, "a").unwrap()["geometry"]["w"] == 1280
    });
    pila.settle();
    assert!(near(pila.pixel(1.0, 1.0), BAR_FOCUSED, 2));
    assert_eq!(pila.pixel(1.0, 798.0), [255, 0, 0, 255]);
}

#[test]
fn the_bell_flash_lightens_the_window() {
    let mut pila = TestPila::with_config("[animations]\nenabled = true\nspeed = 4\n");
    let client = pila.client();
    let a = client
        .window("a")
        .size(400, 300)
        .color([255, 40, 40, 40])
        .create();
    pila.wait_focus("a");
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    pila.settle();
    let base = pila.pixel((x + 200) as f64, (y + 150) as f64);
    use wayland_protocols::xdg::system_bell::v1::client::xdg_system_bell_v1::XdgSystemBellV1;
    let bell: XdgSystemBellV1 = client.bind("xdg_system_bell_v1", 1);
    bell.ring(Some(&client.surface(a)));
    client.flush();
    let mut lighter = false;
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_millis(400) && !lighter {
        let px = pila.pixel((x + 200) as f64, (y + 150) as f64);
        lighter = px[0] > base[0] + 10;
    }
    assert!(lighter, "no flash over {base:?}");
    pila.wait("the flash ends", |s| {
        window_in(s, "a").unwrap()["flashing"] == false
    });
    pila.settle();
    assert_eq!(pila.pixel((x + 200) as f64, (y + 150) as f64), base);
}

#[test]
fn the_snap_preview_is_drawn_while_dragging_to_an_edge() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    pila.move_to((x + 100) as f64, (y + 100) as f64);
    pila.key_press("Super");
    pila.press("left");
    pila.wait("a move grab", |s| s["pointer"]["grab"] == "move");
    pila.move_to(0.0, 400.0);
    pila.wait("a preview", |s| !s["snap_preview"].is_null());
    pila.settle();
    let px = pila.pixel(300.0, 700.0);
    assert_ne!(px, BACKGROUND, "no preview drawn");
    pila.release("left");
    pila.key_release("Super");
}

/// Two Bottom layers, red on the left half and green on the right; a
/// transparent Top layer of 200x200 across the middle, named `namespace`.
fn halves_and_a_glass(pila: &mut TestPila, client: &Client, namespace: &str) {
    for (name, anchor, color) in [
        (
            "left",
            Anchor::Top | Anchor::Bottom | Anchor::Left,
            [255, 255, 0, 0],
        ),
        (
            "right",
            Anchor::Top | Anchor::Bottom | Anchor::Right,
            [255, 0, 255, 0],
        ),
    ] {
        let (surf, _) = client.layer_surface(
            name,
            &LayerSpec {
                layer: Layer::Bottom,
                anchor,
                size: (640, 0),
                ..LayerSpec::default()
            },
            color,
        );
        client.wait_surface_configured(surf);
    }
    let (glass, _) = client.layer_surface(
        namespace,
        &LayerSpec {
            layer: Layer::Top,
            size: (200, 200),
            ..LayerSpec::default()
        },
        [0, 0, 0, 0],
    );
    client.wait_surface_configured(glass);
    pila.settle();
}

#[test]
fn a_blur_rule_blurs_whats_behind_a_layer() {
    let mut pila = TestPila::with_config("[[layer-rule]]\nnamespace = \"glass\"\nblur = true\n");
    let client = pila.client();
    halves_and_a_glass(&mut pila, &client, "glass");
    // Just left of the red/green edge, under the glass: a mix (once the
    // glass has been drawn with what's behind it).
    wait_mixed(&mut pila, 636.0, 400.0);
    // Outside the glass: sharp.
    assert_eq!(pila.pixel(636.0, 100.0), [255, 0, 0, 255]);
}

#[test]
fn without_blur_the_glass_is_clear() {
    let mut pila = TestPila::start();
    let client = pila.client();
    halves_and_a_glass(&mut pila, &client, "glass");
    assert_eq!(pila.pixel(636.0, 400.0), [255, 0, 0, 255]);
}

#[test]
fn a_client_can_ask_for_blur_behind_a_region() {
    let mut pila = TestPila::start();
    let client = pila.client();
    halves_and_a_glass(&mut pila, &client, "glass");
    set_blur_region(&mut pila, &client, Surf(2), &[(0, 0, 200, 200)]);
    wait_mixed(&mut pila, 636.0, 400.0);
}

const BLUE: [u8; 4] = [255, 0, 0, 255];

/// A red background layer on `output` and an empty glass over its center,
/// blurred by rule.
fn red_wallpaper_and_a_glass(pila: &mut TestPila, client: &Client, output: Option<&str>) {
    let (wallpaper, _) = client.layer_surface(
        "wallpaper",
        &LayerSpec {
            layer: Layer::Background,
            anchor: Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right,
            size: (0, 0),
            output: output.map(str::to_string),
            ..LayerSpec::default()
        },
        RED,
    );
    client.wait_surface_configured(wallpaper);
    let (glass, _) = client.layer_surface(
        "glass",
        &LayerSpec {
            layer: Layer::Top,
            size: (200, 200),
            output: output.map(str::to_string),
            ..LayerSpec::default()
        },
        [0, 0, 0, 0],
    );
    client.wait_surface_configured(glass);
    pila.settle();
}

/// Wait until the pixel is `color`, more or less: a blur takes a frame.
#[track_caller]
fn wait_near(pila: &mut TestPila, x: f64, y: f64, color: [u8; 4]) {
    pila.wait_pixel_is(x, y, &format!("near {color:?}"), |px| near(px, color, 3));
}

/// Wait until the pixel is a mix of red and green: blurred across the edge
/// between them.
#[track_caller]
fn wait_mixed(pila: &mut TestPila, x: f64, y: f64) {
    pila.wait_pixel_is(x, y, "a mix of red and green", mixed);
}

fn mixed(px: [u8; 4]) -> bool {
    px[0] > 20 && px[1] > 20
}

const GLASS_RULE: &str = "[[layer-rule]]\nnamespace = \"glass\"\nblur = true\n";

#[test]
fn a_blur_shows_a_window_mapped_after_the_surface() {
    let mut pila = TestPila::with_config(GLASS_RULE);
    let client = pila.client();
    red_wallpaper_and_a_glass(&mut pila, &client, None);
    wait_near(&mut pila, 640.0, 400.0, [255, 0, 0, 255]);
    client.window("a").maximized().color(BLUE).create();
    pila.wait_focus("a");
    pila.settle();
    wait_near(&mut pila, 640.0, 400.0, [0, 0, 255, 255]);
}

#[test]
fn a_blur_shows_a_window_mapped_before_the_surface() {
    let mut pila = TestPila::with_config(GLASS_RULE);
    let client = pila.client();
    client.window("a").maximized().color(BLUE).create();
    pila.wait_focus("a");
    red_wallpaper_and_a_glass(&mut pila, &client, None);
    wait_near(&mut pila, 640.0, 400.0, [0, 0, 255, 255]);
}

#[test]
fn a_blur_shows_a_window_with_rounded_corners() {
    let mut pila = TestPila::with_config(&format!("corner-radius = 12\n{GLASS_RULE}"));
    let client = pila.client();
    client.window("a").size(800, 600).color(BLUE).create();
    pila.wait_focus("a");
    red_wallpaper_and_a_glass(&mut pila, &client, None);
    wait_near(&mut pila, 640.0, 400.0, [0, 0, 255, 255]);
}

#[test]
fn a_blur_on_a_second_output_shows_its_window() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1280x800".into()),
        config: GLASS_RULE.into(),
        ..Default::default()
    });
    let client = pila.client();
    pila.move_to(1920.0, 400.0);
    red_wallpaper_and_a_glass(&mut pila, &client, Some("B"));
    wait_near(&mut pila, 1920.0, 400.0, [255, 0, 0, 255]);
    client.window("a").maximized().color(BLUE).create();
    pila.wait_focus("a");
    pila.settle();
    let (x, ..) = rect(&pila.window("a")["geometry"]);
    assert!(x >= 1280, "on output B");
    wait_near(&mut pila, 1920.0, 400.0, [0, 0, 255, 255]);
}

#[test]
fn a_blur_keeps_to_the_clients_region() {
    let mut pila = TestPila::start();
    let client = pila.client();
    halves_and_a_glass(&mut pila, &client, "glass");
    // The glass is at 540,300. Two strips, as a client tessellates a rounded
    // shape: their bounding box covers the whole glass, they don't.
    let (surface, effect) = set_blur_region(
        &mut pila,
        &client,
        Surf(2),
        &[(150, 0, 50, 20), (0, 20, 200, 180)],
    );
    // Inside the region, by the red/green edge: mixed.
    wait_mixed(&mut pila, 636.0, 400.0);
    // The cut corner, in the strips' bounding box and by the same edge: sharp.
    assert_eq!(pila.pixel(636.0, 305.0), [255, 0, 0, 255]);
    // Another shape, with nothing changed behind it: the corner is covered,
    // the one at the bottom is cut.
    reshape_blur(
        &mut pila,
        &client,
        &surface,
        &effect,
        &[(0, 0, 200, 180), (150, 180, 50, 20)],
    );
    wait_mixed(&mut pila, 636.0, 305.0);
    assert_eq!(pila.pixel(636.0, 495.0), [255, 0, 0, 255]);
}

/// A red wallpaper and a glass on both outputs, as a shell does.
fn glasses_on_two_outputs(config: &str) -> (TestPila, Client) {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1280x800".into()),
        config: config.into(),
        ..Default::default()
    });
    let client = pila.client();
    red_wallpaper_and_a_glass(&mut pila, &client, Some("A"));
    red_wallpaper_and_a_glass(&mut pila, &client, Some("B"));
    wait_near(&mut pila, 640.0, 400.0, [255, 0, 0, 255]);
    wait_near(&mut pila, 1920.0, 400.0, [255, 0, 0, 255]);
    (pila, client)
}

#[test]
fn blurs_on_two_outputs_show_their_windows() {
    let (mut pila, client) = glasses_on_two_outputs(GLASS_RULE);
    pila.move_to(1920.0, 100.0);
    client.window("a").maximized().color(BLUE).create();
    pila.wait_focus("a");
    pila.settle();
    wait_near(&mut pila, 1920.0, 400.0, [0, 0, 255, 255]);
    wait_near(&mut pila, 640.0, 400.0, [255, 0, 0, 255]);
}

#[test]
fn blurs_on_two_outputs_show_a_window_that_opens_animated() {
    let config = format!("{GLASS_RULE}\n[animations]\nenabled = true\n");
    let (mut pila, client) = glasses_on_two_outputs(&config);
    pila.move_to(1920.0, 100.0);
    client.window("a").maximized().color(BLUE).create();
    pila.wait_focus("a");
    pila.settle();
    wait_near(&mut pila, 1920.0, 400.0, [0, 0, 255, 255]);
}

#[test]
fn blurs_on_two_outputs_keep_their_backdrops() {
    // A backdrop is allocated once per surface and output.
    let (mut pila, _client) = glasses_on_two_outputs(GLASS_RULE);
    for x in 0..20 {
        pila.move_to(100.0 + x as f64, 100.0);
        pila.move_to(1400.0 + x as f64, 100.0);
    }
    let state = pila.settle();
    assert_eq!(state["blur"]["allocated"], 2, "backdrops allocated");
}

/// Ask for blur behind `rects` of a surface, in this order: its background
/// effect object, for later changes (`reshape_blur`).
fn set_blur_region(
    pila: &mut TestPila,
    client: &Client,
    glass: Surf,
    rects: &[(i32, i32, i32, i32)],
) -> (WlSurface, ExtBackgroundEffectSurfaceV1) {
    let surface = client.wl_surface_of(glass);
    let manager: ExtBackgroundEffectManagerV1 = client.bind("ext_background_effect_manager_v1", 1);
    let effect = manager.get_background_effect(&surface, &client.qh, "effect".into());
    reshape_blur(pila, client, &surface, &effect, rects);
    (surface, effect)
}

/// Give a surface's blur a new region, `rects`.
fn reshape_blur(
    pila: &mut TestPila,
    client: &Client,
    surface: &WlSurface,
    effect: &ExtBackgroundEffectSurfaceV1,
    rects: &[(i32, i32, i32, i32)],
) {
    let compositor: WlCompositor = client.lock().compositor.clone().unwrap();
    let region = compositor.create_region(&client.qh, "region".into());
    for (x, y, w, h) in rects {
        region.add(*x, *y, *w, *h);
    }
    effect.set_blur_region(Some(&region));
    surface.commit();
    client.flush();
    pila.settle();
}

#[test]
fn a_blur_has_no_gaps_at_a_fractional_scale() {
    // 1024x640 logical: red and green halves, the glass at 412,220, which is
    // the physical pixel 515,275.
    let mut pila = TestPila::with_config("[[output]]\nname = \"HEADLESS-1\"\nscale = 1.25\n");
    let client = pila.client();
    for (name, anchor, color) in [
        ("left", Anchor::Left, [255, 255, 0, 0]),
        ("right", Anchor::Right, [255, 0, 255, 0]),
    ] {
        let (surf, _) = client.layer_surface(
            name,
            &LayerSpec {
                layer: Layer::Bottom,
                anchor: Anchor::Top | Anchor::Bottom | anchor,
                size: (512, 0),
                ..LayerSpec::default()
            },
            color,
        );
        client.wait_surface_configured(surf);
    }
    let (glass, _) = client.layer_surface(
        "glass",
        &LayerSpec {
            layer: Layer::Top,
            size: (200, 200),
            ..LayerSpec::default()
        },
        [0, 0, 0, 0],
    );
    client.wait_surface_configured(glass);
    // One logical row at a time, as a client tessellates a rounded shape.
    let rows: Vec<_> = (0..200).map(|y| (0, y, 200, 1)).collect();
    set_blur_region(&mut pila, &client, glass, &rows);
    // By the red/green edge, every physical row of the glass is mixed.
    wait_mixed(&mut pila, 510.0, 320.0);
    for row in 276..524 {
        let px = pila.pixel(510.0, row as f64 / 1.25);
        assert!(mixed(px), "the physical row {row} is sharp: {px:?}");
    }
}

#[test]
fn a_blur_follows_a_change_of_scale() {
    // A glass over the whole output, whose physical size doesn't change.
    let mut pila = TestPila::with_config(&format!(
        "[[output]]\nname = \"HEADLESS-1\"\nscale = 2\n{GLASS_RULE}"
    ));
    let client = pila.client();
    let (left, _) = client.layer_surface(
        "left",
        &LayerSpec {
            layer: Layer::Bottom,
            anchor: Anchor::Top | Anchor::Bottom | Anchor::Left,
            size: (320, 0),
            ..LayerSpec::default()
        },
        [255, 255, 0, 0],
    );
    client.wait_surface_configured(left);
    let (glass, _) = client.layer_surface(
        "glass",
        &LayerSpec {
            layer: Layer::Top,
            anchor: Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right,
            size: (0, 0),
            ..LayerSpec::default()
        },
        [0, 0, 0, 0],
    );
    client.wait_surface_configured(glass);
    pila.settle();
    // 320 logical pixels are half of the screen, 640x400 here.
    wait_near(&mut pila, 100.0, 200.0, [255, 0, 0, 255]);
    pila.write_config(&format!(
        "[[output]]\nname = \"HEADLESS-1\"\nscale = 1\n{GLASS_RULE}"
    ));
    pila.wait("the new scale", |s| s["outputs"][0]["scale"] == 1.0);
    pila.settle();
    // And a quarter of it now: drawn at the old scale, it would be half.
    wait_near(&mut pila, 100.0, 400.0, [255, 0, 0, 255]);
    wait_near(&mut pila, 900.0, 400.0, BACKGROUND);
    let px = pila.pixel(500.0, 400.0);
    assert!(near(px, BACKGROUND, 3), "the red goes too far: {px:?}");
}

#[test]
fn damage_far_from_the_region_doesnt_blur_again() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").size(100, 100).color(BLUE).create();
    pila.wait_focus("a");
    let (glass, _) = client.layer_surface(
        "glass",
        &LayerSpec {
            layer: Layer::Top,
            anchor: Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right,
            size: (0, 0),
            ..LayerSpec::default()
        },
        [0, 0, 0, 0],
    );
    client.wait_surface_configured(glass);
    // Bars at the top and the bottom; the window is in the middle, between
    // them, in their bounding box.
    set_blur_region(
        &mut pila,
        &client,
        glass,
        &[(0, 0, 1280, 20), (0, 780, 1280, 20)],
    );
    let blurred = pila.settle()["blur"]["blurred"].as_u64().unwrap();
    assert!(blurred > 0);
    for _ in 0..5 {
        client.draw(a, None);
        client.roundtrip();
        pila.settle();
    }
    assert_eq!(pila.settle()["blur"]["blurred"].as_u64().unwrap(), blurred);
}

#[test]
fn an_output_turned_off_gives_its_backdrops_up() {
    use wayland_protocols_wlr::output_power_management::v1::client::{
        zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1, zwlr_output_power_v1::Mode,
    };
    let (mut pila, client) = glasses_on_two_outputs(GLASS_RULE);
    pila.wait("both backdrops", |s| s["blur"]["entries"] == 2);
    let manager: ZwlrOutputPowerManagerV1 = client.bind("zwlr_output_power_manager_v1", 1);
    let power = manager.get_output_power(&client.output("B"), &client.qh, "power".into());
    power.set_mode(Mode::Off);
    client.flush();
    pila.wait("only one left", |s| s["blur"]["entries"] == 1);
    power.set_mode(Mode::On);
    client.flush();
    pila.wait("back", |s| s["blur"]["entries"] == 2);
}

#[test]
fn a_blur_keeps_to_the_clients_region_on_a_scaled_output() {
    // 640x400 logical: red and green halves, the glass at 220,100.
    let mut pila = TestPila::with_config("[[output]]\nname = \"HEADLESS-1\"\nscale = 2\n");
    let client = pila.client();
    for (name, anchor, color) in [
        ("left", Anchor::Left, [255, 255, 0, 0]),
        ("right", Anchor::Right, [255, 0, 255, 0]),
    ] {
        let (surf, _) = client.layer_surface(
            name,
            &LayerSpec {
                layer: Layer::Bottom,
                anchor: Anchor::Top | Anchor::Bottom | anchor,
                size: (320, 0),
                ..LayerSpec::default()
            },
            color,
        );
        client.wait_surface_configured(surf);
    }
    let (glass, _) = client.layer_surface(
        "glass",
        &LayerSpec {
            layer: Layer::Top,
            size: (200, 200),
            ..LayerSpec::default()
        },
        [0, 0, 0, 0],
    );
    client.wait_surface_configured(glass);
    set_blur_region(
        &mut pila,
        &client,
        glass,
        &[(150, 0, 50, 20), (0, 20, 200, 180)],
    );
    wait_mixed(&mut pila, 318.0, 200.0);
    assert_eq!(pila.pixel(318.0, 105.0), [255, 0, 0, 255]);
}
