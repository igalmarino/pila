//! `pila msg` and the IPC socket: through the real command, and by hand for
//! clients that misbehave.

mod common;

use common::client::Ev;
use common::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

fn socket(pila: &TestPila) -> std::path::PathBuf {
    pila.dir.join(format!("pila-{}.sock", pila.display))
}

/// `pila msg <args>` as a program in this session would run it.
fn msg(pila: &TestPila, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pila"))
        .arg("msg")
        .args(args)
        .env_remove("PILA_SOCKET")
        .env("WAYLAND_DISPLAY", &pila.display)
        .env("XDG_RUNTIME_DIR", &pila.dir)
        .output()
        .expect("run pila msg")
}

fn text(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn error(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The JSON answer to `pila msg --json <args>`, which must succeed.
#[track_caller]
fn query(pila: &TestPila, args: &[&str]) -> Value {
    let mut all = vec!["--json"];
    all.extend_from_slice(args);
    let out = msg(pila, &all);
    assert!(out.status.success(), "{args:?}: {}", error(&out));
    let answer: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(answer["ok"], true);
    answer
}

/// `pila msg action <action...>`, which must succeed.
#[track_caller]
fn action(pila: &TestPila, words: &[&str]) {
    let mut all = vec!["action"];
    all.extend_from_slice(words);
    let out = msg(pila, &all);
    assert!(out.status.success(), "{words:?}: {}", error(&out));
}

fn named<'a>(answer: &'a Value, app_id: &str) -> &'a Value {
    answer["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["app_id"] == app_id)
        .unwrap_or_else(|| panic!("no {app_id} in {answer:#}"))
}

/// A connection by hand.
struct Raw {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Raw {
    fn connect(pila: &TestPila) -> Self {
        let writer = UnixStream::connect(socket(pila)).unwrap();
        writer
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let reader = BufReader::new(writer.try_clone().unwrap());
        Self { writer, reader }
    }

    fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).unwrap();
    }

    fn answer(&mut self) -> Value {
        let mut line = String::new();
        self.reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|_| panic!("bad answer: {line:?}"))
    }

    /// Whether pila hung up.
    fn closed(&mut self) -> bool {
        let mut rest = Vec::new();
        match self.reader.read_to_end(&mut rest) {
            Ok(_) => true,
            // What it hadn't read yet makes it a reset rather than an end.
            Err(err) => err.kind() == std::io::ErrorKind::ConnectionReset,
        }
    }
}

#[test]
fn windows_are_listed_wherever_they_are() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1280x800".into()),
        ..Options::default()
    });
    let client = pila.client();
    client.window("gone").create();
    pila.wait_focus("gone");
    pila.combo("Super+Shift+2");
    pila.wait("moved", |s| window_in(s, "gone").unwrap()["workspace"] == 2);
    client.window("small").create();
    pila.wait_focus("small");
    pila.combo("Super+n");
    pila.wait("minimized", |s| {
        window_in(s, "small").unwrap()["minimized"] == true
    });
    client
        .window("shown")
        .title("A title")
        .size(400, 300)
        .create();
    pila.wait_focus("shown");
    pila.combo("Super+t");
    pila.wait("marked", |s| {
        window_in(s, "shown").unwrap()["always_on_top"] == true
    });
    let state = pila.settle();
    let real = window_in(&state, "shown").unwrap();
    let (x, y, w, h) = rect(&real["geometry"]);

    let answer = query(&pila, &["windows"]);
    assert_eq!(answer["protocol"], 1);
    assert_eq!(answer["windows"].as_array().unwrap().len(), 3);
    let shown = named(&answer, "shown");
    assert_eq!(shown["title"], "A title");
    assert_eq!(shown["workspace"], 1);
    assert_eq!(shown["output"], "A");
    assert_eq!(shown["focused"], true);
    assert_eq!(shown["always_on_top"], true);
    assert_eq!(shown["x11"], false);
    assert_eq!(
        (&shown["x"], &shown["y"], &shown["width"], &shown["height"]),
        (&x.into(), &y.into(), &w.into(), &h.into())
    );
    assert!(!shown["id"].as_str().unwrap().is_empty());
    assert_eq!(named(&answer, "gone")["workspace"], 2);
    assert_eq!(named(&answer, "gone")["focused"], false);
    assert_eq!(named(&answer, "small")["minimized"], true);

    let listed = text(&msg(&pila, &["windows"]));
    assert!(listed.contains("shown: A title"), "{listed}");
    assert!(listed.contains("focused"), "{listed}");
    assert!(listed.contains("on-top"), "{listed}");
    assert_eq!(listed.lines().count(), 3, "{listed}");
}

#[test]
fn outputs_and_workspaces_are_listed() {
    let pila = TestPila::start_with(Options {
        config: "workspaces = 3\n[[output]]\nname = \"B\"\nscale = 2\ntransform = \"90\"\n".into(),
        outputs: Some("A:1280x800@60=Acme/One,B:1000x600@60".into()),
        ..Options::default()
    });
    let answer = query(&pila, &["outputs"]);
    let outputs = answer["outputs"].as_array().unwrap();
    assert_eq!(outputs.len(), 2);
    let a = outputs.iter().find(|o| o["name"] == "A").unwrap();
    assert_eq!(a["make"], "Acme");
    assert_eq!(a["model"], "One");
    assert_eq!((&a["x"], &a["y"]), (&0.into(), &0.into()));
    assert_eq!((&a["width"], &a["height"]), (&1280.into(), &800.into()));
    assert_eq!(a["scale"], 1.0);
    assert_eq!(a["transform"], "normal");
    assert_eq!(a["refresh"], 60000);
    let b = outputs.iter().find(|o| o["name"] == "B").unwrap();
    assert_eq!(b["scale"], 2.0);
    assert_eq!(b["transform"], "90");
    let focused: Vec<_> = outputs.iter().filter(|o| o["focused"] == true).collect();
    assert_eq!(focused.len(), 1);
    assert!(text(&msg(&pila, &["outputs"])).contains("1280x800+0+0"));

    let answer = query(&pila, &["workspaces"]);
    let workspaces = answer["workspaces"].as_array().unwrap();
    assert_eq!(workspaces.len(), 3);
    assert_eq!(workspaces[0]["index"], 1);
    assert_eq!(workspaces[0]["active"], true);
    assert_eq!(workspaces[1]["active"], false);
    assert!(text(&msg(&pila, &["workspaces"])).contains("1: 0 windows  [active]"));
}

#[test]
fn focused_says_which_window_or_none() {
    let mut pila = TestPila::start();
    assert!(query(&pila, &["focused"])["window"].is_null());
    assert!(text(&msg(&pila, &["focused"])).contains("no window"));
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    assert_eq!(query(&pila, &["focused"])["window"]["app_id"], "a");
    assert!(text(&msg(&pila, &["focused"])).contains("a: "));
}

#[test]
fn version_says_pilas() {
    let pila = TestPila::start();
    let answer = query(&pila, &["version"]);
    assert_eq!(answer["protocol"], 1);
    assert!(answer["version"]
        .as_str()
        .unwrap()
        .starts_with(env!("CARGO_PKG_VERSION")));
    assert!(text(&msg(&pila, &["version"])).starts_with("pila "));
}

#[test]
fn an_action_runs_as_a_binding_would() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    action(&pila, &["workspace", "2"]);
    pila.wait("workspace 2", |s| s["workspace"] == 2);
    action(&pila, &["workspace-prev"]);
    pila.wait_focus("a");
    action(&pila, &["maximize"]);
    pila.wait("maximized", |s| {
        window_in(s, "a").unwrap()["maximized"] == true
    });
    action(&pila, &["toggle-always-on-top"]);
    pila.wait("marked", |s| {
        window_in(s, "a").unwrap()["always_on_top"] == true
    });
    let path = pila.dir.join("spawned");
    action(&pila, &["spawn", "echo done >", path.to_str().unwrap()]);
    pila.wait_file("spawned");
    // "none" does nothing, as in [bindings].
    action(&pila, &["none"]);
}

#[test]
fn switching_windows_takes_one_step_with_no_key_held() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    client.window("b").create();
    pila.wait_focus("b");
    action(&pila, &["cycle-focus"]);
    let state = pila.wait_focus("a");
    assert!(state["switch"].is_null(), "{}", state["switch"]);
    action(&pila, &["cycle-focus"]);
    pila.wait_focus("b");
}

#[test]
fn a_bad_action_says_why_and_exits_1() {
    let pila = TestPila::start();
    let out = msg(&pila, &["action", "no-such-action"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(error(&out).contains("unknown action"), "{}", error(&out));
}

#[test]
fn focus_and_close_take_a_window_by_its_id() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").create();
    pila.wait_focus("a");
    client.window("b").create();
    pila.wait_focus("b");
    pila.combo("Super+Shift+2");
    pila.wait("b moved", |s| window_in(s, "b").unwrap()["workspace"] == 2);
    let answer = query(&pila, &["windows"]);
    let a_id = named(&answer, "a")["id"].as_str().unwrap().to_string();
    let b_id = named(&answer, "b")["id"].as_str().unwrap().to_string();

    // On another workspace: it goes there.
    assert!(msg(&pila, &["focus", &b_id]).status.success());
    let state = pila.wait_focus("b");
    assert_eq!(state["workspace"], 2);

    assert!(msg(&pila, &["close", &a_id]).status.success());
    client.roundtrip();
    assert!(client.with_window(a, |w| w.close_requested));

    for args in [["focus", "no-such-id"], ["close", "no-such-id"]] {
        let out = msg(&pila, &args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(error(&out).contains("no window"), "{}", error(&out));
    }
}

#[test]
fn a_locked_session_refuses_actions_and_still_answers_questions() {
    let mut pila = TestPila::start();
    let apps = pila.client();
    let a = apps.window("a").create();
    pila.wait_focus("a");
    let locker = pila.client();
    let _lock = locker.lock_session();
    locker.wait_logged("lock", "Locked");
    pila.wait("locked", |s| s["locked"] == true);
    let id = named(&query(&pila, &["windows"]), "a")["id"]
        .as_str()
        .unwrap()
        .to_string();
    for args in [
        vec!["action", "quit"],
        vec!["action", "spawn", "true"],
        vec!["action", "workspace", "2"],
        vec!["focus", &id],
        vec!["close", &id],
    ] {
        let out = msg(&pila, &args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(error(&out).contains("locked"), "{}", error(&out));
    }
    assert!(pila.alive());
    apps.roundtrip();
    assert!(!apps.with_window(a, |w| w.close_requested));
    assert_eq!(pila.state()["workspace"], 1);
    assert!(msg(&pila, &["outputs"]).status.success());
    // Turning the monitors off and on gets past nothing: idling locks, then
    // turns them off.
    action(&pila, &["power-off-monitors"]);
    pila.wait("off", |s| s["outputs"][0]["power"] == false);
    assert_eq!(query(&pila, &["outputs"])["outputs"][0]["power"], false);
    action(&pila, &["power-on-monitors"]);
    pila.wait("on", |s| s["outputs"][0]["power"] == true);
    assert_eq!(query(&pila, &["outputs"])["outputs"][0]["power"], true);
}

#[test]
fn a_bad_request_gets_an_error_and_the_connection_goes_on() {
    let pila = TestPila::start();
    let mut raw = Raw::connect(&pila);
    for bad in [
        &b"not json\n"[..],
        b"[]\n",
        b"{}\n",
        b"{\"request\": 3}\n",
        b"{\"request\": \"nothing\"}\n",
        b"{\"request\": \"action\"}\n",
        b"{\"request\": \"focus\"}\n",
        b"\xff\xfe\n",
        b"\n",
    ] {
        raw.send(bad);
        let answer = raw.answer();
        assert_eq!(answer["ok"], false, "{bad:?}");
        assert!(answer["error"].is_string(), "{bad:?}");
    }
    raw.send(b"{\"request\": \"version\"}\n");
    assert_eq!(raw.answer()["ok"], true);
}

#[test]
fn requests_can_come_together_or_in_pieces() {
    let pila = TestPila::start();
    let mut raw = Raw::connect(&pila);
    raw.send(b"{\"request\": \"version\"}\n{\"request\": \"outputs\"}\n{\"req");
    assert!(raw.answer()["version"].is_string());
    assert!(raw.answer()["outputs"].is_array());
    std::thread::sleep(Duration::from_millis(50));
    raw.send(b"uest\": \"workspaces\"}\n");
    assert!(raw.answer()["workspaces"].is_array());
}

#[test]
fn a_request_too_long_ends_that_connection_only() {
    let mut pila = TestPila::start();
    let mut long = Raw::connect(&pila);
    // More than the limit, with no end of line yet.
    let _ = long.writer.write_all(&vec![b'x'; 200 * 1024]);
    assert!(long.closed());
    // One that does end, past the limit.
    let mut ended = Raw::connect(&pila);
    let mut line = vec![b' '; 65 * 1024];
    line.push(b'\n');
    let _ = ended.writer.write_all(&line);
    assert!(ended.closed());
    assert!(pila.alive());
    assert!(msg(&pila, &["version"]).status.success());
}

#[test]
fn too_many_clients_are_turned_away_until_one_leaves() {
    let pila = TestPila::start();
    let mut clients: Vec<Raw> = (0..32).map(|_| Raw::connect(&pila)).collect();
    // All of them accepted by now.
    let last = clients.last_mut().unwrap();
    last.send(b"{\"request\": \"version\"}\n");
    assert_eq!(last.answer()["ok"], true);
    let out = msg(&pila, &["version"]);
    assert_eq!(out.status.code(), Some(3), "{}", error(&out));
    clients.pop();
    let started = Instant::now();
    while !msg(&pila, &["version"]).status.success() {
        assert!(started.elapsed() < TIMEOUT, "its place wasn't freed");
    }
}

#[test]
fn a_client_that_doesnt_read_doesnt_hold_pila_back() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("a").create();
    pila.wait_focus("a");
    let mut slow = Raw::connect(&pila);
    let request: Vec<u8> = b"{\"request\": \"windows\"}\n".repeat(30_000);
    let writer = slow.writer.try_clone().unwrap();
    let flood = std::thread::spawn(move || {
        let mut writer = writer;
        // Fails once pila hangs up, which is the point.
        let _ = writer.write_all(&request);
    });
    // Meanwhile pila answers others and draws.
    let started = Instant::now();
    let surface = client.surface(window);
    for i in 0..5 {
        assert!(msg(&pila, &["version"]).status.success());
        let tag = format!("frame{i}");
        let _frame = surface.frame(&client.qh, tag.clone());
        client.draw(window, None);
        client.wait_logged(&tag, "Done");
    }
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "it was held back"
    );
    flood.join().unwrap();
    // It got cut off well before all its answers: they'd take over 3 MiB.
    let mut read = Vec::new();
    let _ = slow.reader.read_to_end(&mut read);
    let answers = read.iter().filter(|b| **b == b'\n').count();
    assert!(answers < 30_000, "it got all {answers} answers");
    assert!(pila.alive());
}

#[test]
fn a_client_that_reads_late_gets_all_its_answers() {
    let mut pila = TestPila::start();
    let client = pila.client();
    for name in ["a", "b", "c"] {
        client.window(name).create();
        pila.wait_focus(name);
    }
    let mut raw = Raw::connect(&pila);
    raw.send(b"{\"request\": \"windows\"}\n");
    let size = raw.answer().to_string().len() + 1;
    // More than the socket holds, less than the limit.
    let count = 700 * 1024 / size;
    raw.send(&b"{\"request\": \"windows\"}\n".repeat(count));
    std::thread::sleep(Duration::from_millis(300));
    for i in 0..count {
        let answer = raw.answer();
        assert_eq!(answer["windows"].as_array().unwrap().len(), 3, "answer {i}");
    }
    assert!(pila.alive());
}

#[test]
fn a_client_that_leaves_halfway_does_no_harm() {
    let mut pila = TestPila::start();
    {
        let mut raw = Raw::connect(&pila);
        raw.send(b"{\"request\": \"win");
    }
    {
        // Gone with its answer on the way.
        let mut raw = Raw::connect(&pila);
        raw.send(b"{\"request\": \"windows\"}\n");
    }
    assert!(pila.alive());
    assert!(msg(&pila, &["version"]).status.success());
}

#[test]
fn an_answer_is_written_before_a_quit_ends_pila() {
    let mut pila = TestPila::start();
    let out = msg(&pila, &["action", "quit"]);
    assert!(out.status.success(), "{}", error(&out));
    assert!(pila.wait_exit().success());
}

#[test]
fn the_socket_is_private_given_to_programs_and_gone_at_the_end() {
    let mut pila = TestPila::start_with(Options {
        command: Some("printf %s \"$PILA_SOCKET\" > \"$XDG_RUNTIME_DIR/its-socket\"".into()),
        ..Options::default()
    });
    let path = socket(&pila);
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    assert_eq!(pila.wait_file("its-socket"), path.to_str().unwrap());

    // PILA_SOCKET is enough, wherever it's run from.
    let out = Command::new(env!("CARGO_BIN_EXE_pila"))
        .args(["msg", "version"])
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("XDG_RUNTIME_DIR")
        .env("PILA_SOCKET", &path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", error(&out));

    action(&pila, &["quit"]);
    assert!(pila.wait_exit().success());
    assert!(!path.exists());
}

#[test]
fn a_socket_left_by_a_crash_is_replaced() {
    // The Wayland socket's names go in order, so the first pila in a new
    // directory gets wayland-1 (wayland-0 if that's free).
    let dir = std::env::temp_dir().join(format!("pila-ipc-stale-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    for n in 0..4 {
        std::fs::write(dir.join(format!("pila-wayland-{n}.sock")), "left behind").unwrap();
    }
    let mut pila = Command::new(env!("CARGO_BIN_EXE_pila"))
        .args(["--backend", "headless", "--config", "/nonexistent"])
        .args(["-c", "pila msg action quit"])
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .env("XDG_RUNTIME_DIR", &dir)
        .env("XDG_CONFIG_HOME", &dir)
        .env("XDG_STATE_HOME", &dir)
        .env(
            "PATH",
            format!(
                "{}:{}",
                std::path::Path::new(env!("CARGO_BIN_EXE_pila"))
                    .parent()
                    .unwrap()
                    .display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    // It quits through the new socket, or not at all.
    let started = Instant::now();
    let status = loop {
        if let Some(status) = pila.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > TIMEOUT {
            let _ = pila.kill();
            let _ = pila.wait();
            panic!("the socket wasn't replaced");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success());
}

#[test]
fn without_a_pila_it_exits_3_and_says_so() {
    let dir = std::env::temp_dir().join(format!("pila-msg-none-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let run = |envs: &[(&str, &std::ffi::OsStr)]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_pila"));
        cmd.args(["msg", "windows"])
            .env_remove("PILA_SOCKET")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("XDG_RUNTIME_DIR");
        for (name, value) in envs {
            cmd.env(name, value);
        }
        cmd.output().unwrap()
    };
    let out = run(&[]);
    assert_eq!(out.status.code(), Some(3));
    assert!(error(&out).contains("PILA_SOCKET"), "{}", error(&out));
    let out = run(&[
        ("WAYLAND_DISPLAY", "wayland-99".as_ref()),
        ("XDG_RUNTIME_DIR", dir.as_os_str()),
    ]);
    assert_eq!(out.status.code(), Some(3));
    assert!(
        error(&out).contains("can't talk to pila"),
        "{}",
        error(&out)
    );
}

#[test]
fn a_pila_that_doesnt_answer_times_out() {
    let dir = std::env::temp_dir().join(format!("pila-msg-silent-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("silent.sock");
    let _ = std::fs::remove_file(&path);
    let _listener = UnixListener::bind(&path).unwrap();
    let started = Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_pila"))
        .args(["msg", "windows"])
        .env("PILA_SOCKET", &path)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3), "{}", error(&out));
    assert!(started.elapsed() < Duration::from_secs(20));
}

#[test]
fn bad_arguments_exit_2_and_show_the_usage() {
    let pila = TestPila::start();
    for args in [
        &[][..],
        &["nothing"],
        &["windows", "extra"],
        &["action"],
        &["focus"],
        &["close", "a", "b"],
    ] {
        let out = msg(&pila, args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(error(&out).contains("Usage: pila msg"), "{args:?}");
    }
    let out = msg(&pila, &["--help"]);
    assert!(out.status.success());
    assert!(text(&out).contains("Usage: pila msg"));
}

#[test]
fn a_title_cant_reach_the_terminal_as_it_is() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").title("evil\u{1b}[2Jtitle").create();
    pila.wait_focus("a");
    let listed = text(&msg(&pila, &["windows"]));
    assert!(!listed.contains('\u{1b}'), "{listed:?}");
    assert!(listed.contains("evil?[2Jtitle"), "{listed:?}");
    // The JSON has it as it is, escaped.
    assert_eq!(
        named(&query(&pila, &["windows"]), "a")["title"],
        "evil\u{1b}[2Jtitle"
    );
}

// --- Events ------------------------------------------------------------------

/// A raw subscription: the answer, then events, one per line.
struct Subscription {
    reader: BufReader<UnixStream>,
}

impl Subscription {
    fn new(pila: &TestPila, events: Option<&[&str]>) -> (Subscription, Value) {
        let mut stream = UnixStream::connect(socket(pila)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let request = match events {
            Some(events) => serde_json::json!({ "request": "subscribe", "events": events }),
            None => serde_json::json!({ "request": "subscribe" }),
        };
        writeln!(stream, "{request}").unwrap();
        let mut subscription = Subscription {
            reader: BufReader::new(stream),
        };
        let answer = subscription.line().expect("an answer");
        (subscription, answer)
    }

    /// The next line, or `None` if none comes within the read timeout.
    fn line(&mut self) -> Option<Value> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(n) if n > 0 => Some(serde_json::from_str(&line).unwrap()),
            _ => None,
        }
    }

    /// The next `kind` event for which `f` holds, skipping others.
    #[track_caller]
    fn wait(&mut self, kind: &str, what: &str, f: impl Fn(&Value) -> bool) -> Value {
        loop {
            let event = self
                .line()
                .unwrap_or_else(|| panic!("no {kind} event: {what}"));
            if event["event"] == kind && f(&event) {
                return event;
            }
        }
    }

    /// The events that come within `time`.
    fn during(&mut self, time: Duration) -> Vec<Value> {
        self.reader.get_ref().set_read_timeout(Some(time)).unwrap();
        let mut events = Vec::new();
        while let Some(event) = self.line() {
            events.push(event);
        }
        events
    }
}

fn app_ids(event: &Value) -> Vec<String> {
    event["windows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["app_id"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_subscriber_gets_the_state_then_each_change() {
    let pila = TestPila::start();
    let (mut events, answer) = Subscription::new(&pila, None);
    assert_eq!(answer["ok"], true);
    assert_eq!(answer["protocol"], 1);
    // First, each kind as it is.
    let first: Vec<Value> = (0..3).map(|_| events.line().unwrap()).collect();
    let kinds: Vec<&str> = first.iter().map(|e| e["event"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["windows", "workspaces", "outputs"]);
    assert_eq!(first[0]["windows"], serde_json::json!([]));
    assert_eq!(first[2]["outputs"][0]["name"], "HEADLESS-1");

    let client = pila.client();
    let a = client.window("a").create();
    let opened = events.wait("windows", "a opens", |e| app_ids(e) == ["a"]);
    let window = &opened["windows"][0];
    assert!(
        window["x"].is_null() && window["width"].is_null(),
        "{window}"
    );
    assert_eq!(window["workspace"], 1);
    // Each change comes once, in the same order as the first events.
    events.wait("workspaces", "a counted", |e| {
        e["workspaces"][0]["windows"] == 1
    });
    client.request(a, |t| t.set_title("renamed".into()));
    events.wait("windows", "the new title", |e| {
        e["windows"][0]["title"] == "renamed"
    });
    action(&pila, &["workspace", "2"]);
    events.wait("workspaces", "workspace 2 active", |e| {
        e["workspaces"][1]["active"] == true
    });
    client.destroy(a);
    events.wait("windows", "a gone", |e| app_ids(e).is_empty());
}

#[test]
fn a_subscriber_gets_only_what_it_asked_for_and_no_moves() {
    let mut pila = TestPila::start();
    let (mut workspaces, _) = Subscription::new(&pila, Some(&["workspaces"]));
    assert_eq!(workspaces.line().unwrap()["event"], "workspaces");
    let client = pila.client();
    client.window("a").size(300, 200).create();
    pila.wait_focus("a");
    workspaces.wait("workspaces", "a counted", |e| {
        e["workspaces"][0]["windows"] == 1
    });

    // A move changes where the window is, which events leave out.
    let (mut windows, _) = Subscription::new(&pila, Some(&["windows"]));
    assert_eq!(windows.line().unwrap()["event"], "windows");
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    pila.move_to((x + 50) as f64, (y + 50) as f64);
    pila.key_press("Super");
    pila.press("left");
    for _ in 0..5 {
        pila.move_by(20.0, 10.0);
    }
    pila.release("left");
    pila.key_release("Super");
    pila.wait("moved", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]).0 == x + 100
    });
    let moved = windows.during(Duration::from_millis(300));
    assert!(moved.is_empty(), "{moved:?}");
    let other = workspaces.during(Duration::from_millis(100));
    assert!(
        other.iter().all(|e| e["event"] == "workspaces"),
        "{other:?}"
    );
}

#[test]
fn subscribing_to_something_unknown_says_so() {
    let pila = TestPila::start();
    let (_, answer) = Subscription::new(&pila, Some(&["weather"]));
    assert_eq!(answer["ok"], false);
    assert!(
        answer["error"].as_str().unwrap().contains("weather"),
        "{answer}"
    );
}

#[test]
fn pila_msg_subscribe_prints_events_as_they_come() {
    let pila = TestPila::start();
    let mut child = Command::new(env!("CARGO_BIN_EXE_pila"))
        .args(["msg", "subscribe", "workspaces"])
        .env_remove("PILA_SOCKET")
        .env("WAYLAND_DISPLAY", &pila.display)
        .env("XDG_RUNTIME_DIR", &pila.dir)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    assert_eq!(line, "== workspaces\n");
    line.clear();
    out.read_line(&mut line).unwrap();
    assert!(line.starts_with("1: 0 windows  [active]"), "{line:?}");
    action(&pila, &["workspace", "2"]);
    let mut seen = String::new();
    while !seen.contains("2: 0 windows  [active]") {
        line.clear();
        assert!(out.read_line(&mut line).unwrap() > 0, "{seen}");
        seen.push_str(&line);
    }
    child.kill().unwrap();
    let _ = child.wait();
}

fn layout_names(layouts: &Value) -> Vec<String> {
    layouts
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["name"].as_str().unwrap().to_string())
        .collect()
}

/// `keyboard` lists the layouts; `switch-layout` changes the one in use
/// (next and prev wrap around), the focused app gets it, and subscribers
/// hear about it, also when an xkb option switches it.
#[test]
fn keyboard_layouts_are_listed_switched_and_followed() {
    let mut pila = TestPila::start_with(Options {
        config: "[keyboard]\nlayout = \"us,de\"\noptions = \"grp:caps_toggle\"\n".into(),
        ..Options::default()
    });
    let app = pila.client();
    app.window("editor").create();
    pila.wait_focus("editor");
    let (mut events, _) = Subscription::new(&pila, Some(&["keyboard"]));
    let first = events.line().unwrap();
    assert_eq!(first["event"], "keyboard");
    assert_eq!(layout_names(&first["layouts"]), ["English (US)", "German"]);
    assert_eq!(first["layouts"][0]["active"], true);

    action(&pila, &["switch-layout", "next"]);
    events.wait("keyboard", "German", |e| e["layouts"][1]["active"] == true);
    app.wait("the app types in German", |s| {
        s.events
            .iter()
            .rfind(|e| matches!(e, Ev::Modifiers { .. }))
            .is_some_and(|e| matches!(e, Ev::Modifiers { group: 1, .. }))
    });
    action(&pila, &["switch-layout", "1"]);
    events.wait("keyboard", "English again", |e| {
        e["layouts"][0]["active"] == true
    });
    action(&pila, &["switch-layout", "prev"]);
    events.wait("keyboard", "around to German", |e| {
        e["layouts"][1]["active"] == true
    });
    pila.key("code:66"); // Caps Lock switches layouts with this option
    events.wait("keyboard", "back to English", |e| {
        e["layouts"][0]["active"] == true
    });

    let answer = query(&pila, &["keyboard"]);
    assert_eq!(layout_names(&answer["layouts"]), ["English (US)", "German"]);
    assert!(
        text(&msg(&pila, &["keyboard"])).contains("1: English (US)  [active]"),
        "{}",
        text(&msg(&pila, &["keyboard"]))
    );
    let out = msg(&pila, &["action", "switch-layout", "0"]);
    assert_eq!(out.status.code(), Some(1), "{}", error(&out));
}
