//! Protocols about input: idle notifications and inhibitors, pointer
//! constraints and relative motion, keyboard-shortcuts inhibitors, the
//! virtual keyboard, xdg-activation.

mod common;

use std::time::Duration;

use common::client::Ev;
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
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;

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
    pila.combo("Super+Shift+e");
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

// --- Virtual keyboard ----------------------------------------------------------

#[test]
fn a_virtual_keyboard_types_into_the_focused_window_past_the_bindings() {
    let mut pila = TestPila::start();
    let app = pila.client();
    let window = app.window("editor").create();
    pila.wait_focus("editor");
    let typist = pila.client();
    let (format, keymap) = {
        app.wait("a keymap", |s| s.keymap.is_some());
        app.lock().keymap.clone().unwrap()
    };
    let manager: ZwpVirtualKeyboardManagerV1 = typist.bind("zwp_virtual_keyboard_manager_v1", 1);
    let keyboard = manager.create_virtual_keyboard(&typist.seat(), &typist.qh, "vk".into());
    let fd = memfd_with(&keymap);
    use std::os::fd::AsFd;
    keyboard.keymap(format, fd.as_fd(), keymap.len() as u32 + 1);
    // Super (125) + q (16) held: not a binding through the virtual keyboard.
    keyboard.modifiers(64, 0, 0, 0);
    keyboard.key(1, 16, 1);
    keyboard.key(2, 16, 0);
    keyboard.modifiers(0, 0, 0, 0);
    typist.flush();
    app.wait("q from the virtual keyboard", |s| {
        s.events.iter().any(|e| {
            *e == Ev::Key {
                key: 16,
                pressed: false,
            }
        })
    });
    app.roundtrip();
    assert!(!app.with_window(window, |w| w.close_requested));
}

fn memfd_with(text: &str) -> std::os::fd::OwnedFd {
    use std::io::Write;
    use std::os::fd::FromRawFd;
    // SAFETY: a new fd, owned by the File.
    let fd = unsafe { libc::memfd_create(c"keymap".as_ptr(), libc::MFD_CLOEXEC) };
    assert!(fd >= 0);
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    file.write_all(text.as_bytes()).unwrap();
    file.write_all(&[0]).unwrap();
    file.into()
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
