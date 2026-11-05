//! Protocols about surfaces and clients: the globals offered and what
//! sandboxed clients see, the system bell, selections (clipboard, primary,
//! data-control), scales and viewports, cursors, subsurfaces, frame
//! callbacks and presentation feedback.

mod common;

use std::os::{fd::AsFd, unix::net::UnixListener};

use common::*;
use wayland_client::protocol::{
    wl_data_device_manager::WlDataDeviceManager, wl_pointer::WlPointer,
};
use wayland_protocols::{
    ext::data_control::v1::client::ext_data_control_manager_v1::ExtDataControlManagerV1,
    wp::{
        cursor_shape::v1::client::{
            wp_cursor_shape_device_v1::Shape, wp_cursor_shape_manager_v1::WpCursorShapeManagerV1,
        },
        fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        presentation_time::client::wp_presentation::WpPresentation,
        primary_selection::zv1::client::zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1,
        security_context::v1::client::wp_security_context_manager_v1::WpSecurityContextManagerV1,
        single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1,
        viewporter::client::wp_viewporter::WpViewporter,
    },
    xdg::system_bell::v1::client::xdg_system_bell_v1::XdgSystemBellV1,
};

const PUBLIC: &[&str] = &[
    "ext_background_effect_manager_v1",
    "ext_idle_notifier_v1",
    "ext_workspace_manager_v1",
    "wl_compositor",
    "wl_data_device_manager",
    "wl_output",
    "wl_seat",
    "wl_shm",
    "wl_subcompositor",
    "wp_content_type_manager_v1",
    "wp_cursor_shape_manager_v1",
    "wp_fractional_scale_manager_v1",
    "wp_presentation",
    "wp_single_pixel_buffer_manager_v1",
    "wp_viewporter",
    "xdg_activation_v1",
    "xdg_system_bell_v1",
    "xdg_toplevel_icon_manager_v1",
    "xdg_wm_base",
    "zwp_idle_inhibit_manager_v1",
    "zwp_keyboard_shortcuts_inhibit_manager_v1",
    "zwp_linux_dmabuf_v1",
    "zwp_pointer_constraints_v1",
    "zwp_pointer_gestures_v1",
    "zwp_primary_selection_device_manager_v1",
    "zwp_relative_pointer_manager_v1",
    "zxdg_decoration_manager_v1",
    "zxdg_exporter_v2",
    "zxdg_importer_v2",
    "zxdg_output_manager_v1",
];

/// Hidden from sandboxed clients (`state::is_privileged`).
const PRIVILEGED: &[&str] = &[
    "ext_data_control_manager_v1",
    "ext_foreign_toplevel_image_capture_source_manager_v1",
    "ext_foreign_toplevel_list_v1",
    "ext_image_copy_capture_manager_v1",
    "ext_output_image_capture_source_manager_v1",
    "ext_session_lock_manager_v1",
    "wp_security_context_manager_v1",
    "zwlr_foreign_toplevel_manager_v1",
    "zwlr_gamma_control_manager_v1",
    "zwlr_layer_shell_v1",
    "zwlr_output_manager_v1",
    "zwlr_output_power_manager_v1",
    "zwlr_screencopy_manager_v1",
    "zwp_virtual_keyboard_manager_v1",
];

fn interfaces(client: &Client) -> Vec<String> {
    let mut names: Vec<String> = client
        .lock()
        .globals
        .iter()
        .map(|(_, i, _)| i.clone())
        .collect();
    names.sort();
    names.dedup();
    names
}

#[test]
fn pila_offers_exactly_these_globals() {
    let pila = TestPila::start();
    let client = pila.client();
    let mut expected: Vec<String> = PUBLIC
        .iter()
        .chain(PRIVILEGED)
        .map(|s| s.to_string())
        .collect();
    expected.sort();
    assert_eq!(interfaces(&client), expected);
}

#[test]
fn sandboxed_clients_dont_see_privileged_globals() {
    let pila = TestPila::start();
    let client = pila.client();
    let manager: WpSecurityContextManagerV1 = client.bind("wp_security_context_manager_v1", 1);
    let path = pila.dir.join("sandbox");
    let listener = UnixListener::bind(&path).unwrap();
    let mut fds = [0; 2];
    // SAFETY: two new fds, owned below.
    assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
    let (close_read, _close_write) = unsafe {
        use std::os::fd::{FromRawFd, OwnedFd};
        (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1]))
    };
    let context = manager.create_listener(
        listener.as_fd(),
        close_read.as_fd(),
        &client.qh,
        "context".into(),
    );
    context.set_sandbox_engine("org.flatpak".into());
    context.set_app_id("org.example.App".into());
    context.set_instance_id("1".into());
    context.commit();
    client.flush();
    client.roundtrip();
    let sandboxed = Client::connect(&path);
    let seen = interfaces(&sandboxed);
    for name in PRIVILEGED {
        assert!(
            !seen.contains(&name.to_string()),
            "a sandboxed client sees {name}"
        );
    }
    for name in PUBLIC {
        assert!(
            seen.contains(&name.to_string()),
            "a sandboxed client misses {name}"
        );
    }
    // It works as a normal client otherwise.
    sandboxed.window("flatpak").create();
}

// --- Bell -----------------------------------------------------------------------

#[test]
fn the_bell_flashes_the_window_that_rang() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").create();
    pila.wait_window("a");
    client.window("b").create();
    pila.wait_focus("b");
    let bell: XdgSystemBellV1 = client.bind("xdg_system_bell_v1", 1);
    bell.ring(Some(&client.surface(a)));
    client.flush();
    let state = pila.wait("a flashes", |s| {
        window_in(s, "a").unwrap()["flashing"] == true
    });
    assert_eq!(window_in(&state, "b").unwrap()["flashing"], false);
    pila.wait("the flash ends", |s| {
        window_in(s, "a").unwrap()["flashing"] == false
    });
}

#[test]
fn a_bell_without_a_surface_flashes_the_focused_window() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    let bell: XdgSystemBellV1 = client.bind("xdg_system_bell_v1", 1);
    bell.ring(None);
    client.flush();
    pila.wait("a flashes", |s| {
        window_in(s, "a").unwrap()["flashing"] == true
    });
}

#[test]
fn a_subsurfaces_bell_flashes_its_window() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").create();
    pila.wait_window("a");
    let (sub, _) = client.subsurface(
        &client.surface(a),
        "sub",
        (10, 10),
        (50, 50),
        [255, 0, 255, 0],
    );
    client.surface(a).commit();
    let bell: XdgSystemBellV1 = client.bind("xdg_system_bell_v1", 1);
    bell.ring(Some(&client.wl_surface_of(sub)));
    client.flush();
    pila.wait("a flashes", |s| {
        window_in(s, "a").unwrap()["flashing"] == true
    });
}

// --- Selections -----------------------------------------------------------------

#[test]
fn the_clipboard_goes_from_one_client_to_another() {
    let mut pila = TestPila::start();
    let source_client = pila.client();
    source_client.window("source").create();
    pila.wait_focus("source");
    let manager: WlDataDeviceManager = source_client.bind("wl_data_device_manager", 3);
    let device = manager.get_data_device(&source_client.seat(), &source_client.qh, "device".into());
    let source = source_client.data_source("source", "copied text");
    source_client.roundtrip();
    device.set_selection(Some(&source), source_client.last_serial());
    source_client.flush();
    source_client.roundtrip();

    let reader = pila.client();
    let reader_manager: WlDataDeviceManager = reader.bind("wl_data_device_manager", 3);
    let _reader_device =
        reader_manager.get_data_device(&reader.seat(), &reader.qh, "device".into());
    reader.window("reader").create();
    pila.wait_focus("reader");
    reader.wait("a selection offer", |s| s.data_offer.is_some());
    let offer = reader.lock().data_offer.clone().unwrap();
    let text = reader.receive(|fd| offer.receive("text/plain".into(), fd));
    assert_eq!(text, "copied text");
}

#[test]
fn an_unfocused_client_gets_no_offer() {
    let mut pila = TestPila::start();
    let source_client = pila.client();
    source_client.window("source").create();
    pila.wait_focus("source");
    let manager: WlDataDeviceManager = source_client.bind("wl_data_device_manager", 3);
    let device = manager.get_data_device(&source_client.seat(), &source_client.qh, "device".into());
    let source = source_client.data_source("source", "secret");
    device.set_selection(Some(&source), source_client.last_serial());
    source_client.flush();
    source_client.roundtrip();
    let other = pila.client();
    let other_manager: WlDataDeviceManager = other.bind("wl_data_device_manager", 3);
    let _device = other_manager.get_data_device(&other.seat(), &other.qh, "device".into());
    other.roundtrip();
    assert!(other.lock().data_offer.is_none());
}

#[test]
fn data_control_reads_and_sets_the_clipboard() {
    let mut pila = TestPila::start();
    let app = pila.client();
    app.window("app").create();
    pila.wait_focus("app");
    let manager: WlDataDeviceManager = app.bind("wl_data_device_manager", 3);
    let device = manager.get_data_device(&app.seat(), &app.qh, "device".into());
    let source = app.data_source("source", "from the app");
    device.set_selection(Some(&source), app.last_serial());
    app.flush();

    // A clipboard manager (wl-paste) reads it without focus.
    let manager_client = pila.client();
    let control: ExtDataControlManagerV1 = manager_client.bind("ext_data_control_manager_v1", 1);
    let _control_device =
        control.get_data_device(&manager_client.seat(), &manager_client.qh, "control".into());
    manager_client.flush();
    manager_client.wait("an offer", |s| {
        s.control_offer.is_some() && s.has_logged("data_control_offer", "from the app")
            || s.control_offer.is_some()
    });
    let offer = manager_client.lock().control_offer.clone().unwrap();
    let text = manager_client.receive(|fd| offer.receive("text/plain".into(), fd));
    assert_eq!(text, "from the app");

    // And sets it (wl-copy): the focused app gets the new offer.
    let new_source = control.create_data_source(
        &manager_client.qh,
        common::protocols::SourceText {
            tag: "control_source".into(),
            text: "from wl-copy".into(),
        },
    );
    new_source.offer("text/plain".into());
    _control_device.set_selection(Some(&new_source));
    manager_client.flush();
    app.wait("the new offer", |s| {
        s.has_logged("device", "Selection") && s.data_offer.is_some()
    });
    app.roundtrip();
    let offer = app.lock().data_offer.clone().unwrap();
    let text = app.receive(|fd| offer.receive("text/plain".into(), fd));
    assert_eq!(text, "from wl-copy");
}

#[test]
fn the_primary_selection_goes_between_clients() {
    let mut pila = TestPila::start();
    let source_client = pila.client();
    source_client.window("source").create();
    pila.wait_focus("source");
    let manager: ZwpPrimarySelectionDeviceManagerV1 =
        source_client.bind("zwp_primary_selection_device_manager_v1", 1);
    let device = manager.get_device(&source_client.seat(), &source_client.qh, "primary".into());
    let source = manager.create_source(
        &source_client.qh,
        common::protocols::SourceText {
            tag: "primary_source".into(),
            text: "selected".into(),
        },
    );
    source.offer("text/plain".into());
    device.set_selection(Some(&source), source_client.last_serial());
    source_client.flush();
    source_client.roundtrip();

    let reader = pila.client();
    let reader_manager: ZwpPrimarySelectionDeviceManagerV1 =
        reader.bind("zwp_primary_selection_device_manager_v1", 1);
    let _device = reader_manager.get_device(&reader.seat(), &reader.qh, "primary".into());
    reader.window("reader").create();
    pila.wait_focus("reader");
    reader.wait("a primary offer", |s| s.primary_offer.is_some());
    let offer = reader.lock().primary_offer.clone().unwrap();
    let text = reader.receive(|fd| offer.receive("text/plain".into(), fd));
    assert_eq!(text, "selected");
}

// --- Scales and viewports ---------------------------------------------------------

#[test]
fn a_fractional_output_scale_reaches_the_window() {
    let mut pila = TestPila::with_config("[[output]]\nname = \"HEADLESS-1\"\nscale = 1.5\n");
    let client = pila.client();
    let fractional: WpFractionalScaleManagerV1 = client.bind("wp_fractional_scale_manager_v1", 1);
    let window = client.window("a").create();
    let _scale =
        fractional.get_fractional_scale(&client.surface(window), &client.qh, "fractional".into());
    client.flush();
    pila.wait_window("a");
    client.wait_logged("fractional", "PreferredScale { scale: 180 }");
    client.wait("the buffer scale", |s| {
        s.windows[window.0].preferred_scale == Some(2)
    });
    let state = pila.state();
    // The output is 1280x800 pixels: about 853x533 logical.
    let (_, _, w, h) = rect(&state["outputs"][0]["geometry"]);
    assert!((w - 853).abs() <= 1 && (h - 533).abs() <= 1, "{w}x{h}");
}

#[test]
fn a_window_moved_to_another_output_gets_its_scale() {
    let mut pila = TestPila::start_with(Options {
        config: "[[output]]\nname = \"B\"\nscale = 2\n".into(),
        outputs: Some("A:1280x800,B:1280x800".into()),
        ..Options::default()
    });
    let client = pila.client();
    let window = client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    assert_ne!(client.with_window(window, |w| w.preferred_scale), Some(2));
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    pila.move_to((x + 100) as f64, (y + 100) as f64);
    pila.key_press("Super");
    pila.press("left");
    pila.move_by(900.0, 0.0);
    pila.release("left");
    pila.key_release("Super");
    pila.wait("a on B", |s| window_in(s, "a").unwrap()["output"] == "B");
    client.wait("scale 2", |s| {
        s.windows[window.0].preferred_scale == Some(2)
    });
    client.wait("entered B", |s| {
        s.windows[window.0].outputs.contains(&"B".to_string())
    });
}

#[test]
fn a_viewport_sets_the_window_size_and_single_pixel_buffers_fill_it() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("a").create();
    client.set_auto_draw(window, false);
    client.wait("the first configure", |s| {
        !s.windows[window.0].configures.is_empty()
    });
    let viewporter: WpViewporter = client.bind("wp_viewporter", 1);
    let pixels: WpSinglePixelBufferManagerV1 = client.bind("wp_single_pixel_buffer_manager_v1", 1);
    let surface = client.surface(window);
    let viewport = viewporter.get_viewport(&surface, &client.qh, "viewport".into());
    viewport.set_destination(300, 200);
    // Opaque green: 32-bit channels.
    let buffer = pixels.create_u32_rgba_buffer(0, u32::MAX, 0, u32::MAX, &client.qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 1, 1);
    surface.commit();
    client.flush();
    let w = pila.wait_window("a");
    assert_eq!((rect(&w["geometry"]).2, rect(&w["geometry"]).3), (300, 200));
    pila.settle();
    let (x, y, ..) = rect(&w["geometry"]);
    assert_eq!(
        pila.pixel((x + 150) as f64, (y + 100) as f64),
        [0, 255, 0, 255]
    );
}

// --- Cursors --------------------------------------------------------------------

fn pointer(client: &Client) -> WlPointer {
    client.lock().pointer.clone().expect("a pointer")
}

#[test]
fn a_client_sets_named_hidden_and_surface_cursors() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    pila.move_to((x + 50) as f64, (y + 50) as f64);
    client.wait("pointer enter", |s| s.pointer_name.as_deref() == Some("a"));
    let serial = client.lock().pointer_enter_serial;
    let shapes: WpCursorShapeManagerV1 = client.bind("wp_cursor_shape_manager_v1", 1);
    let device = shapes.get_pointer(&pointer(&client), &client.qh, "shape".into());
    device.set_shape(serial, Shape::Text);
    client.flush();
    pila.wait("a text cursor", |s| s["pointer"]["cursor"] == "text");
    pointer(&client).set_cursor(serial, None, 0, 0);
    client.flush();
    pila.wait("no cursor", |s| s["pointer"]["cursor"] == "hidden");
    let (cursor, _) = {
        let compositor = client.lock().compositor.clone().unwrap();
        let surface = compositor.create_surface(&client.qh, ());
        let shm = client.lock().shm.clone().unwrap();
        common::client::paint(&shm, &client.qh, &surface, [255, 255, 255, 255], (16, 16));
        (surface, ())
    };
    pointer(&client).set_cursor(serial, Some(&cursor), 0, 0);
    client.flush();
    pila.wait("a surface cursor", |s| s["pointer"]["cursor"] == "surface");
    // Leaving the window: the default arrow again.
    pila.move_to(5.0, 5.0);
    pila.wait("the default cursor", |s| {
        s["pointer"]["cursor"] == "default"
    });
}

#[test]
fn a_stale_serial_cant_set_the_cursor() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    pila.move_to((x + 50) as f64, (y + 50) as f64);
    client.wait("pointer enter", |s| s.pointer_name.as_deref() == Some("a"));
    let old = client.lock().pointer_enter_serial;
    pila.move_to(5.0, 5.0);
    pila.move_to((x + 50) as f64, (y + 50) as f64);
    client.wait("a second enter", |s| s.pointer_enter_serial != old);
    pointer(&client).set_cursor(old, None, 0, 0);
    client.flush();
    client.roundtrip();
    pila.stays("not hidden", std::time::Duration::from_millis(100), |s| {
        s["pointer"]["cursor"] != "hidden"
    });
}

// --- Subsurfaces ------------------------------------------------------------------

#[test]
fn a_subsurface_is_drawn_and_gets_the_pointer() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client
        .window("a")
        .size(400, 300)
        .color([255, 255, 0, 0])
        .create();
    pila.wait_focus("a");
    client.subsurface(
        &client.surface(a),
        "sub",
        (20, 30),
        (60, 40),
        [255, 0, 255, 0],
    );
    client.surface(a).commit();
    client.flush();
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    pila.wait("drawn", |_| true);
    pila.settle();
    assert_eq!(
        pila.pixel((x + 40) as f64, (y + 50) as f64),
        [0, 255, 0, 255]
    );
    assert_eq!(
        pila.pixel((x + 200) as f64, (y + 200) as f64),
        [255, 0, 0, 255]
    );
    pila.move_to((x + 40) as f64, (y + 50) as f64);
    client.wait("the pointer on the subsurface", |s| {
        s.pointer_name.as_deref() == Some("sub")
    });
}

// --- Frames ---------------------------------------------------------------------

#[test]
fn frame_callbacks_and_presentation_feedback_follow_a_commit() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("a").create();
    pila.wait_window("a");
    let presentation: WpPresentation = client.bind("wp_presentation", 2);
    let surface = client.surface(window);
    let _feedback = presentation.feedback(&surface, &client.qh, "feedback".into());
    let _frame = surface.frame(&client.qh, "frame".to_string());
    client.draw(window, None);
    client.wait_logged("frame", "Done");
    client.wait_logged("feedback", "Presented");
    assert!(client.has_logged("wp_presentation", "ClockId"));
}

#[test]
fn a_covered_window_gets_frame_callbacks_about_once_a_second() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    client.window("cover").fullscreen().create();
    pila.wait("the cover", |s| {
        window_in(s, "cover").is_some_and(|w| rect(&w["geometry"]) == (0, 0, 1280, 800))
    });
    pila.settle();
    // An animating client: a frame callback with every commit.
    let surface = client.surface(window);
    let started = std::time::Instant::now();
    let mut done = 0;
    while started.elapsed() < std::time::Duration::from_millis(2500) {
        let tag = format!("frame{done}");
        let _frame = surface.frame(&client.qh, tag.clone());
        client.draw(window, None);
        client.wait_logged(&tag, "Done");
        done += 1;
    }
    // Not at 60 Hz: about one per second.
    assert!((2..=4).contains(&done), "{done} frame callbacks in 2.5 s");
}
