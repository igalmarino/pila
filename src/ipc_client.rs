//! `pila msg`: asks a running pila (`ipc.rs`) and prints the answer.

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    time::Duration,
};

use serde_json::{json, Value};

const USAGE: &str = "\
Usage: pila msg [--json] <request>

Requests:
  windows             The windows, on all workspaces
  outputs             The monitors
  workspaces          The workspaces
  focused             The focused window
  version             pila's version
  action <action>     Run an action, as in [bindings]: action workspace 2
  focus <id>          Focus a window, by its id in `windows`
  close <id>          Ask a window to close

Options:
  --json              Print pila's answer as it is (docs/IPC.md)";

/// How long to wait for a pila that doesn't answer.
const TIMEOUT: Duration = Duration::from_secs(5);

const EXIT_REFUSED: i32 = 1;
const EXIT_USAGE: i32 = 2;
const EXIT_NO_PILA: i32 = 3;

/// `PILA_SOCKET`, or the socket of the pila on `WAYLAND_DISPLAY`.
fn socket_path() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("PILA_SOCKET").filter(|path| !path.is_empty()) {
        return Ok(path.into());
    }
    let display = std::env::var_os("WAYLAND_DISPLAY")
        .filter(|display| !display.is_empty())
        .ok_or("neither PILA_SOCKET nor WAYLAND_DISPLAY is set: is this in a pila session?")?;
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR is not set")?;
    // WAYLAND_DISPLAY may be a path; the socket's named after its last part.
    let display = PathBuf::from(display);
    let display = display
        .file_name()
        .ok_or("WAYLAND_DISPLAY has no name in it")?;
    let mut name = std::ffi::OsString::from("pila-");
    name.push(display);
    name.push(".sock");
    Ok(PathBuf::from(runtime).join(name))
}

/// The request for the arguments after `msg`.
fn request(args: &[String]) -> Result<Value, String> {
    let (name, rest) = args.split_first().ok_or("no request")?;
    let one_id = || match rest {
        [id] => Ok(id.as_str()),
        _ => Err(format!("{name} takes one window id")),
    };
    match name.as_str() {
        "windows" | "outputs" | "workspaces" | "focused" | "version" if rest.is_empty() => {
            Ok(json!({ "request": name }))
        }
        "windows" | "outputs" | "workspaces" | "focused" | "version" => {
            Err(format!("{name} takes no arguments"))
        }
        "action" if rest.is_empty() => Err("action needs an action".to_string()),
        "action" => Ok(json!({ "request": "action", "action": rest.join(" ") })),
        "focus" | "close" => Ok(json!({ "request": name, "id": one_id()? })),
        other => Err(format!("unknown request {other:?}")),
    }
}

/// Send `request`; pila's answer as it wrote it.
fn ask(path: &std::path::Path, request: &Value) -> Result<String, String> {
    let shown = path.display();
    let fail = |err: std::io::Error| format!("can't talk to pila at {shown}: {err}");
    let mut stream = UnixStream::connect(path).map_err(fail)?;
    stream.set_read_timeout(Some(TIMEOUT)).map_err(fail)?;
    stream.set_write_timeout(Some(TIMEOUT)).map_err(fail)?;
    stream
        .write_all(format!("{request}\n").as_bytes())
        .map_err(fail)?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).map_err(fail)?;
    if line.is_empty() {
        return Err(format!("pila closed the connection at {shown}"));
    }
    Ok(line)
}

/// A window title and such, safe to print on a terminal: it comes from any
/// program, and could otherwise hold escape sequences.
fn plain(value: &Value) -> String {
    value
        .as_str()
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

fn window_line(window: &Value) -> String {
    let mut marks: Vec<&str> = Vec::new();
    for (field, mark) in [
        ("focused", "focused"),
        ("minimized", "minimized"),
        ("maximized", "maximized"),
        ("fullscreen", "fullscreen"),
        ("always_on_top", "on-top"),
        ("all_workspaces", "all-workspaces"),
        ("urgent", "urgent"),
        ("x11", "x11"),
    ] {
        if window[field] == true {
            marks.push(mark);
        }
    }
    let snapped = plain(&window["snapped"]);
    if !snapped.is_empty() {
        marks.push(&snapped);
    }
    format!(
        "{}  workspace {}  {}x{}+{}+{}  {}: {}{}",
        plain(&window["id"]),
        window["workspace"],
        window["width"],
        window["height"],
        window["x"],
        window["y"],
        plain(&window["app_id"]),
        plain(&window["title"]),
        if marks.is_empty() {
            String::new()
        } else {
            format!("  [{}]", marks.join(", "))
        },
    )
}

/// The lines to print for an answer to `name`.
fn text_of(name: &str, answer: &Value) -> Vec<String> {
    let list = |field: &str| answer[field].as_array().cloned().unwrap_or_default();
    match name {
        "windows" => list("windows").iter().map(window_line).collect(),
        "focused" => match answer["window"].is_null() {
            true => vec!["no window has the focus".to_string()],
            false => vec![window_line(&answer["window"])],
        },
        "outputs" => list("outputs")
            .iter()
            .map(|o| {
                format!(
                    "{}  {}x{}+{}+{}  scale {}{}",
                    plain(&o["name"]),
                    o["width"],
                    o["height"],
                    o["x"],
                    o["y"],
                    o["scale"],
                    if o["focused"] == true {
                        "  [focused]"
                    } else {
                        ""
                    },
                )
            })
            .collect(),
        "workspaces" => list("workspaces")
            .iter()
            .map(|w| {
                format!(
                    "{}: {} {}{}",
                    w["index"],
                    w["windows"],
                    if w["windows"] == 1 {
                        "window"
                    } else {
                        "windows"
                    },
                    if w["active"] == true {
                        "  [active]"
                    } else {
                        ""
                    },
                )
            })
            .collect(),
        "version" => vec![format!("pila {}", plain(&answer["version"]))],
        _ => Vec::new(),
    }
}

/// `pila msg`, with the arguments after `msg`. Returns the exit status.
pub fn run(args: &[String]) -> i32 {
    let (json_output, args): (bool, &[String]) = match args.split_first() {
        Some((first, rest)) if first == "--json" => (true, rest),
        _ => (false, args),
    };
    if args.first().is_some_and(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return 0;
    }
    let request = match request(args) {
        Ok(request) => request,
        Err(err) => {
            eprintln!("pila msg: {err}\n\n{USAGE}");
            return EXIT_USAGE;
        }
    };
    let line = match socket_path().and_then(|path| ask(&path, &request)) {
        Ok(line) => line,
        Err(err) => {
            eprintln!("pila msg: {err}");
            return EXIT_NO_PILA;
        }
    };
    let answer: Value = match serde_json::from_str(&line) {
        Ok(answer) => answer,
        Err(err) => {
            eprintln!("pila msg: a bad answer from pila: {err}");
            return EXIT_NO_PILA;
        }
    };
    if answer["ok"] != true {
        eprintln!("pila msg: {}", plain(&answer["error"]));
        return EXIT_REFUSED;
    }
    // A closed pipe (`| head`) isn't an error worth saying.
    let mut out = std::io::stdout().lock();
    if json_output {
        let _ = out.write_all(line.as_bytes());
    } else {
        for text in text_of(request["request"].as_str().unwrap_or_default(), &answer) {
            if writeln!(out, "{text}").is_err() {
                break;
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_cant_send_a_terminal_escapes() {
        assert_eq!(plain(&json!("evil\u{1b}[2Jtitle\n")), "evil?[2Jtitle?");
    }

    #[test]
    fn an_action_is_what_follows_it() {
        let args = [
            "action".to_string(),
            "workspace".to_string(),
            "2".to_string(),
        ];
        assert_eq!(
            request(&args).unwrap(),
            json!({ "request": "action", "action": "workspace 2" })
        );
    }
}
