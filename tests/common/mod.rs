//! The harness of the integration tests: a headless pila per test (its own
//! runtime directory, so tests run in parallel), driven through its control
//! socket (`src/test_control.rs`), with synthetic clients (`client.rs`).
//!
//! Conditions are waited for (`TestPila::wait`), never slept on: every
//! check polls pila's state until it holds or a timeout fails the test with
//! the state and the end of pila's log.

#![allow(dead_code)]

pub mod client;
pub mod protocols;
pub mod session_protocol;

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

pub use serde_json::{json, Value};

pub use client::Client;

/// How long a condition may take before the test fails.
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// What a test starts pila with.
#[derive(Default)]
pub struct Options {
    /// The config file (TOML). `[animations]` defaults to off, so results
    /// don't depend on timing; `[keyboard]` to the US layout.
    pub config: String,
    /// `--headless-outputs`.
    pub outputs: Option<String>,
    /// `-c`.
    pub command: Option<String>,
    /// Extra environment.
    pub env: Vec<(String, String)>,
}

pub struct TestPila {
    child: Child,
    pub dir: PathBuf,
    pub config_path: PathBuf,
    pub log_path: PathBuf,
    /// `WAYLAND_DISPLAY` of this pila.
    pub display: String,
    control: BufReader<UnixStream>,
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// The config as written: the test's, plus the defaults it doesn't set.
fn full_config(config: &str) -> String {
    let mut text = config.to_string();
    if !config.contains("[animations]") {
        text.push_str("\n[animations]\nenabled = false\n");
    }
    if !config.contains("[keyboard]") {
        text.push_str("\n[keyboard]\nlayout = \"us\"\n");
    }
    text
}

impl TestPila {
    pub fn start() -> Self {
        Self::start_with(Options::default())
    }

    pub fn with_config(config: &str) -> Self {
        Self::start_with(Options {
            config: config.to_string(),
            ..Options::default()
        })
    }

    pub fn start_with(options: Options) -> Self {
        // Short: a socket path must fit in 108 bytes.
        let dir = std::env::temp_dir().join(format!(
            "pila-t{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the runtime dir");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let config_dir = dir.join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        let config_path = config_dir.join("config.toml");
        std::fs::write(&config_path, full_config(&options.config)).unwrap();
        let log_path = dir.join("pila.log");
        let log = std::fs::File::create(&log_path).unwrap();

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_pila"));
        cmd.arg("--backend")
            .arg("headless")
            .arg("--config")
            .arg(&config_path);
        if let Some(outputs) = &options.outputs {
            cmd.arg("--headless-outputs").arg(outputs);
        }
        if let Some(command) = &options.command {
            cmd.arg("-c").arg(command);
        }
        for var in [
            "WAYLAND_DISPLAY",
            "WAYLAND_SOCKET",
            "DISPLAY",
            "XKB_DEFAULT_LAYOUT",
            "XKB_DEFAULT_VARIANT",
            "XKB_DEFAULT_OPTIONS",
            "XKB_DEFAULT_MODEL",
            "XKB_DEFAULT_RULES",
            "PILA_MOD",
            "PILA_TERMINAL",
            "PILA_LAUNCHER",
            "DBUS_SESSION_BUS_ADDRESS",
        ] {
            cmd.env_remove(var);
        }
        cmd.env("XDG_RUNTIME_DIR", &dir)
            .env("XDG_CONFIG_HOME", dir.join("xdg-config"))
            .env("XDG_STATE_HOME", dir.join("state"))
            .env(
                "RUST_LOG",
                std::env::var("PILA_TEST_LOG").unwrap_or_else(|_| "debug,smithay=info".into()),
            )
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log);
        for (k, v) in &options.env {
            cmd.env(k, v);
        }
        let child = cmd.spawn().expect("start pila");

        // The control socket appears once pila is set up.
        let started = Instant::now();
        let (control, display) = loop {
            let found = std::fs::read_dir(&dir).ok().and_then(|entries| {
                entries.flatten().find_map(|e| {
                    let name = e.file_name().into_string().ok()?;
                    let display = name.strip_suffix(".control")?.to_string();
                    Some((e.path(), display))
                })
            });
            if let Some((path, display)) = found {
                if let Ok(stream) = UnixStream::connect(&path) {
                    break (stream, display);
                }
            }
            if started.elapsed() > TIMEOUT {
                let log = std::fs::read_to_string(&log_path).unwrap_or_default();
                panic!("pila didn't start; its log:\n{}", tail(&log, 60));
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        control
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        TestPila {
            child,
            dir,
            config_path,
            log_path,
            display,
            control: BufReader::new(control),
        }
    }

    /// Send a request; panics if pila refuses it.
    pub fn request(&mut self, request: Value) -> Value {
        let response = self.try_request(request.clone());
        assert_eq!(
            response["ok"], true,
            "request {request} failed: {}",
            response["error"]
        );
        response
    }

    pub fn try_request(&mut self, request: Value) -> Value {
        let mut line = request.to_string();
        line.push('\n');
        if let Err(err) = self.control.get_mut().write_all(line.as_bytes()) {
            panic!("control socket: {err}\n{}", self.log_tail());
        }
        let mut response = String::new();
        match self.control.read_line(&mut response) {
            Ok(n) if n > 0 => serde_json::from_str(&response).expect("a JSON response"),
            other => panic!(
                "no response to {request} ({other:?}); pila died?\n{}",
                self.log_tail()
            ),
        }
    }

    pub fn state(&mut self) -> Value {
        self.request(json!({ "cmd": "state" }))
    }

    /// Wait until `cond` holds for the state; returns that state. Fails the
    /// test after `TIMEOUT`, showing the state and the log.
    #[track_caller]
    pub fn wait(&mut self, what: &str, cond: impl Fn(&Value) -> bool) -> Value {
        let started = Instant::now();
        loop {
            let state = self.state();
            if cond(&state) {
                return state;
            }
            if started.elapsed() > TIMEOUT {
                panic!(
                    "timed out waiting for: {what}\nstate: {}\n--- log ---\n{}",
                    serde_json::to_string_pretty(&state).unwrap(),
                    self.log_tail()
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Check that `cond` keeps holding for `duration` (for "nothing
    /// happens" checks).
    #[track_caller]
    pub fn stays(&mut self, what: &str, duration: Duration, cond: impl Fn(&Value) -> bool) {
        let started = Instant::now();
        while started.elapsed() < duration {
            let state = self.state();
            assert!(
                cond(&state),
                "expected to stay true: {what}\nstate: {}\n--- log ---\n{}",
                serde_json::to_string_pretty(&state).unwrap(),
                self.log_tail()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Wait until nothing is drawing or animating.
    pub fn settle(&mut self) -> Value {
        self.wait("pila to settle", |s| {
            s["redraw_pending"] == false && s["animations"]["running"] == false
        })
    }

    /// The window with `app_id`; panics if there's none or several.
    #[track_caller]
    pub fn window(&mut self, app_id: &str) -> Value {
        let state = self.state();
        window_in(&state, app_id)
            .unwrap_or_else(|| panic!("no window {app_id:?}; state: {state:#}"))
            .clone()
    }

    /// Wait for a mapped window with `app_id`; returns it.
    #[track_caller]
    pub fn wait_window(&mut self, app_id: &str) -> Value {
        let state = self.wait(&format!("window {app_id:?} to map"), |s| {
            window_in(s, app_id).is_some_and(|w| w["mapped"] == true)
        });
        window_in(&state, app_id).unwrap().clone()
    }

    /// Wait for `app_id` to have keyboard focus.
    #[track_caller]
    pub fn wait_focus(&mut self, app_id: &str) -> Value {
        self.wait(&format!("{app_id:?} to have focus"), |s| {
            s["focused"] == app_id
        })
    }

    // --- Input ---------------------------------------------------------

    pub fn key(&mut self, key: &str) {
        self.request(json!({ "cmd": "key", "key": key, "state": "tap" }));
    }

    pub fn key_press(&mut self, key: &str) {
        self.request(json!({ "cmd": "key", "key": key, "state": "press" }));
    }

    pub fn key_release(&mut self, key: &str) {
        self.request(json!({ "cmd": "key", "key": key, "state": "release" }));
    }

    /// `Super+Shift+q`: pressed in order, released in reverse.
    pub fn combo(&mut self, keys: &str) {
        self.request(json!({ "cmd": "combo", "keys": keys }));
    }

    pub fn move_to(&mut self, x: f64, y: f64) {
        self.request(json!({ "cmd": "move", "x": x, "y": y }));
    }

    pub fn move_by(&mut self, dx: f64, dy: f64) {
        self.request(json!({ "cmd": "rel", "dx": dx, "dy": dy }));
    }

    pub fn click(&mut self, button: &str) {
        self.request(json!({ "cmd": "button", "button": button, "state": "click" }));
    }

    pub fn press(&mut self, button: &str) {
        self.request(json!({ "cmd": "button", "button": button, "state": "press" }));
    }

    pub fn release(&mut self, button: &str) {
        self.request(json!({ "cmd": "button", "button": button, "state": "release" }));
    }

    /// Move to (x, y) and click there with the left button.
    pub fn click_at(&mut self, x: f64, y: f64) {
        self.move_to(x, y);
        self.click("left");
    }

    pub fn scroll(&mut self, source: &str, h: f64, v: f64) {
        self.request(json!({ "cmd": "axis", "source": source, "h": h, "v": v }));
    }

    pub fn gesture(&mut self, request: Value) {
        let mut request = request;
        request["cmd"] = "gesture".into();
        self.request(request);
    }

    /// The color at a global logical point: [r, g, b, a].
    pub fn pixel(&mut self, x: f64, y: f64) -> [u8; 4] {
        let r = self.request(json!({ "cmd": "pixels", "x": x, "y": y, "w": 1, "h": 1 }));
        let px: Vec<u8> = r["rgba"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u8)
            .collect();
        [px[0], px[1], px[2], px[3]]
    }

    /// Wait until the pixel at (x, y) is `color` (for content a client
    /// draws on its own schedule, e.g. through Xwayland).
    #[track_caller]
    pub fn wait_pixel(&mut self, x: f64, y: f64, color: [u8; 4]) {
        let started = Instant::now();
        loop {
            let px = self.pixel(x, y);
            if px == color {
                return;
            }
            if started.elapsed() > TIMEOUT {
                panic!("the pixel at {x},{y} is {px:?}, not {color:?}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Replace the config file (pila reloads it).
    pub fn write_config(&mut self, config: &str) {
        // Atomically, as editors do: the reload never sees half a file.
        let tmp = self.config_path.with_extension("tmp");
        std::fs::write(&tmp, full_config(config)).unwrap();
        std::fs::rename(&tmp, &self.config_path).unwrap();
    }

    /// The process id of this pila.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Connect a synthetic client.
    pub fn client(&self) -> Client {
        Client::connect(&self.dir.join(&self.display))
    }

    pub fn log(&self) -> String {
        std::fs::read_to_string(&self.log_path).unwrap_or_default()
    }

    pub fn log_tail(&self) -> String {
        tail(&self.log(), 80)
    }

    /// Wait until the log has a line containing `text`.
    #[track_caller]
    pub fn wait_log(&mut self, text: &str) {
        let started = Instant::now();
        while !self.log().contains(text) {
            if started.elapsed() > TIMEOUT {
                panic!("no {text:?} in the log:\n{}", self.log_tail());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Wait for a file in pila's runtime directory (`$XDG_RUNTIME_DIR`,
    /// which spawned commands inherit) to exist and be non-empty; its
    /// contents.
    #[track_caller]
    pub fn wait_file(&self, name: &str) -> String {
        let path = self.dir.join(name);
        let started = Instant::now();
        loop {
            if let Ok(text) = std::fs::read_to_string(&path) {
                if !text.is_empty() {
                    return text;
                }
            }
            if started.elapsed() > TIMEOUT {
                panic!(
                    "no file {name} from a spawned command:\n{}",
                    self.log_tail()
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Whether pila is still running.
    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Wait for pila to exit by itself; its exit status.
    pub fn wait_exit(&mut self) -> std::process::ExitStatus {
        let started = Instant::now();
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                return status;
            }
            if started.elapsed() > TIMEOUT {
                panic!("pila didn't exit:\n{}", self.log_tail());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for TestPila {
    fn drop(&mut self) {
        // SIGTERM quits pila cleanly (Xwayland stopped, a coverage profile
        // written); SIGKILL if it doesn't.
        // SAFETY: a signal to our own child.
        unsafe { libc::kill(self.child.id() as i32, libc::SIGTERM) };
        let started = Instant::now();
        while matches!(self.child.try_wait(), Ok(None))
            && started.elapsed() < Duration::from_secs(2)
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let log = self.log();
        if std::thread::panicking() {
            eprintln!(
                "--- pila's log ({}) ---\n{}",
                self.log_path.display(),
                tail(&log, 120)
            );
        }
        // A crash fails the test even if its checks passed.
        let panicked = log.contains("panicked");
        if !std::thread::panicking() || std::env::var_os("PILA_TEST_KEEP").is_none() {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
        if panicked && !std::thread::panicking() {
            panic!("pila panicked:\n{}", tail(&log, 120));
        }
    }
}

fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

/// The window with `app_id` in a state; panics if several match.
pub fn window_in<'a>(state: &'a Value, app_id: &str) -> Option<&'a Value> {
    let mut found = state["windows"]
        .as_array()?
        .iter()
        .filter(|w| w["app_id"] == app_id);
    let first = found.next();
    assert!(found.next().is_none(), "several windows {app_id:?}");
    first
}

/// A rectangle of the state as (x, y, w, h).
pub fn rect(value: &Value) -> (i64, i64, i64, i64) {
    (
        value["x"].as_i64().unwrap(),
        value["y"].as_i64().unwrap(),
        value["w"].as_i64().unwrap(),
        value["h"].as_i64().unwrap(),
    )
}

/// The app ids of the mapped windows, back to front.
pub fn stacking(state: &Value) -> Vec<String> {
    let mut windows: Vec<&Value> = state["windows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["stack"].is_number())
        .collect();
    windows.sort_by_key(|w| w["stack"].as_u64());
    windows
        .iter()
        .map(|w| w["app_id"].as_str().unwrap().to_string())
        .collect()
}

pub fn path_exists(path: &Path) -> bool {
    path.exists()
}
