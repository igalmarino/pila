//! What users and other programs rely on keeps working: a config with every
//! key Pila takes (`tests/fixtures/every-key.toml`), every action's name and
//! every field of `pila msg --json`'s answers and events (`docs/IPC.md`,
//! "What's guaranteed"). Adding is free; these fail when something is taken
//! away or renamed.

mod common;

use common::*;
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const EVERY_KEY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/every-key.toml");

/// `pila --check-config --config <path>`: whether it passed, and what it said.
fn check_config(path: &Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_pila"))
        .args(["--check-config", "--config"])
        .arg(path)
        .output()
        .expect("run pila --check-config");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

/// What `pila --check-config` says about a config of `text`.
fn check_text(name: &str, text: &str) -> String {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("compat-{}-{name}.toml", std::process::id()));
    std::fs::write(&path, text).unwrap();
    let (passed, said) = check_config(&path);
    std::fs::remove_file(&path).ok();
    assert!(!passed, "{name}: {text:?} passed");
    said
}

/// The keys a table takes, from what Pila says about one it doesn't know.
fn known_keys(table: &str, header: &str) -> BTreeSet<String> {
    let said = check_text(table, &format!("{header}\nzzz-not-a-key = 1\n"));
    let expected = said
        .split_once("expected")
        .unwrap_or_else(|| panic!("{table}: no list of keys in {said:?}"))
        .1;
    expected
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

fn every_key() -> toml::Table {
    let text = std::fs::read_to_string(EVERY_KEY).unwrap();
    text.parse().expect("every-key.toml is TOML")
}

/// The keys `path` (`input.mouse`, or `window-rule` for all of them) has in
/// every-key.toml.
fn keys_written(config: &toml::Table, path: &str) -> BTreeSet<String> {
    let mut tables = vec![config];
    for part in path.split('.').filter(|p| !p.is_empty()) {
        tables = tables
            .into_iter()
            .flat_map(|t| match t.get(part) {
                Some(toml::Value::Table(table)) => vec![table],
                Some(toml::Value::Array(items)) => {
                    items.iter().filter_map(|i| i.as_table()).collect()
                }
                _ => vec![],
            })
            .collect();
    }
    tables.iter().flat_map(|t| t.keys().cloned()).collect()
}

#[test]
fn a_config_with_every_key_still_checks() {
    let (passed, said) = check_config(Path::new(EVERY_KEY));
    assert!(passed, "{said}");
}

#[test]
fn the_config_with_every_key_has_every_key() {
    let config = every_key();
    let tables = [
        ("", ""),
        ("shadow", "[shadow]"),
        ("colors", "[colors]"),
        ("animations", "[animations]"),
        ("cursor", "[cursor]"),
        ("keyboard", "[keyboard]"),
        ("input", "[input]"),
        ("input.touchpad", "[input.touchpad]"),
        ("input.mouse", "[input.mouse]"),
        ("output", "[[output]]\nname = \"X\""),
        ("window-rule", "[[window-rule]]"),
        ("layer-rule", "[[layer-rule]]"),
    ];
    for (path, header) in tables {
        let known = known_keys(path, header);
        assert!(known.len() > 1, "[{path}]: {known:?}");
        let written = keys_written(&config, path);
        let missing: Vec<_> = known.difference(&written).collect();
        assert!(
            missing.is_empty(),
            "tests/fixtures/every-key.toml lacks {missing:?} in [{path}]: add them"
        );
    }
}

#[test]
fn the_config_with_every_key_binds_every_action() {
    let said = check_text("action", "[bindings]\n\"Mod+X\" = \"not-an-action\"\n");
    let list = said
        .rsplit_once(" (")
        .and_then(|(_, list)| list.strip_suffix(" or none)\n"))
        .unwrap_or_else(|| panic!("no list of actions in {said:?}"));
    let config = every_key();
    let bound: Vec<&str> = config["bindings"]
        .as_table()
        .unwrap()
        .values()
        .filter_map(|v| v.as_str())
        .collect();
    let names: Vec<&str> = list
        .split(", ")
        .map(|a| a.split(' ').next().unwrap())
        .collect();
    assert!(names.len() > 30, "{names:?}");
    for name in names {
        let is_bound = bound.iter().any(|action| {
            *action == name
                || action.starts_with(&format!("{name} "))
                || action.starts_with(&format!("{name}-"))
        });
        assert!(
            is_bound,
            "tests/fixtures/every-key.toml binds no {name:?}: add it under [bindings]"
        );
    }
}

/// The fields each kind of object has had since `protocol` 1, as
/// `docs/IPC.md` lists them. Only ever add to these.
const WINDOW: &[&str] = &[
    "id",
    "app_id",
    "title",
    "workspace",
    "output",
    "focused",
    "x",
    "y",
    "width",
    "height",
    "minimized",
    "maximized",
    "fullscreen",
    "snapped",
    "always_on_top",
    "all_workspaces",
    "urgent",
    "x11",
];
/// Windows in events leave these out: they change in every frame of a move.
const WINDOW_NOT_IN_EVENTS: &[&str] = &["x", "y", "width", "height"];
const OUTPUT: &[&str] = &[
    "name",
    "make",
    "model",
    "x",
    "y",
    "width",
    "height",
    "refresh",
    "scale",
    "transform",
    "focused",
    "power",
];
const WORKSPACE: &[&str] = &["index", "active", "windows"];
const LAYOUT: &[&str] = &["index", "name", "active"];

#[track_caller]
fn has_fields(what: &str, object: &Value, fields: &[&str]) {
    for field in fields {
        assert!(
            object.get(field).is_some(),
            "{what} lost {field:?}: {object:#}"
        );
    }
}

/// Every object of `list`, which mustn't be empty, has `fields`.
#[track_caller]
fn all_have(what: &str, list: &Value, fields: &[&str]) {
    let items = list.as_array().unwrap_or_else(|| panic!("{what}: {list}"));
    assert!(!items.is_empty(), "{what}: none to check");
    for item in items {
        has_fields(what, item, fields);
    }
}

/// A connection to `pila`'s IPC socket.
struct Ipc {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Ipc {
    fn connect(pila: &TestPila) -> Self {
        let path = pila.dir.join(format!("pila-{}.sock", pila.display));
        let writer = UnixStream::connect(path).unwrap();
        writer
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let reader = BufReader::new(writer.try_clone().unwrap());
        Ipc { writer, reader }
    }

    fn line(&mut self) -> Value {
        let mut line = String::new();
        self.reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|_| panic!("bad line: {line:?}"))
    }

    /// The answer to `request`, which must succeed with `protocol` 1.
    #[track_caller]
    fn ask(&mut self, request: Value) -> Value {
        let mut text = request.to_string();
        text.push('\n');
        self.writer.write_all(text.as_bytes()).unwrap();
        let answer = self.line();
        assert_eq!(answer["ok"], true, "{request}: {answer}");
        assert_eq!(answer["protocol"], 1, "{request}: {answer}");
        answer
    }
}

#[test]
fn ipc_answers_and_events_keep_their_fields() {
    let mut pila = TestPila::start_with(Options {
        config: "[keyboard]\nlayout = \"us,de\"\n".into(),
        ..Options::default()
    });
    let client = pila.client();
    client.window("a").title("A").create();
    pila.wait_focus("a");
    let mut ipc = Ipc::connect(&pila);
    let ask = |ipc: &mut Ipc, request: &str| ipc.ask(serde_json::json!({ "request": request }));

    let version = ask(&mut ipc, "version");
    assert!(version["version"].is_string(), "{version}");
    all_have("a window", &ask(&mut ipc, "windows")["windows"], WINDOW);
    has_fields(
        "the focused window",
        &ask(&mut ipc, "focused")["window"],
        WINDOW,
    );
    all_have("an output", &ask(&mut ipc, "outputs")["outputs"], OUTPUT);
    all_have(
        "a workspace",
        &ask(&mut ipc, "workspaces")["workspaces"],
        WORKSPACE,
    );
    all_have(
        "a keyboard layout",
        &ask(&mut ipc, "keyboard")["layouts"],
        LAYOUT,
    );

    let kinds = ["windows", "workspaces", "outputs", "keyboard"];
    ipc.ask(serde_json::json!({ "request": "subscribe", "events": kinds }));
    let mut seen = BTreeSet::new();
    let window_in_events: Vec<&str> = WINDOW
        .iter()
        .copied()
        .filter(|f| !WINDOW_NOT_IN_EVENTS.contains(f))
        .collect();
    let deadline = Instant::now() + Duration::from_secs(10);
    while seen.len() < kinds.len() {
        assert!(Instant::now() < deadline, "events seen: {seen:?}");
        let event = ipc.line();
        let kind = event["event"].as_str().unwrap_or_default().to_string();
        match kind.as_str() {
            "windows" => all_have("a window event", &event["windows"], &window_in_events),
            "workspaces" => all_have("a workspaces event", &event["workspaces"], WORKSPACE),
            "outputs" => all_have("an outputs event", &event["outputs"], OUTPUT),
            "keyboard" => all_have("a keyboard event", &event["layouts"], LAYOUT),
            _ => panic!("unknown event: {event}"),
        }
        seen.insert(kind);
    }
}
