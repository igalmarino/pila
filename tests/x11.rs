//! X11 windows through pila's Xwayland: mapping and WM_CLASS, focus,
//! closing (WM_DELETE_WINDOW), _NET_WM_STATE requests, minimizing,
//! override-redirect windows, rules, the clipboard between X11 and Wayland.
//! Skipped when Xwayland isn't installed.

mod common;

use std::time::{Duration, Instant};

use common::*;
use x11rb::{
    connection::Connection,
    protocol::{
        xproto::{
            AtomEnum, ClientMessageEvent, ConnectionExt, CreateWindowAux, EventMask, PropMode,
            WindowClass,
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

impl X11 {
    /// Connect once Xwayland is up; `None` if it isn't installed.
    fn connect(pila: &mut TestPila) -> Option<X11> {
        if std::process::Command::new("sh")
            .args(["-c", "command -v Xwayland"])
            .output()
            .map(|o| !o.status.success())
            .unwrap_or(true)
        {
            eprintln!("Xwayland not installed: skipped");
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
        let display = format!(":{}", state["xwayland"].as_u64().unwrap());
        let (conn, screen) = x11rb::connect(Some(&display)).expect("connect to Xwayland");
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
        self.conn.map_window(id).unwrap();
        self.conn.flush().unwrap();
        id
    }

    /// An override-redirect window (a menu) at (x, y).
    fn menu(&self, at: (i16, i16), size: (u16, u16)) -> u32 {
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
                    .override_redirect(1),
            )
            .unwrap();
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

/// The Xwayland that `pila` started.
fn xwayland_pid(pila: &TestPila) -> u32 {
    let wanted = format!("PPid:\t{}\n", pila.pid());
    std::fs::read_dir("/proc")
        .unwrap()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .find(|pid| {
            let status = std::fs::read_to_string(format!("/proc/{pid}/status"));
            status.is_ok_and(|s| s.starts_with("Name:\tXwayland\n") && s.contains(&wanted))
        })
        .expect("pila's Xwayland")
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
    let status = std::fs::read_to_string(format!("/proc/{}/status", xwayland_pid(&pila))).unwrap();
    let blocked = status.lines().find(|l| l.starts_with("SigBlk:")).unwrap();
    assert!(blocked.ends_with("0000000000000000"), "{blocked}");
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
    pila.wait("its windows gone and a new Xwayland", |s| {
        x11_window(s, "old").is_none() && s["xwayland"].is_number()
    });
    let x11 = X11::connect(&mut pila).unwrap();
    x11.window("new", (200, 200), 0xff0000);
    pila.wait("a window on the new one", |s| {
        x11_window(s, "new").is_some()
    });
    assert_ne!(xwayland_pid(&pila), pid);
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
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
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
    let id = x.window("XApp", (300, 200), 0xff0000);
    pila.wait_focus("XApp");
    x.wait_event("focus in", |e| matches!(e, Event::FocusIn(_)));
    let clipboard = x.atom("CLIPBOARD");
    let utf8 = x.atom("UTF8_STRING");
    let property = x.atom("PILA_TEST");
    let started = Instant::now();
    let text = loop {
        x.conn
            .convert_selection(id, clipboard, utf8, property, x11rb::CURRENT_TIME)
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
            break String::from_utf8_lossy(&reply.value).to_string();
        }
        x.events.borrow_mut().clear();
        assert!(started.elapsed() < TIMEOUT, "no clipboard for X");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(text, "to X");
}

#[test]
fn the_clipboard_goes_from_x11_to_a_wayland_client() {
    use x11rb::protocol::xproto::{SelectionNotifyEvent, SELECTION_NOTIFY_EVENT};
    let mut pila = TestPila::start();
    let Some(x) = X11::connect(&mut pila) else {
        return;
    };
    let id = x.window("XApp", (300, 200), 0xff0000);
    pila.wait_focus("XApp");
    let clipboard = x.atom("CLIPBOARD");
    let utf8 = x.atom("UTF8_STRING");
    let targets = x.atom("TARGETS");
    x.conn
        .set_selection_owner(id, clipboard, x11rb::CURRENT_TIME)
        .unwrap();
    x.conn.flush().unwrap();

    use wayland_client::protocol::wl_data_device_manager::WlDataDeviceManager;
    let reader = pila.client();
    let manager: WlDataDeviceManager = reader.bind("wl_data_device_manager", 3);
    let _device = manager.get_data_device(&reader.seat(), &reader.qh, "device".into());
    // The X app answers selection requests from a thread while the Wayland
    // client reads.
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let answering = {
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
                                b"from X",
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
    reader.window("reader").create();
    pila.wait_focus("reader");
    reader.wait("an offer", |s| s.data_offer.is_some());
    let offer = reader.lock().data_offer.clone().unwrap();
    let text = reader.receive(|fd| offer.receive("text/plain;charset=utf-8".into(), fd));
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = answering.join();
    assert_eq!(text, "from X");
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
