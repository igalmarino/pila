//! Screen capture: ext-image-copy-capture of outputs and windows, and
//! wlr-screencopy; the pixels, damage, constraints and sources going away.

mod common;

use std::time::Duration;

use common::protocols::{read_pixel, LayerSpec};
use common::*;
use wayland_client::protocol::wl_shm::Format;
use wayland_protocols::ext::{
    foreign_toplevel_list::v1::client::{
        ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
        ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1,
    },
    image_capture_source::v1::client::{
        ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1,
        ext_image_capture_source_v1::ExtImageCaptureSourceV1,
        ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
    },
    image_copy_capture::v1::client::{
        ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1,
        ext_image_copy_capture_manager_v1::{
            ExtImageCopyCaptureManagerV1, Options as CaptureOptions,
        },
        ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1,
    },
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::Layer, zwlr_layer_surface_v1::Anchor,
};
use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1;

const RED: [u8; 4] = [255, 255, 0, 0];
const BACKGROUND: [u8; 3] = [31, 33, 38];

fn output_source(client: &Client, output: &str) -> ExtImageCaptureSourceV1 {
    let manager: ExtOutputImageCaptureSourceManagerV1 =
        client.bind("ext_output_image_capture_source_manager_v1", 1);
    manager.create_source(&client.output(output), &client.qh, "source".into())
}

fn window_source(client: &Client, app_id: &str) -> ExtImageCaptureSourceV1 {
    let _list: ExtForeignToplevelListV1 = client.bind("ext_foreign_toplevel_list_v1", 1);
    let text = format!("AppId {{ app_id: \"{app_id}\" }}");
    client.wait_logged("ext_toplevel", &text);
    let handle: ExtForeignToplevelHandleV1 = client.object("ext_toplevel", &text);
    let manager: ExtForeignToplevelImageCaptureSourceManagerV1 =
        client.bind("ext_foreign_toplevel_image_capture_source_manager_v1", 1);
    manager.create_source(&handle, &client.qh, "source".into())
}

/// A session on `source`; waits for its constraints, returns the buffer size.
fn session(
    client: &Client,
    source: &ExtImageCaptureSourceV1,
    tag: &str,
) -> (ExtImageCopyCaptureSessionV1, (i32, i32)) {
    let manager: ExtImageCopyCaptureManagerV1 = client.bind("ext_image_copy_capture_manager_v1", 1);
    let session = manager.create_session(source, CaptureOptions::empty(), &client.qh, tag.into());
    client.flush();
    client.wait_logged(tag, "Done");
    let size = client
        .logged(tag)
        .iter()
        .rev()
        .find_map(|e| {
            let rest = e.strip_prefix("BufferSize { width: ")?;
            let (w, rest) = rest.split_once(", height: ")?;
            Some((w.parse().ok()?, rest.trim_end_matches(" }").parse().ok()?))
        })
        .expect("a buffer size");
    (session, size)
}

/// Capture one frame into a new buffer of `size`; the frame and the buffer.
fn capture(
    client: &Client,
    session: &ExtImageCopyCaptureSessionV1,
    size: (i32, i32),
    tag: &str,
) -> (ExtImageCopyCaptureFrameV1, std::fs::File) {
    let frame = session.create_frame(&client.qh, tag.into());
    let (buffer, file) = client.capture_buffer(size, Format::Xrgb8888);
    frame.attach_buffer(&buffer);
    frame.damage_buffer(0, 0, size.0, size.1);
    frame.capture();
    client.flush();
    (frame, file)
}

#[test]
fn an_output_capture_has_the_screens_pixels() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).color(RED).create();
    pila.wait_focus("a");
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    pila.settle();
    let source = output_source(&client, "HEADLESS-1");
    let (session, size) = session(&client, &source, "session");
    assert_eq!(size, (1280, 800));
    assert!(client.has_logged("session", "ShmFormat"));
    let (_frame, file) = capture(&client, &session, size, "frame");
    client.wait_logged("frame", "Ready");
    assert_eq!(
        read_pixel(&file, 1280, (x + 50) as i32, (y + 50) as i32),
        [255, 0, 0]
    );
    assert_eq!(read_pixel(&file, 1280, 5, 795), BACKGROUND);
    // Upright: the top of the frame is the top of the screen.
    assert_eq!(
        read_pixel(&file, 1280, (x + 50) as i32, (y - 40) as i32),
        BACKGROUND
    );
}

#[test]
fn a_window_capture_has_only_the_window() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).color(RED).create();
    pila.wait_window("a");
    // Another window on top of it.
    client
        .window("cover")
        .size(400, 300)
        .color([255, 0, 0, 255])
        .create();
    pila.wait_focus("cover");
    pila.settle();
    let source = window_source(&client, "a");
    let (session, size) = session(&client, &source, "session");
    // Its geometry, without the title bar.
    assert_eq!(size, (400, 300));
    let (_frame, file) = capture(&client, &session, size, "frame");
    client.wait_logged("frame", "Ready");
    for (px, py) in [(1, 1), (200, 150), (398, 298)] {
        assert_eq!(read_pixel(&file, 400, px, py), [255, 0, 0], "at {px},{py}");
    }
}

#[test]
fn a_later_frame_waits_for_damage() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").size(400, 300).color(RED).create();
    pila.wait_focus("a");
    pila.settle();
    let source = window_source(&client, "a");
    let (session, size) = session(&client, &source, "session");
    let (first, _) = capture(&client, &session, size, "first");
    client.wait_logged("first", "Ready");
    // One frame at a time per session.
    first.destroy();
    let (_second, file) = capture(&client, &session, size, "second");
    std::thread::sleep(Duration::from_millis(200));
    client.roundtrip();
    assert!(
        !client.has_logged("second", "Ready"),
        "a frame without damage"
    );
    client.with_window(a, |w| w.color = [255, 0, 255, 0]);
    client.draw(a, None);
    client.wait_logged("second", "Ready");
    assert_eq!(read_pixel(&file, 400, 200, 150), [0, 255, 0]);
}

#[test]
fn a_buffer_of_the_wrong_size_fails() {
    let pila = TestPila::start();
    let client = pila.client();
    let source = output_source(&client, "HEADLESS-1");
    let (session, _) = session(&client, &source, "session");
    let (_frame, _) = capture(&client, &session, (100, 100), "frame");
    client.wait_logged("frame", "BufferConstraints");
    let _ = pila;
}

#[test]
fn closing_the_window_stops_its_session() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").create();
    pila.wait_window("a");
    let source = window_source(&client, "a");
    let (session, size) = session(&client, &source, "session");
    let (first, _) = capture(&client, &session, size, "first");
    client.wait_logged("first", "Ready");
    first.destroy();
    // A frame waiting for damage when the window goes.
    let (_second, _) = capture(&client, &session, size, "second");
    client.destroy(a);
    client.wait_logged("session", "Stopped");
}

#[test]
fn closing_the_window_stops_an_idle_session() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").create();
    pila.wait_window("a");
    let source = window_source(&client, "a");
    let (_session, _) = session(&client, &source, "session");
    client.destroy(a);
    client.wait_logged("session", "Stopped");
}

#[test]
fn unplugging_the_output_stops_its_session() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1024x768".into()),
        ..Default::default()
    });
    let client = pila.client();
    let source = output_source(&client, "B");
    let (_session, size) = session(&client, &source, "session");
    assert_eq!(size, (1024, 768));
    pila.request(json!({ "cmd": "output_remove", "name": "B" }));
    client.wait_logged("session", "Stopped");
}

#[test]
fn a_scaled_output_is_captured_at_its_pixel_size() {
    let pila = TestPila::with_config("[[output]]\nname = \"HEADLESS-1\"\nscale = 2\n");
    let client = pila.client();
    let source = output_source(&client, "HEADLESS-1");
    let (_session, size) = session(&client, &source, "session");
    assert_eq!(size, (1280, 800));
}

#[test]
fn window_captures_wait_while_locked() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).color(RED).create();
    pila.wait_focus("a");
    let source = window_source(&client, "a");
    let (session, size) = session(&client, &source, "session");
    let locker = pila.client();
    let lock = locker.lock_session();
    locker.lock_surface(&lock, "HEADLESS-1", [255, 0, 0, 0]);
    locker.wait_logged("lock", "Locked");
    let (_frame, file) = capture(&client, &session, size, "frame");
    std::thread::sleep(Duration::from_millis(200));
    client.roundtrip();
    assert!(
        !client.has_logged("frame", "Ready"),
        "a window captured while locked"
    );
    lock.unlock_and_destroy();
    locker.flush();
    pila.wait("unlocked", |s| s["locked"] == false);
    client.wait_logged("frame", "Ready");
    assert_eq!(read_pixel(&file, 400, 200, 150), [255, 0, 0]);
}

#[test]
fn an_output_capture_while_locked_shows_the_lock() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).color(RED).create();
    pila.wait_focus("a");
    let locker = pila.client();
    let lock = locker.lock_session();
    locker.lock_surface(&lock, "HEADLESS-1", [255, 0, 0, 255]);
    locker.wait_logged("lock", "Locked");
    let source = output_source(&client, "HEADLESS-1");
    let (session, size) = session(&client, &source, "session");
    let (_frame, file) = capture(&client, &session, size, "frame");
    client.wait_logged("frame", "Ready");
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    assert_eq!(
        read_pixel(&file, 1280, (x + 50) as i32, (y + 50) as i32),
        [0, 0, 255]
    );
}

// --- wlr-screencopy -----------------------------------------------------------------

#[test]
fn screencopy_copies_an_output() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).color(RED).create();
    pila.wait_focus("a");
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    pila.settle();
    let manager: ZwlrScreencopyManagerV1 = client.bind("zwlr_screencopy_manager_v1", 3);
    let frame = manager.capture_output(0, &client.output("HEADLESS-1"), &client.qh, "copy".into());
    client.flush();
    client.wait_logged("copy", "BufferDone");
    assert!(client.has_logged("copy", "width: 1280, height: 800, stride: 5120"));
    let (buffer, file) = client.capture_buffer((1280, 800), Format::Xrgb8888);
    frame.copy(&buffer);
    client.flush();
    client.wait_logged("copy", "Ready");
    assert_eq!(
        read_pixel(&file, 1280, (x + 50) as i32, (y + 50) as i32),
        [255, 0, 0]
    );
    assert_eq!(read_pixel(&file, 1280, 5, 5), BACKGROUND);
}

#[test]
fn screencopy_of_a_region() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).color(RED).create();
    pila.wait_focus("a");
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    pila.settle();
    let manager: ZwlrScreencopyManagerV1 = client.bind("zwlr_screencopy_manager_v1", 3);
    // Straddling the window's top-left corner.
    let frame = manager.capture_output_region(
        0,
        &client.output("HEADLESS-1"),
        (x - 50) as i32,
        (y - 50) as i32,
        100,
        100,
        &client.qh,
        "copy".into(),
    );
    client.flush();
    client.wait_logged("copy", "BufferDone");
    assert!(client.has_logged("copy", "width: 100, height: 100"));
    let (buffer, file) = client.capture_buffer((100, 100), Format::Xrgb8888);
    frame.copy(&buffer);
    client.flush();
    client.wait_logged("copy", "Ready");
    assert_eq!(read_pixel(&file, 100, 75, 75), [255, 0, 0]);
    assert_eq!(read_pixel(&file, 100, 75, 5), BACKGROUND);
    assert_eq!(read_pixel(&file, 100, 5, 75), BACKGROUND);
}

#[test]
fn screencopy_of_a_region_has_its_blurred_backdrop_in_place() {
    let mut pila = TestPila::with_config("[[layer-rule]]\nnamespace = \"glass\"\nblur = true\n");
    let client = pila.client();
    // Red and green halves, and a glass over the edge between them at 540,300.
    for (name, anchor, color) in [
        ("left", Anchor::Left, [255, 255, 0, 0]),
        ("right", Anchor::Right, [255, 0, 255, 0]),
    ] {
        let (surf, _) = client.layer_surface(
            name,
            &LayerSpec {
                layer: Layer::Bottom,
                anchor: Anchor::Top | Anchor::Bottom | anchor,
                size: (640, 0),
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
    pila.settle();
    let manager: ZwlrScreencopyManagerV1 = client.bind("zwlr_screencopy_manager_v1", 3);
    let frame = manager.capture_output_region(
        0,
        &client.output("HEADLESS-1"),
        540,
        300,
        200,
        200,
        &client.qh,
        "copy".into(),
    );
    client.flush();
    client.wait_logged("copy", "BufferDone");
    let (buffer, file) = client.capture_buffer((200, 200), Format::Xrgb8888);
    frame.copy(&buffer);
    client.flush();
    client.wait_logged("copy", "Ready");
    // Just left of the edge, under the glass: a mix, as on the screen.
    let [r, g, _] = read_pixel(&file, 200, 96, 100);
    assert!(r > 20 && g > 20, "not blurred: {r},{g}");
}

#[test]
fn screencopy_with_damage_starts_over_when_the_scale_changes() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).color(RED).create();
    pila.wait_focus("a");
    pila.settle();
    let manager: ZwlrScreencopyManagerV1 = client.bind("zwlr_screencopy_manager_v1", 3);
    let output = client.output("HEADLESS-1");
    let first = manager.capture_output(0, &output, &client.qh, "first".into());
    client.flush();
    client.wait_logged("first", "BufferDone");
    let (buffer, _) = client.capture_buffer((1280, 800), Format::Xrgb8888);
    first.copy_with_damage(&buffer);
    client.flush();
    client.wait_logged("first", "Ready");
    // The same size in pixels, so the frame is still good.
    pila.write_config("[[output]]\nname = \"HEADLESS-1\"\nscale = 2\n");
    pila.wait("the new scale", |s| s["outputs"][0]["scale"] == 2.0);
    pila.settle();
    let second = manager.capture_output(0, &output, &client.qh, "second".into());
    client.flush();
    client.wait_logged("second", "BufferDone");
    let (buffer, _) = client.capture_buffer((1280, 800), Format::Xrgb8888);
    second.copy_with_damage(&buffer);
    client.flush();
    client.wait_logged("second", "Ready");
    assert!(
        client.has_logged("second", "width: 1280, height: 800"),
        "not everything is damaged: {:?}",
        client.logged("second")
    );
}

#[test]
fn screencopy_with_damage_waits_for_a_change() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").size(400, 300).color(RED).create();
    pila.wait_focus("a");
    pila.settle();
    let manager: ZwlrScreencopyManagerV1 = client.bind("zwlr_screencopy_manager_v1", 3);
    let output = client.output("HEADLESS-1");
    let first = manager.capture_output(0, &output, &client.qh, "first".into());
    client.flush();
    client.wait_logged("first", "BufferDone");
    let (buffer, _) = client.capture_buffer((1280, 800), Format::Xrgb8888);
    first.copy_with_damage(&buffer);
    client.flush();
    client.wait_logged("first", "Ready");
    let second = manager.capture_output(0, &output, &client.qh, "second".into());
    client.flush();
    client.wait_logged("second", "BufferDone");
    let (buffer, _) = client.capture_buffer((1280, 800), Format::Xrgb8888);
    second.copy_with_damage(&buffer);
    client.flush();
    std::thread::sleep(Duration::from_millis(200));
    client.roundtrip();
    assert!(!client.has_logged("second", "Ready"));
    client.draw(a, None);
    client.wait_logged("second", "Damage");
    client.wait_logged("second", "Ready");
}

#[test]
fn screencopy_of_an_unplugged_output_fails() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1024x768".into()),
        ..Default::default()
    });
    let client = pila.client();
    let manager: ZwlrScreencopyManagerV1 = client.bind("zwlr_screencopy_manager_v1", 3);
    let frame = manager.capture_output(0, &client.output("B"), &client.qh, "copy".into());
    client.flush();
    client.wait_logged("copy", "BufferDone");
    let (buffer, _) = client.capture_buffer((1024, 768), Format::Xrgb8888);
    pila.request(json!({ "cmd": "output_remove", "name": "B" }));
    frame.copy_with_damage(&buffer);
    client.flush();
    client.wait_logged("copy", "Failed");
}
