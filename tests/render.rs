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
