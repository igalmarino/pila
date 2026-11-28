//! Config reloads: each section applied while running, invalid files kept
//! out, autostart not run again, systemd told once that the session is ready.

mod common;

use common::*;

const BAR: i64 = 28;

#[test]
fn a_new_binding_works_after_a_reload() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").create();
    pila.wait_focus("a");
    pila.write_config("[bindings]\n\"Mod+x\" = \"close\"\n");
    pila.wait_log("config reloaded");
    pila.combo("Super+x");
    client.wait("close request", |s| s.windows[a.0].close_requested);
}

/// A symlinked config (dotfiles managers keep the file elsewhere) is
/// reloaded when the file it points to is edited.
#[test]
fn a_symlinked_config_reloads_when_its_file_changes() {
    let mut pila = TestPila::start_with(Options {
        config_symlink: true,
        ..Options::default()
    });
    pila.wait_log("watching the config for changes");
    pila.write_config("[bindings]\n\"Mod+n\" = \"spawn echo ran > $XDG_RUNTIME_DIR/ran\"\n");
    pila.wait_log("config reloaded");
    pila.combo("Super+n");
    pila.wait_file("ran");
}

/// How many times "config reloaded" is in `pila`'s log.
fn reloads(pila: &TestPila) -> usize {
    pila.log().matches("config reloaded").count()
}

/// Wait for a reload past the `before`th.
fn wait_reload(pila: &TestPila, before: usize) {
    let started = std::time::Instant::now();
    while reloads(pila) <= before {
        assert!(
            started.elapsed() < TIMEOUT,
            "no reload:\n{}",
            pila.log_tail()
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// A symlinked config pointed to another file, in another directory (stow,
/// home-manager): reloaded then, and when that file is edited afterwards.
#[test]
fn a_symlinked_config_follows_its_new_target() {
    let mut pila = TestPila::start_with(Options {
        config_symlink: true,
        ..Options::default()
    });
    pila.wait_log("watching the config for changes");
    let text = std::fs::read_to_string(&pila.config_path).unwrap();
    let elsewhere = pila.dir.join("generation-2");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let target = elsewhere.join("pila.toml");
    std::fs::write(&target, &text).unwrap();
    // As stow and home-manager do: a new link put in place of the old one.
    let link = pila.dir.join("config/config.toml");
    let tmp = pila.dir.join("config/config.toml.new");
    std::os::unix::fs::symlink(&target, &tmp).unwrap();
    std::fs::rename(&tmp, &link).unwrap();
    wait_reload(&pila, 0);
    let before = reloads(&pila);
    let tmp = elsewhere.join("pila.toml.tmp");
    std::fs::write(
        &tmp,
        format!("{text}\n[bindings]\n\"Mod+n\" = \"spawn echo ran > $XDG_RUNTIME_DIR/ran\"\n"),
    )
    .unwrap();
    std::fs::rename(&tmp, &target).unwrap();
    wait_reload(&pila, before);
    pila.combo("Super+n");
    pila.wait_file("ran");
}

/// More changes in the config's directory than inotify keeps while Pila
/// isn't reading: the queue overflows, the config's own change is lost
/// with the rest, and Pila reloads anyway.
#[test]
fn a_config_change_lost_to_an_overflow_still_reloads() {
    let mut pila = TestPila::start();
    pila.wait_log("watching the config for changes");
    let max: usize = std::fs::read_to_string("/proc/sys/fs/inotify/max_queued_events")
        .ok()
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or(16384);
    let pid = pila.pid() as i32;
    assert_eq!(unsafe { libc::kill(pid, libc::SIGSTOP) }, 0);
    let junk = pila.dir.join("config/junk");
    for _ in 0..max / 2 + 1 {
        std::fs::write(&junk, "").unwrap();
        std::fs::remove_file(&junk).unwrap();
    }
    pila.write_config("[bindings]\n\"Mod+n\" = \"spawn echo ran > $XDG_RUNTIME_DIR/ran\"\n");
    assert_eq!(unsafe { libc::kill(pid, libc::SIGCONT) }, 0);
    wait_reload(&pila, 0);
    pila.combo("Super+n");
    pila.wait_file("ran");
}

#[test]
fn an_invalid_file_keeps_the_running_config() {
    let mut pila = TestPila::with_config("[bindings]\n\"Mod+x\" = \"close\"\n");
    let client = pila.client();
    let a = client.window("a").create();
    pila.wait_focus("a");
    pila.write_config("[bindings]\n\"Mod+x\" = \"no-such-action\"\n");
    pila.wait_log("config not reloaded; keeping the running one");
    pila.combo("Super+x");
    client.wait("close request", |s| s.windows[a.0].close_requested);
}

#[test]
fn an_invalid_file_at_startup_is_notified() {
    // pila still starts, on the defaults, and says so on screen as well as in
    // the log: `workspaces` would be 2 if the file had been used.
    let mut pila = TestPila::with_config("workspaces = 2\n[[window-rule]]\nno-such-field = 1\n");
    assert_ne!(pila.state()["workspaces"], 2);
    pila.wait_log("summary=\"Invalid config, using the defaults\"");
    assert!(pila.log().contains("no-such-field"));
}

/// A file that's valid but has a mistake Pila can tell (an `[[output]]`
/// named twice) is used and the mistake shown, at startup and on reload.
#[test]
fn a_config_warning_is_notified() {
    let twice = "[[output]]\nname = \"DP-1\"\nscale = 2\n[[output]]\nname = \"DP-1\"\nscale = 1\n";
    let mut pila = TestPila::with_config(&format!("workspaces = 2\n{twice}"));
    assert_eq!(pila.state()["workspaces"], 2);
    pila.wait_log("summary=\"Config warning\"");
    assert!(pila.log().contains("only the first is used"));
    pila.wait_log("watching the config for changes");
    pila.write_config(&format!("workspaces = 3\n{twice}"));
    pila.wait_log("summary=\"Config reloaded with a warning\"");
    assert_eq!(pila.state()["workspaces"], 3);
}

/// Without a session bus, notifications only go to the log, which says
/// why once.
#[test]
fn no_session_bus_is_logged_once() {
    // The harness gives Pila no D-Bus variables.
    let mut pila = TestPila::start();
    // Whatever level the log is at: a session's has no debug lines.
    let warning = "WARN pila::notifications: no desktop notifications";
    pila.write_config("workspaces = = 6\n");
    pila.wait_log(warning);
    pila.write_config("workspaces = = 7\n");
    let reloads = |log: &str| log.matches("config not reloaded").count();
    let started = std::time::Instant::now();
    while reloads(&pila.log()) < 2 {
        assert!(started.elapsed() < common::TIMEOUT, "{}", pila.log_tail());
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        pila.log().matches(warning).count(),
        1,
        "{}",
        pila.log_tail()
    );
}

/// A bus with no notification daemon on it yet is logged; one started
/// later (by `autostart`, say) gets the next notification.
#[test]
fn a_notification_daemon_started_later_gets_the_next_one() {
    let bus = notifications::Bus::start();
    let mut pila = TestPila::start_with(Options {
        env: vec![bus.env()],
        ..Default::default()
    });
    pila.write_config("workspaces = = 6\n");
    pila.wait_log("WARN pila::notifications: no desktop notifications");
    let daemon = bus.notification_daemon();
    pila.write_config("workspaces = = 7\n");
    let shown = daemon.wait_for(1);
    assert_eq!(shown[0].summary, "Config not reloaded", "{shown:#?}");
}

/// Notifications go to the daemon on the session bus, from `pila`; an error
/// stays on screen longer than a success.
#[test]
fn an_error_stays_on_screen_longer_than_a_success() {
    let bus = notifications::Bus::start();
    let daemon = bus.notification_daemon();
    let mut pila = TestPila::start_with(Options {
        env: vec![bus.env()],
        ..Default::default()
    });
    pila.write_config("workspaces = 3\n");
    let shown = daemon.wait_for(1);
    assert_eq!(
        (shown[0].app_name.as_str(), shown[0].summary.as_str()),
        ("pila", "Config reloaded")
    );
    assert_eq!(shown[0].expire_timeout, 3000, "{shown:#?}");
    pila.write_config("workspaces = = 6\n");
    let shown = daemon.wait_for(2);
    assert_eq!(shown[1].summary, "Config not reloaded");
    assert!(shown[1].body.contains("invalid string"), "{shown:#?}");
    assert_eq!(shown[1].expire_timeout, 10000, "{shown:#?}");
}

#[test]
fn a_syntax_error_keeps_the_running_config() {
    let mut pila = TestPila::with_config("workspaces = 2\n");
    pila.write_config("workspaces = = 6\n");
    pila.wait_log("config not reloaded");
    assert_eq!(pila.state()["workspaces"], 2);
}

#[test]
fn the_keyboard_layout_changes_on_reload() {
    // The key that types y on a US keyboard types z on a German one.
    let mut pila = TestPila::with_config("[bindings]\n\"Mod+z\" = \"close\"\n");
    let client = pila.client();
    let a = client.window("a").create();
    pila.wait_focus("a");
    // evdev KEY_Y (21) is xkb keycode 29.
    pila.request(json!({ "cmd": "key", "key": "Super", "state": "press" }));
    pila.request(json!({ "cmd": "key", "key": "code:29", "state": "tap" }));
    pila.request(json!({ "cmd": "key", "key": "Super", "state": "release" }));
    client.roundtrip();
    assert!(!client.with_window(a, |w| w.close_requested));
    pila.write_config("[bindings]\n\"Mod+z\" = \"close\"\n\n[keyboard]\nlayout = \"de\"\n");
    pila.wait_log("config reloaded");
    pila.request(json!({ "cmd": "key", "key": "Super", "state": "press" }));
    pila.request(json!({ "cmd": "key", "key": "code:29", "state": "tap" }));
    pila.request(json!({ "cmd": "key", "key": "Super", "state": "release" }));
    client.wait("close request", |s| s.windows[a.0].close_requested);
}

#[test]
fn an_invalid_keymap_keeps_the_current_one() {
    let mut pila = TestPila::start();
    pila.write_config("[keyboard]\nlayout = \"no-such-layout\"\n");
    pila.wait_log("invalid keyboard config; keeping the current keymap");
    let client = pila.client();
    let a = client.window("a").create();
    pila.wait_focus("a");
    pila.combo("Super+Shift+q");
    client.wait("close request", |s| s.windows[a.0].close_requested);
}

/// A layout xkbcommon can't build at startup (a typo) leaves Pila with the
/// default keymap rather than no keyboard, and says so on screen.
#[test]
fn an_invalid_keymap_at_startup_falls_back_to_the_default_one() {
    let mut pila = TestPila::with_config("[keyboard]\nlayout = \"no-such-layout\"\n");
    pila.wait_log("invalid keyboard config; using the default keymap");
    pila.wait_log("summary=\"Keyboard layout not found\"");
    assert!(pila.log().contains("no-such-layout"), "{}", pila.log_tail());
    let client = pila.client();
    let a = client.window("a").create();
    pila.wait_focus("a");
    pila.combo("Super+Shift+q");
    client.wait("close request", |s| s.windows[a.0].close_requested);
}

#[test]
fn the_mod_key_changes_on_reload() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").create();
    pila.wait_focus("a");
    pila.write_config("mod = \"alt\"\n");
    pila.wait_log("config reloaded");
    pila.combo("Alt+Shift+q");
    client.wait("close request", |s| s.windows[a.0].close_requested);
}

#[test]
fn output_scale_position_and_transform_apply_on_reload() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1280x800".into()),
        ..Options::default()
    });
    pila.write_config(
        "[[output]]\nname = \"A\"\nscale = 2\n\n\
         [[output]]\nname = \"B\"\nposition = [0, 400]\ntransform = \"90\"\n",
    );
    let state = pila.wait("the new layout", |s| s["outputs"][0]["scale"] == 2.0);
    let outputs = state["outputs"].as_array().unwrap();
    assert_eq!(rect(&outputs[0]["geometry"]), (0, 0, 640, 400));
    // Rotated a quarter turn: logical width and height swap.
    assert_eq!(rect(&outputs[1]["geometry"]), (0, 400, 800, 1280));
    assert_eq!(outputs[1]["transform"], "_90");
}

#[test]
fn an_output_disabled_by_the_config_moves_its_windows() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1280x800".into()),
        ..Options::default()
    });
    pila.move_to(1900.0, 400.0);
    let client = pila.client();
    client.window("a").create();
    let w = pila.wait_window("a");
    assert_eq!(w["output"], "B");
    pila.write_config("[[output]]\nname = \"B\"\nenabled = false\n");
    let state = pila.wait("B off", |s| s["outputs"][1]["enabled"] == false);
    assert!(state["outputs"][1]["geometry"].is_null());
    let state = pila.wait("a moved onto A", |s| {
        window_in(s, "a").unwrap()["output"] == "A"
    });
    let (x, y, w, h) = rect(&window_in(&state, "a").unwrap()["geometry"]);
    assert!(
        x >= 0 && y >= BAR && x + w <= 1280 && y + h <= 800,
        "{x},{y} {w}x{h}"
    );
    // And back.
    pila.write_config("");
    pila.wait("B on", |s| s["outputs"][1]["enabled"] == true);
}

#[test]
fn every_output_off_leaves_one_on() {
    let mut pila = TestPila::start();
    pila.write_config("[[output]]\nname = \"HEADLESS-1\"\nenabled = false\n");
    pila.wait_log("config reloaded");
    let state = pila.settle();
    assert_eq!(state["outputs"][0]["enabled"], true);
}

#[test]
fn autostart_runs_once_not_on_reload() {
    let mut pila =
        TestPila::with_config("autostart = [\"echo started >> $XDG_RUNTIME_DIR/autostart\"]\n");
    assert_eq!(pila.wait_file("autostart"), "started\n");
    pila.write_config("autostart = [\"echo again >> $XDG_RUNTIME_DIR/autostart\"]\n");
    pila.wait_log("config reloaded");
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(pila.wait_file("autostart"), "started\n");
}

#[test]
fn pila_tells_systemd_once_it_is_ready_and_keeps_its_socket_from_children() {
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::{SocketAddr, UnixDatagram};
    // An abstract name, as systemd's own is.
    let name = format!("pila-test-notify-{}", std::process::id());
    let systemd = UnixDatagram::bind_addr(&SocketAddr::from_abstract_name(&name).unwrap()).unwrap();
    systemd.set_read_timeout(Some(TIMEOUT)).unwrap();
    let mut pila = TestPila::start_with(Options {
        config: "autostart = [\"env > $XDG_RUNTIME_DIR/env\"]\n".into(),
        env: vec![("NOTIFY_SOCKET".into(), format!("@{name}"))],
        ..Options::default()
    });
    let mut message = [0; 64];
    let len = systemd.recv(&mut message).expect("READY=1 from pila");
    assert_eq!(&message[..len], b"READY=1");
    let env = pila.wait_file("env");
    assert!(
        !env.lines().any(|l| l.starts_with("NOTIFY_SOCKET=")),
        "NOTIFY_SOCKET reached the startup program:\n{env}"
    );
    // Only once, also after a reload.
    pila.write_config("autostart = []\n");
    pila.wait_log("config reloaded");
    pila.settle();
    systemd.set_nonblocking(true).unwrap();
    assert!(systemd.recv(&mut message).is_err(), "a second message");
}

#[test]
fn a_command_line_command_replaces_autostart() {
    let pila = TestPila::start_with(Options {
        config: "autostart = [\"echo autostart > $XDG_RUNTIME_DIR/ran\"]\n".into(),
        command: Some("echo command > $XDG_RUNTIME_DIR/ran".into()),
        ..Options::default()
    });
    assert_eq!(pila.wait_file("ran"), "command\n");
}

/// The example's own `autostart` commands (waybar, mako, swayidle), also in a
/// file made from it, are left out without a word where their program isn't
/// installed: the user didn't choose them. A command of the user's own still
/// runs, and says so in a notification when its program is missing.
#[test]
fn the_examples_autostart_skips_what_isnt_installed_silently() {
    let defaults = pila::config::Config::from_toml("", &|_| None)
        .unwrap()
        .autostart;
    assert!(!defaults.is_empty());
    let mut autostart: Vec<String> = defaults.iter().map(|c| format!("{c:?}")).collect();
    autostart.push("\"surely-not-installed-xyz\"".into());
    autostart.push("\"echo mine > $XDG_RUNTIME_DIR/ran\"".into());
    // A PATH with nothing on it: none of the defaults' programs are there
    // (echo is the shell's).
    let empty = std::env::temp_dir().join(format!("pila-empty-path-{}", std::process::id()));
    std::fs::create_dir_all(&empty).unwrap();
    let bus = notifications::Bus::start();
    let daemon = bus.notification_daemon();
    let mut pila = TestPila::start_with(Options {
        config: format!("autostart = [{}]\n", autostart.join(", ")),
        env: vec![("PATH".into(), empty.display().to_string()), bus.env()],
        ..Options::default()
    });
    assert_eq!(pila.wait_file("ran"), "mine\n");
    let shown = daemon.wait_for(1);
    assert!(
        shown[0]
            .summary
            .contains("surely-not-installed-xyz isn't installed"),
        "{shown:#?}"
    );
    pila.stays(
        "one notification",
        std::time::Duration::from_millis(300),
        |_| daemon.shown().len() == 1,
    );
    let log = pila.log();
    for command in &defaults {
        assert!(
            !log.contains(&format!("spawning command={command:?}")),
            "{command} was started:\n{log}"
        );
    }
    assert_eq!(
        log.matches("not starting the example's autostart command")
            .count(),
        defaults.len(),
        "{log}"
    );
    let _ = std::fs::remove_dir_all(&empty);
}

#[test]
fn a_reload_with_an_unchanged_file_keeps_windows_as_they_are() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    let before = pila.window("a");
    pila.write_config("");
    pila.wait_log("config reloaded");
    let after = pila.settle();
    assert_eq!(
        window_in(&after, "a").unwrap()["geometry"],
        before["geometry"]
    );
    assert_eq!(after["focused"], "a");
}

/// With no config file at the default path, Pila writes the example there
/// and follows it from then on.
#[test]
fn a_missing_config_is_created_from_the_example_and_followed() {
    let mut pila = TestPila::start_with(Options {
        default_config_path: true,
        ..Options::default()
    });
    pila.wait_log("created the config from the example");
    assert_eq!(
        std::fs::read_to_string(&pila.config_path).unwrap(),
        include_str!("../config.example.toml")
    );
    let client = pila.client();
    let a = client.window("a").create();
    pila.wait_focus("a");
    pila.write_config("[bindings]\n\"Mod+x\" = \"close\"\n");
    pila.wait_log("config reloaded");
    pila.combo("Super+x");
    client.wait("close request", |s| s.windows[a.0].close_requested);
}

#[test]
fn an_existing_config_is_left_as_it_is() {
    let mut pila = TestPila::start_with(Options {
        default_config_path: true,
        config: "# mine\n".into(),
        ..Options::default()
    });
    pila.wait_log("watching the config for changes");
    assert_eq!(
        std::fs::read_to_string(&pila.config_path).unwrap(),
        "# mine\n"
    );
    assert!(!pila.log().contains("created the config from the example"));
}

/// Mod2, Num Lock in the keymaps Pila makes.
const NUM_LOCK: u32 = 16;

/// Whether the client's last modifiers event has Num Lock on.
fn num_lock_on(s: &common::client::Shared) -> Option<bool> {
    s.events.iter().rev().find_map(|e| match e {
        common::client::Ev::Modifiers { locked, .. } => Some(locked & NUM_LOCK != 0),
        _ => None,
    })
}

/// `[keyboard] numlock = true` starts Pila with Num Lock on; without it,
/// it starts off.
#[test]
fn num_lock_starts_on_when_asked() {
    let mut pila = TestPila::with_config("[keyboard]\nnumlock = true\n");
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    client.wait("Num Lock on", |s| num_lock_on(s) == Some(true));

    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    client.wait("Num Lock off", |s| num_lock_on(s) == Some(false));
}

/// A new keymap (another layout on reload) starts with every lock off:
/// with `numlock = true`, Num Lock comes back on with it.
#[test]
fn num_lock_stays_on_when_the_keymap_changes() {
    let mut pila = TestPila::with_config("[keyboard]\nnumlock = true\n");
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    client.wait("Num Lock on", |s| num_lock_on(s) == Some(true));
    pila.write_config("[keyboard]\nlayout = \"de\"\nnumlock = true\n");
    pila.wait_log("config reloaded");
    // A window taking the focus is told the modifiers as they are now.
    client.clear_events();
    client.window("b").create();
    pila.wait_focus("b");
    client.wait("Num Lock on with the new keymap", |s| {
        num_lock_on(s) == Some(true)
    });
}
