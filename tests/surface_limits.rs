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
