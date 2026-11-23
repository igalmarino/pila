//! Session lock (ext-session-lock): what's shown, where input goes, what's
//! refused while locked, unlocking, a dead or rogue locker, takeovers.

mod common;

use common::*;
use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_v1::ExtSessionLockV1;

const LOCK_COLOR: [u8; 4] = [255, 10, 20, 30];
const LOCK_RGBA: [u8; 4] = [10, 20, 30, 255];

/// A window "a", then a locker locked with a surface on HEADLESS-1.
fn locked() -> (TestPila, Client, Client, ExtSessionLockV1) {
    locked_with("")
}

fn locked_with(config: &str) -> (TestPila, Client, Client, ExtSessionLockV1) {
    let mut pila = TestPila::with_config(config);
    let apps = pila.client();
    apps.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let locker = pila.client();
    let lock = locker.lock_session();
    let surf = locker.lock_surface(&lock, "HEADLESS-1", LOCK_COLOR);
    assert_eq!(locker.wait_surface_configured(surf), (0, 0, 1280, 800));
    locker.wait_logged("lock", "Locked");
    // Its buffer, committed on the configure, in before a test reads pixels:
    // the lock may be confirmed with the output still black.
    locker.roundtrip();
    pila.wait("locked", |s| {
        s["locked"] == true && s["lock_pending"] == false
    });
    (pila, apps, locker, lock)
}

#[test]
fn locking_shows_only_the_lock_surface_and_gives_it_the_keyboard() {
    let (mut pila, _apps, locker, _lock) = locked();
    let state = pila.settle();
    assert_eq!(state["keyboard_focus"], "lock");
    let (x, y, ..) = rect(&window_in(&state, "a").unwrap()["geometry"]);
    assert_eq!(pila.pixel((x + 10) as f64, (y + 10) as f64), LOCK_RGBA);
    assert_eq!(pila.pixel(1.0, 1.0), LOCK_RGBA);
    pila.key("a");
    locker.wait("the key", |s| {
        s.events.iter().any(|e| {
            *e == common::client::Ev::Key {
                key: 30,
                pressed: false,
            }
        })
    });
}

#[test]
fn shaking_the_pointer_grows_the_cursor_on_the_lock_screen_too() {
    let (mut pila, _apps, _locker, _lock) =
        locked_with("[cursor]\nshake-to-find = true\n[animations]\nenabled = false\nspeed = 4\n");
    pila.move_to(640.0, 400.0);
    pila.wait_pixel(650.0, 431.0, LOCK_RGBA);
    pila.shake_pointer(640.0, 400.0);
    pila.wait("magnified", |s| s["pointer"]["magnification"] == 3.0);
    // The built-in arrow's white inside, three times as far, over the lock
    // surface.
    pila.wait_pixel(650.0, 431.0, [255, 255, 255, 255]);
    pila.wait("back to its size", |s| s["pointer"]["magnification"] == 1.0);
    pila.wait_pixel(650.0, 431.0, LOCK_RGBA);
}

#[test]
fn bindings_and_workspace_switches_dont_run_while_locked() {
    let (mut pila, apps, locker, _lock) = locked();
    pila.combo("Super+Shift+q");
    pila.combo("Super+2");
    pila.combo("Super+m");
    // Mod+wheel too, and the notch goes to the lock screen.
    pila.key_press("Super_L");
    pila.scroll("wheel", 0.0, 1.0);
    pila.key_release("Super_L");
    locker.wait("the notch", |s| {
        s.events
            .iter()
            .any(|e| matches!(e, common::client::Ev::AxisV120 { value: 120, .. }))
    });
    apps.roundtrip();
    let state = pila.settle();
    assert_eq!(state["workspace"], 1);
    assert!(!apps.with_window(common::client::Window(0), |w| w.close_requested));
    assert_eq!(window_in(&state, "a").unwrap()["maximized"], false);
}

#[test]
fn tapping_mod_doesnt_run_the_launcher_while_locked() {
    let (mut pila, _apps, locker, _lock) = locked_with("launcher = \"exec sleep 600\"\n");
    pila.key("Super");
    // The locker gets the key, and nothing ran.
    const KEY_LEFTMETA: u32 = 125;
    locker.wait("Super's release", |s| {
        s.events.iter().any(|e| {
            *e == common::client::Ev::Key {
                key: KEY_LEFTMETA,
                pressed: false,
            }
        })
    });
    assert_eq!(pila.state()["children"], 0);
}

#[test]
fn stacking_bindings_dont_run_while_locked() {
    let (mut pila, _windows, _locker, _lock) = locked();
    let before = pila.state();
    for combo in ["Super+t", "Super+p", "Super+h", "Super+o", "Super+Shift+o"] {
        pila.combo(combo);
    }
    pila.settle();
    let after = pila.state();
    assert_eq!(stacking(&after), stacking(&before));
    for window in after["windows"].as_array().unwrap() {
        assert_eq!(window["always_on_top"], false);
        assert_eq!(window["all_workspaces"], false);
        assert_eq!(window["minimized"], false);
    }
}

#[test]
fn clicks_dont_reach_windows_while_locked() {
    let (mut pila, apps, locker, _lock) = locked();
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    apps.clear_events();
    pila.click_at((x + 10) as f64, (y + 10) as f64);
    locker.wait("the click on the lock surface", |s| {
        s.events
            .iter()
            .any(|e| matches!(e, common::client::Ev::Button { pressed: false, .. }))
    });
    apps.roundtrip();
    assert!(apps.buttons().is_empty());
}

#[test]
fn new_windows_dont_take_focus_while_locked() {
    let (mut pila, apps, _locker, _lock) = locked();
    apps.window("b").create();
    pila.wait_window("b");
    let state = pila.settle();
    assert_eq!(state["keyboard_focus"], "lock");
}

#[test]
fn unlocking_gives_the_screen_and_keyboard_back() {
    let (mut pila, _apps, locker, lock) = locked();
    lock.unlock_and_destroy();
    locker.flush();
    let state = pila.wait("unlocked", |s| s["locked"] == false);
    let state = if state["focused"] == "a" {
        state
    } else {
        pila.wait_focus("a")
    };
    let (x, y, ..) = rect(&window_in(&state, "a").unwrap()["geometry"]);
    pila.settle();
    assert_ne!(pila.pixel((x + 10) as f64, (y + 10) as f64), LOCK_RGBA);
}

#[test]
fn a_dead_locker_leaves_the_session_locked_and_another_can_take_over() {
    let (mut pila, _apps, mut locker, _lock) = locked();
    locker.disconnect();
    pila.stays("still locked", std::time::Duration::from_millis(200), |s| {
        s["locked"] == true
    });
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    pila.settle();
    // Nothing of the session shows.
    let px = pila.pixel((x + 10) as f64, (y + 10) as f64);
    assert_ne!(px, [0, 0, 255, 255]);
    let rescue = pila.client();
    let lock = rescue.lock_session();
    rescue.lock_surface(&lock, "HEADLESS-1", LOCK_COLOR);
    rescue.wait_logged("lock", "Locked");
    lock.unlock_and_destroy();
    rescue.flush();
    pila.wait("unlocked", |s| s["locked"] == false);
}

#[test]
fn a_second_lock_while_locked_is_refused() {
    let (pila, _apps, _locker, _lock) = locked();
    let other = pila.client();
    let _second = other.lock_session();
    other.wait_logged("lock", "Finished");
}

/// A refused lock's `unlock_and_destroy` is a protocol error and leaves the
/// session locked: only the lock in force unlocks it.
#[test]
fn unlocking_through_a_refused_lock_is_an_error_and_unlocks_nothing() {
    let (mut pila, _apps, _locker, _lock) = locked();
    let other = pila.client();
    let second = other.lock_session();
    other.wait_logged("lock", "Finished");
    other.expect_error();
    second.unlock_and_destroy();
    other.flush();
    other.wait_error();
    pila.stays("still locked", std::time::Duration::from_millis(200), |s| {
        s["locked"] == true
    });
}

/// A locker asking to unlock before it was told `locked` gets a protocol
/// error, and the session doesn't unlock.
#[test]
fn unlocking_before_the_lock_is_confirmed_is_an_error_and_unlocks_nothing() {
    let mut pila = TestPila::start();
    let locker = pila.client();
    let manager: wayland_protocols::ext::session_lock::v1::client::ext_session_lock_manager_v1::ExtSessionLockManagerV1 =
        locker.bind("ext_session_lock_manager_v1", 1);
    locker.expect_error();
    // Both in one write, so Pila reads them before it can draw the locked
    // frame that confirms the lock.
    let lock = manager.lock(&locker.qh, "lock".to_string());
    lock.unlock_and_destroy();
    locker.flush();
    locker.wait_error();
    assert!(!locker.logged("lock").iter().any(|e| e.contains("Locked")));
    pila.stays("still locked", std::time::Duration::from_millis(200), |s| {
        s["locked"] == true
    });
}

/// Lock surfaces belong to a lock object, not to a client: the locker's own
/// second, refused lock can't put a surface over its real lock screen.
#[test]
fn a_refused_lock_cant_cover_the_screen() {
    let (mut pila, _apps, locker, _lock) = locked();
    let second = locker.lock_session();
    locker.wait("the second lock refused", |s| {
        s.log
            .iter()
            .filter(|l| l.tag == "lock" && l.event == "Finished")
            .count()
            == 1
    });
    let _surf = locker.lock_surface(&second, "HEADLESS-1", [255, 200, 0, 0]);
    locker.roundtrip();
    pila.settle();
    assert_eq!(pila.pixel(640.0, 400.0), LOCK_RGBA);
}

#[test]
fn destroying_the_lock_without_unlocking_keeps_it_locked() {
    let (mut pila, _apps, locker, lock) = locked();
    lock.destroy();
    locker.flush();
    pila.stays("still locked", std::time::Duration::from_millis(200), |s| {
        s["locked"] == true
    });
}

#[test]
fn an_output_without_a_lock_surface_shows_black() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1024x768".into()),
        ..Options::default()
    });
    let locker = pila.client();
    let lock = locker.lock_session();
    let surf = locker.lock_surface(&lock, "A", LOCK_COLOR);
    locker.wait_surface_configured(surf);
    locker.wait_logged("lock", "Locked");
    // Its buffer, committed on the configure, in before reading pixels: the
    // lock may be confirmed with A still black (CI found it under load).
    locker.roundtrip();
    pila.settle();
    assert_eq!(pila.pixel(10.0, 10.0), LOCK_RGBA);
    assert_eq!(pila.pixel(1300.0, 10.0), [0, 0, 0, 255]);
}

#[test]
fn popup_grabs_and_moves_are_refused_while_locked() {
    let (mut pila, _apps, _locker, _lock) = locked();
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    pila.move_to((x + 20) as f64, (y + 20) as f64);
    pila.key_press("Super");
    pila.press("left");
    pila.stays("no move grab", std::time::Duration::from_millis(100), |s| {
        s["pointer"]["grab"] != "move"
    });
    pila.release("left");
    pila.key_release("Super");
}

/// After unlocking, Qt destroys a lock surface's role object, then commits
/// the bare surface with no buffer (a Qt locker does it on every unlock):
/// that must not cost the locker its connection.
#[test]
fn a_surface_committed_after_its_lock_surface_is_destroyed_keeps_the_client() {
    let mut pila = TestPila::start();
    let locker = pila.client();
    let lock = locker.lock_session();
    let (surf, lock_surface) = locker.lock_surface_object(&lock, "HEADLESS-1", LOCK_COLOR);
    locker.wait_surface_configured(surf);
    locker.wait_logged("lock", "Locked");
    pila.wait("locked", |s| {
        s["locked"] == true && s["lock_pending"] == false
    });
    lock.unlock_and_destroy();
    let surface = locker.wl_surface_of(surf);
    lock_surface.destroy();
    surface.attach(None, 0, 0);
    surface.commit();
    locker.roundtrip();
    pila.wait("unlocked", |s| s["locked"] == false);
    assert_eq!(locker.error(), None);
}

/// A token an app got before the screen locked doesn't bring a window back
/// while it's locked.
#[test]
fn activation_does_nothing_while_locked() {
    use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;
    let mut pila = TestPila::start();
    let apps = pila.client();
    let hidden = apps.window("hidden").create();
    pila.wait_focus("hidden");
    pila.combo("Super+n");
    pila.wait("minimized", |s| {
        window_in(s, "hidden").unwrap()["minimized"] == true
    });
    apps.window("a").create();
    pila.wait_focus("a");
    pila.key("a");
    apps.wait("the key", |s| {
        s.events.iter().any(|e| {
            *e == common::client::Ev::Key {
                key: 30,
                pressed: false,
            }
        })
    });
    let activation: XdgActivationV1 = apps.bind("xdg_activation_v1", 1);
    let token = activation.get_activation_token(&apps.qh, "token".into());
    token.set_serial(apps.last_serial(), &apps.seat());
    token.commit();
    apps.flush();
    apps.wait_logged("token", "Done");
    let text = apps.logged("token")[0].clone();
    let token_string = text.split('"').nth(1).unwrap().to_string();

    let locker = pila.client();
    let lock = locker.lock_session();
    let surf = locker.lock_surface(&lock, "HEADLESS-1", LOCK_COLOR);
    locker.wait_surface_configured(surf);
    pila.wait("locked", |s| s["locked"] == true);
    activation.activate(token_string, &apps.surface(hidden));
    apps.roundtrip();
    let state = pila.settle();
    assert_eq!(window_in(&state, "hidden").unwrap()["minimized"], true);
}

/// Whether `event` is input: focus or anything sent to a focus.
fn is_input(event: &common::client::Ev) -> bool {
    use common::client::Ev;
    matches!(
        event,
        Ev::KeyboardEnter(_)
            | Ev::Key { .. }
            | Ev::Modifiers { .. }
            | Ev::PointerEnter { .. }
            | Ev::Motion { .. }
            | Ev::Button { .. }
            | Ev::Axis { .. }
            | Ev::AxisV120 { .. }
    )
}

/// Locked, everything that could hand a client the keyboard or the pointer,
/// or show it, is tried: every action as a binding or `pila msg` runs it
/// (`run_action`; the list comes from Pila, so a new action is tried too),
/// the gestures, keys, clicks, the pointer and scrolling over a window,
/// xdg-activation with a token from before the lock, a taskbar's activate, a
/// pager's workspace activate and the window menu. Only the lock surfaces
/// get input, and only they are drawn.
#[test]
fn nothing_but_the_lock_gets_input_or_shows_while_locked() {
    use wayland_protocols::ext::workspace::v1::client::{
        ext_workspace_handle_v1::ExtWorkspaceHandleV1,
        ext_workspace_manager_v1::ExtWorkspaceManagerV1,
    };
    use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;
    use wayland_protocols_wlr::foreign_toplevel::v1::client::{
        zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
        zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1,
    };
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1024x768".into()),
        // Actions that start programs start nothing that shows.
        config: "terminal = \"true\"\nlauncher = \"true\"\n".into(),
        ..Options::default()
    });
    pila.move_to(100.0, 100.0);
    let apps = pila.client();
    let hidden = apps.window("hidden").create();
    pila.wait_focus("hidden");
    pila.combo("Super+n");
    pila.wait("minimized", |s| {
        window_in(s, "hidden").unwrap()["minimized"] == true
    });
    let a = apps.window("a").size(400, 300).create();
    let state = pila.wait_focus("a");
    let (x, y, ..) = rect(&window_in(&state, "a").unwrap()["geometry"]);
    // A token and a press serial from before the lock.
    pila.move_to((x + 20) as f64, (y + 20) as f64);
    pila.click("left");
    apps.wait("the click", |s| {
        s.events
            .iter()
            .any(|e| matches!(e, common::client::Ev::Button { pressed: false, .. }))
    });
    let serial = apps.last_serial();
    let activation: XdgActivationV1 = apps.bind("xdg_activation_v1", 1);
    let token = activation.get_activation_token(&apps.qh, "token".into());
    token.set_serial(serial, &apps.seat());
    token.commit();
    apps.flush();
    apps.wait_logged("token", "Done");
    let token_string = apps.logged("token")[0]
        .split('"')
        .nth(1)
        .unwrap()
        .to_string();
    let shell = pila.client();
    let _toplevels: ZwlrForeignToplevelManagerV1 =
        shell.bind("zwlr_foreign_toplevel_manager_v1", 3);
    let workspaces: ExtWorkspaceManagerV1 = shell.bind("ext_workspace_manager_v1", 1);
    shell.wait_logged("ext_workspace_manager_v1", "Done");
    for app_id in ["a", "hidden"] {
        shell.wait_logged("wlr_toplevel", &format!("AppId {{ app_id: \"{app_id}\" }}"));
    }

    let locker = pila.client();
    let lock = locker.lock_session();
    for output in ["A", "B"] {
        let surf = locker.lock_surface(&lock, output, LOCK_COLOR);
        locker.wait_surface_configured(surf);
    }
    locker.wait_logged("lock", "Locked");
    pila.wait("locked", |s| {
        s["locked"] == true && s["lock_pending"] == false
    });
    apps.roundtrip();
    apps.clear_events();

    // Every action, as a binding runs it.
    let actions = pila.request(json!({ "cmd": "actions" }));
    let actions = actions["actions"].as_array().unwrap().clone();
    assert!(actions.len() > 30, "{actions:?}");
    for (index, name) in actions.iter().enumerate() {
        // It ends Pila (and debug builds allow it while locked, to escape a
        // nested session).
        if name == "Quit" {
            continue;
        }
        pila.request(json!({ "cmd": "action", "index": index }));
    }
    // The gestures, every way.
    for fingers in [3, 4] {
        for (dx, dy) in [(300.0, 0.0), (-300.0, 0.0), (0.0, 300.0), (0.0, -300.0)] {
            pila.gesture(json!({ "kind": "swipe", "phase": "begin", "fingers": fingers }));
            for _ in 0..5 {
                pila.gesture(
                    json!({ "kind": "swipe", "phase": "update", "dx": dx / 5.0, "dy": dy / 5.0 }),
                );
            }
            pila.gesture(json!({ "kind": "swipe", "phase": "end" }));
        }
    }
    pila.gesture(json!({ "kind": "pinch", "phase": "begin", "fingers": 2 }));
    pila.gesture(json!({ "kind": "pinch", "phase": "update", "scale": 2.0 }));
    pila.gesture(json!({ "kind": "pinch", "phase": "end" }));
    pila.gesture(json!({ "kind": "hold", "phase": "begin", "fingers": 3 }));
    pila.gesture(json!({ "kind": "hold", "phase": "end" }));
    // Input over the window, and keys.
    pila.move_to((x + 30) as f64, (y + 30) as f64);
    pila.click("left");
    pila.click("right");
    pila.scroll("wheel", 0.0, 15.0);
    pila.key("a");
    pila.combo("Super+Tab");
    // The window menu, with the serial of a press from before the lock.
    let seat = apps.seat();
    apps.request(a, |t| t.show_window_menu(&seat, serial, 10, 10));
    // Activation, a taskbar's and a pager's.
    activation.activate(token_string, &apps.surface(hidden));
    for app_id in ["a", "hidden"] {
        let handle: ZwlrForeignToplevelHandleV1 =
            shell.object("wlr_toplevel", &format!("AppId {{ app_id: \"{app_id}\" }}"));
        handle.activate(&shell.seat());
        handle.unset_minimized();
    }
    let two: ExtWorkspaceHandleV1 = shell.object("workspace", "Name { name: \"2\" }");
    two.activate();
    workspaces.commit();
    shell.flush();
    apps.roundtrip();
    shell.roundtrip();
    let state = pila.settle();

    assert_eq!(state["locked"], true);
    assert_eq!(state["keyboard_focus"], "lock");
    assert!(state["window_menu"].is_null(), "{state:#}");
    apps.roundtrip();
    let leaked: Vec<_> = apps.events().into_iter().filter(is_input).collect();
    assert!(leaked.is_empty(), "input reached an app: {leaked:?}");
    // Nothing but the lock on either output.
    for (ox, w, h) in [(0, 1280, 800), (1280, 1024, 768)] {
        for i in 0..5 {
            for j in 0..5 {
                let px = (ox + w * (2 * i + 1) / 10) as f64;
                let py = (h * (2 * j + 1) / 10) as f64;
                assert_eq!(pila.pixel(px, py), LOCK_RGBA, "at {px},{py}");
            }
        }
    }
}

/// The keyboard can't go to anything but a lock surface while locked, from
/// any path: the lowest layer refuses it (and in a debug build, as tests
/// run, says so by crashing, to show the path that tried).
#[test]
fn the_keyboard_is_refused_to_anything_but_the_lock_at_the_lowest_layer() {
    let (mut pila, apps, _locker, _lock) = locked();
    apps.clear_events();
    pila.expect_crash();
    pila.request(json!({ "cmd": "keyboard_focus", "app_id": "a" }));
    pila.wait_exit();
    assert!(
        pila.log().contains("keyboard focus refused while locked"),
        "{}",
        pila.log_tail()
    );
    assert!(!apps.events().iter().any(is_input));
}

/// The same for the pointer.
#[test]
fn the_pointer_is_refused_to_anything_but_the_lock_at_the_lowest_layer() {
    let (mut pila, apps, _locker, _lock) = locked();
    apps.clear_events();
    pila.expect_crash();
    pila.request(json!({ "cmd": "pointer_focus", "app_id": "a" }));
    pila.wait_exit();
    assert!(
        pila.log().contains("pointer focus refused while locked"),
        "{}",
        pila.log_tail()
    );
    assert!(!apps.events().iter().any(is_input));
}

/// A binding repeating while its key is held stops when the session locks.
#[test]
fn locking_stops_a_repeating_binding() {
    let mut pila = TestPila::with_config(
        "[keyboard]\nrepeat-delay = 150\nrepeat-rate = 20\n\n[bindings]\n\
         \"Mod+x\" = { action = \"spawn echo >> $XDG_RUNTIME_DIR/runs\", repeat = true }\n",
    );
    let runs = |pila: &TestPila| {
        std::fs::read_to_string(pila.dir.join("runs")).map_or(0, |text| text.lines().count())
    };
    pila.key_press("Super");
    pila.key_press("x");
    pila.wait_file("runs");
    let locker = pila.client();
    let lock = locker.lock_session();
    locker.lock_surface(&lock, "HEADLESS-1", LOCK_COLOR);
    locker.wait_logged("lock", "Locked");
    // A run may be on its way as the lock came.
    std::thread::sleep(std::time::Duration::from_millis(300));
    let stopped = runs(&pila);
    std::thread::sleep(std::time::Duration::from_millis(400));
    assert_eq!(runs(&pila), stopped, "the binding ran on while locked");
}

/// A Mod+Tab in progress as the session locks (swayidle locking while Mod is
/// held) is over: its switcher is gone, and a click where a tile was goes to
/// the lock surface, activating no window.
#[test]
fn locking_ends_a_switch_and_its_switcher() {
    let mut pila = TestPila::start();
    let apps = pila.client();
    apps.window("a").size(400, 300).create();
    pila.wait_focus("a");
    apps.window("b").size(400, 300).create();
    pila.wait_focus("b");
    pila.key_press("Super");
    pila.key("Tab");
    let state = pila.wait("the switcher", |s| s["switcher_view"]["shown"] == true);
    let tile = state["switcher_view"]["tiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["app_id"] == "b")
        .unwrap()["rect"]
        .clone();
    let f = |k: &str| tile[k].as_f64().unwrap();
    let (x, y) = (f("x") + f("w") / 2.0, f("y") + f("h") / 2.0);

    let locker = pila.client();
    let lock = locker.lock_session();
    locker.lock_surface(&lock, "HEADLESS-1", LOCK_COLOR);
    locker.wait_logged("lock", "Locked");
    let state = pila.wait("locked", |s| s["locked"] == true);
    assert!(state["switch"].is_null(), "the switch is over");
    assert_eq!(state["switcher_view"]["shown"], false);

    apps.clear_events();
    pila.click_at(x, y);
    locker.wait("the click on the lock surface", |s| {
        s.events
            .iter()
            .any(|e| matches!(e, common::client::Ev::Button { pressed: false, .. }))
    });
    pila.key_release("Super");
    pila.settle();
    assert!(pila.state()["switch"].is_null());
}

/// An output that changes while locked (here its scale) has its lock surface
/// sized to it again: one left at the old size would leave part of the
/// screen uncovered, or hang off it.
#[test]
fn a_lock_surface_follows_its_output_changing_while_locked() {
    let mut pila = TestPila::start();
    let locker = pila.client();
    let lock = locker.lock_session();
    let surf = locker.lock_surface(&lock, "HEADLESS-1", LOCK_COLOR);
    assert_eq!(locker.wait_surface_configured(surf), (0, 0, 1280, 800));
    locker.wait_logged("lock", "Locked");
    pila.wait("locked", |s| s["locked"] == true);

    pila.write_config("[[output]]\nname = \"HEADLESS-1\"\nscale = 2\n");
    locker.wait("the new size", |s| {
        s.surfaces[surf.0]
            .configures
            .last()
            .is_some_and(|c| (c.2, c.3) == (640, 400))
    });
    assert_eq!(pila.state()["locked"], true);
}
