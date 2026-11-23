//! X11 windows through pila's Xwayland: mapping and WM_CLASS, focus,
//! closing (WM_DELETE_WINDOW), _NET_WM_STATE requests, minimizing,
//! override-redirect windows, rules, the clipboard between X11 and Wayland;
//! Xwayland starting when an app connects, and stopping when none is left.
//! Skipped when Xwayland isn't installed.

mod common;

use std::time::{Duration, Instant};

use common::*;
use x11rb::{
    connection::Connection,
    protocol::{
        xproto::{
            AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt, CreateWindowAux,
            EventMask, PropMode, WindowClass,
        },
        Event,
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

/// An X11 client connected to a test pila's Xwayland.
struct X11 {
    conn: RustConnection,
    root: u32,
    events: std::cell::RefCell<Vec<Event>>,
}

/// Whether Xwayland is installed (else the test is skipped).
fn xwayland_installed() -> bool {
    installed("Xwayland")
}

/// Whether `program` is in `PATH` (else the test is skipped).
fn installed(program: &str) -> bool {
    let installed = std::process::Command::new("sh")
        .args(["-c", &format!("command -v {program}")])
        .output()
        .is_ok_and(|o| o.status.success());
    if !installed {
        eprintln!("{program} not installed: skipped");
    }
    installed
}

impl X11 {
    /// Connect to the display pila reserved, which starts Xwayland; `None`
    /// if it isn't installed.
    fn connect(pila: &mut TestPila) -> Option<X11> {
        if !xwayland_installed() {
            return None;
        }
        let started = Instant::now();
        while !pila.state()["xwayland"].is_number() {
            assert!(
                !pila.log().contains("X11 apps won't run"),
                "Xwayland is installed but didn't start:\n{}",
                pila.log_tail()
            );
            assert!(started.elapsed() < TIMEOUT, "Xwayland never started");
            std::thread::sleep(Duration::from_millis(10));
        }
        let state = pila.state();
        let display = state["xwayland"].as_u64().unwrap() as u32;
        let (conn, screen) =
            pila::xauth::connect(display, &cookie(&state)).expect("connect to Xwayland");
        let root = conn.setup().roots[screen].root;
        Some(X11 {
            conn,
            root,
            events: Default::default(),
        })
    }

    fn atom(&self, name: &str) -> u32 {
        self.conn
            .intern_atom(false, name.as_bytes())
            .unwrap()
            .reply()
            .unwrap()
            .atom
    }

    /// A mapped window of `size` with WM_CLASS `class`, in `color` (RGB).
    fn window(&self, class: &str, size: (u16, u16), color: u32) -> u32 {
        self.window_in_state(class, size, color, false)
    }

    /// A window that asks to start minimized (`WM_HINTS` IconicState) if
    /// `iconic`.
    fn window_in_state(&self, class: &str, size: (u16, u16), color: u32, iconic: bool) -> u32 {
        let id = self.unmapped_window(class, size, color, iconic);
        self.conn.map_window(id).unwrap();
        self.conn.flush().unwrap();
        id
    }

    /// `window_in_state`, not mapped yet: for properties set before.
    fn unmapped_window(&self, class: &str, size: (u16, u16), color: u32, iconic: bool) -> u32 {
        let id = self.conn.generate_id().unwrap();
        self.conn
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                id,
                self.root,
                0,
                0,
                size.0,
                size.1,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new().background_pixel(color).event_mask(
                    EventMask::KEY_PRESS
                        | EventMask::KEY_RELEASE
                        | EventMask::BUTTON_PRESS
                        | EventMask::FOCUS_CHANGE
                        | EventMask::STRUCTURE_NOTIFY
                        | EventMask::PROPERTY_CHANGE,
                ),
            )
            .unwrap();
        let wm_class = format!("{}\0{class}\0", class.to_lowercase());
        self.conn
            .change_property8(
                PropMode::REPLACE,
                id,
                AtomEnum::WM_CLASS,
                AtomEnum::STRING,
                wm_class.as_bytes(),
            )
            .unwrap();
        self.conn
            .change_property8(
                PropMode::REPLACE,
                id,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                class.as_bytes(),
            )
            .unwrap();
        let protocols = self.atom("WM_PROTOCOLS");
        let delete = self.atom("WM_DELETE_WINDOW");
        self.conn
            .change_property32(PropMode::REPLACE, id, protocols, AtomEnum::ATOM, &[delete])
            .unwrap();
        if iconic {
            // flags StateHint, input, initial_state IconicState, then unused fields.
            let hints = [2, 0, 3, 0, 0, 0, 0, 0, 0];
            self.conn
                .change_property32(
                    PropMode::REPLACE,
                    id,
                    AtomEnum::WM_HINTS,
                    AtomEnum::WM_HINTS,
                    &hints,
                )
                .unwrap();
        }
        id
    }

    /// An override-redirect window (a menu) at (x, y), typed as toolkits
    /// type their menus (`_NET_WM_WINDOW_TYPE_POPUP_MENU`).
    fn menu(&self, at: (i16, i16), size: (u16, u16)) -> u32 {
        self.override_redirect(at, size, Some("_NET_WM_WINDOW_TYPE_POPUP_MENU"))
    }

    /// An override-redirect window at (x, y) of window type `kind`, or of
    /// none (a launcher like dmenu), selecting key presses.
    fn override_redirect(&self, at: (i16, i16), size: (u16, u16), kind: Option<&str>) -> u32 {
        let id = self.conn.generate_id().unwrap();
        self.conn
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                id,
                self.root,
                at.0,
                at.1,
                size.0,
                size.1,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new()
                    .background_pixel(0x00ff00)
                    .override_redirect(1)
                    .event_mask(EventMask::KEY_PRESS | EventMask::FOCUS_CHANGE),
            )
            .unwrap();
        if let Some(kind) = kind {
            let (property, kind) = (self.atom("_NET_WM_WINDOW_TYPE"), self.atom(kind));
            self.conn
                .change_property32(PropMode::REPLACE, id, property, AtomEnum::ATOM, &[kind])
                .unwrap();
        }
        self.conn.map_window(id).unwrap();
        self.conn.flush().unwrap();
        id
    }

    /// A client message to the root window, as EWMH requests are sent.
    fn root_message(&self, window: u32, kind: &str, data: [u32; 5]) {
        let event = ClientMessageEvent::new(32, window, self.atom(kind), data);
        self.conn
            .send_event(
                false,
                self.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                event,
            )
            .unwrap();
        self.conn.flush().unwrap();
    }

    /// _NET_WM_STATE add (1) or remove (0) of `states`.
    fn net_wm_state(&self, window: u32, add: bool, states: &[&str]) {
        let a = states.first().map(|s| self.atom(s)).unwrap_or(0);
        let b = states.get(1).map(|s| self.atom(s)).unwrap_or(0);
        self.root_message(window, "_NET_WM_STATE", [u32::from(add), a, b, 1, 0]);
    }

    fn pump(&self) {
        while let Some(event) = self.conn.poll_for_event().unwrap() {
            self.events.borrow_mut().push(event);
        }
    }

    /// Wait until an event satisfies `f`.
    #[track_caller]
    fn wait_event(&self, what: &str, f: impl Fn(&Event) -> bool) {
        let started = Instant::now();
        loop {
            self.pump();
            if self.events.borrow().iter().any(&f) {
                return;
            }
            assert!(
                started.elapsed() < TIMEOUT,
                "X11: timed out waiting for {what}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn net_wm_state_of(&self, window: u32) -> Vec<String> {
        let atom = self.atom("_NET_WM_STATE");
        let reply = self
            .conn
            .get_property(false, window, atom, AtomEnum::ATOM, 0, 32)
            .unwrap()
            .reply()
            .unwrap();
        reply
            .value32()
            .into_iter()
            .flatten()
            .map(|a| {
                String::from_utf8_lossy(&self.conn.get_atom_name(a).unwrap().reply().unwrap().name)
                    .to_string()
            })
            .collect()
    }
}

/// The Xwayland that `pila` started, if one runs.
fn find_xwayland(pila: &TestPila) -> Option<u32> {
    let wanted = format!("PPid:\t{}\n", pila.pid());
    std::fs::read_dir("/proc")
        .unwrap()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .find(|pid| {
            let status = std::fs::read_to_string(format!("/proc/{pid}/status"));
            status.is_ok_and(|s| {
                s.starts_with("Name:\tXwayland\n")
                    && s.contains(&wanted)
                    && !s.contains("State:\tZ")
            })
        })
}

fn xwayland_pid(pila: &TestPila) -> u32 {
    find_xwayland(pila).expect("pila's Xwayland")
}

/// The cookie in the Xauthority file Pila gives X11 apps (`XAUTHORITY`).
fn cookie(state: &serde_json::Value) -> Vec<u8> {
    let path = state["xauthority"].as_str().expect("an Xauthority file");
    pila::xauth::read_cookie(std::path::Path::new(path)).expect("a cookie in it")
}

/// The display pila reserved for X11 apps, as `DISPLAY` names it.
fn reserved_display(pila: &mut TestPila) -> String {
    let state = pila.wait("a reserved X11 display", |s| s["xwayland"].is_number());
    format!(":{}", state["xwayland"].as_u64().unwrap())
}

/// How many files pila has open.
fn open_files(pila: &TestPila) -> usize {
    std::fs::read_dir(format!("/proc/{}/fd", pila.pid()))
        .unwrap()
        .count()
}

#[test]
fn focus_doesnt_land_on_an_x11_menu_on_top() {
    let mut pila = TestPila::start();
    let Some(x11) = X11::connect(&mut pila) else {
        return;
    };
    x11.window("x", (200, 200), 0xff0000);
    pila.wait_focus("x");
    let client = pila.client();
    let w = client.window("w").create();
    pila.wait_focus("w");
    x11.menu((100, 120), (80, 60));
    pila.wait("the menu", |s| {
        s["windows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["override_redirect"] == true)
    });
    client.destroy(w);
    client.flush();
    pila.wait_focus("x");
}

#[test]
fn an_x11_window_can_stay_on_top() {
    let mut pila = TestPila::start();
    let Some(x11) = X11::connect(&mut pila) else {
        return;
    };
    x11.window("x", (200, 200), 0xff0000);
    pila.wait_focus("x");
    pila.combo("Super+t");
    pila.wait("marked", |s| {
        window_in(s, "x").unwrap()["always_on_top"] == true
    });
    let client = pila.client();
    client.window("w").create();
    pila.wait_focus("w");
    assert_eq!(stacking(&pila.state()), ["w", "x"]);
}

#[test]
fn xwayland_starts_with_no_signals_blocked() {
    let mut pila = TestPila::start();
    if X11::connect(&mut pila).is_none() {
        return;
    }
    // Xwayland blocks signals for a moment of its own while it starts; a
    // mask inherited from pila stays.
    let pid = xwayland_pid(&pila);
    let started = Instant::now();
    loop {
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
        let blocked = status.lines().find(|l| l.starts_with("SigBlk:")).unwrap();
        if blocked.ends_with("0000000000000000") {
            break;
        }
        assert!(started.elapsed() < TIMEOUT, "{blocked}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Pila holds an X display from startup and starts Xwayland only when an
/// X11 app connects to it; the session doesn't wait for X.
#[test]
fn xwayland_starts_when_an_x11_app_connects() {
    if !xwayland_installed() {
        return;
    }
    let mut pila = TestPila::start_with(Options {
        command: Some("echo $DISPLAY $XAUTHORITY > $XDG_RUNTIME_DIR/ran".into()),
        ..Options::default()
    });
    let display = reserved_display(&mut pila);
    let xauthority = pila.state()["xauthority"].as_str().unwrap().to_string();
    assert_eq!(
        pila.wait_file("ran").trim(),
        format!("{display} {xauthority}")
    );
    pila.stays(
        "no Xwayland until an X11 app comes",
        Duration::from_millis(500),
        |s| s["xwayland_running"] == false,
    );
    assert_eq!(find_xwayland(&pila), None);
    let x11 = X11::connect(&mut pila).unwrap();
    x11.window("first", (200, 200), 0xff0000);
    pila.wait("its window", |s| x11_window(s, "first").is_some());
    assert!(pila.state()["xwayland_running"] == true);
    xwayland_pid(&pila);
}

/// The reserved display listens only on its socket file, whose permissions
/// keep other users out, never on an abstract socket, which any process in
/// the network namespace reaches: a Flatpak with network access and no X11
/// permission, or another user, would get the clipboard and XTEST into X11
/// apps. X11 apps still connect, through the file.
#[test]
fn the_x11_display_has_no_abstract_socket() {
    if !xwayland_installed() {
        return;
    }
    let mut pila = TestPila::start();
    let display = reserved_display(&mut pila);
    let file = format!("/tmp/.X11-unix/X{}", &display[1..]);
    let unix = std::fs::read_to_string("/proc/net/unix").unwrap();
    let names: Vec<&str> = unix
        .lines()
        .filter_map(|line| line.split_whitespace().nth(7))
        .collect();
    assert!(names.contains(&file.as_str()), "no socket file {file}");
    let abstract_name = format!("@{file}");
    assert!(
        !names.contains(&abstract_name.as_str()),
        "the display listens on {abstract_name}"
    );
    let x11 = X11::connect(&mut pila).unwrap();
    x11.window("through-the-file", (200, 200), 0xff0000);
    pila.wait("its window", |s| {
        x11_window(s, "through-the-file").is_some()
    });
}

/// With `idle-timeout`, Xwayland stops once its last X11 client leaves, and
/// the next one starts it again on the same display, time after time.
#[test]
fn xwayland_stops_when_idle_if_asked() {
    if !xwayland_installed() {
        return;
    }
    let mut pila = TestPila::start_with(Options {
        config: "[xwayland]\nidle-timeout = 1\n".into(),
        ..Options::default()
    });
    let display = reserved_display(&mut pila);
    let mut files = Vec::new();
    for round in 0..3 {
        let x11 = X11::connect(&mut pila).unwrap();
        let class = format!("round{round}");
        x11.window(&class, (200, 200), 0xff0000);
        pila.wait("the window", |s| x11_window(s, &class).is_some());
        // A window open: it keeps running past the timeout.
        pila.stays(
            "Xwayland running with a window",
            Duration::from_millis(1500),
            |s| s["xwayland_running"] == true,
        );
        drop(x11);
        pila.wait("Xwayland stopped", |s| {
            s["xwayland_running"] == false && x11_window(s, &class).is_none()
        });
        let started = Instant::now();
        while find_xwayland(&pila).is_some() {
            assert!(started.elapsed() < TIMEOUT, "Xwayland's process stays");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(reserved_display(&mut pila), display);
        let socket = format!("/tmp/.X11-unix/X{}", &display[1..]);
        assert!(std::path::Path::new(&socket).exists(), "{socket} gone");
        files.push(open_files(&pila));
    }
    // Nothing left behind by each Xwayland (its sockets, the idle checks').
    assert_eq!(files[1], files[2], "{files:?}");
}

/// An X11 client with no window (a settings daemon) keeps Xwayland running.
#[test]
fn an_x11_client_without_windows_keeps_xwayland_running() {
    if !xwayland_installed() {
        return;
    }
    let mut pila = TestPila::start_with(Options {
        config: "[xwayland]\nidle-timeout = 1\n".into(),
        ..Options::default()
    });
    let x11 = X11::connect(&mut pila).unwrap();
    pila.wait("Xwayland up", |s| s["xwayland_running"] == true);
    pila.stays(
        "Xwayland running for a client",
        Duration::from_millis(2500),
        |s| s["xwayland_running"] == true,
    );
    drop(x11);
    pila.wait("Xwayland stopped", |s| s["xwayland_running"] == false);
}

#[test]
fn xwayland_comes_back_after_a_crash() {
    let mut pila = TestPila::start();
    let Some(x11) = X11::connect(&mut pila) else {
        return;
    };
    x11.window("old", (200, 200), 0xff0000);
    pila.wait("the window", |s| x11_window(s, "old").is_some());
    let pid = xwayland_pid(&pila);
    // SAFETY: only a signal, to a process of this test's.
    assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGKILL) }, 0);
    pila.wait("its windows gone", |s| {
        x11_window(s, "old").is_none() && s["xwayland_running"] == false
    });
    // Started again by the next X11 app, on the same display.
    assert_eq!(find_xwayland(&pila), None);
    let x11 = X11::connect(&mut pila).unwrap();
    x11.window("new", (200, 200), 0xff0000);
    pila.wait("a window on the new one", |s| {
        x11_window(s, "new").is_some()
    });
    assert_ne!(xwayland_pid(&pila), pid);
}

/// Whether the X socket of `display` (`:n`) is there.
/// Xwayland runs with `-auth`: a client without the cookie Pila gave its
/// apps (another user, a sandbox without X11 permission that reached the
/// socket) is refused, even while Xwayland runs.
#[test]
fn the_x11_display_takes_only_its_cookie() {
    let mut pila = TestPila::start();
    let Some(_x11) = X11::connect(&mut pila) else {
        return;
    };
    let state = pila.state();
    let display = state["xwayland"].as_u64().unwrap() as u32;
    assert!(pila::xauth::connect(display, &[]).is_err(), "no cookie");
    let mut wrong = cookie(&state);
    wrong[0] ^= 0xff;
    assert!(
        pila::xauth::connect(display, &wrong).is_err(),
        "a wrong one"
    );
    let path = state["xauthority"].as_str().unwrap();
    let mode =
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(path).unwrap().permissions());
    assert_eq!(mode & 0o777, 0o600, "{path} is the user's alone");
}

/// An X11 app Pila starts gets in with the cookie, through libXau's reading
/// of `XAUTHORITY` (xdpyinfo, as any Xlib app).
#[test]
fn an_x11_app_pila_starts_gets_in() {
    if !xwayland_installed() || !installed("xdpyinfo") {
        return;
    }
    let pila = TestPila::start_with(Options {
        command: Some(
            "xdpyinfo > $XDG_RUNTIME_DIR/info 2>&1; echo $? > $XDG_RUNTIME_DIR/ran".into(),
        ),
        ..Options::default()
    });
    let status = pila.wait_file("ran");
    let info = pila.wait_file("info");
    assert_eq!(status.trim(), "0", "{info}");
    assert!(info.contains("name of display"), "{info}");
}

/// Its cookie goes with the display: the file is gone once Pila lets the
/// display go, and turned on again, the display gets a new one.
#[test]
fn the_cookie_goes_with_the_display() {
    if !xwayland_installed() {
        return;
    }
    let mut pila = TestPila::start();
    let state = pila.wait("a reserved X11 display", |s| s["xwayland"].is_number());
    let path = state["xauthority"].as_str().unwrap().to_string();
    let first = cookie(&state);
    pila.write_config("[xwayland]\nenabled = false\n");
    pila.wait("the display given back", |s| s["xwayland"].is_null());
    assert!(!std::path::Path::new(&path).exists(), "{path} stays");
    pila.write_config("");
    let state = pila.wait("a reserved X11 display", |s| s["xwayland"].is_number());
    assert_ne!(cookie(&state), first);
}

fn x_socket_exists(display: &str) -> bool {
    std::path::Path::new(&format!("/tmp/.X11-unix/X{}", &display[1..])).exists()
}

/// `[xwayland] enabled = false`: no display reserved, no `DISPLAY` for what
/// Pila starts, no Xwayland.
#[test]
fn xwayland_can_be_turned_off() {
    if !xwayland_installed() {
        return;
    }
    let mut pila = TestPila::start_with(Options {
        config: "[xwayland]\nenabled = false\n".into(),
        command: Some("echo \"[$DISPLAY][$XAUTHORITY]\" > $XDG_RUNTIME_DIR/ran".into()),
        env: vec![("XAUTHORITY".into(), "/somewhere/else".into())],
        ..Options::default()
    });
    assert_eq!(pila.wait_file("ran").trim(), "[][]");
    assert!(pila.state()["xwayland"].is_null());
    assert!(pila.state()["xauthority"].is_null());
    assert!(pila.log().contains("Xwayland is turned off"));
    assert_eq!(find_xwayland(&pila), None);
}

/// Turned on, the display is reserved at once; turned off with no X11 app,
/// it's given back at once.
#[test]
fn xwayland_turned_on_and_off_on_save() {
    if !xwayland_installed() {
        return;
    }
    let mut pila = TestPila::with_config("[xwayland]\nenabled = false\n");
    assert!(pila.state()["xwayland"].is_null());
    pila.write_config("[xwayland]\nenabled = true\n");
    pila.wait_log("config reloaded");
    let display = reserved_display(&mut pila);
    assert!(x_socket_exists(&display));
    pila.write_config("[xwayland]\nenabled = false\n");
    pila.wait("the display given back", |s| s["xwayland"].is_null());
    assert!(!x_socket_exists(&display), "{display}'s socket stays");
    // And on again: an X11 app runs.
    pila.write_config("");
    let x11 = X11::connect(&mut pila).unwrap();
    x11.window("x", (200, 200), 0xff0000);
    pila.wait("its window", |s| x11_window(s, "x").is_some());
}

/// Turned off while X11 apps run, Xwayland stays for them; once it stops
/// (here for being idle) it isn't started again, and new programs get no
/// `DISPLAY` meanwhile.
#[test]
fn xwayland_turned_off_while_running_stays_for_its_apps() {
    if !xwayland_installed() {
        return;
    }
    let bind = "[bindings]\n\"Mod+x\" = \"spawn echo [$DISPLAY] > $XDG_RUNTIME_DIR/env\"\n";
    let mut pila = TestPila::with_config(bind);
    let x11 = X11::connect(&mut pila).unwrap();
    x11.window("x", (200, 200), 0xff0000);
    pila.wait("its window", |s| x11_window(s, "x").is_some());
    let display = reserved_display(&mut pila);
    pila.write_config(&format!(
        "{bind}[xwayland]\nenabled = false\nidle-timeout = 1\n"
    ));
    pila.wait_log("config reloaded");
    pila.stays(
        "Xwayland running for its window",
        Duration::from_millis(1500),
        |s| s["xwayland_running"] == true && x11_window(s, "x").is_some(),
    );
    pila.combo("Super+x");
    assert_eq!(pila.wait_file("env").trim(), "[]");
    drop(x11);
    pila.wait("Xwayland stopped and the display given back", |s| {
        s["xwayland_running"] == false && s["xwayland"].is_null()
    });
    assert!(!x_socket_exists(&display), "{display}'s socket stays");
}

fn x11_window<'a>(state: &'a Value, class: &str) -> Option<&'a Value> {
    window_in(state, class).filter(|w| w["x11"] == true)
}

#[test]
fn an_x11_window_is_managed_by_its_wm_class() {
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    x.window("XApp", (300, 200), 0xff0000);
    let state = pila.wait("the X11 window", |s| {
        x11_window(s, "XApp").is_some_and(|w| w["mapped"] == true)
    });
    let w = x11_window(&state, "XApp").unwrap();
    assert_eq!((rect(&w["geometry"]).2, rect(&w["geometry"]).3), (300, 200));
    assert_eq!(w["title"], "XApp");
    pila.wait_focus("XApp");
    pila.settle();
    let (gx, gy, ..) = rect(&w["geometry"]);
    // Xwayland draws the content. Without a GPU (CI's container, or software
    // rendering here) it may never present the window's background, so it's only
    // checked with one (pila's side, mapping the window, is checked above anyway).
    let software = std::env::var_os("PILA_HEADLESS_DEVICE").is_some_and(|d| d == "software");
    if std::env::var_os("CI").is_none() && !software {
        pila.wait_pixel((gx + 150) as f64, (gy + 100) as f64, [255, 0, 0, 255]);
    }
}

#[test]
fn keys_reach_the_focused_x11_window() {
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    x.window("XApp", (300, 200), 0xff0000);
    pila.wait_focus("XApp");
    x.wait_event("focus in", |e| matches!(e, Event::FocusIn(_)));
    pila.key("a");
    // X keycode = evdev + 8: a is 38.
    x.wait_event(
        "the key",
        |e| matches!(e, Event::KeyPress(k) if k.detail == 38),
    );
}

#[test]
fn mod_shift_q_asks_an_x11_window_to_close() {
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    x.window("XApp", (300, 200), 0xff0000);
    pila.wait_focus("XApp");
    pila.combo("Super+Shift+q");
    let delete = x.atom("WM_DELETE_WINDOW");
    x.wait_event(
        "WM_DELETE_WINDOW",
        |e| matches!(e, Event::ClientMessage(m) if m.data.as_data32()[0] == delete),
    );
}

#[test]
fn net_wm_state_requests_maximize_and_fullscreen() {
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    let id = x.window("XApp", (300, 200), 0xff0000);
    pila.wait_focus("XApp");
    x.net_wm_state(
        id,
        true,
        &[
            "_NET_WM_STATE_MAXIMIZED_VERT",
            "_NET_WM_STATE_MAXIMIZED_HORZ",
        ],
    );
    pila.wait("maximized", |s| {
        x11_window(s, "XApp").is_some_and(|w| rect(&w["geometry"]) == (0, 28, 1280, 772))
    });
    assert!(x
        .net_wm_state_of(id)
        .iter()
        .any(|s| s == "_NET_WM_STATE_MAXIMIZED_VERT"));
    x.net_wm_state(
        id,
        false,
        &[
            "_NET_WM_STATE_MAXIMIZED_VERT",
            "_NET_WM_STATE_MAXIMIZED_HORZ",
        ],
    );
    pila.wait("restored", |s| {
        x11_window(s, "XApp").unwrap()["geometry"]["w"] == 300
    });
    x.net_wm_state(id, true, &["_NET_WM_STATE_FULLSCREEN"]);
    pila.wait("fullscreen", |s| {
        x11_window(s, "XApp").is_some_and(|w| rect(&w["geometry"]) == (0, 0, 1280, 800))
    });
    // X is told the new geometry directly.
    let geometry = x.conn.get_geometry(id).unwrap().reply().unwrap();
    assert_eq!((geometry.width, geometry.height), (1280, 800));
    x.net_wm_state(id, false, &["_NET_WM_STATE_FULLSCREEN"]);
    pila.wait("out of fullscreen", |s| {
        x11_window(s, "XApp").is_some_and(|w| w["fullscreen"] == false && w["geometry"]["w"] == 300)
    });
}

#[test]
fn pila_maximizing_an_x11_window_sets_its_state() {
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    let id = x.window("XApp", (300, 200), 0xff0000);
    pila.wait_focus("XApp");
    pila.combo("Super+m");
    pila.wait("maximized", |s| {
        x11_window(s, "XApp").unwrap()["geometry"]["w"] == 1280
    });
    let started = Instant::now();
    while !x
        .net_wm_state_of(id)
        .iter()
        .any(|s| s == "_NET_WM_STATE_MAXIMIZED_HORZ")
    {
        assert!(
            started.elapsed() < TIMEOUT,
            "no _NET_WM_STATE_MAXIMIZED_HORZ"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// An X11 app that restores itself maximized sets _NET_WM_STATE before it
/// maps its window: it opens maximized.
#[test]
fn an_x11_window_maximized_before_it_maps_opens_maximized() {
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    let id = x.unmapped_window("XApp", (300, 200), 0xff0000, false);
    let states = [
        x.atom("_NET_WM_STATE_MAXIMIZED_VERT"),
        x.atom("_NET_WM_STATE_MAXIMIZED_HORZ"),
    ];
    x.conn
        .change_property32(
            PropMode::REPLACE,
            id,
            x.atom("_NET_WM_STATE"),
            AtomEnum::ATOM,
            &states,
        )
        .unwrap();
    x.conn.map_window(id).unwrap();
    x.conn.flush().unwrap();
    pila.wait("open maximized", |s| {
        x11_window(s, "XApp").is_some_and(|w| rect(&w["geometry"]) == (0, 28, 1280, 772))
    });
}

/// Snapping a maximized X11 window tells X it's no longer maximized: an app
/// that reads its state (to draw its own maximize button) doesn't think it is.
#[test]
fn snapping_a_maximized_x11_window_unsets_its_maximized_state() {
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    let id = x.window("XApp", (300, 200), 0xff0000);
    pila.wait_focus("XApp");
    pila.combo("Super+m");
    pila.wait("maximized", |s| {
        x11_window(s, "XApp").unwrap()["geometry"]["w"] == 1280
    });
    let maximized = |x: &X11| {
        x.net_wm_state_of(id)
            .iter()
            .any(|s| s == "_NET_WM_STATE_MAXIMIZED_HORZ")
    };
    let started = Instant::now();
    while !maximized(&x) {
        assert!(started.elapsed() < TIMEOUT, "never maximized for X");
        std::thread::sleep(Duration::from_millis(10));
    }
    pila.combo("Super+Left");
    pila.wait("the left half", |s| {
        x11_window(s, "XApp").is_some_and(|w| rect(&w["geometry"]) == (0, 28, 640, 772))
    });
    let started = Instant::now();
    while maximized(&x) {
        assert!(started.elapsed() < TIMEOUT, "still maximized for X");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// An override-redirect window (a menu, a tooltip) that moves itself is
/// drawn where it went.
#[test]
fn an_override_redirect_window_that_moves_is_followed() {
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    x.window("XApp", (300, 200), 0xff0000);
    pila.wait_focus("XApp");
    let menu = x.menu((100, 120), (80, 60));
    let menu_geometry = |s: &Value| {
        s["windows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["override_redirect"] == true)
            .map(|w| rect(&w["geometry"]))
    };
    pila.wait("the menu", |s| menu_geometry(s) == Some((100, 120, 80, 60)));
    x.conn
        .configure_window(menu, &ConfigureWindowAux::new().x(300).y(250))
        .unwrap();
    x.conn.flush().unwrap();
    pila.wait("the menu where it moved", |s| {
        menu_geometry(s) == Some((300, 250, 80, 60))
    });
}

#[test]
fn an_iconify_request_minimizes() {
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    let id = x.window("XApp", (300, 200), 0xff0000);
    pila.wait_focus("XApp");
    // WM_CHANGE_STATE IconicState (3).
    x.root_message(id, "WM_CHANGE_STATE", [3, 0, 0, 0, 0]);
    pila.wait("minimized", |s| {
        x11_window(s, "XApp").unwrap()["minimized"] == true
    });
    let started = Instant::now();
    while !x
        .net_wm_state_of(id)
        .iter()
        .any(|s| s == "_NET_WM_STATE_HIDDEN")
    {
        assert!(started.elapsed() < TIMEOUT, "no _NET_WM_STATE_HIDDEN");
        std::thread::sleep(Duration::from_millis(10));
    }
    pila.combo("Super+Tab");
    pila.wait("back", |s| {
        x11_window(s, "XApp").unwrap()["minimized"] == false
    });
}

/// A minimized X11 window that asks for NormalState (`WM_CHANGE_STATE` 1,
/// as an app raising itself from a tray icon does) comes back.
#[test]
fn a_normal_state_request_restores_a_minimized_x11_window() {
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    let id = x.window("XApp", (300, 200), 0xff0000);
    pila.wait_focus("XApp");
    x.root_message(id, "WM_CHANGE_STATE", [3, 0, 0, 0, 0]);
    pila.wait("minimized", |s| {
        x11_window(s, "XApp").unwrap()["minimized"] == true
    });
    x.root_message(id, "WM_CHANGE_STATE", [1, 0, 0, 0, 0]);
    pila.wait("back and focused", |s| {
        x11_window(s, "XApp").is_some_and(|w| w["minimized"] == false && w["mapped"] == true)
            && s["keyboard_focus"]
                .as_str()
                .is_some_and(|f| f.contains("XApp"))
    });
}

#[test]
fn a_transient_x11_window_stays_above_its_parent() {
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    let parent = x.window("XParent", (400, 300), 0xff0000);
    pila.wait_focus("XParent");
    let child = x.window("XChild", (100, 100), 0x00ff00);
    pila.wait_focus("XChild");
    x.conn
        .change_property32(
            PropMode::REPLACE,
            child,
            AtomEnum::WM_TRANSIENT_FOR,
            AtomEnum::WINDOW,
            &[parent],
        )
        .unwrap();
    x.conn.flush().unwrap();
    // Set while the child is above: nothing to see yet. Focusing the parent
    // must not bury the child; a cycle (the parent on the child) must not hang.
    x.conn
        .change_property32(
            PropMode::REPLACE,
            parent,
            AtomEnum::WM_TRANSIENT_FOR,
            AtomEnum::WINDOW,
            &[child],
        )
        .unwrap();
    x.conn.flush().unwrap();
    x.conn
        .delete_property(parent, AtomEnum::WM_TRANSIENT_FOR.into())
        .unwrap();
    x.conn.flush().unwrap();
    let state = pila.wait_focus("XChild");
    let geometry = rect(&x11_window(&state, "XParent").unwrap()["geometry"]);
    pila.stays(
        "the transient set",
        std::time::Duration::from_millis(100),
        |_| true,
    );
    pila.click_at(geometry.0 as f64 + 5.0, geometry.1 as f64 + 5.0);
    let state = pila.wait_focus("XParent");
    assert_eq!(stacking(&state), vec!["XParent", "XChild"]);
}

/// A parent and a dialog set on it (`WM_TRANSIENT_FOR`), modal from the start
/// (`_NET_WM_STATE_MODAL` set before mapping) if `modal`; the dialog has the
/// focus.
fn x11_parent_and_dialog(pila: &mut TestPila, x: &X11, modal: bool) -> (u32, u32) {
    let parent = x.window("XParent", (400, 300), 0xff0000);
    pila.wait_focus("XParent");
    let dialog = x.unmapped_window("XDialog", (100, 100), 0x00ff00, false);
    x.conn
        .change_property32(
            PropMode::REPLACE,
            dialog,
            AtomEnum::WM_TRANSIENT_FOR,
            AtomEnum::WINDOW,
            &[parent],
        )
        .unwrap();
    if modal {
        let state = x.atom("_NET_WM_STATE");
        let modal = x.atom("_NET_WM_STATE_MODAL");
        x.conn
            .change_property32(PropMode::REPLACE, dialog, state, AtomEnum::ATOM, &[modal])
            .unwrap();
    }
    x.conn.map_window(dialog).unwrap();
    x.conn.flush().unwrap();
    pila.wait_focus("XDialog");
    (parent, dialog)
}

/// Clicks a corner of the parent, which the centered dialog doesn't cover.
fn click_x11_parent(pila: &mut TestPila) {
    let (gx, gy, ..) = rect(&x11_window(&pila.state(), "XParent").unwrap()["geometry"]);
    pila.click_at(gx as f64 + 5.0, gy as f64 + 5.0);
}

#[test]
fn clicking_the_parent_of_a_modal_x11_dialog_focuses_the_dialog() {
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    x11_parent_and_dialog(&mut pila, &x, true);
    let client = pila.client();
    client.window("other").size(100, 100).create();
    pila.wait_focus("other");
    click_x11_parent(&mut pila);
    let state = pila.wait_focus("XDialog");
    assert_eq!(stacking(&state), vec!["other", "XParent", "XDialog"]);
}

#[test]
fn an_x11_dialog_that_isnt_modal_lets_the_parent_have_the_focus() {
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    x11_parent_and_dialog(&mut pila, &x, false);
    click_x11_parent(&mut pila);
    pila.wait_focus("XParent");
}

/// `_NET_WM_STATE_MODAL` asked for after mapping takes the focus from the
/// parent and shows in the window's `_NET_WM_STATE`; taken back, the parent
/// can have the focus again.
#[test]
fn an_x11_dialog_turning_modal_takes_the_focus_from_its_parent() {
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    let (_, dialog) = x11_parent_and_dialog(&mut pila, &x, false);
    click_x11_parent(&mut pila);
    pila.wait_focus("XParent");
    x.net_wm_state(dialog, true, &["_NET_WM_STATE_MODAL"]);
    pila.wait_focus("XDialog");
    assert!(x
        .net_wm_state_of(dialog)
        .iter()
        .any(|s| s == "_NET_WM_STATE_MODAL"));
    x.net_wm_state(dialog, false, &["_NET_WM_STATE_MODAL"]);
    let started = Instant::now();
    while x
        .net_wm_state_of(dialog)
        .iter()
        .any(|s| s == "_NET_WM_STATE_MODAL")
    {
        assert!(started.elapsed() < TIMEOUT, "the modal state stayed");
        std::thread::sleep(Duration::from_millis(10));
    }
    click_x11_parent(&mut pila);
    pila.wait_focus("XParent");
}

#[test]
fn clicking_an_x11_window_focuses_it() {
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    x.window("XApp", (300, 200), 0xff0000);
    pila.wait_focus("XApp");
    let client = pila.client();
    client.window("wayland").size(200, 100).create();
    pila.wait_focus("wayland");
    let (gx, gy, ..) = rect(&pila.window("XApp")["geometry"]);
    pila.click_at((gx + 5) as f64, (gy + 5) as f64);
    pila.wait_focus("XApp");
    x.wait_event("the click", |e| matches!(e, Event::ButtonPress(_)));
}

/// An X11 app may ask to start minimized (`WM_HINTS` IconicState): it maps
/// minimized, and the window that had the focus keeps it.
#[test]
fn an_x11_window_can_start_minimized() {
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    x.window("XApp", (300, 200), 0xff0000);
    pila.wait_focus("XApp");
    x.window_in_state("Iconic", (300, 200), 0x00ff00, true);
    let state = pila.wait("Iconic minimized", |s| {
        window_in(s, "Iconic").is_some_and(|w| w["minimized"] == true)
    });
    assert_eq!(state["focused"], "XApp");
}

#[test]
fn an_override_redirect_window_goes_where_x_puts_it_and_isnt_switched_to() {
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    x.window("XApp", (300, 200), 0xff0000);
    pila.wait_focus("XApp");
    x.menu((100, 120), (80, 60));
    let state = pila.wait("the menu", |s| {
        s["windows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["override_redirect"] == true)
    });
    let menu = state["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["override_redirect"] == true)
        .unwrap();
    assert_eq!(rect(&menu["geometry"]), (100, 120, 80, 60));
    assert_eq!(state["focused"], "XApp");
    pila.key_press("Super");
    pila.key("Tab");
    let state = pila.settle();
    assert!(state["switch"].is_null(), "only one switchable window");
    pila.key_release("Super");
}

/// A launcher like dmenu: an override-redirect window with no menu type
/// that grabs the keyboard on the root window. X only gets keys while one of
/// Xwayland's surfaces has the keyboard, so the launcher takes it from the
/// Wayland window that had it, and gives it back when it closes (issue #5).
#[test]
fn an_x11_launcher_takes_the_keyboard_from_a_wayland_window() {
    use x11rb::protocol::xproto::GrabMode;
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    let client = pila.client();
    let _w = client.window("w").create();
    pila.wait_focus("w");
    let grabbed = x
        .conn
        .grab_keyboard(
            true,
            x.root,
            x11rb::CURRENT_TIME,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        )
        .unwrap()
        .reply()
        .unwrap();
    assert_eq!(grabbed.status, x11rb::protocol::xproto::GrabStatus::SUCCESS);
    let launcher = x.override_redirect((0, 0), (400, 30), None);
    pila.wait("the launcher", |s| {
        s["windows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["override_redirect"] == true)
    });
    pila.settle();
    pila.key("a");
    // X keycode = evdev + 8: a is 38.
    x.wait_event(
        "the key",
        |e| matches!(e, Event::KeyPress(k) if k.detail == 38),
    );
    x.conn.ungrab_keyboard(x11rb::CURRENT_TIME).unwrap();
    x.conn.unmap_window(launcher).unwrap();
    x.conn.flush().unwrap();
    pila.wait_focus("w");
}

/// A launcher mapped while the session is locked leaves the keyboard with
/// the lock surface.
#[test]
fn an_x11_launcher_doesnt_take_the_keyboard_while_locked() {
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    let locker = pila.client();
    let lock = locker.lock_session();
    let surf = locker.lock_surface(&lock, "HEADLESS-1", [0, 0, 255, 255]);
    locker.wait_surface_configured(surf);
    locker.wait_logged("lock", "Locked");
    pila.wait("locked", |s| {
        s["locked"] == true && s["lock_pending"] == false
    });
    x.override_redirect((0, 0), (400, 30), None);
    pila.wait("the launcher", |s| {
        s["windows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["override_redirect"] == true)
    });
    assert_eq!(pila.settle()["keyboard_focus"], "lock");
}

/// An X11 menu (override-redirect, typed as one) leaves the keyboard where
/// it is: its app's own window keeps it.
#[test]
fn an_x11_menu_leaves_the_keyboard_where_it_is() {
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    let client = pila.client();
    let _w = client.window("w").create();
    pila.wait_focus("w");
    for kind in [
        "_NET_WM_WINDOW_TYPE_POPUP_MENU",
        "_NET_WM_WINDOW_TYPE_DROPDOWN_MENU",
        "_NET_WM_WINDOW_TYPE_TOOLTIP",
        "_NET_WM_WINDOW_TYPE_COMBO",
    ] {
        x.override_redirect((0, 0), (100, 30), Some(kind));
    }
    pila.wait("the menus", |s| {
        s["windows"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|w| w["override_redirect"] == true)
            .count()
            == 4
    });
    pila.stays("the focus on w", Duration::from_millis(300), |s| {
        s["focused"] == "w"
    });
}

#[test]
fn rules_size_and_fill_x11_windows() {
    let mut pila = TestPila::with_config(
        "[[window-rule]]\napp-id = \"Sized\"\nsize = [500, 300]\n\n\
         [[window-rule]]\napp-id = \"Full\"\nopen-fullscreen = true\n",
    );
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    x.window("Sized", (100, 100), 0xff0000);
    pila.wait("sized", |s| {
        x11_window(s, "Sized")
            .is_some_and(|w| w["geometry"]["w"] == 500 && w["geometry"]["h"] == 300)
    });
    x.window("Full", (100, 100), 0x00ff00);
    pila.wait("fullscreen", |s| {
        x11_window(s, "Full").is_some_and(|w| rect(&w["geometry"]) == (0, 0, 1280, 800))
    });
}

/// An X11 app sets the urgency hint (`WM_HINTS` UrgencyHint) when it wants
/// attention: the window is urgent until it has the focus.
#[test]
fn an_x11_urgency_hint_makes_the_window_urgent() {
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    let id = x.window("XApp", (300, 200), 0xff0000);
    pila.wait_focus("XApp");
    let client = pila.client();
    client.window("editor").create();
    pila.wait_focus("editor");
    // flags UrgencyHint, then unused fields.
    x.conn
        .change_property32(
            PropMode::REPLACE,
            id,
            AtomEnum::WM_HINTS,
            AtomEnum::WM_HINTS,
            &[256, 0, 0, 0, 0, 0, 0, 0, 0],
        )
        .unwrap();
    x.conn.flush().unwrap();
    pila.wait("XApp urgent", |s| {
        x11_window(s, "XApp").is_some_and(|w| w["urgent"] == true)
    });
    x.conn
        .change_property32(
            PropMode::REPLACE,
            id,
            AtomEnum::WM_HINTS,
            AtomEnum::WM_HINTS,
            &[0; 9],
        )
        .unwrap();
    x.conn.flush().unwrap();
    pila.wait("XApp no longer urgent", |s| {
        x11_window(s, "XApp").is_some_and(|w| w["urgent"] == false)
    });
}

/// A rule on an X11 window's title follows its `WM_NAME`.
#[test]
fn rules_follow_an_x11_title() {
    let mut pila = TestPila::with_config("[[window-rule]]\ntitle = \"two\"\nopacity = 0.3\n");
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    let id = x.window("XApp", (300, 200), 0xff0000);
    let state = pila.wait_focus("XApp");
    assert!(x11_window(&state, "XApp").unwrap()["opacity"].is_null());
    x.conn
        .change_property8(
            PropMode::REPLACE,
            id,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            b"two",
        )
        .unwrap();
    x.conn.flush().unwrap();
    pila.wait("the title rule applied", |s| {
        x11_window(s, "XApp").is_some_and(|w| w["opacity"] == 0.3)
    });
}

#[test]
fn an_x11_window_on_another_workspace_is_hidden_from_x_too() {
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    let id = x.window("XApp", (300, 200), 0xff0000);
    pila.wait_focus("XApp");
    pila.combo("Super+2");
    pila.wait("2", |s| s["workspace"] == 2);
    pila.combo("Super+1");
    pila.wait_focus("XApp");
    // Still a working X window afterwards.
    let geometry = x.conn.get_geometry(id).unwrap().reply().unwrap();
    assert_eq!((geometry.width, geometry.height), (300, 200));
}

#[test]
fn a_moved_x11_window_is_told_where_it_is() {
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    let id = x.window("XApp", (300, 200), 0xff0000);
    pila.wait_focus("XApp");
    let (gx, gy, ..) = rect(&pila.window("XApp")["geometry"]);
    pila.combo("Super+Shift+Left");
    pila.wait("moved", |s| {
        x11_window(s, "XApp").unwrap()["geometry"]["x"] == gx - 40
    });
    let started = Instant::now();
    loop {
        let g = x.conn.get_geometry(id).unwrap().reply().unwrap();
        let t = x
            .conn
            .translate_coordinates(id, x.root, 0, 0)
            .unwrap()
            .reply()
            .unwrap();
        if (t.dst_x as i64, t.dst_y as i64) == (gx - 40, gy) && g.width == 300 {
            break;
        }
        assert!(
            started.elapsed() < TIMEOUT,
            "X thinks it's at {},{}",
            t.dst_x,
            t.dst_y
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn the_clipboard_goes_from_wayland_to_a_focused_x11_app() {
    clipboard_from_wayland_to_x11(true);
}

/// Xwayland starts after a Wayland client set the clipboard: X gets it too.
#[test]
fn the_clipboard_set_before_xwayland_starts_reaches_x11_apps() {
    clipboard_from_wayland_to_x11(false);
}

fn clipboard_from_wayland_to_x11(x11_first: bool) {
    if !xwayland_installed() {
        return;
    }
    let mut pila = TestPila::start();
    let x11 = x11_first.then(|| X11::connect(&mut pila).unwrap());
    let source_client = pila.client();
    source_client.window("source").create();
    pila.wait_focus("source");
    use wayland_client::protocol::wl_data_device_manager::WlDataDeviceManager;
    let manager: WlDataDeviceManager = source_client.bind("wl_data_device_manager", 3);
    let device = manager.get_data_device(&source_client.seat(), &source_client.qh, "device".into());
    let source = source_client.data_source("source", "to X");
    // As real apps do: X11's UTF8_STRING is text/plain;charset=utf-8.
    source.offer("text/plain;charset=utf-8".into());
    device.set_selection(Some(&source), source_client.last_serial());
    source_client.flush();
    source_client.roundtrip();
    let x = x11.unwrap_or_else(|| {
        assert!(pila.state()["xwayland_running"] == false);
        X11::connect(&mut pila).unwrap()
    });
    let id = x.window("XApp", (300, 200), 0xff0000);
    pila.wait_focus("XApp");
    x.wait_event("focus in", |e| matches!(e, Event::FocusIn(_)));
    assert_eq!(x11_reads(&x, id, "CLIPBOARD"), "to X");
}

/// The text the X11 window `id` reads from `selection` (`CLIPBOARD` or
/// `PRIMARY`) as UTF8_STRING, asking again until there is some.
fn x11_reads(x: &X11, id: u32, selection: &str) -> String {
    let selection = x.atom(selection);
    let utf8 = x.atom("UTF8_STRING");
    let property = x.atom("PILA_TEST");
    let started = Instant::now();
    loop {
        x.conn
            .convert_selection(id, selection, utf8, property, x11rb::CURRENT_TIME)
            .unwrap();
        x.conn.flush().unwrap();
        x.wait_event("SelectionNotify", |e| {
            matches!(e, Event::SelectionNotify(_))
        });
        let reply = x
            .conn
            .get_property(true, id, property, AtomEnum::ANY, 0, 1024)
            .unwrap()
            .reply()
            .unwrap();
        if !reply.value.is_empty() {
            return String::from_utf8_lossy(&reply.value).to_string();
        }
        x.events.borrow_mut().clear();
        assert!(started.elapsed() < TIMEOUT, "no selection for X");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn the_clipboard_goes_from_x11_to_a_wayland_client() {
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    let id = x.window("XApp", (300, 200), 0xff0000);
    pila.wait_focus("XApp");
    let answering = X11Selection::own(x, id, "CLIPBOARD", "from X");

    use wayland_client::protocol::wl_data_device_manager::WlDataDeviceManager;
    let reader = pila.client();
    let manager: WlDataDeviceManager = reader.bind("wl_data_device_manager", 3);
    let _device = manager.get_data_device(&reader.seat(), &reader.qh, "device".into());
    reader.window("reader").create();
    pila.wait_focus("reader");
    reader.wait("an offer", |s| s.data_offer.is_some());
    let offer = reader.lock().data_offer.clone().unwrap();
    let text = reader.receive(|fd| offer.receive("text/plain;charset=utf-8".into(), fd));
    drop(answering);
    assert_eq!(text, "from X");
}

/// An X11 app owning `selection` with `text` (UTF8_STRING), answering
/// requests for it from a thread while a Wayland client reads; until
/// dropped.
struct X11Selection {
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl X11Selection {
    fn own(x: X11, id: u32, selection: &str, text: &'static str) -> Self {
        use x11rb::protocol::xproto::{SelectionNotifyEvent, SELECTION_NOTIFY_EVENT};
        let selection = x.atom(selection);
        let utf8 = x.atom("UTF8_STRING");
        let targets = x.atom("TARGETS");
        x.conn
            .set_selection_owner(id, selection, x11rb::CURRENT_TIME)
            .unwrap();
        x.conn.flush().unwrap();
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread = {
            let done = done.clone();
            std::thread::spawn(move || {
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    while let Some(event) = x.conn.poll_for_event().unwrap() {
                        let Event::SelectionRequest(request) = event else {
                            continue;
                        };
                        if request.target == targets {
                            x.conn
                                .change_property32(
                                    PropMode::REPLACE,
                                    request.requestor,
                                    request.property,
                                    AtomEnum::ATOM,
                                    &[targets, utf8],
                                )
                                .unwrap();
                        } else {
                            x.conn
                                .change_property8(
                                    PropMode::REPLACE,
                                    request.requestor,
                                    request.property,
                                    request.target,
                                    text.as_bytes(),
                                )
                                .unwrap();
                        }
                        let notify = SelectionNotifyEvent {
                            response_type: SELECTION_NOTIFY_EVENT,
                            sequence: 0,
                            time: request.time,
                            requestor: request.requestor,
                            selection: request.selection,
                            target: request.target,
                            property: request.property,
                        };
                        x.conn
                            .send_event(false, request.requestor, EventMask::NO_EVENT, notify)
                            .unwrap();
                        x.conn.flush().unwrap();
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            })
        };
        X11Selection {
            done,
            thread: Some(thread),
        }
    }
}

impl Drop for X11Selection {
    fn drop(&mut self) {
        self.done.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The primary selection (middle-click paste) goes from an X11 app to a
/// Wayland one, and from a Wayland app to an X11 one, as the clipboard does.
#[test]
fn the_primary_selection_goes_between_x11_and_wayland() {
    use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1;
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    let id = x.window("XApp", (300, 200), 0xff0000);
    pila.wait_focus("XApp");
    let answering = X11Selection::own(x, id, "PRIMARY", "selected in X");

    let wayland = pila.client();
    let manager: ZwpPrimarySelectionDeviceManagerV1 =
        wayland.bind("zwp_primary_selection_device_manager_v1", 1);
    let device = manager.get_device(&wayland.seat(), &wayland.qh, "primary".into());
    wayland.window("wayland").create();
    pila.wait_focus("wayland");
    wayland.wait("a primary offer", |s| s.primary_offer.is_some());
    let offer = wayland.lock().primary_offer.clone().unwrap();
    let text = wayland.receive(|fd| offer.receive("text/plain;charset=utf-8".into(), fd));
    drop(answering);
    assert_eq!(text, "selected in X");

    // Back the other way, to a new X11 client.
    let source = manager.create_source(
        &wayland.qh,
        common::protocols::SourceText {
            tag: "primary_source".into(),
            text: "selected in Wayland".into(),
        },
    );
    source.offer("text/plain;charset=utf-8".into());
    device.set_selection(Some(&source), wayland.last_serial());
    wayland.flush();
    wayland.roundtrip();
    let x = X11::connect(&mut pila).unwrap();
    let id = x.window("XReader", (300, 200), 0x00ff00);
    pila.wait_focus("XReader");
    x.wait_event("focus in", |e| matches!(e, Event::FocusIn(_)));
    assert_eq!(x11_reads(&x, id, "PRIMARY"), "selected in Wayland");
}

#[test]
fn an_unfocused_x11_app_cant_read_the_clipboard() {
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    let id = x.window("XApp", (300, 200), 0xff0000);
    pila.wait_focus("XApp");
    let source_client = pila.client();
    source_client.window("source").create();
    pila.wait_focus("source");
    use wayland_client::protocol::wl_data_device_manager::WlDataDeviceManager;
    let manager: WlDataDeviceManager = source_client.bind("wl_data_device_manager", 3);
    let device = manager.get_data_device(&source_client.seat(), &source_client.qh, "device".into());
    let source = source_client.data_source("source", "secret");
    source.offer("text/plain;charset=utf-8".into());
    device.set_selection(Some(&source), source_client.last_serial());
    source_client.flush();
    source_client.roundtrip();
    let clipboard = x.atom("CLIPBOARD");
    let utf8 = x.atom("UTF8_STRING");
    let property = x.atom("PILA_TEST");
    x.conn
        .convert_selection(id, clipboard, utf8, property, x11rb::CURRENT_TIME)
        .unwrap();
    x.conn.flush().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let reply = x
        .conn
        .get_property(false, id, property, AtomEnum::ANY, 0, 1024)
        .unwrap()
        .reply()
        .unwrap();
    assert_ne!(String::from_utf8_lossy(&reply.value), "secret");
}

/// An Xwayland that dies before it's ready (a broken install, a crash)
/// leaves X11 apps out, not the session: the startup command runs, apps
/// fail rather than hang, and after a few tries pila stops starting it.
#[test]
fn a_broken_xwayland_stops_being_started() {
    let bin = std::env::temp_dir().join(format!("pila-xwayland-dies-{}", std::process::id()));
    std::fs::create_dir_all(&bin).unwrap();
    let xwayland = bin.join("Xwayland");
    std::fs::write(&xwayland, "#!/bin/sh\nkill -ABRT $$\n").unwrap();
    std::fs::set_permissions(
        &xwayland,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut pila = TestPila::start_with(Options {
        command: Some("echo started > $XDG_RUNTIME_DIR/ran".into()),
        env: vec![("PATH".into(), path)],
        ..Options::default()
    });
    assert_eq!(pila.wait_file("ran"), "started\n");
    let display = reserved_display(&mut pila);
    // Each try fails the app waiting; after the last one it's refused.
    let started = Instant::now();
    while !pila.log().contains("Xwayland keeps failing") {
        assert!(started.elapsed() < TIMEOUT, "Xwayland never given up on");
        assert!(x11rb::connect(Some(&display)).is_err());
    }
    pila.wait("no display left", |s| s["xwayland"].is_null());
    assert!(pila.log().contains("Xwayland failed to start"));
    let _ = std::fs::remove_dir_all(&bin);
}

/// Mod+right drag resizes an X11 window too: from the top-left quarter its
/// top-left corner follows the pointer and the bottom-right one stays.
#[test]
fn mod_right_drag_resizes_an_x11_window_from_its_corner() {
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    x.window("XApp", (300, 200), 0xff0000);
    pila.wait_focus("XApp");
    let (gx, gy, gw, gh) = rect(&pila.window("XApp")["geometry"]);
    pila.move_to((gx + 30) as f64, (gy + 30) as f64);
    pila.key_press("Super");
    pila.press("right");
    pila.wait("a resize grab", |s| s["pointer"]["grab"] == "resize");
    pila.move_by(-40.0, -30.0);
    pila.wait("resized up and left", |s| {
        x11_window(s, "XApp")
            .is_some_and(|w| rect(&w["geometry"]) == (gx - 40, gy - 30, gw + 40, gh + 30))
    });
    pila.release("right");
    pila.key_release("Super");
    pila.wait("the grab ends", |s| s["pointer"]["grabbed"] == false);
}

/// An X11 window's own opacity (`_NET_WM_WINDOW_OPACITY`, as compositing
/// apps and `xprop`/`picom-trans` set it) over the background, with the
/// window rule `config` (none, or rounded corners).
fn x11_opacity_shows(config: &str) {
    let mut pila = TestPila::with_config(config);
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    let id = x.unmapped_window("Translucent", (300, 200), 0xff0000, false);
    let opacity = x.atom("_NET_WM_WINDOW_OPACITY");
    x.conn
        .change_property32(
            PropMode::REPLACE,
            id,
            opacity,
            AtomEnum::CARDINAL,
            &[u32::MAX / 2],
        )
        .unwrap();
    x.conn.map_window(id).unwrap();
    x.conn.flush().unwrap();
    let state = pila.wait("the window", |s| x11_window(s, "Translucent").is_some());
    let (wx, wy, w, h) = rect(&x11_window(&state, "Translucent").unwrap()["geometry"]);
    // Red at half over the background ([31, 33, 38]): about [143, 16, 19].
    pila.wait_pixel_is(
        (wx + w / 2) as f64,
        (wy + h / 2) as f64,
        "red at half opacity over the background",
        |[r, g, b, _]| (130..=156).contains(&r) && g < 30 && b < 30,
    );
}

#[test]
fn an_x11_windows_own_opacity_shows() {
    x11_opacity_shows("");
}

#[test]
fn an_x11_windows_own_opacity_shows_with_rounded_corners() {
    x11_opacity_shows("[[window-rule]]\napp-id = \"Translucent\"\ncorner-radius = 24\n");
}

/// An X11 app that grabs the keyboard (a VM, a remote desktop: XGrabKeyboard,
/// which Xwayland turns into xwayland-keyboard-grab) gets Pila's bindings'
/// keys while it has the focus, as with keyboard-shortcuts-inhibit; ungrabbed,
/// they're Pila's again.
#[test]
fn an_x11_keyboard_grab_gets_pilas_keys_while_focused() {
    use x11rb::protocol::xproto::GrabMode;
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    let window = x.window("XApp", (300, 200), 0xff0000);
    pila.wait_focus("XApp");
    x.wait_event("focus in", |e| matches!(e, Event::FocusIn(_)));
    let grabbed = x
        .conn
        .grab_keyboard(
            false,
            window,
            x11rb::CURRENT_TIME,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        )
        .unwrap()
        .reply()
        .unwrap();
    assert_eq!(grabbed.status, x11rb::protocol::xproto::GrabStatus::SUCCESS);
    pila.wait("the grab", |s| s["shortcuts_inhibited"] == true);
    // Super+m maximizes no more: m goes to the app (X keycode = evdev + 8).
    pila.combo("Super+m");
    x.wait_event("m", |e| matches!(e, Event::KeyPress(k) if k.detail == 58));
    assert_eq!(
        x11_window(&pila.settle(), "XApp").unwrap()["maximized"],
        false
    );
    // Ungrabbed: Pila's again.
    x.conn.ungrab_keyboard(x11rb::CURRENT_TIME).unwrap();
    x.conn.flush().unwrap();
    pila.wait("no grab", |s| s["shortcuts_inhibited"] == false);
    pila.combo("Super+m");
    pila.wait("maximized", |s| {
        x11_window(s, "XApp").is_some_and(|w| w["maximized"] == true)
    });
}

/// An X11 keyboard grab doesn't hold the focus: a click on another window
/// takes it, and Pila's bindings work there.
#[test]
fn an_x11_keyboard_grab_doesnt_keep_the_focus() {
    use x11rb::protocol::xproto::GrabMode;
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    let client = pila.client();
    client.window("b").size(300, 200).create();
    pila.wait_focus("b");
    let window = x.window("XApp", (300, 200), 0xff0000);
    pila.wait_focus("XApp");
    x.wait_event("focus in", |e| matches!(e, Event::FocusIn(_)));
    x.conn
        .grab_keyboard(
            false,
            window,
            x11rb::CURRENT_TIME,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        )
        .unwrap()
        .reply()
        .unwrap();
    pila.wait("the grab", |s| s["shortcuts_inhibited"] == true);
    // The Wayland window, clear of the X11 one.
    let state = pila.settle();
    let (bx, by, bw, bh) = rect(&window_in(&state, "b").unwrap()["geometry"]);
    let (xx, xy, xw, xh) = rect(&x11_window(&state, "XApp").unwrap()["geometry"]);
    let spot = [
        (bx + 5, by + 5),
        (bx + bw - 5, by + bh - 5),
        (bx + 5, by + bh - 5),
        (bx + bw - 5, by + 5),
    ]
    .into_iter()
    .find(|&(px, py)| !(xx..xx + xw).contains(&px) || !(xy..xy + xh).contains(&py))
    .expect("a part of b the X11 window doesn't cover");
    pila.click_at(spot.0 as f64, spot.1 as f64);
    pila.wait_focus("b");
    assert_eq!(pila.state()["shortcuts_inhibited"], false);
    pila.combo("Super+m");
    pila.wait("b maximized", |s| {
        window_in(s, "b").is_some_and(|w| w["maximized"] == true)
    });
}

/// An X11 app resizing its own window (`ConfigureWindow`, as xterm does for
/// an escape sequence or a game for a new resolution): the size is the
/// app's, the place stays Pila's.
#[test]
fn an_x11_window_can_resize_itself() {
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    let id = x.window("XApp", (300, 200), 0xff0000);
    pila.wait_focus("XApp");
    let (gx, gy, ..) = rect(&pila.window("XApp")["geometry"]);
    x.conn
        .configure_window(
            id,
            &ConfigureWindowAux::new().x(5).y(5).width(500).height(350),
        )
        .unwrap();
    x.conn.flush().unwrap();
    pila.wait("resized where it was", |s| {
        x11_window(s, "XApp").is_some_and(|w| rect(&w["geometry"]) == (gx, gy, 500, 350))
    });
    let started = Instant::now();
    while x.conn.get_geometry(id).unwrap().reply().unwrap().width != 500 {
        assert!(started.elapsed() < TIMEOUT, "X wasn't told the new size");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Press the left button at (x, y), on an X11 window, and wait for the app
/// to see it.
fn press_on_x11(pila: &mut TestPila, x: &X11, at: (i64, i64)) {
    let presses = |x: &X11| {
        x.events
            .borrow()
            .iter()
            .filter(|e| matches!(e, Event::ButtonPress(_)))
            .count()
    };
    x.pump();
    let before = presses(x);
    pila.move_to(at.0 as f64, at.1 as f64);
    pila.press("left");
    let started = Instant::now();
    loop {
        x.pump();
        if presses(x) > before {
            break;
        }
        assert!(started.elapsed() < TIMEOUT, "X11: no button press");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// An X11 app with a title bar or handles of its own (Electron, Chrome,
/// Steam) hands a drag of them to the window manager with
/// `_NET_WM_MOVERESIZE` while the button is held: Pila moves or resizes the
/// window until it's released, from the edge the app named.
#[test]
fn an_x11_app_can_start_a_move_or_resize_of_its_window() {
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    let id = x.window("XApp", (300, 200), 0xff0000);
    pila.wait_focus("XApp");
    let (gx, gy, gw, gh) = rect(&pila.window("XApp")["geometry"]);
    // `_NET_WM_MOVERESIZE`: x_root, y_root, direction, button, source (an app).
    let drag = |pila: &mut TestPila, at: (i64, i64), direction: u32, by: (f64, f64)| {
        press_on_x11(pila, &x, at);
        x.root_message(
            id,
            "_NET_WM_MOVERESIZE",
            [at.0 as u32, at.1 as u32, direction, 1, 1],
        );
        let kind = if direction == 8 { "move" } else { "resize" };
        pila.wait(kind, |s| s["pointer"]["grab"] == kind);
        pila.move_by(by.0, by.1);
    };
    let geometry_is = |pila: &mut TestPila, what: &str, want: (i64, i64, i64, i64)| {
        pila.wait(what, |s| {
            x11_window(s, "XApp").is_some_and(|w| rect(&w["geometry"]) == want)
        });
        pila.release("left");
        pila.wait("the grab ends", |s| s["pointer"]["grabbed"] == false);
    };

    // Moved (direction 8), by the top of the window.
    drag(&mut pila, (gx + 50, gy + 10), 8, (60.0, 40.0));
    let (gx, gy) = (gx + 60, gy + 40);
    geometry_is(&mut pila, "moved", (gx, gy, gw, gh));
    // Resized from the bottom right corner (4): the top left stays.
    drag(&mut pila, (gx + gw - 5, gy + gh - 5), 4, (50.0, 30.0));
    let (gw, gh) = (gw + 50, gh + 30);
    geometry_is(&mut pila, "resized from the bottom right", (gx, gy, gw, gh));
    // From the top left corner (0): the bottom right stays.
    drag(&mut pila, (gx + 5, gy + 5), 0, (-30.0, -20.0));
    geometry_is(
        &mut pila,
        "resized from the top left",
        (gx - 30, gy - 20, gw + 30, gh + 20),
    );
}

/// An X11 app outside a sandbox asking to share the screen (no app id) is
/// named by the X11 window it says it asks from (`parent_window`
/// `x11:<id>`): its class.
#[test]
fn an_x11_app_is_named_by_its_window_in_the_share_picker() {
    use common::pipewire::{Daemon, Portal};
    let bus = notifications::Bus::start();
    let pipewire = Daemon::start();
    let mut pila = TestPila::start_with(Options {
        env: vec![bus.env(), pipewire.env()],
        ..Options::default()
    });
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    let id = x.window("XApp", (300, 200), 0xff0000);
    pila.wait("the X11 window", |s| {
        x11_window(s, "XApp").is_some_and(|w| w["mapped"] == true)
    });
    let mut portal = Portal::connect(&bus.address);
    portal.parent_window = format!("x11:{id:x}");
    let started = portal.request("x11named", "", 1);
    let state = pila.wait("the picker", |s| s["share_picker"]["shown"] == true);
    assert_eq!(state["share_picker"]["app"], "XApp");
    pila.key("Escape");
    assert_eq!(started.answer().0, 1);
}
