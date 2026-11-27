//! Protocols about input: idle notifications and inhibitors, pointer
//! constraints and relative motion, keyboard-shortcuts inhibitors, the
//! virtual keyboard, xdg-activation.

mod common;

use std::time::Duration;

use common::client::Ev;
use common::protocols::{keymap_of, virtual_keyboard, LayerSpec};
use common::*;
use wayland_client::protocol::wl_pointer::WlPointer;
use wayland_protocols::{
    ext::idle_notify::v1::client::ext_idle_notifier_v1::ExtIdleNotifierV1,
    wp::{
        idle_inhibit::zv1::client::zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1,
        keyboard_shortcuts_inhibit::zv1::client::zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1,
        pointer_constraints::zv1::client::zwp_pointer_constraints_v1::{
            Lifetime, ZwpPointerConstraintsV1,
        },
        relative_pointer::zv1::client::zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1,
    },
    xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1,
};

fn pointer(client: &Client) -> WlPointer {
    client.lock().pointer.clone().expect("a pointer")
}

// --- Idle --------------------------------------------------------------------

#[test]
fn idle_notifications_fire_after_the_timeout_and_resume_on_input() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let notifier: ExtIdleNotifierV1 = client.bind("ext_idle_notifier_v1", 1);
    let _n = notifier.get_idle_notification(100, &client.seat(), &client.qh, "idle".into());
    client.flush();
    client.wait_logged("idle", "Idled");
    pila.move_by(5.0, 5.0);
    client.wait_logged("idle", "Resumed");
}

#[test]
fn an_idle_inhibitor_on_a_shown_window_stops_idling() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("video").create();
    pila.wait_window("video");
    let manager: ZwpIdleInhibitManagerV1 = client.bind("zwp_idle_inhibit_manager_v1", 1);
    let inhibitor =
        manager.create_inhibitor(&client.surface(window), &client.qh, "inhibitor".into());
    let notifier: ExtIdleNotifierV1 = client.bind("ext_idle_notifier_v1", 1);
    let _n = notifier.get_idle_notification(100, &client.seat(), &client.qh, "idle".into());
    client.flush();
    std::thread::sleep(Duration::from_millis(400));
    client.roundtrip();
    assert!(
        !client.has_logged("idle", "Idled"),
        "idled despite the inhibitor"
    );
    inhibitor.destroy();
    client.flush();
    client.wait_logged("idle", "Idled");
}

#[test]
fn an_inhibitor_on_a_minimized_window_doesnt_count() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("video").create();
    pila.wait_focus("video");
    let manager: ZwpIdleInhibitManagerV1 = client.bind("zwp_idle_inhibit_manager_v1", 1);
    let _inhibitor =
        manager.create_inhibitor(&client.surface(window), &client.qh, "inhibitor".into());
    pila.combo("Super+n");
    pila.wait("minimized", |s| {
        window_in(s, "video").unwrap()["minimized"] == true
    });
    let notifier: ExtIdleNotifierV1 = client.bind("ext_idle_notifier_v1", 1);
    let _n = notifier.get_idle_notification(100, &client.seat(), &client.qh, "idle".into());
    client.flush();
    client.wait_logged("idle", "Idled");
}

/// Pila on a session bus of the test's own, where it serves
/// `org.freedesktop.ScreenSaver` (`screensaver.rs`), and a client.
fn on_a_bus() -> (TestPila, notifications::Bus, Client) {
    let bus = notifications::Bus::start();
    let pila = TestPila::start_with(Options {
        env: vec![bus.env()],
        ..Default::default()
    });
    let client = pila.client();
    (pila, bus, client)
}

/// An app on `bus`, as Firefox calls the screensaver.
fn app_on(bus: &notifications::Bus) -> zbus::Connection {
    zbus::block_on(zbus::connection::Builder::address(bus.address.as_str()).build()).unwrap()
}

/// `Inhibit` at `path`: tried again until Pila has taken the name.
fn inhibit(app: &zbus::Connection, path: &str) -> u32 {
    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        let reply = zbus::block_on(app.call_method(
            Some("org.freedesktop.ScreenSaver"),
            path,
            Some("org.freedesktop.ScreenSaver"),
            "Inhibit",
            &("Firefox", "video-playing"),
        ));
        match reply {
            Ok(reply) => return reply.body().deserialize().unwrap(),
            Err(_) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(err) => panic!("no screensaver on the bus: {err}"),
        }
    }
}

fn uninhibit(app: &zbus::Connection, path: &str, cookie: u32) {
    zbus::block_on(app.call_method(
        Some("org.freedesktop.ScreenSaver"),
        path,
        Some("org.freedesktop.ScreenSaver"),
        "UnInhibit",
        &(cookie,),
    ))
    .unwrap();
}

/// `method` of the screensaver at its full path, with `body`.
fn screensaver_call<B>(
    app: &zbus::Connection,
    method: &str,
    body: &B,
) -> zbus::Result<zbus::Message>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    zbus::block_on(app.call_method(
        Some("org.freedesktop.ScreenSaver"),
        "/org/freedesktop/ScreenSaver",
        Some("org.freedesktop.ScreenSaver"),
        method,
        body,
    ))
}

/// `GetActive`'s answer.
fn screensaver_active(app: &zbus::Connection) -> bool {
    screensaver_call(app, "GetActive", &())
        .unwrap()
        .body()
        .deserialize()
        .unwrap()
}

/// An idle notification of 100 ms.
fn idle_notification(
    client: &Client,
) -> wayland_protocols::ext::idle_notify::v1::client::ext_idle_notification_v1::ExtIdleNotificationV1
{
    let notifier: ExtIdleNotifierV1 = client.bind("ext_idle_notifier_v1", 1);
    let n = notifier.get_idle_notification(100, &client.seat(), &client.qh, "idle".into());
    client.flush();
    n
}

#[test]
fn a_dbus_screensaver_inhibit_stops_idling() {
    let (mut pila, bus, client) = on_a_bus();
    let app = app_on(&bus);
    for path in ["/org/freedesktop/ScreenSaver", "/ScreenSaver"] {
        let cookie = inhibit(&app, path);
        let state = pila.wait("inhibited", |s| s["idle_inhibited"] == true);
        assert_eq!(state["idle_inhibitors"][0]["app"], "Firefox");
        assert_eq!(state["idle_inhibitors"][0]["reason"], "video-playing");
        let _n = idle_notification(&client);
        std::thread::sleep(Duration::from_millis(400));
        client.roundtrip();
        assert!(
            !client.has_logged("idle", "Idled"),
            "idled despite the inhibit at {path}"
        );
        uninhibit(&app, path, cookie);
        client.wait_logged("idle", "Idled");
        pila.wait("not inhibited", |s| s["idle_inhibited"] == false);
        client.lock().log.clear();
    }
}

#[test]
fn a_dbus_inhibit_ends_when_its_app_leaves_the_bus() {
    let (mut pila, bus, client) = on_a_bus();
    let app = app_on(&bus);
    inhibit(&app, "/org/freedesktop/ScreenSaver");
    pila.wait("inhibited", |s| s["idle_inhibited"] == true);
    drop(app);
    pila.wait("not inhibited", |s| {
        s["idle_inhibited"] == false && s["idle_inhibitors"] == serde_json::json!([])
    });
    let _n = idle_notification(&client);
    client.wait_logged("idle", "Idled");
}

#[test]
fn a_dbus_inhibit_cookie_is_given_back_only_by_its_app() {
    let (mut pila, bus, _client) = on_a_bus();
    let app = app_on(&bus);
    let other = app_on(&bus);
    let cookie = inhibit(&app, "/org/freedesktop/ScreenSaver");
    pila.wait("inhibited", |s| s["idle_inhibited"] == true);
    uninhibit(&other, "/org/freedesktop/ScreenSaver", cookie);
    pila.stays("inhibited", Duration::from_millis(300), |s| {
        s["idle_inhibited"] == true
    });
    uninhibit(&app, "/org/freedesktop/ScreenSaver", cookie);
    pila.wait("not inhibited", |s| s["idle_inhibited"] == false);
}

/// `SimulateUserActivity` counts as activity: an idle client resumes.
#[test]
fn a_dbus_simulate_user_activity_counts_as_activity() {
    let (_pila, bus, client) = on_a_bus();
    let app = app_on(&bus);
    // Served once an inhibit gets through.
    let cookie = inhibit(&app, "/org/freedesktop/ScreenSaver");
    uninhibit(&app, "/org/freedesktop/ScreenSaver", cookie);
    let _n = idle_notification(&client);
    client.wait_logged("idle", "Idled");
    screensaver_call(&app, "SimulateUserActivity", &()).unwrap();
    client.wait_logged("idle", "Resumed");
}

/// `GetActive` answers whether the screen is locked.
#[test]
fn the_dbus_screensaver_is_active_while_the_session_is_locked() {
    let (mut pila, bus, _client) = on_a_bus();
    let app = app_on(&bus);
    let cookie = inhibit(&app, "/org/freedesktop/ScreenSaver");
    uninhibit(&app, "/org/freedesktop/ScreenSaver", cookie);
    assert!(!screensaver_active(&app));
    let locker = pila.client();
    let lock = locker.lock_session();
    locker.lock_surface(&lock, "HEADLESS-1", [0, 0, 0, 255]);
    locker.wait_logged("lock", "Locked");
    pila.wait("locked", |s| s["locked"] == true);
    assert!(screensaver_active(&app));
    lock.unlock_and_destroy();
    locker.flush();
    pila.wait("unlocked", |s| s["locked"] == false);
    assert!(!screensaver_active(&app));
}

/// `pila msg idle-inhibitors` lists what keeps the screen on: an app's
/// D-Bus cookie, a visible window's Wayland inhibitor, not a minimized one's.
#[test]
fn pila_msg_lists_what_keeps_the_screen_on() {
    let (mut pila, bus, client) = on_a_bus();
    let listed = |pila: &TestPila| -> serde_json::Value {
        let out = pila.msg(&["--json", "idle-inhibitors"]);
        assert!(out.status.success(), "{out:?}");
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()["idle_inhibitors"].clone()
    };
    assert_eq!(listed(&pila), serde_json::json!([]));
    let out = pila.msg(&["idle-inhibitors"]);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "nothing keeps the screen on"
    );

    let app = app_on(&bus);
    let cookie = inhibit(&app, "/org/freedesktop/ScreenSaver");
    let manager: ZwpIdleInhibitManagerV1 = client.bind("zwp_idle_inhibit_manager_v1", 1);
    let video = client.window("video").create();
    pila.wait_window("video");
    let _shown = manager.create_inhibitor(&client.surface(video), &client.qh, "shown".into());
    let hidden = client.window("hidden").create();
    pila.wait_focus("hidden");
    let _hidden = manager.create_inhibitor(&client.surface(hidden), &client.qh, "hidden".into());
    client.flush();
    pila.combo("Super+n");
    pila.wait("minimized", |s| {
        window_in(s, "hidden").unwrap()["minimized"] == true
    });
    let expected = serde_json::json!([
        { "source": "dbus", "app": "Firefox", "reason": "video-playing" },
        { "source": "wayland", "app": "video", "reason": null },
    ]);
    let deadline = std::time::Instant::now() + TIMEOUT;
    while listed(&pila) != expected {
        assert!(std::time::Instant::now() < deadline, "{}", listed(&pila));
        std::thread::sleep(Duration::from_millis(20));
    }
    let out = String::from_utf8_lossy(&pila.msg(&["idle-inhibitors"]).stdout).to_string();
    assert_eq!(out.trim(), "dbus  Firefox  (video-playing)\nwayland  video");

    uninhibit(&app, "/org/freedesktop/ScreenSaver", cookie);
    let deadline = std::time::Instant::now() + TIMEOUT;
    while listed(&pila)[0]["source"] != "wayland" {
        assert!(std::time::Instant::now() < deadline, "{}", listed(&pila));
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Another program has `org.freedesktop.ScreenSaver`: Pila leaves it to
/// them, and serves nothing at its paths on its own connection either.
#[test]
fn a_screensaver_name_someone_else_has_leaves_no_objects_behind() {
    let bus = notifications::Bus::start();
    let other = app_on(&bus);
    zbus::block_on(other.request_name("org.freedesktop.ScreenSaver")).unwrap();
    let mut pila = TestPila::start_with(Options {
        env: vec![bus.env()],
        ..Default::default()
    });
    // Logged once the screensaver's serving is over.
    pila.wait_log("portal: serving ScreenCast");
    let app = app_on(&bus);
    let reply = zbus::block_on(app.call_method(
        Some("org.freedesktop.impl.portal.desktop.pila"),
        "/org/freedesktop/ScreenSaver",
        Some("org.freedesktop.ScreenSaver"),
        "Inhibit",
        &("Firefox", "video-playing"),
    ));
    assert!(
        matches!(&reply, Err(zbus::Error::FDO(err))
            if matches!(**err, zbus::fdo::Error::UnknownObject(_))),
        "{reply:?}"
    );
    // The other program still has the name.
    let owner = zbus::block_on(async {
        zbus::fdo::DBusProxy::new(&app)
            .await?
            .get_name_owner("org.freedesktop.ScreenSaver".try_into().unwrap())
            .await
    })
    .unwrap();
    assert_eq!(owner.as_str(), other.unique_name().unwrap().as_str());
}

// --- Pointer constraints -----------------------------------------------------

#[test]
fn a_locked_pointer_stays_put_and_gets_relative_motion() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("game").size(400, 300).create();
    pila.wait_focus("game");
    let (x, y, ..) = rect(&pila.window("game")["geometry"]);
    pila.move_to((x + 100) as f64, (y + 100) as f64);
    client.wait("pointer enter", |s| {
        s.pointer_name.as_deref() == Some("game")
    });
    let relative: ZwpRelativePointerManagerV1 = client.bind("zwp_relative_pointer_manager_v1", 1);
    let _rel = relative.get_relative_pointer(&pointer(&client), &client.qh, "relative".into());
    let constraints: ZwpPointerConstraintsV1 = client.bind("zwp_pointer_constraints_v1", 1);
    let _locked = constraints.lock_pointer(
        &client.surface(window),
        &pointer(&client),
        None,
        Lifetime::Persistent,
        &client.qh,
        "locked".into(),
    );
    client.flush();
    client.wait_logged("locked", "Locked");
    pila.wait("constrained", |s| s["pointer"]["constrained"] == true);
    pila.move_by(50.0, 30.0);
    client.wait_logged("relative", "RelativeMotion");
    assert!(client.has_logged("relative", "dx: 50.0"));
    let state = pila.state();
    assert_eq!(
        (
            state["pointer"]["x"].as_f64().unwrap(),
            state["pointer"]["y"].as_f64().unwrap()
        ),
        ((x + 100) as f64, (y + 100) as f64)
    );
    // Mod+Tab to another window releases it.
    let other = pila.client();
    other.window("other").create();
    pila.wait_focus("other");
    client.wait_logged("locked", "Unlocked");
    pila.wait("released", |s| s["pointer"]["constrained"] == false);
}

#[test]
fn going_to_another_output_leaves_a_locked_pointer_where_it_is() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1280x800".into()),
        ..Options::default()
    });
    let client = pila.client();
    let window = client.window("game").size(400, 300).create();
    pila.wait_focus("game");
    let (x, y, ..) = rect(&pila.window("game")["geometry"]);
    pila.move_to((x + 100) as f64, (y + 100) as f64);
    client.wait("pointer enter", |s| {
        s.pointer_name.as_deref() == Some("game")
    });
    let constraints: ZwpPointerConstraintsV1 = client.bind("zwp_pointer_constraints_v1", 1);
    let _locked = constraints.lock_pointer(
        &client.surface(window),
        &pointer(&client),
        None,
        Lifetime::Persistent,
        &client.qh,
        "locked".into(),
    );
    client.flush();
    client.wait_logged("locked", "Locked");
    pila.wait("constrained", |s| s["pointer"]["constrained"] == true);
    pila.combo("Super+o");
    pila.wait("focus on the other output", |s| s["focused"].is_null());
    assert_eq!(
        pila.state()["pointer"]["x"].as_f64().unwrap(),
        (x + 100) as f64
    );
}

#[test]
fn a_lock_only_activates_on_the_focused_window_under_the_pointer() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("game").size(400, 300).create();
    pila.wait_focus("game");
    // The pointer is elsewhere.
    pila.move_to(5.0, 5.0);
    let constraints: ZwpPointerConstraintsV1 = client.bind("zwp_pointer_constraints_v1", 1);
    let _locked = constraints.lock_pointer(
        &client.surface(window),
        &pointer(&client),
        None,
        Lifetime::Persistent,
        &client.qh,
        "locked".into(),
    );
    client.flush();
    client.roundtrip();
    assert!(!client.has_logged("locked", "Locked"));
    let (x, y, ..) = rect(&pila.window("game")["geometry"]);
    pila.move_to((x + 50) as f64, (y + 50) as f64);
    client.wait_logged("locked", "Locked");
}

#[test]
fn a_confined_pointer_stays_inside_the_surface() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("game").size(400, 300).create();
    pila.wait_focus("game");
    let (x, y, w, h) = rect(&pila.window("game")["geometry"]);
    pila.move_to((x + 100) as f64, (y + 100) as f64);
    let constraints: ZwpPointerConstraintsV1 = client.bind("zwp_pointer_constraints_v1", 1);
    let _confined = constraints.confine_pointer(
        &client.surface(window),
        &pointer(&client),
        None,
        Lifetime::Persistent,
        &client.qh,
        "confined".into(),
    );
    client.flush();
    client.wait_logged("confined", "Confined");
    pila.move_by(1000.0, 1000.0);
    let state = pila.settle();
    let (px, py) = (
        state["pointer"]["x"].as_f64().unwrap(),
        state["pointer"]["y"].as_f64().unwrap(),
    );
    assert!(
        px < (x + w) as f64 && py < (y + h) as f64 && px >= x as f64 && py >= y as f64,
        "pointer at {px},{py} left {x},{y} {w}x{h}"
    );
}

/// A locked client draws its own cursor and may say where it is
/// (`set_cursor_position_hint`, applied on commit): Pila's pointer goes
/// there, so unlocking doesn't jump, but never outside the window.
#[test]
fn a_locked_pointer_follows_the_clients_hint_within_its_window() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("game").size(400, 300).create();
    pila.wait_focus("game");
    let (x, y, _, h) = rect(&pila.window("game")["geometry"]);
    pila.move_to((x + 100) as f64, (y + 100) as f64);
    let constraints: ZwpPointerConstraintsV1 = client.bind("zwp_pointer_constraints_v1", 1);
    let locked = constraints.lock_pointer(
        &client.surface(window),
        &pointer(&client),
        None,
        Lifetime::Persistent,
        &client.qh,
        "locked".into(),
    );
    client.flush();
    client.wait_logged("locked", "Locked");
    let at = |s: &Value| (s["pointer"]["x"].as_f64(), s["pointer"]["y"].as_f64());
    locked.set_cursor_position_hint(50.0, 60.0);
    client.surface(window).commit();
    client.flush();
    pila.wait("the pointer at the hint", |s| {
        at(s) == (Some((x + 50) as f64), Some((y + 60) as f64))
    });
    // Past the window's edges: its last pixels.
    locked.set_cursor_position_hint(-20.0, 5000.0);
    client.surface(window).commit();
    client.flush();
    pila.wait("the pointer clamped to the window", |s| {
        at(s) == (Some(x as f64), Some((y + h - 1) as f64))
    });
}

/// A layer surface with the keyboard (a full-screen overlay, a game's
/// launcher) can lock the pointer too, and its cursor hint moves Pila's
/// pointer from the layer surface's own origin, kept on it.
#[test]
fn a_layer_surface_can_lock_the_pointer_and_hint_where_it_is() {
    use wayland_protocols_wlr::layer_shell::v1::client::{
        zwlr_layer_shell_v1::Layer, zwlr_layer_surface_v1::KeyboardInteractivity,
    };
    let mut pila = TestPila::start();
    let client = pila.client();
    let (overlay, _layer) = client.layer_surface(
        "overlay",
        &LayerSpec {
            layer: Layer::Overlay,
            keyboard: KeyboardInteractivity::Exclusive,
            size: (300, 200),
            ..LayerSpec::default()
        },
        [255, 0, 0, 255],
    );
    client.wait_surface_configured(overlay);
    let state = pila.wait("the overlay with the keyboard", |s| {
        s["keyboard_focus"] == "layer:overlay"
    });
    let geometry = |s: &Value| {
        s["layers"]
            .as_array()
            .and_then(|l| l.iter().find(|l| l["namespace"] == "overlay"))
            .map(|l| rect(&l["geometry"]))
    };
    let state = if geometry(&state).is_some_and(|g| g.2 > 0) {
        state
    } else {
        pila.wait("its size", |s| geometry(s).is_some_and(|g| g.2 > 0))
    };
    let (x, y, w, h) = geometry(&state).unwrap();
    assert_eq!((w, h), (300, 200));
    pila.move_to((x + 10) as f64, (y + 10) as f64);
    client.wait("pointer enter", |s| {
        s.pointer_name.as_deref() == Some("overlay")
    });

    let constraints: ZwpPointerConstraintsV1 = client.bind("zwp_pointer_constraints_v1", 1);
    let surface = client.wl_surface_of(overlay);
    let locked = constraints.lock_pointer(
        &surface,
        &pointer(&client),
        None,
        Lifetime::Persistent,
        &client.qh,
        "locked".into(),
    );
    client.flush();
    client.wait_logged("locked", "Locked");
    let at = |s: &Value| (s["pointer"]["x"].as_f64(), s["pointer"]["y"].as_f64());
    locked.set_cursor_position_hint(120.0, 80.0);
    surface.commit();
    client.flush();
    pila.wait("the pointer at the hint", |s| {
        at(s) == (Some((x + 120) as f64), Some((y + 80) as f64))
    });
    locked.set_cursor_position_hint(5000.0, -20.0);
    surface.commit();
    client.flush();
    pila.wait("the pointer kept on the overlay", |s| {
        at(s) == (Some((x + w - 1) as f64), Some(y as f64))
    });
}

#[test]
fn switching_to_an_empty_workspace_releases_a_locked_pointer() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("game").size(400, 300).create();
    pila.wait_focus("game");
    let (x, y, ..) = rect(&pila.window("game")["geometry"]);
    pila.move_to((x + 100) as f64, (y + 100) as f64);
    let constraints: ZwpPointerConstraintsV1 = client.bind("zwp_pointer_constraints_v1", 1);
    let _locked = constraints.lock_pointer(
        &client.surface(window),
        &pointer(&client),
        None,
        Lifetime::Persistent,
        &client.qh,
        "locked".into(),
    );
    client.flush();
    client.wait_logged("locked", "Locked");
    pila.combo("Super+2");
    client.wait_logged("locked", "Unlocked");
    pila.move_by(100.0, 0.0);
    let state = pila.wait("the pointer moves", |s| {
        s["pointer"]["constrained"] == false
    });
    assert_eq!(state["pointer"]["x"].as_f64(), Some((x + 200) as f64));
}

#[test]
fn a_oneshot_lock_ends_when_released() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("game").size(400, 300).create();
    pila.wait_focus("game");
    let (x, y, ..) = rect(&pila.window("game")["geometry"]);
    pila.move_to((x + 100) as f64, (y + 100) as f64);
    let constraints: ZwpPointerConstraintsV1 = client.bind("zwp_pointer_constraints_v1", 1);
    let _locked = constraints.lock_pointer(
        &client.surface(window),
        &pointer(&client),
        None,
        Lifetime::Oneshot,
        &client.qh,
        "locked".into(),
    );
    client.flush();
    client.wait_logged("locked", "Locked");
    pila.combo("Super+n");
    pila.wait("minimized", |s| {
        window_in(s, "game").unwrap()["minimized"] == true
    });
    client.wait_logged("locked", "Unlocked");
    pila.combo("Super+Tab");
    pila.wait_focus("game");
    pila.move_by(1.0, 1.0);
    client.roundtrip();
    assert_eq!(
        client
            .logged("locked")
            .iter()
            .filter(|e| e.contains("Locked"))
            .count(),
        1,
        "a oneshot lock activated again"
    );
}

// --- Shortcuts inhibitor -----------------------------------------------------

#[test]
fn an_inhibitor_gets_the_bindings_keys_but_not_quit() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("vm").create();
    pila.wait_focus("vm");
    let manager: ZwpKeyboardShortcutsInhibitManagerV1 =
        client.bind("zwp_keyboard_shortcuts_inhibit_manager_v1", 1);
    let inhibitor = manager.inhibit_shortcuts(
        &client.surface(window),
        &client.seat(),
        &client.qh,
        "inhibitor".into(),
    );
    client.flush();
    client.wait_logged("inhibitor", "Active");
    pila.combo("Super+Shift+q");
    client.wait("q to reach the client", |s| {
        s.events.iter().any(|e| {
            *e == Ev::Key {
                key: 16,
                pressed: false,
            }
        })
    });
    assert!(!client.with_window(window, |w| w.close_requested));
    // Destroyed: bindings work again.
    inhibitor.destroy();
    client.flush();
    client.roundtrip();
    pila.combo("Super+Shift+q");
    client.wait("close request", |s| s.windows[window.0].close_requested);
}

#[test]
fn an_inhibitor_keeps_a_mod_tap_from_running() {
    let mut pila = TestPila::with_config("launcher = \"exec sleep 600\"\n");
    let client = pila.client();
    let window = client.window("vm").create();
    pila.wait_focus("vm");
    let manager: ZwpKeyboardShortcutsInhibitManagerV1 =
        client.bind("zwp_keyboard_shortcuts_inhibit_manager_v1", 1);
    let inhibitor = manager.inhibit_shortcuts(
        &client.surface(window),
        &client.seat(),
        &client.qh,
        "inhibitor".into(),
    );
    client.flush();
    client.wait_logged("inhibitor", "Active");
    pila.key("Super");
    assert_eq!(pila.state()["children"], 0);
    inhibitor.destroy();
    client.flush();
    client.roundtrip();
    pila.key("Super");
    assert_eq!(pila.state()["children"], 1);
}

#[test]
fn quit_still_works_with_an_inhibitor() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("vm").create();
    pila.wait_focus("vm");
    let manager: ZwpKeyboardShortcutsInhibitManagerV1 =
        client.bind("zwp_keyboard_shortcuts_inhibit_manager_v1", 1);
    let _inhibitor = manager.inhibit_shortcuts(
        &client.surface(window),
        &client.seat(),
        &client.qh,
        "inhibitor".into(),
    );
    client.flush();
    client.wait_logged("inhibitor", "Active");
    // Its dialog opens, and takes the keys from the inhibiting window.
    pila.combo("Super+Shift+e");
    pila.wait("the quit dialog", |s| {
        s["quit_dialog"]["shown"]["text"] == true
    });
    pila.key("Return");
    assert!(pila.wait_exit().success());
}

#[test]
fn an_inhibitor_only_counts_while_its_surface_has_focus() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let vm = client.window("vm").create();
    pila.wait_focus("vm");
    let manager: ZwpKeyboardShortcutsInhibitManagerV1 =
        client.bind("zwp_keyboard_shortcuts_inhibit_manager_v1", 1);
    let _inhibitor = manager.inhibit_shortcuts(
        &client.surface(vm),
        &client.seat(),
        &client.qh,
        "inhibitor".into(),
    );
    client.flush();
    client.wait_logged("inhibitor", "Active");
    let other = client.window("other").create();
    pila.wait_focus("other");
    pila.combo("Super+Shift+q");
    client.wait("other asked to close", |s| {
        s.windows[other.0].close_requested
    });
}

// --- Drag and drop -------------------------------------------------------------

/// A client drags text from its window and drops it on another client's:
/// the target is offered it while the pointer is over it, gets the drop and
/// reads the text.
#[test]
fn a_drag_reaches_another_window_follows_the_pointer_and_drops() {
    use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
    let mut pila = TestPila::start();
    let source_client = pila.client();
    let source_window = source_client.window("source").size(400, 300).create();
    pila.wait_focus("source");
    let target_client = pila.client();
    target_client.window("target").size(400, 300).create();
    let state = pila.wait_focus("target");
    let manager: WlDataDeviceManager = target_client.bind("wl_data_device_manager", 3);
    let _device = manager.get_data_device(&target_client.seat(), &target_client.qh, "dnd".into());
    target_client.flush();

    // The target opens cascaded from the source: grab the source's visible corner.
    let (sx, sy, ..) = rect(&window_in(&state, "source").unwrap()["geometry"]);
    let (tx, ty, tw, th) = rect(&window_in(&state, "target").unwrap()["geometry"]);
    pila.move_to(sx as f64 + 8.0, sy as f64 + 8.0);
    pila.press("left");
    source_client.wait("the press", |s| {
        s.events
            .iter()
            .any(|e| matches!(e, Ev::Button { pressed: true, .. }))
    });
    let source = source_client.data_source("drag", "dragged text");
    source.set_actions(DndAction::Copy);
    let manager: WlDataDeviceManager = source_client.bind("wl_data_device_manager", 3);
    let device = manager.get_data_device(&source_client.seat(), &source_client.qh, "src".into());
    let serial = source_client.last_serial();
    device.start_drag(
        Some(&source),
        &source_client.surface(source_window),
        None,
        serial,
    );
    source_client.flush();

    // Pressing raised the source: the target shows past its far corner.
    pila.move_to((tx + tw - 8) as f64, (ty + th - 8) as f64);
    target_client.wait("the drag entering", |s| s.dnd_offer.is_some());
    let (offer, enter_serial) = target_client.lock().dnd_offer.clone().unwrap();
    offer.accept(enter_serial, Some("text/plain".into()));
    offer.set_actions(DndAction::Copy, DndAction::Copy);
    target_client.flush();
    // (Smithay never sends the source `wl_data_source.target`.)
    target_client.roundtrip();
    // Moving over it: the target follows where the drag is, in its own
    // coordinates, to show where it would drop.
    pila.move_by(-20.0, -15.0);
    target_client.wait_logged("dnd", &format!("x: {}.0, y: {}.0", tw - 28, th - 23));
    pila.release("left");
    target_client.wait_logged("dnd", "Drop");
    let text = target_client.receive(|fd| offer.receive("text/plain".into(), fd));
    assert_eq!(text, "dragged text");
}

/// A drop nobody accepted cancels the drag: the source hears it, the window
/// under the pointer gets no drop, and clicks reach windows again.
#[test]
fn a_drop_nobody_accepts_is_cancelled_and_clicks_work_again() {
    use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
    let mut pila = TestPila::start();
    let source_client = pila.client();
    let source_window = source_client.window("source").size(400, 300).create();
    pila.wait_focus("source");
    let target_client = pila.client();
    target_client.window("target").size(400, 300).create();
    let state = pila.wait_focus("target");
    let manager: WlDataDeviceManager = target_client.bind("wl_data_device_manager", 3);
    let _device = manager.get_data_device(&target_client.seat(), &target_client.qh, "dnd".into());
    target_client.flush();

    let (sx, sy, ..) = rect(&window_in(&state, "source").unwrap()["geometry"]);
    let (tx, ty, tw, th) = rect(&window_in(&state, "target").unwrap()["geometry"]);
    pila.move_to(sx as f64 + 8.0, sy as f64 + 8.0);
    pila.press("left");
    source_client.wait("the press", |s| {
        s.events
            .iter()
            .any(|e| matches!(e, Ev::Button { pressed: true, .. }))
    });
    let source = source_client.data_source("drag", "dragged text");
    source.set_actions(DndAction::Copy);
    let manager: WlDataDeviceManager = source_client.bind("wl_data_device_manager", 3);
    let device = manager.get_data_device(&source_client.seat(), &source_client.qh, "src".into());
    let serial = source_client.last_serial();
    device.start_drag(
        Some(&source),
        &source_client.surface(source_window),
        None,
        serial,
    );
    source_client.flush();

    // Over the target, which never accepts.
    let (fx, fy) = ((tx + tw - 8) as f64, (ty + th - 8) as f64);
    pila.move_to(fx, fy);
    target_client.wait("the drag entering", |s| s.dnd_offer.is_some());
    pila.release("left");
    source_client.wait_logged("drag", "Cancelled");
    target_client.roundtrip();
    assert!(
        !target_client.has_logged("dnd", "Drop"),
        "a drop: {:?}",
        target_client.logged("dnd")
    );

    // The drag's grab is over: a click goes to the target again.
    pila.click("left");
    target_client.wait("the click", |s| {
        s.events
            .iter()
            .any(|e| matches!(e, Ev::Button { pressed: false, .. }))
    });
}

/// A drag's icon follows the pointer while it lasts, over the windows, and
/// goes with the drop.
/// A drag that ends some other way than a release (here the session locked
/// in the middle of it) is cancelled: the source hears it and its icon is
/// gone once the screen is back.
#[test]
fn locking_cancels_a_drag_and_takes_its_icon_away() {
    use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
    const ICON: [u8; 4] = [255, 255, 0, 255]; // magenta
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
            .any(|e| matches!(e, Ev::Button { pressed: true, .. }))
    });
    let source = client.data_source("drag", "dragged text");
    source.set_actions(DndAction::Copy);
    let manager: WlDataDeviceManager = client.bind("wl_data_device_manager", 3);
    let device = manager.get_data_device(&client.seat(), &client.qh, "src".into());
    let icon = client.painted_surface(ICON, (20, 20));
    let serial = client.last_serial();
    device.start_drag(Some(&source), &client.surface(window), Some(&icon), serial);
    client.flush();
    pila.move_to(x as f64 + 150.0, y as f64 + 120.0);
    client.roundtrip();
    pila.settle();
    assert_eq!(
        pila.pixel(x as f64 + 160.0, y as f64 + 130.0),
        [255, 0, 255, 255]
    );

    let locker = pila.client();
    let lock = locker.lock_session();
    let _surface = locker.lock_surface(&lock, "HEADLESS-1", [255, 10, 20, 30]);
    locker.wait_logged("lock", "Locked");
    client.wait_logged("drag", "Cancelled");
    lock.unlock_and_destroy();
    locker.flush();
    pila.wait("unlocked", |s| s["locked"] == false);
    pila.release("left");
    pila.settle();
    assert_ne!(
        pila.pixel(x as f64 + 160.0, y as f64 + 130.0),
        [255, 0, 255, 255],
        "the drag's icon stayed on screen"
    );
}

#[test]
fn a_drag_shows_its_icon_under_the_pointer() {
    use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
    const ICON: [u8; 4] = [255, 255, 0, 255]; // magenta
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
            .any(|e| matches!(e, Ev::Button { pressed: true, .. }))
    });
    let source = client.data_source("drag", "dragged text");
    source.set_actions(DndAction::Copy);
    let manager: WlDataDeviceManager = client.bind("wl_data_device_manager", 3);
    let device = manager.get_data_device(&client.seat(), &client.qh, "src".into());
    let icon = client.painted_surface(ICON, (20, 20));
    let serial = client.last_serial();
    device.start_drag(Some(&source), &client.surface(window), Some(&icon), serial);
    client.flush();

    pila.move_to(x as f64 + 150.0, y as f64 + 120.0);
    client.roundtrip();
    pila.settle();
    // Its top-left at the pointer.
    assert_eq!(
        pila.pixel(x as f64 + 160.0, y as f64 + 130.0),
        [255, 0, 255, 255]
    );
    pila.release("left");
    client.wait_logged("drag", "Cancelled");
    pila.settle();
    assert_ne!(
        pila.pixel(x as f64 + 160.0, y as f64 + 130.0),
        [255, 0, 255, 255]
    );
}

// --- Virtual keyboard ----------------------------------------------------------

fn saw_key(client: &Client, key: u32, pressed: bool) -> bool {
    client.events().contains(&Ev::Key { key, pressed })
}

#[test]
fn a_virtual_keyboard_types_into_the_focused_window() {
    let mut pila = TestPila::start();
    let app = pila.client();
    app.window("editor").create();
    pila.wait_focus("editor");
    let typist = pila.client();
    let keyboard = virtual_keyboard(&typist, &keymap_of(&app));
    keyboard.key(1, 16, 1);
    keyboard.key(2, 16, 0);
    typist.flush();
    app.wait("q from the virtual keyboard", |s| {
        s.events.iter().any(|e| {
            *e == Ev::Key {
                key: 16,
                pressed: false,
            }
        })
    });
}

/// A virtual keyboard is a keyboard: an on-screen one can use pila's
/// bindings, and its keys that make one never reach the client.
#[test]
fn a_virtual_keyboard_runs_bindings() {
    let mut pila = TestPila::start();
    let app = pila.client();
    let window = app.window("editor").create();
    pila.wait_focus("editor");
    let typist = pila.client();
    let keyboard = virtual_keyboard(&typist, &keymap_of(&app));
    // Super (125) + Shift (42) + q (16): Mod+Shift+Q, close.
    for (time, key, state) in [
        (1, 125, 1),
        (2, 42, 1),
        (3, 16, 1),
        (4, 16, 0),
        (5, 42, 0),
        (6, 125, 0),
    ] {
        keyboard.key(time, key, state);
    }
    typist.flush();
    app.wait("the close request", |s| s.windows[window.0].close_requested);
    assert!(!saw_key(&app, 16, true));
}

/// A virtual keyboard's modifiers (`wtype -M ctrl`, an on-screen keyboard's
/// Shift latched) are the keyboard's: the focused app is told, and a key
/// typed with them makes the binding they make.
#[test]
fn a_virtual_keyboards_modifiers_reach_the_app_and_bindings() {
    // In xkb's masks of the US keymap: Shift 1, Control 4, Mod4 (Super) 64.
    const SHIFT: u32 = 1;
    const CONTROL: u32 = 4;
    const SUPER: u32 = 64;
    let mut pila = TestPila::start();
    let app = pila.client();
    let window = app.window("editor").create();
    pila.wait_focus("editor");
    let typist = pila.client();
    let keyboard = virtual_keyboard(&typist, &keymap_of(&app));

    keyboard.modifiers(CONTROL, 0, 0, 0);
    typist.flush();
    app.wait("Control held", |s| {
        s.events
            .iter()
            .any(|e| matches!(e, Ev::Modifiers { depressed, .. } if *depressed == CONTROL))
    });
    keyboard.modifiers(0, 0, 0, 0);
    typist.flush();
    app.wait("Control released", |s| {
        matches!(
            s.events
                .iter()
                .rev()
                .find(|e| matches!(e, Ev::Modifiers { .. })),
            Some(Ev::Modifiers { depressed: 0, .. })
        )
    });

    // Super and Shift as modifiers, then q: Mod+Shift+Q, close.
    keyboard.modifiers(SUPER | SHIFT, 0, 0, 0);
    keyboard.key(1, 16, 1);
    keyboard.key(2, 16, 0);
    keyboard.modifiers(0, 0, 0, 0);
    typist.flush();
    app.wait("the close request", |s| s.windows[window.0].close_requested);
    assert!(!saw_key(&app, 16, true));
}

/// Typing on a virtual keyboard (an on-screen one) is the user being active.
#[test]
fn a_virtual_keyboard_counts_as_activity() {
    let mut pila = TestPila::start();
    let app = pila.client();
    app.window("editor").create();
    pila.wait_focus("editor");
    let notifier: ExtIdleNotifierV1 = app.bind("ext_idle_notifier_v1", 1);
    let _n = notifier.get_idle_notification(100, &app.seat(), &app.qh, "idle".into());
    app.flush();
    app.wait_logged("idle", "Idled");
    let typist = pila.client();
    let keyboard = virtual_keyboard(&typist, &keymap_of(&app));
    keyboard.key(1, 16, 1);
    keyboard.key(2, 16, 0);
    typist.flush();
    app.wait_logged("idle", "Resumed");
}

/// A key the user holds stays held when a virtual keyboard (an input method
/// passing keys through) presses and releases the same key.
#[test]
fn a_virtual_release_doesnt_release_a_held_key() {
    let mut pila = TestPila::start();
    let app = pila.client();
    app.window("editor").create();
    pila.wait_focus("editor");
    let keymap = keymap_of(&app);
    pila.key_press("a");
    app.wait("a pressed", |s| {
        s.events.iter().any(|e| {
            *e == Ev::Key {
                key: 30,
                pressed: true,
            }
        })
    });
    let typist = pila.client();
    let keyboard = virtual_keyboard(&typist, &keymap);
    keyboard.key(1, 30, 1);
    keyboard.key(2, 30, 0);
    typist.roundtrip();
    app.roundtrip();
    assert!(!saw_key(&app, 30, false), "released while held");
    pila.key_release("a");
    app.wait("a released", |s| {
        s.events.iter().any(|e| {
            *e == Ev::Key {
                key: 30,
                pressed: false,
            }
        })
    });
}

/// `keymap` with its keycodes section renamed, so a client can tell it apart.
fn marked(keymap: &(u32, String)) -> (u32, String) {
    let (head, tail) = keymap
        .1
        .split_once("xkb_keycodes")
        .expect("a keycodes section");
    let tail = tail.replacen('"', "\"pilavk", 1);
    (keymap.0, format!("{head}xkb_keycodes{tail}"))
}

fn has_marked_keymap(s: &common::client::Shared) -> bool {
    s.keymap.as_ref().is_some_and(|(_, k)| k.contains("pilavk"))
}

/// When the virtual keyboard goes (wtype exits), so does its keymap.
#[test]
fn removing_a_virtual_keyboard_restores_the_keymap() {
    let mut pila = TestPila::start();
    let app = pila.client();
    app.window("editor").create();
    pila.wait_focus("editor");
    let typist = pila.client();
    let keyboard = virtual_keyboard(&typist, &marked(&keymap_of(&app)));
    keyboard.key(1, 16, 1);
    keyboard.key(2, 16, 0);
    typist.flush();
    app.wait("the virtual keyboard's keymap", has_marked_keymap);
    keyboard.destroy();
    typist.flush();
    app.wait("the configured keymap back", |s| {
        s.keymap.is_some() && !has_marked_keymap(s)
    });
}

/// Swapping keymaps resets xkb's state; the user's layout and Num Lock
/// survive a virtual keyboard's turn.
#[test]
fn a_virtual_keyboard_keeps_the_layout_and_locks() {
    let mut pila = TestPila::start_with(Options {
        config: "[keyboard]\nlayout = \"us,de\"\noptions = \"grp:caps_toggle\"\n".into(),
        ..Options::default()
    });
    let app = pila.client();
    app.window("editor").create();
    pila.wait_focus("editor");
    let keymap = keymap_of(&app);
    pila.key("code:66"); // Caps Lock: the next layout here
    pila.key("Num_Lock");
    let num_lock = 16; // Mod2
    let is_users =
        |e: &Ev| matches!(e, Ev::Modifiers { locked, group: 1, .. } if locked & num_lock != 0);
    app.wait("layout 2 and Num Lock", |s| {
        s.events.last().is_some_and(is_users)
    });

    let typist = pila.client();
    let keyboard = virtual_keyboard(&typist, &marked(&keymap));
    keyboard.key(1, 16, 1);
    keyboard.key(2, 16, 0);
    typist.flush();
    app.wait("the virtual keyboard's keymap", has_marked_keymap);
    app.clear_events();
    pila.key("a");
    app.wait("the configured keymap back", |s| {
        s.keymap.is_some() && !has_marked_keymap(s)
    });
    app.wait("layout 2 and Num Lock again", |s| {
        s.events
            .iter()
            .rfind(|e| matches!(e, Ev::Modifiers { .. }))
            .is_some_and(is_users)
    });
}

/// While a virtual keyboard's keymap is in place, `pila msg keyboard` still
/// tells the physical keyboard's layouts, and a switch changes the one it
/// comes back to.
#[test]
fn a_virtual_keyboard_doesnt_change_the_layouts_told() {
    let mut pila = TestPila::start_with(Options {
        config: "[keyboard]\nlayout = \"us,de\"\n".into(),
        ..Options::default()
    });
    let app = pila.client();
    app.window("editor").create();
    pila.wait_focus("editor");
    let keymap = keymap_of(&app);
    let typist = pila.client();
    let keyboard = virtual_keyboard(&typist, &marked(&keymap));
    keyboard.key(1, 16, 1);
    keyboard.key(2, 16, 0);
    typist.flush();
    app.wait("the virtual keyboard's keymap", has_marked_keymap);
    let msg = |args: &[&str]| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_pila"))
            .arg("msg")
            .args(args)
            .env_remove("PILA_SOCKET")
            .env("WAYLAND_DISPLAY", &pila.display)
            .env("XDG_RUNTIME_DIR", &pila.dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice::<Value>(&out.stdout).unwrap_or(Value::Null)
    };
    let layouts = msg(&["--json", "keyboard"])["layouts"].clone();
    assert_eq!(layouts[0]["name"], "English (US)", "{layouts}");
    assert_eq!(layouts[1]["name"], "German", "{layouts}");
    assert_eq!(layouts[0]["active"], true, "{layouts}");
    msg(&["action", "switch-layout", "next"]);
    assert_eq!(msg(&["--json", "keyboard"])["layouts"][1]["active"], true);
    app.clear_events();
    pila.key("a");
    app.wait("the configured keymap back, in German", |s| {
        s.keymap.is_some()
            && !has_marked_keymap(s)
            && s.events
                .iter()
                .rfind(|e| matches!(e, Ev::Modifiers { .. }))
                .is_some_and(|e| matches!(e, Ev::Modifiers { group: 1, .. }))
    });
}

/// Keys from a virtual keyboard are read with its own keymap, which the
/// focused client gets while it types; the next physical key brings the
/// configured keymap back.
#[test]
fn a_virtual_keyboards_keymap_lasts_until_a_physical_key() {
    let mut pila = TestPila::start();
    let app = pila.client();
    app.window("editor").create();
    pila.wait_focus("editor");
    let typist = pila.client();
    let keyboard = virtual_keyboard(&typist, &marked(&keymap_of(&app)));
    keyboard.key(1, 16, 1);
    keyboard.key(2, 16, 0);
    typist.flush();
    app.wait("the virtual keyboard's keymap", has_marked_keymap);
    pila.key("a");
    app.wait("the configured keymap back", |s| {
        s.keymap.is_some() && !has_marked_keymap(s)
    });
}

// --- Activation ----------------------------------------------------------------

#[test]
fn a_focused_client_can_activate_another_window() {
    let mut pila = TestPila::start();
    let other = pila.client();
    let target = other.window("target").create();
    pila.wait_focus("target");
    let client = pila.client();
    client.window("launcher").create();
    pila.wait_focus("launcher");
    // A key press gives it a fresh serial.
    pila.key("a");
    client.wait("the key", |s| {
        s.events.iter().any(|e| {
            *e == Ev::Key {
                key: 30,
                pressed: false,
            }
        })
    });
    let activation: XdgActivationV1 = client.bind("xdg_activation_v1", 1);
    let token = activation.get_activation_token(&client.qh, "token".into());
    token.set_serial(client.last_serial(), &client.seat());
    token.commit();
    client.flush();
    client.wait_logged("token", "Done");
    let text = client.logged("token")[0].clone();
    let token_string = text.split('"').nth(1).expect("a token string").to_string();
    // The other client activates its window with it.
    let other_activation: XdgActivationV1 = other.bind("xdg_activation_v1", 1);
    other_activation.activate(token_string, &other.surface(target));
    other.flush();
    pila.wait_focus("target");
}

/// A token a client got: `serial` of its `seat`, the string once done.
fn token_for(client: &Client, serial: u32) -> String {
    let activation: XdgActivationV1 = client.bind("xdg_activation_v1", 1);
    let token = activation.get_activation_token(&client.qh, "token".into());
    token.set_serial(serial, &client.seat());
    token.commit();
    client.flush();
    client.wait_logged("token", "Done");
    let text = client.logged("token").last().unwrap().clone();
    text.split('"').nth(1).expect("a token string").to_string()
}

/// A notification takes no keyboard, but clicking it is user input: its
/// client may pass the click on as an activation token (open the app).
#[test]
fn a_clicked_notification_can_activate_a_window() {
    use common::protocols::LayerSpec;
    use wayland_protocols_wlr::layer_shell::v1::client::{
        zwlr_layer_shell_v1::Layer, zwlr_layer_surface_v1::Anchor,
    };
    let mut pila = TestPila::start();
    let app = pila.client();
    let target = app.window("target").size(300, 200).create();
    pila.wait_window("target");
    let editor = pila.client();
    editor.window("editor").size(300, 200).create();
    pila.wait_focus("editor");
    let notifier = pila.client();
    let spec = LayerSpec {
        layer: Layer::Top,
        anchor: Anchor::Top | Anchor::Right,
        size: (200, 60),
        ..LayerSpec::default()
    };
    let (note, _) = notifier.layer_surface("notification", &spec, [255, 0, 0, 255]);
    notifier.wait_surface_configured(note);
    // Drawn, not only listed: a layer surface is mapped as it's created,
    // and takes no click before its first buffer.
    pila.wait_pixel(1180.0, 30.0, [0, 0, 255, 255]);
    pila.move_to(1180.0, 30.0);
    pila.click("left");
    notifier.wait("the click", |s| {
        s.events
            .iter()
            .any(|e| matches!(e, Ev::Button { pressed: false, .. }))
    });
    let token = token_for(&notifier, notifier.last_serial());
    let activation: XdgActivationV1 = app.bind("xdg_activation_v1", 1);
    activation.activate(token, &app.surface(target));
    app.flush();
    pila.wait_focus("target");
}

/// The pointer's serial only counts for the client under the pointer: one
/// whose surface it left can't use it to take the focus.
#[test]
fn a_pointer_serial_of_another_surface_doesnt_activate() {
    let mut pila = TestPila::start();
    let thief = pila.client();
    let window = thief.window("thief").size(300, 200).create();
    let state = pila.wait_focus("thief");
    let (x, y, ..) = rect(&window_in(&state, "thief").unwrap()["geometry"]);
    pila.move_to(x as f64 + 20.0, y as f64 + 20.0);
    pila.click("left");
    thief.wait("the click", |s| {
        s.events
            .iter()
            .any(|e| matches!(e, Ev::Button { pressed: false, .. }))
    });
    let serial = thief.last_serial();
    let owner = pila.client();
    owner.window("owner").size(300, 200).create();
    pila.wait_focus("owner");
    pila.move_to(5.0, 790.0); // the desktop: nobody's surface
    let token = token_for(&thief, serial);
    let activation: XdgActivationV1 = thief.bind("xdg_activation_v1", 1);
    activation.activate(token, &thief.surface(window));
    thief.flush();
    pila.stays("owner keeps focus", Duration::from_millis(200), |s| {
        s["focused"] == "owner"
    });
}

/// The pointer only resting on a window isn't the user asking for it: the
/// serial of its enter can't activate it, or an app could take the keyboard
/// (and what's typed next) whenever the pointer passes over it.
#[test]
fn hovering_a_window_doesnt_let_it_take_the_focus() {
    let mut pila = TestPila::start();
    let thief = pila.client();
    let window = thief.window("thief").size(800, 600).create();
    pila.wait_focus("thief");
    let owner = pila.client();
    owner.window("owner").size(200, 100).create();
    let state = pila.wait_focus("owner");
    let (x, y, ..) = rect(&window_in(&state, "thief").unwrap()["geometry"]);
    pila.move_to(x as f64 + 10.0, y as f64 + 10.0);
    thief.wait("the pointer's enter", |s| s.pointer_enter_serial != 0);
    let serial = thief.lock().pointer_enter_serial;
    let token = token_for(&thief, serial);
    let activation: XdgActivationV1 = thief.bind("xdg_activation_v1", 1);
    activation.activate(token, &thief.surface(window));
    thief.flush();
    pila.stays("owner keeps focus", Duration::from_millis(200), |s| {
        s["focused"] == "owner"
    });
}

/// A click counts until the keyboard focus moves on: the app clicked, then
/// another window focused with the pointer left on the app, can't use that
/// click to take the focus back.
#[test]
fn a_click_before_the_focus_moved_on_doesnt_activate() {
    let mut pila = TestPila::start();
    let thief = pila.client();
    let window = thief.window("thief").size(800, 600).create();
    let state = pila.wait_focus("thief");
    let (x, y, ..) = rect(&window_in(&state, "thief").unwrap()["geometry"]);
    pila.move_to(x as f64 + 10.0, y as f64 + 10.0);
    pila.click("left");
    thief.wait("the click", |s| {
        s.events
            .iter()
            .any(|e| matches!(e, Ev::Button { pressed: false, .. }))
    });
    let serial = thief.last_serial();
    let owner = pila.client();
    owner.window("owner").size(200, 100).create();
    pila.wait_focus("owner");
    let token = token_for(&thief, serial);
    let activation: XdgActivationV1 = thief.bind("xdg_activation_v1", 1);
    activation.activate(token, &thief.surface(window));
    thief.flush();
    pila.stays("owner keeps focus", Duration::from_millis(200), |s| {
        s["focused"] == "owner"
    });
}

#[test]
fn an_unfocused_client_cant_steal_focus() {
    let mut pila = TestPila::start();
    let thief = pila.client();
    let window = thief.window("thief").create();
    pila.wait_focus("thief");
    let serial = thief.last_serial();
    let owner = pila.client();
    owner.window("owner").create();
    pila.wait_focus("owner");
    let activation: XdgActivationV1 = thief.bind("xdg_activation_v1", 1);
    let token = activation.get_activation_token(&thief.qh, "token".into());
    token.set_serial(serial, &thief.seat());
    token.commit();
    thief.flush();
    thief.wait_logged("token", "Done");
    let text = thief.logged("token")[0].clone();
    let token_string = text.split('"').nth(1).unwrap().to_string();
    activation.activate(token_string, &thief.surface(window));
    thief.flush();
    pila.stays("owner keeps focus", Duration::from_millis(200), |s| {
        s["focused"] == "owner"
    });
}

#[test]
fn activation_brings_a_window_from_another_workspace_or_minimized() {
    let mut pila = TestPila::start();
    let other = pila.client();
    let target = other.window("target").create();
    pila.wait_focus("target");
    pila.combo("Super+Shift+3");
    pila.wait("target on 3", |s| {
        window_in(s, "target").unwrap()["workspace"] == 3
    });
    let client = pila.client();
    client.window("launcher").create();
    pila.wait_focus("launcher");
    pila.key("a");
    client.wait("the key", |s| {
        s.events.iter().any(|e| {
            *e == Ev::Key {
                key: 30,
                pressed: false,
            }
        })
    });
    let activation: XdgActivationV1 = client.bind("xdg_activation_v1", 1);
    let token = activation.get_activation_token(&client.qh, "token".into());
    token.set_serial(client.last_serial(), &client.seat());
    token.commit();
    client.flush();
    client.wait_logged("token", "Done");
    let text = client.logged("token")[0].clone();
    let token_string = text.split('"').nth(1).unwrap().to_string();
    let other_activation: XdgActivationV1 = other.bind("xdg_activation_v1", 1);
    other_activation.activate(token_string, &other.surface(target));
    other.flush();
    let state = pila.wait_focus("target");
    assert_eq!(state["workspace"], 3);
}

/// A client from before wl_keyboard 7 maps the keymap file with write
/// access, so it gets a plain file (not the sealed one) and reads the keymap
/// in it as a C string: the file ends in a NUL, counted in the size (a fix of
/// Smithay's, 33323a91).
#[test]
fn an_older_client_gets_a_keymap_that_ends_in_nul() {
    let pila = TestPila::start();
    let client = pila.client();
    for version in [1, 6, 7, 9] {
        let tag = format!("keyboard-v{version}");
        let _keyboard = client.keyboard_at_seat_version(version, &tag);
        client.wait_logged(&tag, "Keymap");
        let keymap = client
            .logged(&tag)
            .into_iter()
            .find(|e| e.starts_with("Keymap"))
            .unwrap();
        assert!(
            keymap.contains("nul_terminated=true"),
            "v{version}: {keymap}"
        );
    }
}
