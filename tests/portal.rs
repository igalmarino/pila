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
    // Turned: dma-bufs of the new size, a modifier fixed again.
    pila.write_config("[[output]]\nname = \"HEADLESS-1\"\ntransform = \"90\"\n");
    consumer.wait_frame("turned", |f| f.dmabuf && (f.width, f.height) == (480, 640));
}

#[test]
fn a_share_follows_its_output_turned_or_scaled() {
    let (mut pila, bus, pipewire) = start();
    let mut portal = Portal::connect(&bus.address);
    let node = share(&mut pila, &mut portal, "follows");
    let consumer = pipewire.consume(node);
    consumer.wait_frame("the first", |f| (f.width, f.height) == (640, 480));

    // Turned a quarter: the stream's size is negotiated again, not ended.
    pila.write_config("[[output]]\nname = \"HEADLESS-1\"\ntransform = \"90\"\n");
    consumer.wait_frame("turned", |f| (f.width, f.height) == (480, 640));
    assert_eq!(pila.state()["screencast"]["casts"], 1, "still sharing");

    // Scaled: the same pixels, frames go on.
    pila.write_config("[[output]]\nname = \"HEADLESS-1\"\ntransform = \"90\"\nscale = 2\n");
    pila.wait("the new scale", |s| s["outputs"][0]["scale"] == 2.0);
    let _ = consumer.pending();
    let client = pila.client();
    client.window("after").size(100, 100).color(BLUE).create();
    pila.wait_window("after");
    consumer.wait_frame("after the scale changed", |f| {
        (f.width, f.height) == (480, 640)
    });
    assert_eq!(pila.state()["screencast"]["casts"], 1, "still sharing");
}

#[test]
fn the_picker_looks_like_pilas_other_panels() {
    // The accent, opaque: `[colors] snap-preview` ("#4477cc4d") as the
    // switcher's selected tile has it.
    const ACCENT: [u8; 4] = [0x44, 0x77, 0xcc, 255];
    let (mut pila, bus, _pipewire) = start_with("HEADLESS-1:1280x720");
    let mut portal = Portal::connect(&bus.address);
    let started = portal.request("look", "org.example.Browser", 1);
    let picker = wait_picker(&mut pila);
    pila.settle();
    // The selected tile on a block of the accent, as the switcher's.
    let (tx, ty, _, th) = rect(&picker["tiles"][0]);
    assert_eq!(
        pila.pixel((tx - 3) as f64, (ty + th / 2) as f64),
        ACCENT,
        "the selection's margin"
    );
    // The buttons with rounded corners, as the cheatsheet's keys.
    for button in ["share", "cancel"] {
        let (x, y, w, h) = rect(&picker[button]);
        let middle = pila.pixel((x + w / 2) as f64, (y + h - 2) as f64);
        assert_ne!(
            pila.pixel(x as f64, y as f64),
            middle,
            "{button}'s corner: square"
        );
    }
    pila.key("Escape");
    started.answer();
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

/// What `Start` says of a stream: its `source_type`, its `size`, and
/// whether it has a `position` (a monitor's only).
fn described(properties: &common::pipewire::Vardict) -> (u32, (i32, i32), bool) {
    let source_type = properties
        .get("source_type")
        .map(|v| u32::try_from(v).unwrap())
        .unwrap();
    let size: (i32, i32) = properties
        .get("size")
        .map(|v| v.try_clone().unwrap().try_into().unwrap())
        .unwrap();
    (source_type, size, properties.contains_key("position"))
}

/// The picker's tile showing source `index`, and its center.
fn tile_of(picker: &Value, index: usize) -> (f64, f64) {
    let slot = picker["tile_sources"]
        .as_array()
        .unwrap()
        .iter()
        .position(|i| i == index)
        .unwrap_or_else(|| panic!("source {index} not shown: {picker}"));
    center(&picker["tiles"][slot])
}

/// An app that may share windows too sees them in a section of their own,
/// most recently used first; the one picked is streamed alone, at its size,
/// as a window (no position) in `Start`'s answer.
#[test]
fn an_app_can_share_a_window() {
    let (mut pila, bus, pipewire) = start_with("HEADLESS-1:1280x720");
    let client = pila.client();
    client.window("older").size(300, 200).color(GREEN).create();
    pila.wait_focus("older");
    client.window("newer").size(240, 160).color(BLUE).create();
    pila.wait_focus("newer");
    pila.settle();
    let mut portal = Portal::connect(&bus.address);
    let started = portal.request_sources("w1", "org.example.Browser", 1, 3);
    let picker = wait_picker(&mut pila);
    assert_eq!(
        picker["kinds"],
        serde_json::json!(["output", "window", "window"])
    );
    assert_eq!(
        picker["sources"],
        serde_json::json!(["HEADLESS-1", "newer", "older"])
    );
    assert_eq!(
        picker["headings"],
        serde_json::json!(["Screens", "Windows"])
    );
    assert_eq!(picker["selected"], 0, "the output under the pointer first");
    // Its preview drawn.
    pila.wait("the windows' previews", |s| {
        s["share_picker"]["window_previews"] == 2
    });

    let (x, y) = tile_of(&picker, 2);
    pila.click_at(x, y);
    pila.wait("older selected", |s| s["share_picker"]["selected"] == 2);
    pila.key("Return");
    let (response, streams) = started.answer_streams();
    assert_eq!(response, 0, "Start");
    let (node, properties) = &streams[0];
    assert_eq!(
        described(properties),
        (2, (300, 200), false),
        "a window's stream"
    );

    let consumer = pipewire.consume(*node);
    let frame = consumer.wait_frame("of the window", |f| f.width == 300 && f.height == 200);
    assert_eq!(
        frame.rgba(150, 100),
        rgba(GREEN),
        "the window alone, under the other"
    );
}

/// An app that may share only windows sees them alone, the one used last
/// selected first.
#[test]
fn an_app_that_shares_only_windows_sees_only_windows() {
    let (mut pila, bus, _pipewire) = start();
    let client = pila.client();
    client.window("a").size(200, 150).create();
    pila.wait_focus("a");
    let mut portal = Portal::connect(&bus.address);
    let started = portal.request_sources("w2", "org.example.Browser", 1, 2);
    let picker = wait_picker(&mut pila);
    assert_eq!(picker["kinds"], serde_json::json!(["window"]));
    assert_eq!(picker["headings"], serde_json::json!([]));
    assert_eq!(picker["selected"], 0);
    pila.key("Escape");
    assert_eq!(started.answer().0, 1, "cancelled");
}

/// A shared window keeps streaming while minimized, drawing for the share;
/// resized, the stream follows its size; closed, the share ends.
#[test]
fn a_shared_window_follows_it_until_it_closes() {
    let (mut pila, bus, pipewire) = start_with("HEADLESS-1:1280x720");
    let client = pila.client();
    let window = client.window("shared").size(300, 200).color(GREEN).create();
    pila.wait_focus("shared");
    pila.settle();
    let mut portal = Portal::connect(&bus.address);
    let started = portal.request_sources("w3", "org.example.Browser", 1, 2);
    wait_picker(&mut pila);
    pila.key("Return");
    let (response, nodes) = started.answer();
    assert_eq!(response, 0, "Start");
    let consumer = pipewire.consume(nodes[0]);
    consumer.wait_frame("of the window", |f| f.rgba(150, 100) == rgba(GREEN));

    // Minimized, it still draws for the share.
    client.request(window, |t| t.set_minimized());
    pila.wait("minimized", |s| {
        window_in(s, "shared").unwrap()["minimized"] == true
    });
    client.with_window(window, |w| w.color = BLUE);
    client.draw(window, None);
    consumer.wait_frame("with its new color", |f| f.rgba(150, 100) == rgba(BLUE));
    // An app drawing on frame callbacks keeps drawing: off the screen it
    // gets none, but the share's frames bring them.
    let surface = client.surface(window);
    for n in 0..3 {
        let callback = format!("callback{n}");
        let _callback = surface.frame(&client.qh, callback.clone());
        client.draw(window, None);
        client.wait_logged(&callback, "Done");
    }

    // A new size: the stream's.
    client.draw(window, Some((400, 250)));
    consumer.wait_frame("at its new size", |f| f.width == 400 && f.height == 250);

    // Closed: the share is over.
    client.destroy(window);
    pila.wait("the share over", |s| s["screencast"]["casts"] == 0);
}

/// In a window's stream the pointer's place is in the window's pixels, and
/// it's hidden while the pointer is off the window.
#[test]
fn a_shared_windows_pointer_is_where_it_is_over_it() {
    let (mut pila, bus, pipewire) = start_with("HEADLESS-1:1280x720");
    let client = pila.client();
    client.window("app").size(300, 200).color(BLUE).create();
    pila.wait_focus("app");
    let (x, y, ..) = rect(&pila.window("app")["geometry"]);
    pila.move_to((x + 50) as f64, (y + 40) as f64);
    pila.settle();
    let mut portal = Portal::connect(&bus.address);
    let started = portal.request_sources("w4", "org.example.Browser", 4, 2);
    wait_picker(&mut pila);
    pila.key("Return");
    let (response, nodes) = started.answer();
    assert_eq!(response, 0, "Start");
    let consumer = pipewire.consume(nodes[0]);
    let frame = consumer.wait_frame("with the pointer", |f| f.cursor.is_some());
    let cursor = frame.cursor.unwrap();
    assert_eq!(cursor.position, (50, 40), "in the window's pixels");
    assert_ne!(cursor.bitmap, Some((1, 1, vec![0; 4])), "shown");

    // Off the window: hidden (an empty image).
    pila.move_to((x - 30) as f64, (y + 40) as f64);
    consumer.wait_buffer("the pointer hidden", |f| {
        f.cursor
            .as_ref()
            .is_some_and(|c| c.bitmap == Some((1, 1, vec![0; 4])))
    });
}

/// A shared window's contents stay private while the screen is locked: its
/// changes go out once it's unlocked.
#[test]
fn a_shared_window_sends_nothing_while_locked() {
    let (mut pila, bus, pipewire) = start();
    let client = pila.client();
    let window = client.window("shared").size(200, 150).color(GREEN).create();
    pila.wait_focus("shared");
    pila.settle();
    let mut portal = Portal::connect(&bus.address);
    let started = portal.request_sources("w5", "org.example.Browser", 1, 2);
    wait_picker(&mut pila);
    pila.key("Return");
    let (response, nodes) = started.answer();
    assert_eq!(response, 0, "Start");
    let consumer = pipewire.consume(nodes[0]);
    consumer.wait_frame("of the window", |f| f.rgba(100, 75) == rgba(GREEN));

    let locker = pila.client();
    let lock = locker.lock_session();
    locker.lock_surface(&lock, "HEADLESS-1", [255, 0, 0, 0]);
    locker.wait_logged("lock", "Locked");
    client.with_window(window, |w| w.color = BLUE);
    client.draw(window, None);
    pila.stays(
        "nothing sent",
        std::time::Duration::from_millis(300),
        |_| true,
    );
    assert!(
        consumer
            .pending()
            .iter()
            .all(|f| f.data.is_empty() || f.rgba(100, 75) != rgba(BLUE)),
        "the window's change sent while locked"
    );
    lock.unlock_and_destroy();
    locker.flush();
    pila.wait("unlocked", |s| s["locked"] == false);
    consumer.wait_frame("with its change", |f| f.rgba(100, 75) == rgba(BLUE));
}

/// The PNG a screenshot's URI names: its size and RGBA pixels.
fn screenshot_png(uri: &str) -> ((u32, u32), Vec<u8>) {
    let path = uri.strip_prefix("file://").expect("a file URI");
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut pixels).unwrap();
    assert_eq!(info.color_type, png::ColorType::Rgba);
    ((info.width, info.height), pixels)
}

fn png_pixel(((width, _), pixels): &((u32, u32), Vec<u8>), x: u32, y: u32) -> [u8; 4] {
    let i = ((y * width + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

/// An app its permission store lets take one unasked gets every output at
/// once, as it shows, without a question.
#[test]
fn a_screenshot_taken_unasked_is_the_whole_screen() {
    let (mut pila, bus, _pipewire) = start();
    let client = pila.client();
    client.window("shot").size(200, 150).color(BLUE).create();
    let (x, y) = window_center(&mut pila, "shot");
    pila.settle();
    let expected = pila.pixel(x as f64, y as f64);
    let mut portal = Portal::connect(&bus.address);
    let (response, uri) = portal
        .screenshot("org.example.Browser", false, true)
        .join()
        .unwrap();
    assert_eq!(response, 0, "Screenshot");
    assert_eq!(
        pila.state()["share_picker"]["shown"],
        false,
        "nothing asked"
    );
    let png = screenshot_png(&uri.expect("its URI"));
    assert_eq!(png.0, (640, 480), "the output's size");
    assert_eq!(png_pixel(&png, x, y), expected, "the window in it");
}

/// An app that wants to choose, or that its permission store doesn't let
/// take one unasked, gets the picker: what's picked is taken (a window
/// alone, at its size); Escape takes nothing.
#[test]
fn a_screenshot_of_what_the_user_picks() {
    let (mut pila, bus, _pipewire) = start();
    let client = pila.client();
    client.window("under").size(300, 200).color(GREEN).create();
    pila.wait_focus("under");
    client.window("over").size(200, 150).color(BLUE).create();
    pila.wait_focus("over");
    pila.settle();
    let mut portal = Portal::connect(&bus.address);

    let asked = portal.screenshot("org.example.Browser", true, true);
    let picker = wait_picker(&mut pila);
    assert_eq!(picker["screenshot"], true);
    assert_eq!(
        picker["kinds"],
        serde_json::json!(["output", "window", "window"])
    );
    assert_eq!(picker["sources"][2], "under");
    let (x, y) = tile_of(&picker, 2);
    pila.click_at(x, y);
    pila.wait("under selected", |s| s["share_picker"]["selected"] == 2);
    pila.key("Return");
    let (response, uri) = asked.join().unwrap();
    assert_eq!(response, 0, "Screenshot");
    let png = screenshot_png(&uri.expect("its URI"));
    assert_eq!(png.0, (300, 200), "the window's size");
    assert_eq!(png_pixel(&png, 150, 100), rgba(GREEN), "the window alone");

    // Not allowed to take one unasked: asked, and cancelled.
    let asked = portal.screenshot("org.example.Browser", false, false);
    wait_picker(&mut pila);
    pila.key("Escape");
    assert_eq!(asked.join().unwrap(), (1, None), "cancelled");
}

/// With two outputs side by side, a screenshot taken unasked is both, as
/// they're laid out.
#[test]
fn a_screenshot_taken_unasked_covers_every_output() {
    let (mut pila, bus, _pipewire) = start_with("A:640x480,B:640x480");
    let client = pila.client();
    pila.move_to(960.0, 240.0);
    client.window("right").size(200, 150).color(BLUE).create();
    let (x, y) = window_center(&mut pila, "right");
    assert!(x >= 640, "the window on B: {x}");
    pila.settle();
    let expected = pila.pixel(x as f64, y as f64);
    let mut portal = Portal::connect(&bus.address);
    let (response, uri) = portal
        .screenshot("org.example.Browser", false, true)
        .join()
        .unwrap();
    assert_eq!(response, 0, "Screenshot");
    let png = screenshot_png(&uri.expect("its URI"));
    assert_eq!(png.0, (1280, 480), "both outputs");
    assert_eq!(png_pixel(&png, x, y), expected, "the window on B in it");
}

fn color_pick_open(state: &Value) -> bool {
    state["color_pick"]["open"] == true
}

/// PickColor: the pointer is a crosshair over everything, a panel says what
/// to do, and a click answers with the color under it; the window under it
/// gets no click.
#[test]
fn a_color_is_picked_with_a_click() {
    let (mut pila, bus, _pipewire) = start();
    let client = pila.client();
    client.window("colors").size(200, 150).color(BLUE).create();
    let (x, y) = window_center(&mut pila, "colors");
    pila.move_to(x as f64, y as f64);
    pila.settle();
    client.roundtrip();
    let mut portal = Portal::connect(&bus.address);
    let asked = portal.pick_color("org.example.Paint");
    let state = pila.wait("the color picker", |s| {
        color_pick_open(s) && s["color_pick"]["text"] == true
    });
    assert_eq!(state["pointer"]["cursor"], "crosshair");
    // Its panel drawn, at the top of the output.
    let (px, py, pw, _) = rect(&state["color_pick"]["rect"]);
    let desktop = pila.pixel(5.0, 5.0);
    pila.wait_pixel_is((px + pw - 3) as f64, (py + 3) as f64, "its panel", |p| {
        p != desktop
    });

    pila.click("left");
    let (response, color) = asked.join().unwrap();
    assert_eq!(response, 0, "PickColor");
    let [_, r, g, b] = BLUE;
    let expected = [r, g, b].map(|c| f64::from(c) / 255.0);
    let (cr, cg, cb) = color.expect("a color");
    for (got, want) in [cr, cg, cb].iter().zip(expected) {
        assert!(
            (got - want).abs() < 0.01,
            "{:?} for {expected:?}",
            (cr, cg, cb)
        );
    }
    let state = pila.wait("the picker gone", |s| !color_pick_open(s));
    assert_ne!(state["pointer"]["cursor"], "crosshair");
    client.roundtrip();
    let buttons = client
        .lock()
        .events
        .iter()
        .filter(|e| matches!(e, common::client::Ev::Button { .. }))
        .count();
    assert_eq!(buttons, 0, "the window got the click");
}

/// Escape cancels it, and so does locking the screen; while it's on keys
/// reach no app.
#[test]
fn color_picking_is_cancelled_by_escape_or_the_lock() {
    let (mut pila, bus, _pipewire) = start();
    let client = pila.client();
    client.window("app").size(200, 150).create();
    pila.wait_focus("app");
    let mut portal = Portal::connect(&bus.address);

    let asked = portal.pick_color("org.example.Paint");
    pila.wait("the color picker", color_pick_open);
    pila.key("a");
    pila.key("Escape");
    assert_eq!(asked.join().unwrap(), (1, None), "cancelled");
    client.roundtrip();
    let keys = client
        .lock()
        .events
        .iter()
        .filter(|e| matches!(e, common::client::Ev::Key { .. }))
        .count();
    assert_eq!(keys, 0, "the app got keys");

    let asked = portal.pick_color("org.example.Paint");
    pila.wait("the color picker", color_pick_open);
    let locker = pila.client();
    let lock = locker.lock_session();
    locker.lock_surface(&lock, "HEADLESS-1", [255, 0, 0, 0]);
    locker.wait_logged("lock", "Locked");
    assert_eq!(asked.join().unwrap(), (1, None), "cancelled by the lock");
    assert!(!color_pick_open(&pila.state()));
}

/// An app the portal doesn't name (outside a sandbox: no app id) is named by
/// the window it asks from (`parent_window`, its xdg-foreign handle): its
/// app id, then its desktop entry's name.
#[test]
fn an_app_without_an_app_id_is_named_by_its_window() {
    use wayland_protocols::xdg::foreign::zv2::client::zxdg_exporter_v2::ZxdgExporterV2;
    let (mut pila, bus, _pipewire) = start();
    let client = pila.client();
    let window = client.window("org.example.Browser").size(200, 150).create();
    pila.wait_focus("org.example.Browser");
    let exporter: ZxdgExporterV2 = client.bind("zxdg_exporter_v2", 1);
    let _exported =
        exporter.export_toplevel(&client.surface(window), &client.qh, "exported".into());
    client.flush();
    client.wait_logged("exported", "Handle");
    let event = client.logged("exported").join(" ");
    let handle = event
        .split('"')
        .nth(1)
        .unwrap_or_else(|| panic!("no handle in {event}"))
        .to_string();

    let mut portal = Portal::connect(&bus.address);
    portal.parent_window = format!("wayland:{handle}");
    let started = portal.request("named", "", 1);
    // Its desktop entry's name ("Web Browser"), read on a thread.
    pila.wait("the app named", |s| {
        s["share_picker"]["app"] == "Web Browser"
    });
    pila.key("Escape");
    assert_eq!(started.answer().0, 1);

    // A handle no window was exported as names nothing.
    portal.parent_window = "wayland:nothing-exported-as-this".into();
    let started = portal.request("unnamed", "", 1);
    let picker = wait_picker(&mut pila);
    assert_eq!(picker["app"], Value::Null);
    pila.key("Escape");
    assert_eq!(started.answer().0, 1);
}

/// With more windows than shown, the selected one's tile is the one on the
/// accent, scrolled into view.
#[test]
fn the_selected_window_is_highlighted_when_scrolled() {
    const ACCENT: [u8; 4] = [0x44, 0x77, 0xcc, 255];
    let (mut pila, bus, _pipewire) = start_with("HEADLESS-1:1600x1000");
    let client = pila.client();
    for i in 0..12 {
        let name = format!("w{i:02}");
        client.window(&name).size(120, 90).color(BLUE).create();
        pila.wait_focus(&name);
    }
    pila.settle();
    let mut portal = Portal::connect(&bus.address);
    let started = portal.request_sources("scroll", "org.example.Browser", 1, 2);
    wait_picker(&mut pila);
    // To the last window: its row comes into view.
    for _ in 0..11 {
        pila.key("Right");
    }
    let picker = pila.wait("the last selected", |s| s["share_picker"]["selected"] == 11)
        ["share_picker"]
        .clone();
    pila.wait("its text", |s| s["share_picker"]["text"] == true);
    let slot = picker["tile_sources"]
        .as_array()
        .unwrap()
        .iter()
        .position(|i| i == 11)
        .expect("the last window shown");
    let (x, y, _, h) = rect(&picker["tiles"][slot]);
    pila.settle();
    assert_eq!(
        pila.pixel((x - 3) as f64, (y + h / 2) as f64),
        ACCENT,
        "the selected tile's margin on the accent"
    );
    pila.key("Escape");
    started.answer();
}
