//! Limits on the sizes and places a client gives its surfaces: past
//! `surface_limits::MAX_EXTENT` (32768 logical px) the client gets a protocol
//! error, and Pila never works with the numbers (adding a position to a size
//! near `i32::MAX` overflows all over the layout and drawing code).

mod common;

use common::protocols::{LayerSpec, Placement};
use common::*;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::Anchor;

const RED: [u8; 4] = [255, 255, 0, 0];

/// The client got a protocol error for `what` past the limit, and Pila goes
/// on.
#[track_caller]
fn refused(pila: &mut TestPila, client: &Client, what: &str) {
    let err = client.wait_error();
    assert!(
        err.contains("Protocol error 2 on object wl_surface"),
        "{err}"
    );
    pila.wait_log(&format!("{what} "));
    pila.wait_log("is past the limit of 32768 px");
    assert!(pila.alive());
    let other = pila.client();
    other.window("b").create();
    pila.wait_window("b");
}

#[test]
fn a_window_geometry_past_the_limit_is_an_error() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("a").color(RED).create();
    pila.wait_focus("a");
    client.expect_error();
    client.with_window(window, |w| {
        w.xdg_surface.set_window_geometry(0, 0, 40_000, 100)
    });
    client.surface(window).commit();
    client.flush();
    refused(&mut pila, &client, "a window geometry of");
}

#[test]
fn a_subsurface_placed_past_the_limit_is_an_error() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("a").color(RED).create();
    pila.wait_focus("a");
    client.expect_error();
    // Its own commit (a desynchronized subsurface) applies the position.
    client.subsurface(
        &client.surface(window),
        "sub",
        (i32::MAX - 10, 10),
        (1, 1),
        RED,
    );
    refused(&mut pila, &client, "a subsurface at");
}

/// Each subsurface is within the limit of its parent, but together they
/// place the third one past it.
#[test]
fn nested_subsurfaces_cant_add_up_past_the_limit() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("a").color(RED).create();
    pila.wait_focus("a");
    let mut parent = client.surface(window);
    for i in 0..3 {
        if i == 2 {
            client.expect_error();
        }
        let (surf, _) = client.subsurface(&parent, &format!("sub{i}"), (16_000, 0), (1, 1), RED);
        parent = client.surface_data(surf, |s| s.surface.clone());
        if i < 2 {
            client.roundtrip();
        }
    }
    refused(&mut pila, &client, "a subsurface at");
}

#[test]
fn a_buffer_past_the_limit_is_an_error() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("a").color(RED).create();
    pila.wait_focus("a");
    client.expect_error();
    client.draw(window, Some((40_000, 1)));
    refused(&mut pila, &client, "a buffer of");
}

#[test]
fn a_viewport_past_the_limit_is_an_error() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("a").color(RED).create();
    pila.wait_focus("a");
    let viewporter: WpViewporter = client.bind("wp_viewporter", 1);
    let surface = client.surface(window);
    client.expect_error();
    let viewport = viewporter.get_viewport(&surface, &client.qh, "viewport".into());
    viewport.set_destination(i32::MAX, 100);
    surface.commit();
    client.flush();
    refused(&mut pila, &client, "a viewport of");
}

#[test]
fn a_layer_surface_past_the_limit_is_an_error() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.expect_error();
    client.layer_surface(
        "huge",
        &LayerSpec {
            // Past i32::MAX, Smithay's own debug assertion would stop it
            // first (a negative `Size`), in debug builds only.
            size: (40_000, 30),
            margin: (i32::MAX, 0, 0, 0),
            exclusive_zone: i32::MAX,
            anchor: Anchor::Top,
            ..Default::default()
        },
        RED,
    );
    refused(&mut pila, &client, "a layer surface of");
}

#[test]
fn a_popup_placed_past_the_limit_is_an_error() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("a").color(RED).create();
    pila.wait_focus("a");
    client.expect_error();
    client.popup(
        "menu",
        Ok(window),
        &Placement {
            anchor_rect: (i32::MAX - 10, i32::MAX - 10, 1, 1),
            ..Default::default()
        },
        false,
        RED,
    );
    refused(&mut pila, &client, "an anchor rectangle of");
}

/// Input regions are often "everything" (`INT32_MAX` wide): not an error.
#[test]
fn an_input_region_past_the_limit_is_fine() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("a").color(RED).create();
    pila.wait_focus("a");
    let compositor = client.lock().compositor.clone().unwrap();
    let region = compositor.create_region(&client.qh, "wl_region".into());
    region.add(1, 1, i32::MAX, i32::MAX);
    let surface = client.surface(window);
    surface.set_input_region(Some(&region));
    surface.set_opaque_region(Some(&region));
    surface.commit();
    client.flush();
    client.roundtrip();
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    pila.move_to((x + 50) as f64, (y + 50) as f64);
    client.wait("the pointer on the window", |s| s.pointer_name.is_some());
    pila.settle();
    assert!(pila.alive());
}

// --- Other requests carrying a client's numbers -------------------------------

/// The region of a screencopy is a client's rectangle: a negative size or a
/// place past the limit fails the frame (wlr-screencopy has no error for it).
#[test]
fn a_screencopy_region_past_the_limit_fails_the_frame() {
    use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1;
    let mut pila = TestPila::start();
    let client = pila.client();
    let manager: ZwlrScreencopyManagerV1 = client.bind("zwlr_screencopy_manager_v1", 3);
    let output = client.output("HEADLESS-1");
    for (i, (x, y, w, h)) in [
        (10, 10, -5, 100),
        (i32::MAX, 0, 100, 100),
        (0, 0, i32::MAX, i32::MAX),
    ]
    .into_iter()
    .enumerate()
    {
        let tag = format!("copy{i}");
        manager.capture_output_region(0, &output, x, y, w, h, &client.qh, tag.clone());
        client.flush();
        client.wait_logged(&tag, "Failed");
    }
    pila.settle();
    assert!(pila.alive());
}

/// A cursor's hotspot, set with the cursor or moved by its buffer's
/// offsets (which add up over commits), is a client's point: far off, Pila
/// goes on drawing (Smithay's point arithmetic saturates).
#[test]
fn a_cursor_hotspot_far_off_is_drawn_without_overflow() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    pila.move_to((x + 50) as f64, (y + 50) as f64);
    client.wait("pointer enter", |s| s.pointer_name.as_deref() == Some("a"));
    let serial = client.lock().pointer_enter_serial;
    let cursor = client.painted_surface([255, 255, 255, 255], (16, 16));
    let pointer = client.lock().pointer.clone().expect("a pointer");
    pointer.set_cursor(serial, Some(&cursor), i32::MAX, i32::MIN);
    client.flush();
    pila.wait("a surface cursor", |s| s["pointer"]["cursor"] == "surface");
    pila.move_by(5.0, 5.0);
    pila.settle();
    // Offsets add up over commits.
    pointer.set_cursor(serial, Some(&cursor), 0, 0);
    for i in 0..70_000 {
        cursor.offset(-32768, 32768);
        cursor.commit();
        if i % 1000 == 0 {
            client.roundtrip();
        }
    }
    client.roundtrip();
    pila.move_by(5.0, 5.0);
    pila.settle();
    assert!(pila.alive());
}

/// Where an app asks for its window menu is its point: far off, the menu
/// stays on the output.
#[test]
fn a_window_menu_asked_for_far_off_stays_on_the_output() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client
        .window("a")
        .size(400, 300)
        .decorations(common::client::Decorations::Client)
        .create();
    let state = pila.wait_focus("a");
    let (x, y, ..) = rect(&window_in(&state, "a").unwrap()["geometry"]);
    pila.move_to(x as f64 + 50.0, y as f64 + 10.0);
    pila.press("right");
    client.wait("the press", |s| {
        s.events
            .iter()
            .any(|e| matches!(e, common::client::Ev::Button { pressed: true, .. }))
    });
    let serial = client.last_serial();
    let seat = client.seat();
    client.request(a, |t| t.show_window_menu(&seat, serial, i32::MAX, i32::MAX));
    let state = pila.wait("the menu", |s| !s["window_menu"].is_null());
    let (mx, my, mw, mh) = rect(&state["window_menu"]["geometry"]);
    assert!(
        mx >= 0 && my >= 0 && mx + mw <= 1280 && my + mh <= 800,
        "{state}"
    );
    pila.release("right");
    assert!(pila.alive());
}

/// A drag icon moves by its buffer's offsets, which add up over commits:
/// far off, Pila goes on drawing (Smithay's point arithmetic saturates).
#[test]
fn a_drag_icon_offset_adding_up_far_off_is_drawn_without_overflow() {
    use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("source").size(400, 300).create();
    let state = pila.wait_focus("source");
    let (x, y, ..) = rect(&window_in(&state, "source").unwrap()["geometry"]);
    pila.move_to(x as f64 + 50.0, y as f64 + 50.0);
    pila.press("left");
    client.wait("the press", |s| {
        s.events
            .iter()
            .any(|e| matches!(e, common::client::Ev::Button { pressed: true, .. }))
    });
    let source = client.data_source("drag", "dragged text");
    source.set_actions(DndAction::Copy);
    let manager: WlDataDeviceManager = client.bind("wl_data_device_manager", 3);
    let device = manager.get_data_device(&client.seat(), &client.qh, "src".into());
    let icon = client.painted_surface(RED, (20, 20));
    device.start_drag(
        Some(&source),
        &client.surface(window),
        Some(&icon),
        client.last_serial(),
    );
    client.flush();
    for i in 0..70_000 {
        icon.offset(32768, -32768);
        icon.commit();
        if i % 1000 == 0 {
            client.roundtrip();
        }
    }
    client.roundtrip();
    pila.move_by(10.0, 10.0);
    pila.settle();
    pila.release("left");
    assert!(pila.alive());
}

/// Damage with a negative width or height (found by `fuzz/`'s protocol
/// target): Smithay's wl_surface handler makes a `Size` of it, whose
/// `debug_assert` crashes a debug build of Pila (release builds go on), before
/// Pila sees the request. A fix is Smithay's.
#[test]
#[ignore = "bug: Smithay's wl_surface.damage debug-asserts a negative size"]
fn damage_of_a_negative_size_harms_nothing() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("a").color(RED).create();
    pila.wait_focus("a");
    let surface = client.surface(window);
    surface.damage(0, 0, 10, -10);
    surface.damage_buffer(0, 0, -10, 10);
    surface.commit();
    client.roundtrip();
    assert!(pila.alive());
}
