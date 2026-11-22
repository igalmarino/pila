//! The desktop portal's ScreenCast, served by Pila (`portal.rs`,
//! `screencast.rs`): called over a private session bus as
//! xdg-desktop-portal calls it, with the frames taken from a private
//! PipeWire daemon as an app sharing the screen takes them.

mod common;

use common::pipewire::{Daemon, Portal};
use common::*;

const BLUE: [u8; 4] = [255, 30, 60, 200];
const GREEN: [u8; 4] = [255, 40, 200, 60];

/// Pila on a bus and a PipeWire daemon of the test's own, with one
/// 640x480 output.
fn start() -> (TestPila, notifications::Bus, Daemon) {
    let bus = notifications::Bus::start();
    let pipewire = Daemon::start();
    let pila = TestPila::start_with(Options {
        outputs: Some("HEADLESS-1:640x480".into()),
        env: vec![bus.env(), pipewire.env()],
        ..Default::default()
    });
    (pila, bus, pipewire)
}

/// The center of `app_id`'s window, in the output's pixels.
fn window_center(pila: &mut TestPila, app_id: &str) -> (u32, u32) {
    let (x, y, w, h) = rect(&pila.wait_window(app_id)["geometry"]);
    ((x + w / 2) as u32, (y + h / 2) as u32)
}

#[test]
fn a_shared_output_streams_what_it_shows_and_its_changes() {
    let (mut pila, bus, pipewire) = start();
    let client = pila.client();
    let window = client.window("shared").size(200, 150).color(BLUE).create();
    let (x, y) = window_center(&mut pila, "shared");
    pila.settle();
    let expected = pila.pixel(x as f64, y as f64);

    let mut portal = Portal::connect(&bus.address);
    let (response, nodes) = portal.share("s1", 1);
    assert_eq!(response, 0, "Start");
    assert_eq!(nodes.len(), 1, "one stream");
    let consumer = pipewire.consume(nodes[0]);

    let frame = consumer.wait_frame("of the output", |f| f.width == 640 && f.height == 480);
    assert_eq!(frame.rgba(x, y), expected, "the window in the first frame");

    // A change on the screen sends a frame with it.
    client.with_window(window, |w| w.color = GREEN);
    client.draw(window, None);
    pila.settle();
    let expected = pila.pixel(x as f64, y as f64);
    assert_ne!(frame.rgba(x, y), expected, "the window changed color");
    consumer.wait_frame("with the window's new color", |f| f.rgba(x, y) == expected);
}

#[test]
fn a_still_screen_sends_no_more_frames() {
    let (mut pila, bus, pipewire) = start();
    let mut portal = Portal::connect(&bus.address);
    let (_, nodes) = portal.share("still", 1);
    let consumer = pipewire.consume(nodes[0]);
    consumer.wait_frame("the first", |_| true);
    pila.settle();
    let _ = consumer.pending();
    pila.stays(
        "no new frame",
        std::time::Duration::from_millis(300),
        |_| true,
    );
    assert!(
        consumer.pending().is_empty(),
        "frames of a screen that didn't change"
    );
}

#[test]
fn closing_the_session_stops_its_stream() {
    let (mut pila, bus, pipewire) = start();
    let mut portal = Portal::connect(&bus.address);
    let (_, nodes) = portal.share("closed", 1);
    let consumer = pipewire.consume(nodes[0]);
    consumer.wait_frame("the first", |_| true);
    portal.close("closed");
    pila.wait_log("portal: disconnected from PipeWire");
}
