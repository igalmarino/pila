//! Session lock (ext-session-lock): what's shown, where input goes, what's
//! refused while locked, unlocking, a dead or rogue locker, takeovers.

mod common;

use common::*;
use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_v1::ExtSessionLockV1;

const LOCK_COLOR: [u8; 4] = [255, 10, 20, 30];
const LOCK_RGBA: [u8; 4] = [10, 20, 30, 255];

/// A window "a", then a locker locked with a surface on HEADLESS-1.
fn locked() -> (TestPila, Client, Client, ExtSessionLockV1) {
    let mut pila = TestPila::start();
    let apps = pila.client();
    apps.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let locker = pila.client();
    let lock = locker.lock_session();
    let surf = locker.lock_surface(&lock, "HEADLESS-1", LOCK_COLOR);
    assert_eq!(locker.wait_surface_configured(surf), (0, 0, 1280, 800));
    locker.wait_logged("lock", "Locked");
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
fn bindings_and_workspace_switches_dont_run_while_locked() {
    let (mut pila, apps, _locker, _lock) = locked();
    pila.combo("Super+Shift+q");
    pila.combo("Super+2");
    pila.combo("Super+m");
    apps.roundtrip();
    let state = pila.settle();
    assert_eq!(state["workspace"], 1);
    assert!(!apps.with_window(common::client::Window(0), |w| w.close_requested));
    assert_eq!(window_in(&state, "a").unwrap()["maximized"], false);
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
    locker.lock_surface(&lock, "A", LOCK_COLOR);
    locker.wait_logged("lock", "Locked");
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
/// the bare surface with no buffer (DMS does it on every unlock): that must
/// not cost the locker its connection.
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
