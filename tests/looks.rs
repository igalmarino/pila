//! Shadows and `[colors]`, by pixels. The shadow tests draw on a white
//! background, so a pixel's darkness is the shadow's alone.

mod common;

use common::client::Decorations;
use common::*;
use std::time::Duration;

const WHITE: [u8; 4] = [255, 255, 255, 255];
const RED: [u8; 4] = [255, 255, 0, 0];
const SLOW: &str = "[animations]\nenabled = true\nspeed = 4\n";
const BAR: i64 = 28;

/// A black shadow, half as strong without focus, on white.
fn shadow(offset: [i32; 2]) -> String {
    format!(
        "[shadow]\nsize = 24\noffset = [{}, {}]\ncolor = \"#000000ff\"\n\
         inactive-color = \"#00000080\"\n[colors]\nbackground = \"#ffffff\"\n",
        offset[0], offset[1]
    )
}

/// `app` opens with no title bar, 200x200 at (x, y), with `more`.
fn rule(app: &str, x: i32, y: i32, more: &str) -> String {
    format!(
        "[[window-rule]]\napp-id = \"{app}\"\ntitle-bar = false\nsize = [200, 200]\n\
         position = [{x}, {y}]\n{more}\n"
    )
}

/// A shadow with no offset under `a`, at x 540..740 and y 300..500.
fn one_window(top: &str, more: &str) -> (TestPila, Client) {
    let mut pila = TestPila::with_config(&format!(
        "{top}{}{}",
        shadow([0, 0]),
        rule("a", 540, 300, more)
    ));
    let client = pila.client();
    client.window("a").color(RED).create();
    pila.wait_focus("a");
    pila.settle();
    (pila, client)
}

/// Grey, and clearly not white.
fn dark(px: [u8; 4]) -> bool {
    px[0] == px[1] && px[1] == px[2] && px[0] < 230
}

fn near(a: [u8; 4], b: [u8; 4], tolerance: i32) -> bool {
    (0..3).all(|i| (a[i] as i32 - b[i] as i32).abs() <= tolerance)
}

#[test]
fn a_shadow_is_darkest_at_the_window_and_fades_away() {
    let (mut pila, _client) = one_window("", "");
    let [touching, ..] = pila.pixel(640.0, 501.0);
    let [halfway, ..] = pila.pixel(640.0, 512.0);
    assert!(touching < 20, "{touching}");
    assert!((90..170).contains(&halfway), "{halfway}");
    assert_eq!(pila.pixel(640.0, 530.0), WHITE, "past its size");
    // All around.
    assert!(dark(pila.pixel(530.0, 400.0)));
    assert!(dark(pila.pixel(750.0, 400.0)));
    assert!(dark(pila.pixel(640.0, 290.0)));
    assert_eq!(pila.pixel(640.0, 400.0), [255, 0, 0, 255]);
}

#[test]
fn a_shadows_offset_moves_it() {
    let mut pila =
        TestPila::with_config(&format!("{}{}", shadow([0, 10]), rule("a", 540, 300, "")));
    let client = pila.client();
    client.window("a").color(RED).create();
    pila.wait_focus("a");
    pila.settle();
    assert_eq!(pila.pixel(640.0, 280.0), WHITE);
    assert!(dark(pila.pixel(640.0, 520.0)));
}

#[test]
fn a_shadow_doesnt_show_through_a_clear_window() {
    let mut pila = TestPila::with_config(&format!("{}{}", shadow([0, 0]), rule("a", 540, 300, "")));
    let client = pila.client();
    client.window("a").color([0, 0, 0, 0]).create();
    pila.wait_focus("a");
    pila.settle();
    assert_eq!(pila.pixel(640.0, 400.0), WHITE);
    assert_eq!(pila.pixel(542.0, 400.0), WHITE, "just inside");
    assert!(dark(pila.pixel(538.0, 400.0)), "just outside");
}

#[test]
fn the_focused_windows_shadow_is_stronger() {
    let mut pila = TestPila::with_config(&format!(
        "{}{}{}",
        shadow([0, 0]),
        rule("a", 100, 100, ""),
        rule("b", 600, 100, "")
    ));
    let client = pila.client();
    client.window("a").color(RED).create();
    pila.wait_focus("a");
    client.window("b").color(RED).create();
    pila.wait_focus("b");
    pila.settle();
    let weak = |[v, ..]: [u8; 4]| (100..180).contains(&v);
    let strong = |[v, ..]: [u8; 4]| v < 50;
    assert!(weak(pila.pixel(200.0, 303.0)));
    assert!(strong(pila.pixel(700.0, 303.0)));
    pila.click_at(200.0, 200.0);
    pila.wait_focus("a");
    pila.wait_pixel_is(200.0, 303.0, "strong", strong);
    pila.wait_pixel_is(700.0, 303.0, "weak", weak);
    // Losing focus with nothing raised: only the shadow changes.
    pila.click_at(5.0, 795.0);
    pila.wait("unfocused", |s| s["focused"].is_null());
    pila.wait_pixel_is(200.0, 303.0, "weak", weak);
}

#[test]
fn a_filled_window_has_no_shadow_on_the_next_output() {
    let mut pila = TestPila::start_with(Options {
        config: format!("{}{}", shadow([0, 0]), rule("a", 1060, 300, "")),
        outputs: Some("A:1280x800,B:1280x800".into()),
        ..Options::default()
    });
    let client = pila.client();
    client.window("a").color(RED).create();
    pila.wait_focus("a");
    pila.settle();
    assert!(dark(pila.pixel(1262.0, 400.0)));
    for combo in ["Super+m", "Super+f"] {
        pila.combo(combo);
        pila.wait("it fills the output", |s| {
            window_in(s, "a").unwrap()["geometry"]["w"] == 1280
        });
        pila.settle();
        assert_eq!(pila.pixel(1282.0, 400.0), WHITE, "{combo}");
        pila.combo(combo);
        pila.wait("it's back", |s| {
            window_in(s, "a").unwrap()["geometry"]["w"] == 200
        });
        pila.settle();
        assert!(dark(pila.pixel(1262.0, 400.0)), "{combo}");
    }
}

#[test]
fn a_shadow_crosses_to_the_next_output() {
    let mut pila = TestPila::start_with(Options {
        config: format!("{}{}", shadow([0, 0]), rule("a", 1180, 300, "")),
        outputs: Some("A:1280x800,B:1280x800".into()),
        ..Options::default()
    });
    let client = pila.client();
    client.window("a").color(RED).create();
    pila.wait_focus("a");
    pila.settle();
    assert!(dark(pila.pixel(1200.0, 505.0)), "on the first");
    assert!(dark(pila.pixel(1300.0, 505.0)), "on the second");
}

#[test]
fn shadows_can_be_turned_off() {
    let mut pila = TestPila::with_config(&format!(
        "{}{}",
        shadow([0, 0]).replace("[shadow]\n", "[shadow]\nenabled = false\n"),
        rule("a", 540, 300, "")
    ));
    let client = pila.client();
    client.window("a").color(RED).create();
    pila.wait_focus("a");
    pila.settle();
    assert_eq!(pila.pixel(640.0, 505.0), WHITE);
}

#[test]
fn a_rule_turns_a_shadow_off_or_on() {
    let (mut pila, _client) = one_window("", "shadow = false");
    assert_eq!(pila.pixel(640.0, 505.0), WHITE);

    let mut pila = TestPila::with_config(&format!(
        "{}{}",
        shadow([0, 0]).replace("[shadow]\n", "[shadow]\nenabled = false\n"),
        rule("a", 540, 300, "shadow = true")
    ));
    let client = pila.client();
    client.window("a").color(RED).create();
    pila.wait_focus("a");
    pila.settle();
    assert!(dark(pila.pixel(640.0, 505.0)));
}

/// A window whose client draws 10 pixels past its geometry, as a shadow of
/// its own would be. Returns the pixel just past what it draws.
fn own_shadow(pila: &mut TestPila, client: &Client) -> [u8; 4] {
    let window = client.window("a").color(RED).create();
    pila.wait_focus("a");
    client.with_window(window, |w| {
        w.xdg_surface.set_window_geometry(10, 10, 180, 180);
    });
    client.draw(window, None);
    pila.wait("the new geometry", |s| {
        window_in(s, "a").unwrap()["geometry"]["w"] == 180
    });
    pila.settle();
    let (x, y, w, h) = rect(&pila.window("a")["geometry"]);
    pila.pixel((x + w / 2) as f64, (y + h + 13) as f64)
}

#[test]
fn a_client_that_draws_past_its_geometry_gets_no_second_shadow() {
    let mut pila = TestPila::with_config(&format!("{}{}", shadow([0, 0]), rule("a", 540, 300, "")));
    let client = pila.client();
    assert_eq!(own_shadow(&mut pila, &client), WHITE);

    let mut pila = TestPila::with_config(&format!(
        "{}{}",
        shadow([0, 0]),
        rule("a", 540, 300, "shadow = true")
    ));
    let client = pila.client();
    assert!(dark(own_shadow(&mut pila, &client)), "forced by a rule");
}

#[test]
fn a_shadow_follows_rounded_corners() {
    let (mut pila, _client) = one_window("corner-radius = 20\n", "");
    // Cut off the window, where a shadow shows.
    assert!(dark(pila.pixel(542.0, 302.0)));
    // Along the edges it's as usual.
    assert!(dark(pila.pixel(640.0, 503.0)));
    assert_eq!(pila.pixel(640.0, 400.0), [255, 0, 0, 255]);
}

#[test]
fn a_moved_windows_shadow_leaves_nothing_behind() {
    let (mut pila, _client) = one_window("", "");
    assert!(dark(pila.pixel(530.0, 400.0)));
    for _ in 0..3 {
        pila.combo("Super+Shift+Right");
    }
    pila.wait("it moved", |s| {
        window_in(s, "a").unwrap()["geometry"]["x"] == 660
    });
    pila.settle();
    assert_eq!(pila.pixel(530.0, 400.0), WHITE);
    assert!(dark(pila.pixel(650.0, 400.0)));
}

/// Whether row `y` had a grey pixel (a shadow) after the first 100 ms, until
/// `done`: read whole and at once, so no frame gets between the pixels.
fn saw_shadow(pila: &mut TestPila, y: f64, done: impl Fn(&mut TestPila) -> bool) -> bool {
    let started = std::time::Instant::now();
    let mut seen = false;
    while !done(pila) {
        assert!(started.elapsed() < TIMEOUT, "it never ended");
        if started.elapsed() > Duration::from_millis(100) {
            let row = pila.request(json!({ "cmd": "pixels", "x": 0, "y": y, "w": 1280, "h": 1 }));
            let row: Vec<u8> = row["rgba"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect();
            seen |= row.as_chunks::<4>().0.iter().any(|px| dark(*px));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    seen
}

fn animating(pila: &mut TestPila) -> bool {
    pila.state()["animations"]["running"] == true
}

fn slow_window() -> TestPila {
    TestPila::with_config(&format!(
        "{SLOW}{}{}",
        shadow([0, 0]),
        rule("a", 540, 300, "")
    ))
}

#[test]
fn a_window_opens_with_its_shadow() {
    let mut pila = slow_window();
    let client = pila.client();
    client.window("a").color(RED).create();
    pila.wait_log("starts what=\"window-open\"");
    let seen = saw_shadow(&mut pila, 505.0, |pila| !animating(pila));
    assert!(seen, "no shadow while it opened");
    assert!(dark(pila.pixel(640.0, 505.0)));
}

#[test]
fn a_window_closes_with_its_shadow() {
    let mut pila = slow_window();
    let client = pila.client();
    let window = client.window("a").color(RED).create();
    pila.wait_focus("a");
    pila.settle();
    client.destroy(window);
    pila.wait_log("starts what=\"window-close\"");
    let seen = saw_shadow(&mut pila, 505.0, |pila| !animating(pila));
    assert!(seen, "no shadow while it closed");
    pila.settle();
    assert_eq!(pila.pixel(640.0, 505.0), WHITE);
}

#[test]
fn a_window_slides_away_with_its_shadow() {
    let mut pila = slow_window();
    let client = pila.client();
    client.window("a").color(RED).create();
    pila.wait_focus("a");
    pila.settle();
    pila.combo("Super+2");
    pila.wait_log("starts what=\"workspace\"");
    let seen = saw_shadow(&mut pila, 505.0, |pila| !animating(pila));
    assert!(seen, "no shadow while it slid");
    pila.settle();
    assert_eq!(pila.pixel(640.0, 505.0), WHITE);
    pila.combo("Super+1");
    pila.settle();
    assert!(dark(pila.pixel(640.0, 505.0)));
}

#[test]
fn a_click_on_a_shadow_goes_to_whats_below() {
    let mut pila = TestPila::with_config(&format!(
        "{}{}{}",
        shadow([0, 0]),
        rule("a", 100, 100, ""),
        rule("b", 290, 100, "")
    ));
    let client = pila.client();
    client.window("a").color(RED).create();
    pila.wait_focus("a");
    client.window("b").color(RED).create();
    pila.wait_focus("b");
    pila.settle();
    // In b's shadow, past its resize border, over a.
    pila.click_at(270.0, 200.0);
    pila.wait_focus("a");
}

#[test]
fn a_shadow_draws_nothing_while_nothing_changes() {
    let (mut pila, _client) = one_window("", "");
    let frames = |state: &Value| state["outputs"][0]["frames"].as_u64().unwrap();
    // A slow machine may still be showing the client's last commit; a shadow
    // that asks for frames would never let the count rest.
    let started = std::time::Instant::now();
    let mut before = frames(&pila.settle());
    loop {
        std::thread::sleep(Duration::from_millis(300));
        let now = frames(&pila.state());
        if now == before {
            return;
        }
        assert!(started.elapsed() < TIMEOUT, "it keeps drawing: {now}");
        before = now;
    }
}

#[test]
fn a_reload_applies_shadows() {
    let (mut pila, _client) = one_window("", "");
    assert!(dark(pila.pixel(640.0, 505.0)));
    let config = format!("{}{}", shadow([0, 0]), rule("a", 540, 300, ""));
    let off = config.replace("[shadow]\n", "[shadow]\nenabled = false\n");
    pila.write_config(&off);
    pila.wait_pixel(640.0, 505.0, WHITE);
    pila.write_config(&config);
    pila.wait_pixel_is(640.0, 505.0, "a shadow", dark);
    // Another color, on a window that's otherwise still.
    pila.write_config(&config.replace("color = \"#000000ff\"", "color = \"#ff0000ff\""));
    pila.wait_pixel_is(640.0, 502.0, "a red shadow", |[r, g, ..]| r > 200 && g < 60);
}

const COLORS: &str = "[colors]\nbar = \"#102030\"\nbar-inactive = \"#203040\"\n\
    icon = \"#c0c000\"\nicon-inactive = \"#00c0c0\"\nhover = \"#405060\"\n\
    hover-icon = \"#e000e0\"\nclose-hover = \"#607080\"\n\
    close-hover-icon = \"#00e000\"\nbackground = \"#804020\"\n\
    snap-preview = \"#ff000080\"\n";

#[test]
fn colors_are_drawn() {
    let mut pila = TestPila::with_config(COLORS);
    let client = pila.client();
    client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    pila.settle();
    let (x, y, w, _) = rect(&pila.window("a")["geometry"]);
    let bar_y = y - BAR / 2;
    // In the bar away from its title; in a corner of a button, away from its
    // icon; the middle of the close button's cross.
    let (plain, close_corner, maximize_corner, cross) = (
        ((x + 4) as f64, bar_y as f64),
        ((x + w - 3) as f64, (y - BAR + 3) as f64),
        ((x + w - 36 - 3) as f64, (y - BAR + 3) as f64),
        ((x + w - 18) as f64, bar_y as f64),
    );
    assert_eq!(pila.pixel(5.0, 795.0), [128, 64, 32, 255], "background");
    assert!(near(pila.pixel(plain.0, plain.1), [16, 32, 48, 255], 1));
    assert!(near(pila.pixel(cross.0, cross.1), [192, 192, 0, 255], 40));

    pila.move_to(close_corner.0, close_corner.1);
    pila.settle();
    let px = pila.pixel(close_corner.0, close_corner.1);
    assert!(near(px, [96, 112, 128, 255], 1), "close hover: {px:?}");
    assert!(near(pila.pixel(cross.0, cross.1), [0, 224, 0, 255], 40));

    pila.move_to(maximize_corner.0, maximize_corner.1);
    pila.settle();
    let px = pila.pixel(maximize_corner.0, maximize_corner.1);
    assert!(near(px, [64, 80, 96, 255], 1), "hover: {px:?}");

    pila.click_at(5.0, 795.0);
    pila.wait("unfocused", |s| s["focused"].is_null());
    pila.settle();
    assert!(near(pila.pixel(plain.0, plain.1), [32, 48, 64, 255], 1));
    assert!(near(pila.pixel(cross.0, cross.1), [0, 192, 192, 255], 40));
}

#[test]
fn the_snap_preview_has_its_color() {
    let mut pila = TestPila::with_config(COLORS);
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
    // Half red over the background.
    let px = pila.pixel(300.0, 700.0);
    assert!(near(px, [192, 32, 16, 255], 2), "{px:?}");
    pila.release("left");
    pila.key_release("Super");
}

#[test]
fn a_reload_recolors_whats_already_drawn() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client
        .window("a")
        .size(400, 300)
        .decorations(Decorations::Server)
        .create();
    pila.wait_focus("a");
    pila.settle();
    let (x, y, w, _) = rect(&pila.window("a")["geometry"]);
    let bar_y = (y - BAR / 2) as f64;
    let cross = ((x + w - 18) as f64, bar_y);
    let before = pila.pixel(cross.0, cross.1);
    assert!(!near(before, [192, 192, 0, 255], 40), "{before:?}");
    pila.write_config(COLORS);
    pila.wait_pixel_is((x + 4) as f64, bar_y, "the new bar", |px| {
        near(px, [16, 32, 48, 255], 1)
    });
    let px = pila.pixel(cross.0, cross.1);
    assert!(near(px, [192, 192, 0, 255], 40), "the button: {px:?}");
    assert_eq!(pila.pixel(5.0, 795.0), [128, 64, 32, 255]);
}

/// A window as wide as clients may make one (`surface_limits`'s 32768 px, past
/// any screen): a subsurface at the far end stretches the bounding box
/// Smithay clamps the geometry to. Its title bar is drawn no wider than any
/// screen, not as a texture as wide as the window.
#[test]
fn a_window_as_wide_as_the_limit_is_drawn() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("a").color(RED).create();
    pila.wait_focus("a");
    let wide = 32768;
    client.subsurface(&client.surface(window), "sub", (wide - 1, 10), (1, 1), RED);
    client.with_window(window, |w| {
        w.xdg_surface.set_window_geometry(0, 0, wide, 100);
    });
    client.draw(window, None);
    pila.wait("the wide geometry", |s| {
        window_in(s, "a").unwrap()["geometry"]["w"] == wide
    });
    // Frames go on being drawn, the title bar's among them.
    pila.settle();
    let _ = pila.pixel(10.0, 10.0);
    assert!(pila.alive());
    assert_eq!(pila.window("a")["title_bar"], true);
}
