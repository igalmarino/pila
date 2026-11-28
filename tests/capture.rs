//! Screen capture: ext-image-copy-capture of outputs and windows, and
//! wlr-screencopy; the pixels, damage, constraints and sources going away.

mod common;

use std::time::Duration;

use common::client::Window;
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
        ext_image_copy_capture_cursor_session_v1::{
            Error as CursorSessionError, ExtImageCopyCaptureCursorSessionV1,
        },
        ext_image_copy_capture_frame_v1::{Error as FrameError, ExtImageCopyCaptureFrameV1},
        ext_image_copy_capture_manager_v1::{
            ExtImageCopyCaptureManagerV1, Options as CaptureOptions,
        },
        ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1,
    },
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::Layer, zwlr_layer_surface_v1::Anchor,
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::Error as ScreencopyError,
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

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

/// How many copies into memory that may be new (a client's shm buffer, a
/// screenshot's) were written on the copy thread, off the event loop: the
/// first write into such memory faults its pages in, 9 ms of an 8 MB frame
/// on bare metal.
fn copies_off_loop(state: &Value) -> u64 {
    state["copies_off_loop"].as_u64().expect("copies_off_loop")
}

#[test]
fn a_capture_into_shm_is_written_off_the_event_loop() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).color(RED).create();
    pila.wait_focus("a");
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    let before = copies_off_loop(&pila.settle());
    let source = output_source(&client, "HEADLESS-1");
    let (session, size) = session(&client, &source, "session");
    let (_frame, file) = capture(&client, &session, size, "frame");
    client.wait_logged("frame", "Ready");
    assert_eq!(copies_off_loop(&pila.state()), before + 1);
    assert_eq!(
        read_pixel(&file, 1280, (x + 50) as i32, (y + 50) as i32),
        [255, 0, 0]
    );
}

#[test]
fn a_window_its_client_fades_is_faded_in_a_capture() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("a").size(400, 300).color(RED).create();
    pila.wait_focus("a");
    client.set_auto_draw(window, false);
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    let surface = client.surface(window);
    client.fade(&surface, 0.5);
    surface.commit();
    client.flush();
    client.roundtrip();
    pila.settle();
    let source = output_source(&client, "HEADLESS-1");
    let (session, size) = session(&client, &source, "session");
    let (_frame, file) = capture(&client, &session, size, "frame");
    client.wait_logged("frame", "Ready");
    let pixel = read_pixel(&file, 1280, (x + 50) as i32, (y + 50) as i32);
    let half = |c: u8, d: u8| ((c as i32 + d as i32) / 2) as u8;
    let expected = [
        half(255, BACKGROUND[0]),
        half(0, BACKGROUND[1]),
        half(0, BACKGROUND[2]),
    ];
    assert!(
        pixel
            .iter()
            .zip(expected)
            .all(|(a, b)| (*a as i32 - b as i32).abs() <= 3),
        "{pixel:?}, not about {expected:?}"
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

/// Capture `window` the way a screen share does while its client animates:
/// a frame callback with every commit, the next capture asked for after
/// each frame copied. How long six such frames took.
fn share_six_frames(
    client: &Client,
    window: Window,
    session: &ExtImageCopyCaptureSessionV1,
    size: (i32, i32),
) -> Duration {
    let surface = client.surface(window);
    let started = std::time::Instant::now();
    for n in 0..6 {
        let callback = format!("callback{n}");
        let _callback = surface.frame(&client.qh, callback.clone());
        client.draw(window, None);
        let tag = format!("frame{n}");
        let (frame, _) = capture(client, session, size, &tag);
        client.wait_logged(&callback, "Done");
        client.wait_logged(&tag, "Ready");
        frame.destroy();
    }
    started.elapsed()
}

/// A window session whose first frame is copied already.
fn shared_window(client: &Client, app_id: &str) -> (ExtImageCopyCaptureSessionV1, (i32, i32)) {
    let source = window_source(client, app_id);
    let (session, size) = session(client, &source, "session");
    let (first, _) = capture(client, &session, size, "first");
    client.wait_logged("first", "Ready");
    first.destroy();
    (session, size)
}

#[test]
fn a_covered_window_being_captured_keeps_its_frame_rate() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    client.window("cover").fullscreen().create();
    pila.wait("the cover", |s| {
        window_in(s, "cover").is_some_and(|w| rect(&w["geometry"]) == (0, 0, 1280, 800))
    });
    pila.settle();
    let (session, size) = shared_window(&client, "a");
    // Paced by the screen it would get one frame callback a second: at
    // least 5 s.
    let took = share_six_frames(&client, window, &session, size);
    assert!(took < Duration::from_secs(4), "6 frames in {took:?}");
}

#[test]
fn a_minimized_window_being_captured_keeps_drawing() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let (session, size) = shared_window(&client, "a");
    client.request(window, |t| t.set_minimized());
    pila.wait("minimized", |s| {
        window_in(s, "a").unwrap()["minimized"] == true
    });
    let took = share_six_frames(&client, window, &session, size);
    assert!(took < Duration::from_secs(4), "6 frames in {took:?}");
}

/// A window minimized right after a frame of it was captured on screen
/// still gets its next frame: off every output, the frame isn't held for a
/// refresh of none (the last was timed at the screen's next refresh).
#[test]
fn a_window_minimized_right_after_a_frame_gets_the_next() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    pila.settle();
    let (session, size) = shared_window(&client, "a");
    client.request(window, |t| t.set_minimized());
    client.draw(window, None);
    let (_frame, _) = capture(&client, &session, size, "hidden");
    pila.wait("minimized", |s| {
        window_in(s, "a").unwrap()["minimized"] == true
    });
    pila.settle();
    client.wait_logged("hidden", "Ready");
}

#[test]
fn a_window_on_another_workspace_being_captured_keeps_drawing() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let (session, size) = shared_window(&client, "a");
    pila.combo("Super+2");
    pila.wait("workspace 2", |s| s["workspace"] == 2);
    let took = share_six_frames(&client, window, &session, size);
    assert!(took < Duration::from_secs(4), "6 frames in {took:?}");
}

#[test]
fn a_failed_copy_doesnt_lose_the_next_frame() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").size(400, 300).color(RED).create();
    pila.wait_focus("a");
    pila.settle();
    let (session, size) = shared_window(&client, "a");
    // A change, copied into a buffer that doesn't fit: that frame fails.
    let (failed, _) = capture(&client, &session, (100, 100), "failed");
    client.with_window(a, |w| w.color = [255, 0, 255, 0]);
    client.draw(a, None);
    client.wait_logged("failed", "BufferConstraints");
    failed.destroy();
    // The change was never delivered: the next frame has it, with the
    // screen still.
    let (_next, file) = capture(&client, &session, size, "next");
    client.wait_logged("next", "Ready");
    assert_eq!(read_pixel(&file, 400, 200, 150), [0, 255, 0]);
}

#[test]
fn an_output_is_captured_into_a_dmabuf() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).color(RED).create();
    pila.wait_focus("a");
    pila.settle();
    let source = output_source(&client, "HEADLESS-1");
    let (session, size) = session(&client, &source, "session");
    // A linear dma-buf, which GPUs render to; none without one (CI).
    if !client.has_logged("session", "DmabufFormat") {
        return;
    }
    let Some(buffer) = client.dmabuf_buffer(size) else {
        return;
    };
    let frame = session.create_frame(&client.qh, "frame".into());
    frame.attach_buffer(&buffer);
    frame.damage_buffer(0, 0, size.0, size.1);
    frame.capture();
    client.flush();
    client.wait_logged("frame", "Ready");
}

#[test]
fn capture_textures_are_kept_while_captures_go_on() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").size(400, 300).color(RED).create();
    pila.wait_focus("a");
    pila.settle();
    let textures = |s: &Value| s["capture"]["textures"].as_u64().unwrap();
    // An output and a window: a texture for each size.
    let output = output_source(&client, "HEADLESS-1");
    let (screen, screen_size) = session(&client, &output, "screen");
    let (frame, _) = capture(&client, &screen, screen_size, "screen_frame");
    client.wait_logged("screen_frame", "Ready");
    frame.destroy();
    let (window, window_size) = shared_window(&client, "a");
    pila.wait("two textures", |s| textures(s) == 2);
    // Another frame of the screen reuses its texture, and has the change.
    client.with_window(a, |w| w.color = [255, 0, 255, 0]);
    client.draw(a, None);
    let (frame, file) = capture(&client, &screen, screen_size, "screen_again");
    client.wait_logged("screen_again", "Ready");
    frame.destroy();
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    assert_eq!(
        read_pixel(&file, 1280, (x + 50) as i32, (y + 50) as i32),
        [0, 255, 0]
    );
    assert_eq!(textures(&pila.settle()), 2);
    // No capture left: they go at the next frame.
    screen.destroy();
    window.destroy();
    let _ = window_size;
    client.draw(a, None);
    pila.wait("no texture", |s| textures(s) == 0);
}

/// The presentation time a frame got, from its logged event.
fn presentation_time(client: &Client, tag: &str) -> Duration {
    event_time(client, tag, "PresentationTime")
}

#[test]
fn a_frame_is_presented_at_the_next_refresh() {
    // At 1 Hz, the next refresh is long after the frame is copied and sent.
    let mut pila = TestPila::start_with(Options {
        outputs: Some("SLOW:1280x800@1".into()),
        ..Default::default()
    });
    let client = pila.client();
    client.window("a").size(400, 300).color(RED).create();
    pila.wait_focus("a");
    let source = output_source(&client, "SLOW");
    let (session, size) = session(&client, &source, "session");
    let (_frame, _) = capture(&client, &session, size, "frame");
    client.wait_logged("frame", "Ready");
    let received = monotonic_now();
    // When it shows on the output, not when it was copied (before the
    // client got it).
    let shown = presentation_time(&client, "frame");
    assert!(
        shown > received + Duration::from_millis(300),
        "presented {:?} after the client got it",
        shown.checked_sub(received)
    );
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

/// A frame is used once: what comes after `capture`, or a `capture` with
/// no buffer, is a protocol error (each on a client of its own, since the
/// error ends its connection).
#[test]
fn a_frame_misused_is_a_protocol_error() {
    let pila = TestPila::start();
    type Misuse = fn(&Client, &ExtImageCopyCaptureFrameV1, (i32, i32));
    let cases: [(&str, FrameError, Misuse); 4] = [
        (
            "attach after capture",
            FrameError::AlreadyCaptured,
            |client, frame, size| {
                frame.capture();
                frame.attach_buffer(&client.capture_buffer(size, Format::Xrgb8888).0);
            },
        ),
        (
            "damage after capture",
            FrameError::AlreadyCaptured,
            |_, frame, size| {
                frame.capture();
                frame.damage_buffer(0, 0, size.0, size.1);
            },
        ),
        (
            "capture sent twice",
            FrameError::AlreadyCaptured,
            |_, frame, _| {
                frame.capture();
                frame.capture();
            },
        ),
        (
            "capture without a buffer",
            FrameError::NoBuffer,
            |_, _, _| {},
        ),
    ];
    for (what, code, misuse) in cases {
        let client = pila.client();
        let source = output_source(&client, "HEADLESS-1");
        let (session, size) = session(&client, &source, "session");
        let frame = session.create_frame(&client.qh, "frame".into());
        // A buffer for all but the last case, which captures without one.
        let buffer = (what != "capture without a buffer")
            .then(|| client.capture_buffer(size, Format::Xrgb8888));
        if let Some((buffer, _)) = &buffer {
            frame.attach_buffer(buffer);
        } else {
            client.expect_error();
            frame.capture();
        }
        if buffer.is_some() {
            client.expect_error();
            misuse(&client, &frame, size);
        }
        client.flush();
        let error = client.wait_error();
        let expected = format!(
            "Protocol error {} on object ext_image_copy_capture_frame_v1",
            code as u32
        );
        assert!(error.contains(&expected), "{what}: {error}");
    }
}

/// Pila doesn't capture the pointer apart (clients ask for it painted
/// in): a cursor session's capture session is stopped from the start, and
/// its frames fail. Asking for that session twice is a protocol error.
#[test]
fn a_cursor_session_is_stopped_from_the_start() {
    let pila = TestPila::start();
    let client = pila.client();
    let source = output_source(&client, "HEADLESS-1");
    let manager: ExtImageCopyCaptureManagerV1 = client.bind("ext_image_copy_capture_manager_v1", 1);
    let pointer = client.lock().pointer.clone().expect("a pointer");
    let cursor: ExtImageCopyCaptureCursorSessionV1 =
        manager.create_pointer_cursor_session(&source, &pointer, &client.qh, "cursor".into());
    let session = cursor.get_capture_session(&client.qh, "session".into());
    client.flush();
    client.wait_logged("session", "Stopped");
    let frame = session.create_frame(&client.qh, "frame".into());
    let (buffer, _file) = client.capture_buffer((64, 64), Format::Xrgb8888);
    frame.attach_buffer(&buffer);
    frame.capture();
    client.flush();
    client.wait_logged("frame", "Failed");
    assert!(
        client.has_logged("frame", "Stopped"),
        "{:?}",
        client.logged("frame")
    );
    client.expect_error();
    let _again = cursor.get_capture_session(&client.qh, "again".into());
    client.flush();
    let error = client.wait_error();
    let expected = format!(
        "Protocol error {} on object ext_image_copy_capture_cursor_session_v1",
        CursorSessionError::DuplicateSession as u32
    );
    assert!(error.contains(&expected), "{error}");
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
fn a_window_leaving_the_screen_keeps_its_capture_scale() {
    let mut pila = TestPila::with_config("[[output]]\nname = \"HEADLESS-1\"\nscale = 2\n");
    let client = pila.client();
    let a = client.window("a").size(400, 300).color(RED).create();
    pila.wait_focus("a");
    pila.settle();
    let source = window_source(&client, "a");
    let (session, size) = session(&client, &source, "session");
    assert_eq!(size, (800, 600));
    let (first, _) = capture(&client, &session, size, "first");
    client.wait_logged("first", "Ready");
    first.destroy();
    // Minimized, it's still drawn at that scale: the share goes on at the
    // same size, with no new constraints.
    client.request(a, |t| t.set_minimized());
    pila.wait("minimized", |s| {
        window_in(s, "a").unwrap()["minimized"] == true
    });
    // A change, then the next frame: it has the change.
    client.with_window(a, |w| w.color = [255, 0, 255, 0]);
    client.draw(a, None);
    let (_next, file) = capture(&client, &session, size, "next");
    client.wait_logged("next", "Ready");
    assert!(!client.has_logged("next", "BufferConstraints"));
    assert_eq!(read_pixel(&file, 800, 400, 300), [0, 255, 0]);
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
fn a_screencopy_is_written_off_the_event_loop() {
    let mut pila = TestPila::start();
    let before = copies_off_loop(&pila.settle());
    let client = pila.client();
    let manager: ZwlrScreencopyManagerV1 = client.bind("zwlr_screencopy_manager_v1", 3);
    let frame = manager.capture_output(0, &client.output("HEADLESS-1"), &client.qh, "copy".into());
    client.flush();
    client.wait_logged("copy", "BufferDone");
    let (buffer, file) = client.capture_buffer((1280, 800), Format::Xrgb8888);
    frame.copy(&buffer);
    client.flush();
    client.wait_logged("copy", "Ready");
    assert_eq!(copies_off_loop(&pila.state()), before + 1);
    assert_eq!(read_pixel(&file, 1280, 5, 5), BACKGROUND);
}

/// A screencopy frame is copied once, into a buffer of the size and
/// format it announced: anything else is a protocol error (each on a
/// client of its own, since the error ends its connection).
#[test]
fn a_screencopy_frame_copied_twice_or_into_a_wrong_buffer_is_a_protocol_error() {
    let pila = TestPila::start();
    for (what, code) in [
        ("copied twice", ScreencopyError::AlreadyUsed),
        ("a wrong buffer", ScreencopyError::InvalidBuffer),
    ] {
        let client = pila.client();
        let manager: ZwlrScreencopyManagerV1 = client.bind("zwlr_screencopy_manager_v1", 3);
        let frame =
            manager.capture_output(0, &client.output("HEADLESS-1"), &client.qh, "copy".into());
        client.flush();
        client.wait_logged("copy", "BufferDone");
        if code == ScreencopyError::AlreadyUsed {
            let (buffer, _file) = client.capture_buffer((1280, 800), Format::Xrgb8888);
            frame.copy(&buffer);
            client.expect_error();
            frame.copy(&buffer);
        } else {
            let (buffer, _file) = client.capture_buffer((100, 100), Format::Xrgb8888);
            client.expect_error();
            frame.copy(&buffer);
        }
        client.flush();
        let error = client.wait_error();
        let expected = format!(
            "Protocol error {} on object zwlr_screencopy_frame_v1",
            code as u32
        );
        assert!(error.contains(&expected), "{what}: {error}");
    }
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
fn a_blurred_window_has_its_backdrop_in_a_capture_of_the_screen_only() {
    let mut pila = TestPila::with_config(
        "[[window-rule]]\napp-id = \"glass\"\nblur = true\ntitle-bar = false\n\
         size = [200, 200]\nposition = [540, 300]\n",
    );
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
                size: (640, 0),
                ..LayerSpec::default()
            },
            color,
        );
        client.wait_surface_configured(surf);
    }
    client.window("glass").color([0, 0, 0, 0]).create();
    pila.wait_focus("glass");
    pila.settle();
    pila.wait_pixel_is(636.0, 400.0, "blurred", |[r, g, ..]| r > 20 && g > 20);
    // The whole screen has it, as drawn: a region of it, here.
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
    let [r, g, _] = read_pixel(&file, 200, 96, 100);
    assert!(r > 20 && g > 20, "not blurred: {r},{g}");
    // A capture of the window alone doesn't: it's clear there.
    let source = window_source(&client, "glass");
    let (session, size) = session(&client, &source, "session");
    let (_frame, file) = capture(&client, &session, size, "frame");
    client.wait_logged("frame", "Ready");
    let [r, g, _] = read_pixel(&file, size.0, 96, 100);
    assert!(r < 20 && g < 20, "what's behind it: {r},{g}");
}

#[test]
fn a_shadow_is_in_a_capture_of_the_screen_but_not_of_the_window() {
    let mut pila = TestPila::with_config(
        "[shadow]\nsize = 24\noffset = [0, 0]\ncolor = \"#000000ff\"\n\
         [colors]\nbackground = \"#ffffff\"\n\
         [[window-rule]]\napp-id = \"a\"\ntitle-bar = false\n\
         size = [200, 200]\nposition = [540, 300]\n",
    );
    let client = pila.client();
    client.window("a").color([255, 255, 0, 0]).create();
    pila.wait_focus("a");
    pila.settle();
    // Just below the window, in a capture of that part of the screen.
    let manager: ZwlrScreencopyManagerV1 = client.bind("zwlr_screencopy_manager_v1", 3);
    let frame = manager.capture_output_region(
        0,
        &client.output("HEADLESS-1"),
        540,
        500,
        200,
        20,
        &client.qh,
        "copy".into(),
    );
    client.flush();
    client.wait_logged("copy", "BufferDone");
    let (buffer, file) = client.capture_buffer((200, 20), Format::Xrgb8888);
    frame.copy(&buffer);
    client.flush();
    client.wait_logged("copy", "Ready");
    let [r, g, b] = read_pixel(&file, 200, 100, 3);
    assert!(r < 50 && g < 50 && b < 50, "no shadow: {r},{g},{b}");
    // A capture of the window alone is the window's size.
    let source = window_source(&client, "a");
    let (_session, size) = session(&client, &source, "session");
    assert_eq!(size, (200, 200));
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
    // Nothing left to draw: no frame of another output comes to fail the copy
    // by chance (under load, in CI, none did and it was never answered).
    pila.settle();
    frame.copy_with_damage(&buffer);
    client.flush();
    client.wait_logged("copy", "Failed");
}

/// `scripts/capture-stream` (`pila-check`'s screen share client) without
/// `os.memfd_create`, as with a Python built without it (found on NixOS:
/// the stream scenario failed): its buffers go in an unlinked file instead.
#[test]
fn the_stream_client_works_without_memfd_create() {
    let python = std::process::Command::new("python3")
        .arg("--version")
        .output();
    if !python.is_ok_and(|output| output.status.success()) {
        eprintln!("no python3: skipped");
        return;
    }
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("a").size(200, 150).create();
    pila.wait_window("a");
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/capture-stream");
    let child = std::process::Command::new("python3")
        .arg("-I")
        .arg("-c")
        .arg(format!(
            "import os, sys; del os.memfd_create; sys.argv = ['capture-stream', '1']; \
             exec(compile(open({script:?}).read(), 'capture-stream', 'exec'))"
        ))
        .env("XDG_RUNTIME_DIR", &pila.dir)
        .env("WAYLAND_DISPLAY", &pila.display)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // Frames come as the screen changes: the window draws while it runs.
    let started = std::time::Instant::now();
    while started.elapsed() < Duration::from_millis(1200) {
        client.draw(window, None);
        client.roundtrip();
        std::thread::sleep(Duration::from_millis(30));
    }
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}{stderr}");
    assert!(stdout.contains("failed 0"), "{stdout}{stderr}");
}

/// A monitor turned while a capture of it goes on: its pixel size changes,
/// so the session says the new buffer size, a frame into a buffer of the old
/// one fails with `buffer_constraints`, and one of the new size is captured.
#[test]
fn turning_a_captured_output_changes_its_buffer_size() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let source = output_source(&client, "HEADLESS-1");
    let (session, size) = session(&client, &source, "session");
    assert_eq!(size, (1280, 800));
    // One frame at a time: each is destroyed before the next.
    let (frame, _) = capture(&client, &session, size, "first");
    client.wait_logged("first", "Ready");
    frame.destroy();

    pila.write_config("[[output]]\nname = \"HEADLESS-1\"\ntransform = \"90\"\n");
    pila.wait("turned", |s| s["outputs"][0]["transform"] == "_90");
    pila.settle();
    let (frame, _) = capture(&client, &session, size, "old");
    client.wait_logged("old", "BufferConstraints");
    frame.destroy();
    client.wait("the new size", |s| {
        s.logged("session")
            .iter()
            .any(|e| e == "BufferSize { width: 800, height: 1280 }")
    });
    let (_frame, _) = capture(&client, &session, (800, 1280), "new");
    client.wait_logged("new", "Ready");
}

/// Two wlr-screencopy frames of one refresh period would be presented at the
/// same time (its vblank): recorders that stamp frames with it, such as
/// wf-recorder writing mp4, abort on a timestamp that doesn't advance (#17).
/// Another capture (`capture_due`: a portal share's picker or stream
/// starting, an ext-image-copy-capture frame) or another output drawing
/// copied a frame waiting for damage within the period of the last one: it
/// now waits for its output's next refresh.
#[test]
fn screencopy_frames_are_one_per_refresh() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("SLOW:640x480@2,FAST:640x480@60".into()),
        ..Default::default()
    });
    let client = pila.client();
    let a = client.window("a").size(200, 150).color(RED).create();
    pila.wait_focus("a");
    pila.settle();

    let manager: ZwlrScreencopyManagerV1 = client.bind("zwlr_screencopy_manager_v1", 3);
    let slow = client.output("SLOW");
    let first = manager.capture_output(0, &slow, &client.qh, "first".into());
    client.flush();
    client.wait_logged("first", "BufferDone");
    let (buffer, _) = client.capture_buffer((640, 480), Format::Xrgb8888);
    first.copy_with_damage(&buffer);
    client.flush();
    client.wait_logged("first", "Ready");

    // A change on SLOW and the next frame asked at once, as wf-recorder
    // does; then, within SLOW's period, an ext capture of it and FAST drawing.
    client.with_window(a, |w| w.color = [255, 0, 255, 0]);
    client.draw(a, None);
    let second = manager.capture_output(0, &slow, &client.qh, "second".into());
    client.flush();
    client.wait_logged("second", "BufferDone");
    let (buffer, _) = client.capture_buffer((640, 480), Format::Xrgb8888);
    second.copy_with_damage(&buffer);
    client.flush();
    let source = output_source(&client, "SLOW");
    let (session, size) = session(&client, &source, "ext");
    let (_frame, _) = capture(&client, &session, size, "ext_frame");
    client.wait_logged("ext_frame", "Ready");
    pila.move_to(700.0, 100.0);
    client.roundtrip();

    client.wait_logged("second", "Ready");
    let (first, second) = (
        event_time(&client, "first", "Ready"),
        event_time(&client, "second", "Ready"),
    );
    assert!(
        second >= first + Duration::from_millis(400),
        "presented {:?} after the frame before, in the same refresh",
        second.saturating_sub(first)
    );
}

/// The same for an ext-image-copy-capture session: a frame asked for within
/// the refresh period of the one before (a capture request is processed at
/// once, `capture_due`) waits for the next refresh rather than being
/// presented at the same time.
#[test]
fn capture_frames_are_one_per_refresh() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("SLOW:640x480@2".into()),
        ..Default::default()
    });
    let client = pila.client();
    let a = client.window("a").size(200, 150).color(RED).create();
    pila.wait_focus("a");
    pila.settle();
    let source = output_source(&client, "SLOW");
    let (session, size) = session(&client, &source, "session");
    let (first, _) = capture(&client, &session, size, "first");
    client.wait_logged("first", "Ready");
    first.destroy();
    // A change, and the next frame asked at once.
    client.with_window(a, |w| w.color = [255, 0, 255, 0]);
    client.draw(a, None);
    let (_second, _) = capture(&client, &session, size, "second");
    client.wait_logged("second", "Ready");
    let (first, second) = (
        presentation_time(&client, "first"),
        presentation_time(&client, "second"),
    );
    assert!(
        second >= first + Duration::from_millis(400),
        "presented {:?} after the frame before, in the same refresh",
        second.saturating_sub(first)
    );
    let _ = pila;
}

// --- Capture indicator -------------------------------------------------------

const ACCENT: [u8; 4] = [0x44, 0x77, 0xcc, 255];

/// What the state dump lists as capturing.
fn captures(state: &Value) -> Vec<Value> {
    state["capture_indicator"]["captures"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The test's own program name, as Pila reads it from `/proc/<pid>/exe`.
fn own_name() -> String {
    std::env::current_exe()
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

/// An output captured: listed with the program's name, the middle of its
/// top edge lit on the screen and not in the capture; after the session
/// ends, still for a moment, then neither.
#[test]
fn an_output_being_captured_is_listed_and_lit_on_the_screen_alone() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let corner = (640.0, 0.0);
    let before = pila.settle();
    assert_eq!(captures(&before), Vec::<Value>::new());
    let unlit = pila.pixel(corner.0, corner.1);
    assert_ne!(unlit, ACCENT);
    let top_right = pila.pixel(1279.0, 0.0);
    let source = output_source(&client, "HEADLESS-1");
    let (session, size) = session(&client, &source, "lit");
    let name = own_name();
    let state = pila.wait("listed by name", |s| {
        captures(s)
            .first()
            .is_some_and(|c| c["app"] == name.as_str())
    });
    assert_eq!(
        captures(&state),
        vec![serde_json::json!({
            "app": name,
            "protocol": "ext-image-copy-capture",
            "target": { "kind": "output", "output": "HEADLESS-1" },
        })]
    );
    assert_eq!(
        state["capture_indicator"]["lights"],
        serde_json::json!(["HEADLESS-1"])
    );
    pila.wait_pixel(corner.0, corner.1, ACCENT);
    assert_eq!(pila.pixel(1279.0, 0.0), top_right, "the corner lit");
    // Not in what's captured.
    let (_frame, file) = capture(&client, &session, size, "frame");
    client.wait_logged("frame", "Ready");
    let px = read_pixel(&file, size.0, size.0 / 2, 0);
    assert_eq!(
        px,
        [unlit[0], unlit[1], unlit[2]],
        "the light in the capture"
    );

    session.destroy();
    client.flush();
    let ended = std::time::Instant::now();
    pila.wait("unlisted", |s| captures(s).is_empty());
    assert!(
        ended.elapsed() >= Duration::from_millis(800),
        "gone after {:?}, not held",
        ended.elapsed()
    );
    pila.wait_pixel(corner.0, corner.1, unlit);
}

/// A one-shot screenshot (grim's screencopy: one frame, then gone) is
/// listed for a moment after it's done.
#[test]
fn a_screenshot_is_listed_for_a_moment() {
    let mut pila = TestPila::start();
    let client = pila.client();
    pila.settle();
    let manager: ZwlrScreencopyManagerV1 = client.bind("zwlr_screencopy_manager_v1", 3);
    let frame = manager.capture_output(0, &client.output("HEADLESS-1"), &client.qh, "shot".into());
    client.flush();
    client.wait_logged("shot", "BufferDone");
    let (buffer, _file) = client.capture_buffer((1280, 800), Format::Xrgb8888);
    frame.copy(&buffer);
    client.flush();
    client.wait_logged("shot", "Ready");
    frame.destroy();
    client.flush();
    let state = pila.state();
    let listed = captures(&state);
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0]["protocol"], "wlr-screencopy");
    assert_eq!(listed[0]["target"]["kind"], "output");
    pila.wait("unlisted", |s| captures(s).is_empty());
}

/// A screenshot over before anyone could look is still named afterwards:
/// `captures`' `last` is the capture that ended most recently, with when,
/// and `pila msg captures` says so.
#[test]
fn the_last_capture_is_named_after_it_ends() {
    let mut pila = TestPila::start();
    let client = pila.client();
    pila.settle();
    let json = |pila: &TestPila| -> Value {
        let out = pila.msg(&["--json", "captures"]);
        serde_json::from_slice(&out.stdout).unwrap()
    };
    assert!(json(&pila)["last"].is_null(), "nothing captured yet");

    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let manager: ZwlrScreencopyManagerV1 = client.bind("zwlr_screencopy_manager_v1", 3);
    let frame = manager.capture_output(0, &client.output("HEADLESS-1"), &client.qh, "shot".into());
    client.flush();
    client.wait_logged("shot", "BufferDone");
    let (buffer, _file) = client.capture_buffer((1280, 800), Format::Xrgb8888);
    frame.copy(&buffer);
    client.flush();
    client.wait_logged("shot", "Ready");
    frame.destroy();
    client.flush();
    pila.wait("unlisted", |s| captures(s).is_empty());

    let answer = json(&pila);
    assert_eq!(answer["captures"], serde_json::json!([]));
    let last = &answer["last"];
    assert_eq!(last["app"], own_name().as_str(), "{last}");
    assert_eq!(last["protocol"], "wlr-screencopy");
    assert_eq!(last["target"]["kind"], "output");
    assert_eq!(last["target"]["output"], "HEADLESS-1");
    let ended = last["ended"].as_u64().expect("ended, in Unix seconds");
    assert!(
        ended >= before && ended <= before + 30,
        "{ended} vs {before}"
    );

    let out = pila.msg(&["captures"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.starts_with("nothing is capturing the screen\n"),
        "{text}"
    );
    let expected = format!(
        "last: {}  screen HEADLESS-1  (wlr-screencopy), ",
        own_name()
    );
    assert!(
        text.contains(&expected) && text.trim_end().ends_with(" ago"),
        "{text}"
    );
}

/// Wait for `f` to give something, polling as `TestPila::wait` does.
fn eventually<T>(what: &str, f: impl Fn() -> Option<T>) -> T {
    let started = std::time::Instant::now();
    loop {
        if let Some(value) = f() {
            return value;
        }
        assert!(started.elapsed() < TIMEOUT, "never: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A capture puts an icon in the tray, named in its tooltip, with a Stop in
/// its menu that ends it; the program can't capture again on that
/// connection (it would start over at once), but can once it connects
/// again; with nothing captured, the icon leaves the tray.
#[test]
fn a_capture_is_in_the_tray_and_its_menu_stops_it() {
    let bus = notifications::Bus::start();
    let watcher = bus.tray_watcher();
    let host = bus.host();
    let mut pila = TestPila::start_with(Options {
        env: vec![bus.env()],
        ..Default::default()
    });
    let client = pila.client();
    pila.settle();
    let source = output_source(&client, "HEADLESS-1");
    let (_session, _size) = session(&client, &source, "tray");
    let item = watcher.wait_for(1);
    assert!(item.starts_with("org.kde.StatusNotifierItem-"), "{item}");
    assert_eq!(
        String::try_from(host.property(&item, "Status")).unwrap(),
        "Active"
    );
    let icons: Vec<common::tray::Pixmap> = host.property(&item, "IconPixmap").try_into().unwrap();
    assert!(!icons.is_empty(), "no icon");
    // Named once its executable is read.
    let what = format!("{} (screen HEADLESS-1, ext-image-copy-capture)", own_name());
    eventually("the tooltip names it", || {
        (host.tooltip(&item).1 == what).then_some(())
    });
    assert_eq!(host.tooltip(&item).0, "Capturing the screen");
    let stop = format!("Stop {what}");
    let menu = eventually("a Stop in the menu", || {
        let menu = host.menu(&item);
        (menu.len() == 1 && menu[0].label == stop).then_some(menu)
    });

    host.click(&item, menu[0].id);
    client.wait_logged("tray", "Stopped");
    // Not again on that connection.
    let manager: ExtImageCopyCaptureManagerV1 = client.bind("ext_image_copy_capture_manager_v1", 1);
    let again = output_source(&client, "HEADLESS-1");
    let _again =
        manager.create_session(&again, CaptureOptions::empty(), &client.qh, "again".into());
    client.flush();
    client.wait_logged("again", "Stopped");
    pila.wait("unlisted", |s| captures(s).is_empty());
    eventually("out of the tray", || (!host.on_bus(&item)).then_some(()));

    // Connected again, it may.
    let restarted = pila.client();
    let source = output_source(&restarted, "HEADLESS-1");
    let (_session, _size) = session(&restarted, &source, "restarted");
    assert!(!restarted.has_logged("restarted", "Stopped"));
    assert_eq!(watcher.wait_for(2), item, "back in the tray");
}

/// `[permissions] tray = false`: no icon in the tray.
#[test]
fn the_tray_icon_can_be_turned_off() {
    let bus = notifications::Bus::start();
    let watcher = bus.tray_watcher();
    let mut pila = TestPila::start_with(Options {
        config: "[permissions]\ntray = false\n".into(),
        env: vec![bus.env()],
        ..Default::default()
    });
    let client = pila.client();
    pila.settle();
    let source = output_source(&client, "HEADLESS-1");
    let (_session, _size) = session(&client, &source, "off");
    pila.wait("listed", |s| !captures(s).is_empty());
    pila.stays("not in the tray", Duration::from_millis(300), |_| {
        watcher.registered().is_empty()
    });
}

/// A tray started while something captures (a bar restarted) gets the icon.
#[test]
fn a_tray_started_later_gets_the_icon() {
    let bus = notifications::Bus::start();
    let mut pila = TestPila::start_with(Options {
        env: vec![bus.env()],
        ..Default::default()
    });
    let client = pila.client();
    pila.settle();
    let source = output_source(&client, "HEADLESS-1");
    let (_session, _size) = session(&client, &source, "later");
    pila.wait("listed", |s| !captures(s).is_empty());
    let host = bus.host();
    eventually("the item on the bus", || {
        host.on_bus(&format!("org.kde.StatusNotifierItem-{}-1", pila.pid()))
            .then_some(())
    });
    let watcher = bus.tray_watcher();
    let item = watcher.wait_for(1);
    assert!(host.on_bus(&item));
}

/// A window captured: listed by its app id and id, the monitor it's on lit.
#[test]
fn a_window_being_captured_lights_the_monitor_it_is_on() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1280x800".into()),
        ..Default::default()
    });
    let client = pila.client();
    client.window("shared").size(300, 200).create();
    pila.wait_focus("shared");
    let id = pila.window("shared")["id"].as_str().unwrap().to_string();
    let source = window_source(&client, "shared");
    let (_session, _size) = session(&client, &source, "win");
    let state = pila.wait("listed", |s| !captures(s).is_empty());
    let target = &captures(&state)[0]["target"];
    assert_eq!(target["kind"], "window");
    assert_eq!(target["app_id"], "shared");
    assert_eq!(target["id"], id.as_str());
    let output = target["output"].as_str().unwrap().to_string();
    assert_eq!(
        state["capture_indicator"]["lights"],
        serde_json::json!([output])
    );
}

/// `[permissions] indicator = false`: listed, but no light.
#[test]
fn the_light_can_be_turned_off() {
    let mut pila = TestPila::with_config("[permissions]\nindicator = false\n");
    let client = pila.client();
    let corner = (640.0, 0.0);
    pila.settle();
    let unlit = pila.pixel(corner.0, corner.1);
    let source = output_source(&client, "HEADLESS-1");
    let (_session, _size) = session(&client, &source, "dark");
    let state = pila.wait("listed", |s| !captures(s).is_empty());
    assert_eq!(state["capture_indicator"]["lights"], serde_json::json!([]));
    assert_eq!(pila.pixel(corner.0, corner.1), unlit);
}

/// `pila msg captures`, and `--waybar captures` following them as a waybar
/// custom module: hidden (no text) while nothing captures.
#[test]
fn pila_msg_lists_the_captures_and_follows_them_for_waybar() {
    use std::io::BufRead;
    let mut pila = TestPila::start();
    let client = pila.client();
    pila.settle();
    let out = pila.msg(&["captures"]);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "nothing is capturing the screen"
    );
    let mut waybar = std::process::Command::new(env!("CARGO_BIN_EXE_pila"))
        .args(["msg", "--waybar", "captures"])
        .env_remove("PILA_SOCKET")
        .env("WAYLAND_DISPLAY", &pila.display)
        .env("XDG_RUNTIME_DIR", &pila.dir)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = std::io::BufReader::new(waybar.stdout.take().unwrap()).lines();
    let mut next = || -> Value { serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap() };
    assert_eq!(
        next(),
        serde_json::json!({ "text": "", "tooltip": "", "class": "idle" })
    );

    let source = output_source(&client, "HEADLESS-1");
    let (_session, _size) = session(&client, &source, "msg");
    let name = own_name();
    // Named once its executable is read: maybe a line with its pid first.
    let line = loop {
        let line = next();
        if line["text"] == format!("● {name}").as_str() {
            break line;
        }
    };
    assert_eq!(line["class"], "capturing");
    assert_eq!(
        line["tooltip"],
        format!("{name}  screen HEADLESS-1  (ext-image-copy-capture)").as_str()
    );
    let out = pila.msg(&["captures"]);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("{name}  screen HEADLESS-1  (ext-image-copy-capture)")
    );
    let _ = waybar.kill();
    let _ = waybar.wait();
}
