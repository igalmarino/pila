//! The desktop portal's ScreenCast, served by Pila (`portal.rs`,
//! `screencast.rs`) and its picker (`share_picker.rs`): called over a private
//! session bus as xdg-desktop-portal calls it, with the frames taken from a
//! private PipeWire daemon as an app sharing the screen takes them.

mod common;

use common::pipewire::{Daemon, Portal, Started};
use common::*;
use serde_json::Value;

const BLUE: [u8; 4] = [255, 30, 60, 200];
const GREEN: [u8; 4] = [255, 40, 200, 60];

/// Pila on a bus and a PipeWire daemon of the test's own, with `outputs`;
/// apps find their desktop entries in the test's own data directory (one,
/// `org.example.Browser`, named "Web Browser").
fn start_with(outputs: &str) -> (TestPila, notifications::Bus, Daemon) {
    let bus = notifications::Bus::start();
    let pipewire = Daemon::start();
    let data = std::env::temp_dir().join(format!(
        "pila-portal-data-{}-{}",
        std::process::id(),
        outputs.len() + rand_suffix()
    ));
    std::fs::create_dir_all(data.join("applications")).unwrap();
    std::fs::write(
        data.join("applications/org.example.Browser.desktop"),
        "[Desktop Entry]\nName=Web Browser\nExec=browser\n",
    )
    .unwrap();
    let pila = TestPila::start_with(Options {
        outputs: Some(outputs.into()),
        env: vec![
            bus.env(),
            pipewire.env(),
            ("XDG_DATA_DIRS".into(), data.to_string_lossy().into_owned()),
        ],
        ..Default::default()
    });
    (pila, bus, pipewire)
}

fn rand_suffix() -> usize {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    NEXT.fetch_add(1, Ordering::SeqCst)
}

fn start() -> (TestPila, notifications::Bus, Daemon) {
    start_with("HEADLESS-1:640x480")
}

/// Wait for the picker to be shown, with its text drawn.
fn wait_picker(pila: &mut TestPila) -> Value {
    pila.wait("the share picker", |s| {
        s["share_picker"]["shown"] == true && s["share_picker"]["text"] == true
    })["share_picker"]
        .clone()
}

/// Ask to share, and share what the picker selects first (Enter): the
/// stream's node.
fn share(pila: &mut TestPila, portal: &mut Portal, name: &str) -> u32 {
    share_with_cursor(pila, portal, name, 1)
}

/// `share`, with the pointer hidden (1), drawn into the frames (2) or sent
/// as metadata (4).
fn share_with_cursor(pila: &mut TestPila, portal: &mut Portal, name: &str, cursor: u32) -> u32 {
    let started = portal.request(name, "org.example.Browser", cursor);
    wait_picker(pila);
    pila.key("Return");
    let (response, nodes) = started.answer();
    assert_eq!(response, 0, "Start");
    assert_eq!(nodes.len(), 1, "one stream");
    nodes[0]
}

/// The center of `app_id`'s window, in the output's pixels.
fn window_center(pila: &mut TestPila, app_id: &str) -> (u32, u32) {
    let (x, y, w, h) = rect(&pila.wait_window(app_id)["geometry"]);
    ((x + w / 2) as u32, (y + h / 2) as u32)
}

fn center(rect: &Value) -> (f64, f64) {
    let (x, y, w, h) = common::rect(rect);
    ((x + w / 2) as f64, (y + h / 2) as f64)
}

/// A client's color (ARGB) as a pixel reads (RGBA).
fn rgba([a, r, g, b]: [u8; 4]) -> [u8; 4] {
    [r, g, b, a]
}

fn answered_with(started: Started) -> u32 {
    started.answer().0
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
    let node = share(&mut pila, &mut portal, "s1");
    let consumer = pipewire.consume(node);

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

/// A window "app" with the pointer over it, which sets a `color` cursor
/// surface 16 px square with its hotspot at the top left: the pointer, in
/// the output's pixels.
fn pointer_with_a_cursor_surface(
    pila: &mut TestPila,
    client: &Client,
    color: [u8; 4],
) -> (u32, u32) {
    client.window("app").size(600, 440).color(BLUE).create();
    pila.wait_focus("app");
    let (x, y, ..) = rect(&pila.window("app")["geometry"]);
    let (px, py) = ((x + 200) as f64, (y + 150) as f64);
    pila.move_to(px, py);
    client.wait("pointer enter", |s| {
        s.pointer_name.as_deref() == Some("app")
    });
    let serial = client.lock().pointer_enter_serial;
    let compositor = client.lock().compositor.clone().unwrap();
    let surface = compositor.create_surface(&client.qh, ());
    let shm = client.lock().shm.clone().unwrap();
    common::client::paint(&shm, &client.qh, &surface, color, (16, 16));
    let pointer = client.lock().pointer.clone().expect("a pointer");
    pointer.set_cursor(serial, Some(&surface), 0, 0);
    client.flush();
    pila.wait("a surface cursor", |s| s["pointer"]["cursor"] == "surface");
    pila.wait_pixel(px + 8.0, py + 8.0, rgba(color));
    (px as u32, py as u32)
}

#[test]
fn the_pointer_shows_as_the_app_asks() {
    const RED: [u8; 4] = [255, 220, 40, 40];
    let (mut pila, bus, pipewire) = start();
    let client = pila.client();
    let (px, py) = pointer_with_a_cursor_surface(&mut pila, &client, RED);
    let (x, y) = (px + 8, py + 8);
    let mut portal = Portal::connect(&bus.address);

    // Drawn into the frames.
    let node = share_with_cursor(&mut pila, &mut portal, "drawn", 2);
    let consumer = pipewire.consume(node);
    let frame = consumer.wait_frame("the first", |_| true);
    assert_eq!(
        frame.rgba(x, y),
        rgba(RED),
        "the pointer drawn into the frame"
    );
    drop(consumer);
    portal.close("drawn");

    // Hidden.
    let node = share_with_cursor(&mut pila, &mut portal, "hidden", 1);
    let consumer = pipewire.consume(node);
    let frame = consumer.wait_frame("the first", |_| true);
    assert_eq!(frame.rgba(x, y), rgba(BLUE), "no pointer when hidden");
    drop(consumer);
    portal.close("hidden");

    // Beside the frames, for the app to draw: its place and its image.
    let node = share_with_cursor(&mut pila, &mut portal, "metadata", 4);
    let consumer = pipewire.consume(node);
    let frame = consumer.wait_frame("the first", |f| f.cursor.is_some());
    assert_eq!(frame.rgba(x, y), rgba(BLUE), "not drawn into the frame");
    let cursor = frame.cursor.unwrap();
    assert_eq!(cursor.position, (px as i32, py as i32));
    assert_eq!(cursor.hotspot, (0, 0));
    let (w, h, pixels) = cursor.bitmap.expect("its image with the first frame");
    assert_eq!((w, h), (16, 16));
    let [a, r, g, b] = RED;
    assert_eq!(pixels[..4], [b, g, r, a], "BGRA");
    // Moved: the pointer alone, no new picture, no new image.
    pila.move_to(f64::from(px) + 10.0, f64::from(py) + 5.0);
    let moved = consumer.wait_buffer("the pointer moved", |f| {
        f.cursor
            .as_ref()
            .is_some_and(|c| c.position == (px as i32 + 10, py as i32 + 5))
    });
    assert!(moved.data.is_empty(), "a picture for the pointer alone");
    assert_eq!(moved.cursor.unwrap().bitmap, None, "the same image again");
}

#[test]
fn only_a_share_in_shared_memory_has_a_read_back_texture() {
    let (mut pila, bus, pipewire) = start();
    pila.settle();
    assert_eq!(pila.state()["capture"]["textures"], 0);
    let mut portal = Portal::connect(&bus.address);
    // Shared memory: frames are read back through a texture, made once the
    // buffers are negotiated (ahead of the first frame).
    let node = share(&mut pila, &mut portal, "shm");
    let consumer = pipewire.consume(node);
    consumer.wait_frame("the first", |_| true);
    assert_eq!(
        pila.state()["capture"]["textures"],
        1,
        "a read-back texture"
    );
    drop(consumer);
    portal.close("shm");
    pila.wait("the texture gone with the share", |s| {
        s["capture"]["textures"] == 0
    });
    // Dma-bufs: drawn into, no texture.
    if pila.state()["capture"]["dmabuf"] != true {
        return;
    }
    let node = share(&mut pila, &mut portal, "dmabufs");
    let consumer = pipewire.consume_dmabufs(node);
    consumer.wait_frame("the first", |f| f.dmabuf);
    assert_eq!(
        pila.state()["capture"]["textures"],
        0,
        "no read-back texture"
    );
}

#[test]
fn pipewire_is_connected_from_the_start_and_stays() {
    let (mut pila, bus, pipewire) = start();
    // Connecting took 7 ms on bare metal: paid once, at startup, not as a
    // share starts.
    pila.wait("connected to PipeWire", |s| {
        s["screencast"]["pipewire"] == true
    });
    let mut portal = Portal::connect(&bus.address);
    let node = share(&mut pila, &mut portal, "first");
    let consumer = pipewire.consume(node);
    consumer.wait_frame("the first", |_| true);
    drop(consumer);
    portal.close("first");
    pila.wait("the share gone", |s| s["screencast"]["casts"] == 0);
    assert_eq!(
        pila.state()["screencast"]["pipewire"],
        true,
        "still connected"
    );
    assert_eq!(
        pila.log().matches("portal: connected to PipeWire").count(),
        1,
        "connected once"
    );
}

#[test]
fn an_app_that_takes_dmabufs_gets_the_frames_drawn_into_them() {
    let (mut pila, bus, pipewire) = start();
    if pila.state()["capture"]["dmabuf"] != true {
        eprintln!("no GPU to draw into dma-bufs: skipped");
        return;
    }
    let client = pila.client();
    let window = client.window("shared").size(200, 150).color(BLUE).create();
    let (x, y) = window_center(&mut pila, "shared");
    pila.settle();
    let mut portal = Portal::connect(&bus.address);
    let node = share(&mut pila, &mut portal, "dmabufs");
    let consumer = pipewire.consume_dmabufs(node);
    let frame = consumer.wait_frame("the first", |_| true);
    assert!(frame.dmabuf, "a dma-buf");
    assert_eq!((frame.width, frame.height), (640, 480));
    assert_eq!(frame.rgba(x, y), rgba(BLUE), "the window drawn into it");
    // And the next, of a change.
    client.with_window(window, |w| w.color = GREEN);
    client.draw(window, None);
    consumer.wait_frame("with the window's new color", |f| {
        f.dmabuf && f.rgba(x, y) == rgba(GREEN)
    });
}

#[test]
fn a_still_screen_sends_no_more_frames() {
    let (mut pila, bus, pipewire) = start();
    let mut portal = Portal::connect(&bus.address);
    let node = share(&mut pila, &mut portal, "still");
    let consumer = pipewire.consume(node);
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
    let node = share(&mut pila, &mut portal, "closed");
    let consumer = pipewire.consume(node);
    consumer.wait_frame("the first", |_| true);
    portal.close("closed");
    pila.wait("the stream gone", |s| s["screencast"]["casts"] == 0);
}

#[test]
fn the_picker_names_the_app_and_offers_every_output() {
    let (mut pila, bus, _pipewire) = start_with("A:640x480,B:800x600");
    pila.move_to(900.0, 300.0);
    let mut portal = Portal::connect(&bus.address);
    let started = portal.request("named", "org.example.Browser", 1);
    let picker = wait_picker(&mut pila);
    assert_eq!(picker["app"], "Web Browser", "its desktop entry's name");
    assert_eq!(
        picker["sources"],
        serde_json::json!(["A", "B"]),
        "left to right"
    );
    assert_eq!(picker["output"], "B", "on the output under the pointer");
    assert_eq!(picker["selected"], 1, "that output selected first");
    assert!(
        !started.answered(),
        "nothing shared before the user says so"
    );
    pila.key("Escape");
    assert_eq!(answered_with(started), 1, "cancelled");

    // An app with no desktop entry goes by its id; one with no id, unnamed.
    let started = portal.request("unknown", "org.example.Other", 1);
    assert_eq!(wait_picker(&mut pila)["app"], "org.example.Other");
    pila.key("Escape");
    started.answer();
    let started = portal.request("anonymous", "", 1);
    assert_eq!(wait_picker(&mut pila)["app"], Value::Null);
    pila.key("Escape");
    started.answer();
}

#[test]
fn escape_or_cancel_shares_nothing() {
    let (mut pila, bus, _pipewire) = start();
    let mut portal = Portal::connect(&bus.address);
    let started = portal.request("esc", "org.example.Browser", 1);
    wait_picker(&mut pila);
    pila.key("Escape");
    assert_eq!(answered_with(started), 1);
    pila.wait("the picker gone", |s| s["share_picker"]["shown"] == false);

    let started = portal.request("button", "org.example.Browser", 1);
    let picker = wait_picker(&mut pila);
    let (x, y) = center(&picker["cancel"]);
    pila.click_at(x, y);
    assert_eq!(answered_with(started), 1);
    pila.wait("the picker gone", |s| s["share_picker"]["shown"] == false);
    assert!(
        !pila.log().contains("portal: sharing an output"),
        "nothing was streamed"
    );
}

#[test]
fn the_output_picked_is_the_one_shared() {
    let (mut pila, bus, pipewire) = start_with("A:640x480,B:800x600");
    pila.move_to(100.0, 100.0);
    let mut portal = Portal::connect(&bus.address);

    // By the keys: from A to B, then Enter.
    let started = portal.request("keys", "org.example.Browser", 1);
    assert_eq!(wait_picker(&mut pila)["selected"], 0);
    pila.key("Right");
    pila.wait("B selected", |s| s["share_picker"]["selected"] == 1);
    pila.key("Return");
    let (response, nodes) = started.answer();
    assert_eq!(response, 0);
    let consumer = pipewire.consume(nodes[0]);
    consumer.wait_frame("of B", |f| (f.width, f.height) == (800, 600));
    drop(consumer);
    portal.close("keys");

    // By the pointer: a click on B's tile selects it, Share shares it.
    let started = portal.request("clicks", "org.example.Browser", 1);
    let picker = wait_picker(&mut pila);
    let (x, y) = center(&picker["tiles"][1]);
    pila.click_at(x, y);
    pila.wait("B selected", |s| s["share_picker"]["selected"] == 1);
    assert!(!started.answered(), "a tile only selects");
    let (x, y) = center(&picker["share"]);
    pila.click_at(x, y);
    let (response, nodes) = started.answer();
    assert_eq!(response, 0);
    let consumer = pipewire.consume(nodes[0]);
    consumer.wait_frame("of B", |f| (f.width, f.height) == (800, 600));
}

#[test]
fn nothing_reaches_the_apps_while_the_picker_is_shown() {
    let (mut pila, bus, _pipewire) = start();
    let client = pila.client();
    client.window("app").size(600, 440).create();
    pila.wait_focus("app");
    let mut portal = Portal::connect(&bus.address);
    let started = portal.request("modal", "org.example.Browser", 1);
    let picker = wait_picker(&mut pila);
    client.roundtrip();
    let (keys, buttons) = (client.keys().len(), client.buttons().len());
    pila.key("a");
    // Outside the panel, on the window.
    let (px, py, _, _) = rect(&picker["panel"]);
    pila.click_at((px - 5).max(1) as f64, (py - 5).max(1) as f64);
    pila.settle();
    client.roundtrip();
    assert_eq!(client.keys().len(), keys, "a key went to the app");
    assert_eq!(client.buttons().len(), buttons, "a click went to the app");
    assert_eq!(pila.state()["share_picker"]["shown"], true, "still asking");
    pila.key("Escape");
    started.answer();
}

#[test]
fn requests_wait_their_turn() {
    let (mut pila, bus, _pipewire) = start();
    let mut portal = Portal::connect(&bus.address);
    let first = portal.request("first", "org.example.Browser", 1);
    wait_picker(&mut pila);
    let second = portal.request("second", "org.example.Other", 1);
    pila.wait("the second waiting", |s| s["share_picker"]["waiting"] == 1);
    assert_eq!(pila.state()["share_picker"]["app"], "Web Browser");
    pila.key("Escape");
    assert_eq!(answered_with(first), 1);
    assert_eq!(wait_picker(&mut pila)["app"], "org.example.Other");
    pila.key("Escape");
    assert_eq!(answered_with(second), 1);
}

#[test]
fn the_app_closing_its_session_takes_the_picker_away() {
    let (mut pila, bus, _pipewire) = start();
    let mut portal = Portal::connect(&bus.address);
    let started = portal.request("gone", "org.example.Browser", 1);
    wait_picker(&mut pila);
    portal.close("gone");
    pila.wait("the picker gone", |s| s["share_picker"]["shown"] == false);
    assert_eq!(answered_with(started), 2, "failed: nobody picked");
}

#[test]
fn locking_cancels_the_picker() {
    let (mut pila, bus, _pipewire) = start();
    let mut portal = Portal::connect(&bus.address);
    let started = portal.request("locked", "org.example.Browser", 1);
    wait_picker(&mut pila);
    let locker = pila.client();
    let _lock = locker.lock_session();
    pila.wait("locked", |s| s["locked"] == true);
    assert_eq!(answered_with(started), 1, "cancelled");
    assert_eq!(pila.state()["share_picker"]["shown"], false);
    // And nothing is asked while locked.
    let started = portal.request("whilelocked", "org.example.Browser", 1);
    assert_eq!(answered_with(started), 1);
}

#[test]
fn the_picker_dims_the_screen_and_shows_a_live_preview() {
    let (mut pila, bus, _pipewire) = start_with("HEADLESS-1:1280x720");
    let client = pila.client();
    let window = client.window("shown").size(1280, 720).color(BLUE).create();
    pila.wait_window("shown");
    pila.settle();
    let corner = pila.pixel(2.0, 2.0);

    let mut portal = Portal::connect(&bus.address);
    let started = portal.request("look", "org.example.Browser", 1);
    let picker = wait_picker(&mut pila);
    pila.wait("the preview drawn", |s| s["share_picker"]["previews"] == 1);
    pila.settle();
    let dimmed = pila.pixel(2.0, 2.0);
    assert!(
        dimmed[0] < corner[0] && dimmed[1] < corner[1] && dimmed[2] < corner[2],
        "dimmed: {dimmed:?} from {corner:?}"
    );
    // The preview's middle shows the window.
    let (tx, ty) = center(&picker["tiles"][0]);
    pila.wait_pixel(tx, ty, rgba(BLUE));

    // It follows the window.
    client.with_window(window, |w| w.color = GREEN);
    client.draw(window, None);
    pila.wait_pixel(tx, ty, rgba(GREEN));

    pila.key("Escape");
    started.answer();
    pila.settle();
    assert_eq!(pila.pixel(2.0, 2.0), corner, "undimmed");
}

#[test]
fn startup_programs_wait_for_the_portal() {
    let bus = notifications::Bus::start();
    let pipewire = Daemon::start();
    let mut pila = TestPila::start_with(Options {
        config: "autostart = [\"true\"]\n".into(),
        env: vec![bus.env(), pipewire.env()],
        ..Default::default()
    });
    pila.wait_log("spawning");
    // And what PipeWire loads, off the event loop.
    pila.wait_log("portal: PipeWire warmed up");
    let log = pila.log();
    assert!(
        log.lines()
            .any(|l| l.contains("PipeWire warmed up") && l.contains("connected=true")),
        "PipeWire wasn't warmed up"
    );
    let serving = log.find("portal: serving ScreenCast");
    let spawning = log.find("spawning");
    assert!(
        serving.is_some() && serving < spawning,
        "a startup program ran before Pila served the portal"
    );
}
