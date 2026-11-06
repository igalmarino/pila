//! What's drawn, by pixels: title bars (focused or not, buttons on hover),
//! rounded corners with a bar and when maximized, the bell flash, the snap
//! preview, blur behind layer surfaces.

mod common;

use common::client::Decorations;
use common::protocols::LayerSpec;
use common::*;
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
    let started = std::time::Instant::now();
    loop {
        let px = pila.pixel(636.0, 400.0);
        if px[1] > 20 && px[0] > 20 {
            break;
        }
        assert!(started.elapsed() < TIMEOUT, "not blurred: {px:?}");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
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
    use wayland_client::protocol::wl_compositor::WlCompositor;
    use wayland_protocols::ext::background_effect::v1::client::ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1;
    let mut pila = TestPila::start();
    let client = pila.client();
    halves_and_a_glass(&mut pila, &client, "glass");
    let surface = client.wl_surface_of(common::protocols::Surf(2));
    let manager: ExtBackgroundEffectManagerV1 = client.bind("ext_background_effect_manager_v1", 1);
    let effect = manager.get_background_effect(&surface, &client.qh, "effect".into());
    let compositor: WlCompositor = client.lock().compositor.clone().unwrap();
    let region = compositor.create_region(&client.qh, "region".into());
    region.add(0, 0, 200, 200);
    effect.set_blur_region(Some(&region));
    surface.commit();
    client.flush();
    pila.settle();
    let started = std::time::Instant::now();
    loop {
        let px = pila.pixel(636.0, 400.0);
        if px[1] > 20 && px[0] > 20 {
            break;
        }
        assert!(started.elapsed() < TIMEOUT, "not blurred: {px:?}");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
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
    let started = std::time::Instant::now();
    loop {
        let px = pila.pixel(x, y);
        if near(px, color, 3) {
            return;
        }
        assert!(
            started.elapsed() < TIMEOUT,
            "the pixel is {px:?}, not {color:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
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
    use wayland_client::protocol::wl_compositor::WlCompositor;
    use wayland_protocols::ext::background_effect::v1::client::ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1;
    let mut pila = TestPila::start();
    let client = pila.client();
    halves_and_a_glass(&mut pila, &client, "glass");
    // The glass is at 540,300. Two strips, as a client tessellates a rounded
    // shape: their bounding box covers the whole glass, they don't.
    let surface = client.wl_surface_of(common::protocols::Surf(2));
    let manager: ExtBackgroundEffectManagerV1 = client.bind("ext_background_effect_manager_v1", 1);
    let effect = manager.get_background_effect(&surface, &client.qh, "effect".into());
    let compositor: WlCompositor = client.lock().compositor.clone().unwrap();
    let region = compositor.create_region(&client.qh, "region".into());
    region.add(150, 0, 50, 20);
    region.add(0, 20, 200, 180);
    effect.set_blur_region(Some(&region));
    surface.commit();
    client.flush();
    pila.settle();
    // Inside the region, by the red/green edge: mixed.
    let started = std::time::Instant::now();
    loop {
        let px = pila.pixel(636.0, 400.0);
        if px[1] > 20 && px[0] > 20 {
            break;
        }
        assert!(started.elapsed() < TIMEOUT, "not blurred: {px:?}");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // The cut corner, in the strips' bounding box and by the same edge: sharp.
    assert_eq!(pila.pixel(636.0, 305.0), [255, 0, 0, 255]);
    // Another shape, with nothing changed behind it: the corner is covered,
    // the one at the bottom is cut.
    let region = compositor.create_region(&client.qh, "region2".into());
    region.add(0, 0, 200, 180);
    region.add(150, 180, 50, 20);
    effect.set_blur_region(Some(&region));
    surface.commit();
    client.flush();
    pila.settle();
    let started = std::time::Instant::now();
    loop {
        let px = pila.pixel(636.0, 305.0);
        if px[1] > 20 && px[0] > 20 {
            break;
        }
        assert!(started.elapsed() < TIMEOUT, "not reshaped: {px:?}");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
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
    // Debug logs say when a backdrop is allocated: once per surface and output.
    let (mut pila, _client) = glasses_on_two_outputs(GLASS_RULE);
    for x in 0..20 {
        pila.move_to(100.0 + x as f64, 100.0);
        pila.move_to(1400.0 + x as f64, 100.0);
    }
    pila.settle();
    let allocated = pila.log().matches("blur: allocated").count();
    assert_eq!(allocated, 2, "backdrops allocated");
}

#[test]
fn a_blur_keeps_to_the_clients_region_on_a_scaled_output() {
    use wayland_client::protocol::wl_compositor::WlCompositor;
    use wayland_protocols::ext::background_effect::v1::client::ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1;
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
    let surface = client.wl_surface_of(glass);
    let manager: ExtBackgroundEffectManagerV1 = client.bind("ext_background_effect_manager_v1", 1);
    let effect = manager.get_background_effect(&surface, &client.qh, "effect".into());
    let compositor: WlCompositor = client.lock().compositor.clone().unwrap();
    let region = compositor.create_region(&client.qh, "region".into());
    region.add(150, 0, 50, 20);
    region.add(0, 20, 200, 180);
    effect.set_blur_region(Some(&region));
    surface.commit();
    client.flush();
    pila.settle();
    let started = std::time::Instant::now();
    loop {
        let px = pila.pixel(318.0, 200.0);
        if px[1] > 20 && px[0] > 20 {
            break;
        }
        assert!(started.elapsed() < TIMEOUT, "not blurred: {px:?}");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(pila.pixel(318.0, 105.0), [255, 0, 0, 255]);
}
