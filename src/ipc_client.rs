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
       pila msg --waybar captures

Requests:
  windows             The windows, on all workspaces
  outputs             The monitors
  workspaces          The workspaces
  keyboard            The keyboard layouts, and which is in use
  colors              The colors of [colors], for a bar or launcher
  switch              The Mod+Tab in progress: its windows, most recent
                      first, and the one selected
  focused             The focused window
  version             Pila's version
  subscribe [EVENTS]  Events as they happen: windows, workspaces, outputs,
                      keyboard, colors, switch (all of them unless named),
                      until Pila quits
  action <action>     Run an action, as in [bindings]: action workspace 2
  focus <id>          Focus a window, by its id in `windows`
  close <id>          Ask a window to close
  minimize <id>       Minimize a window; focus brings it back
  screen-shares       The screen shares remembered for apps, which they
                      get again without asking
  forget-screen-share <id>|all
                      Forget one, by its id in `screen-shares`, or all
  capture-permissions Programs allowed in the prompt to capture the screen
                      directly (wlr-screencopy, ext-image-copy-capture),
                      until you log out
  forget-capture-permission <id>|all
                      Forget one, by its id in `capture-permissions`, or all
  idle-inhibitors     The apps keeping the screen on now: over D-Bus
                      (org.freedesktop.ScreenSaver), or a visible window's
                      or layer surface's Wayland idle inhibitor
  captures            The programs capturing the screen now, and what each
                      captures: a monitor, a region or a window

Options:
  --json              Print Pila's answer as it is (docs/IPC.md)
  --waybar            With captures: follow them as a waybar custom module
                      (return-type json), a line each time they change";

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
    Ok(PathBuf::from(runtime).join(crate::ipc::socket_file_name(display)))
}

/// The request for the arguments after `msg`.
fn request(args: &[String]) -> Result<Value, String> {
    let (name, rest) = args.split_first().ok_or("no request")?;
    let one_id = || match rest {
        [id] => Ok(id.as_str()),
        _ => Err(format!("{name} takes one window id")),
    };
    match name.as_str() {
        "windows"
        | "outputs"
        | "workspaces"
        | "keyboard"
        | "colors"
        | "switch"
        | "focused"
        | "version"
        | "screen-shares"
        | "capture-permissions"
        | "idle-inhibitors"
        | "captures" => match rest {
            [] => Ok(json!({ "request": name })),
            _ => Err(format!("{name} takes no arguments")),
        },
        "forget-screen-share" | "forget-capture-permission" => match rest {
            [all] if all == "all" => Ok(json!({ "request": name, "all": true })),
            [id] => Ok(json!({ "request": name, "id": id })),
            _ => Err(format!("{name} takes one id, or all")),
        },
        "action" if rest.is_empty() => Err("action needs an action".to_string()),
        "action" => Ok(json!({ "request": "action", "action": rest.join(" ") })),
        "subscribe" if rest.is_empty() => Ok(json!({ "request": "subscribe" })),
        "subscribe" => Ok(json!({ "request": "subscribe", "events": rest })),
        "focus" | "close" | "minimize" => Ok(json!({ "request": name, "id": one_id()? })),
        other => Err(format!("unknown request {other:?}")),
    }
}

/// Send `request`; pila's answer as it wrote it, and the connection, for
/// the events that follow a subscription.
fn ask(path: &std::path::Path, request: &Value) -> Result<(String, BufReader<UnixStream>), String> {
    let shown = path.display();
    let fail = |err: std::io::Error| format!("can't talk to pila at {shown}: {err}");
    let mut stream = UnixStream::connect(path).map_err(fail)?;
    stream.set_read_timeout(Some(TIMEOUT)).map_err(fail)?;
    stream.set_write_timeout(Some(TIMEOUT)).map_err(fail)?;
    stream
        .write_all(format!("{request}\n").as_bytes())
        .map_err(fail)?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(fail)?;
    if line.is_empty() {
        return Err(format!("pila closed the connection at {shown}"));
    }
    Ok((line, reader))
}

/// A line of JSON from pila, safe to print on a terminal: JSON escapes the
/// controls below U+0020 only, and DEL and the C1 controls (U+0080 to
/// U+009F, CSI among them) would reach it as they are. They can only be in
/// strings, where `\u` escapes say the same.
fn json_safe(line: &str) -> String {
    let mut safe = String::with_capacity(line.len());
    for c in line.chars() {
        if ('\u{7f}'..='\u{9f}').contains(&c) {
            safe.push_str(&format!("\\u{:04x}", c as u32));
        } else {
            safe.push(c);
        }
    }
    safe
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
    // Events leave out where windows are.
    let place = match window["width"].is_null() {
        true => String::new(),
        false => format!(
            "  {}x{}+{}+{}",
            window["width"], window["height"], window["x"], window["y"]
        ),
    };
    format!(
        "{}  workspace {}{place}  {}: {}{}",
        plain(&window["id"]),
        window["workspace"],
        plain(&window["app_id"]),
        plain(&window["title"]),
        if marks.is_empty() {
            String::new()
        } else {
            format!("  [{}]", marks.join(", "))
        },
    )
}

/// A program capturing: who, what, and how.
fn capture_line(capture: &Value) -> String {
    let target = &capture["target"];
    let what = match target["kind"].as_str() {
        Some("output") => format!("screen {}", plain(&target["output"])),
        Some("region") => format!(
            "region of {} {}x{}+{}+{}",
            plain(&target["output"]),
            target["width"],
            target["height"],
            target["x"],
            target["y"]
        ),
        Some("window") => format!("window {}", plain(&target["app_id"])),
        _ => "?".to_string(),
    };
    format!(
        "{}  {what}  ({})",
        plain(&capture["app"]),
        plain(&capture["protocol"])
    )
}

/// How long ago, `seconds` back, roughly: "12 s ago", "5 min ago",
/// "3 h ago", "2 days ago".
fn ago(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds} s ago"),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86400 => format!("{} h ago", seconds / 3600),
        _ => format!("{} days ago", seconds / 86400),
    }
}

/// A line for a waybar custom module (`return-type: json`) about `captures`:
/// no text when nothing captures (waybar hides the module), the app when one
/// does, how many when several do; each capture in the tooltip.
fn waybar_line(captures: &[Value]) -> String {
    let mut apps: Vec<String> = captures.iter().map(|c| plain(&c["app"])).collect();
    apps.sort();
    apps.dedup();
    let text = match apps.as_slice() {
        [] => String::new(),
        [app] => format!("● {app}"),
        apps => format!("● {}", apps.len()),
    };
    let tooltip: Vec<String> = captures.iter().map(capture_line).collect();
    json!({
        "text": text,
        "tooltip": tooltip.join("\n"),
        "class": if captures.is_empty() { "idle" } else { "capturing" },
    })
    .to_string()
}

/// A screen share remembered: its id, the app, what it shares, and how long
/// it's kept.
fn screen_share_line(share: &Value) -> String {
    let shares = &share["shares"];
    let what = match shares["kind"].as_str() {
        Some("output") => plain(&shares["output"]),
        Some("region") => format!(
            "{} region {}x{}+{}+{}",
            plain(&shares["output"]),
            shares["width"],
            shares["height"],
            shares["x"],
            shares["y"]
        ),
        Some("window") => format!(
            "window {}: {}",
            plain(&shares["app_id"]),
            plain(&shares["title"])
        ),
        _ => "?".to_string(),
    };
    format!(
        "{}  {}  {what}{}",
        plain(&share["id"]),
        plain(&share["app"]),
        if share["persistent"] == true {
            ""
        } else {
            "  [while it runs]"
        }
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
                let mut marks = Vec::new();
                if o["focused"] == true {
                    marks.push("focused");
                }
                if o["power"] == false {
                    marks.push("off");
                }
                format!(
                    "{}  {}x{}+{}+{}  scale {}{}",
                    plain(&o["name"]),
                    o["width"],
                    o["height"],
                    o["x"],
                    o["y"],
                    o["scale"],
                    if marks.is_empty() {
                        String::new()
                    } else {
                        format!("  [{}]", marks.join(", "))
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
        "keyboard" => list("layouts")
            .iter()
            .map(|l| {
                format!(
                    "{}: {}{}",
                    l["index"],
                    plain(&l["name"]),
                    if l["active"] == true {
                        "  [active]"
                    } else {
                        ""
                    },
                )
            })
            .collect(),
        "colors" => answer["colors"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(name, color)| format!("{}  {}", name.replace('_', "-"), plain(color)))
            .collect(),
        "switch" => match answer["switch"].as_object() {
            None => vec!["no window switch".to_string()],
            Some(switch) => {
                let mut lines = vec![format!("on {}", plain(&switch["output"]))];
                lines.extend(
                    switch["windows"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|id| {
                            let selected = *id == switch["selected"];
                            format!(
                                "{}{}",
                                plain(id),
                                if selected { "  [selected]" } else { "" }
                            )
                        }),
                );
                lines
            }
        },
        "version" => vec![format!("pila {}", plain(&answer["version"]))],
        "screen-shares" => match list("screen_shares") {
            shares if shares.is_empty() => vec!["no screen share remembered".to_string()],
            shares => shares.iter().map(screen_share_line).collect(),
        },
        "forget-screen-share" | "forget-capture-permission" if answer["forgotten"].is_u64() => {
            vec![format!("forgot {}", answer["forgotten"])]
        }
        "captures" => {
            let mut lines = match list("captures") {
                captures if captures.is_empty() => {
                    vec!["nothing is capturing the screen".to_string()]
                }
                captures => captures.iter().map(capture_line).collect(),
            };
            if let Some(ended) = answer["last"]["ended"].as_u64() {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(ended, |now| now.as_secs());
                lines.push(format!(
                    "last: {}, {}",
                    capture_line(&answer["last"]),
                    ago(now.saturating_sub(ended))
                ));
            }
            lines
        }
        "idle-inhibitors" => match list("idle_inhibitors") {
            inhibitors if inhibitors.is_empty() => vec!["nothing keeps the screen on".to_string()],
            inhibitors => inhibitors
                .iter()
                .map(|i| match i["reason"].as_str() {
                    Some(reason) if !reason.is_empty() => {
                        format!(
                            "{}  {}  ({})",
                            plain(&i["source"]),
                            plain(&i["app"]),
                            plain(&i["reason"])
                        )
                    }
                    _ => format!("{}  {}", plain(&i["source"]), plain(&i["app"])),
                })
                .collect(),
        },
        "capture-permissions" => match list("capture_permissions") {
            grants if grants.is_empty() => vec!["no capture permission allowed".to_string()],
            grants => grants
                .iter()
                .map(|g| format!("{}  {}", plain(&g["id"]), plain(&g["executable"])))
                .collect(),
        },
        _ => Vec::new(),
    }
}

/// `pila msg`, with the arguments after `msg`. Returns the exit status.
pub fn run(args: &[String]) -> i32 {
    if let Some((first, rest)) = args.split_first() {
        if first == "--waybar" {
            return waybar(rest);
        }
    }
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
    let (line, reader) = match socket_path().and_then(|path| ask(&path, &request)) {
        Ok(answer) => answer,
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
    if request["request"] == "subscribe" {
        return follow(reader, json_output);
    }
    // Answered: pila's connection isn't held while printing (to a pager).
    drop(reader);
    // A closed pipe (`| head`) isn't an error worth saying.
    let mut out = std::io::stdout().lock();
    if json_output {
        let _ = out.write_all(json_safe(&line).as_bytes());
    } else {
        for text in text_of(request["request"].as_str().unwrap_or_default(), &answer) {
            if writeln!(out, "{text}").is_err() {
                break;
            }
        }
    }
    0
}

/// `pila msg --waybar captures`: the captures as a waybar custom module
/// follows them, a line now and one each time they change, until pila quits.
fn waybar(args: &[String]) -> i32 {
    if args != ["captures"] {
        eprintln!("pila msg: --waybar goes with captures\n\n{USAGE}");
        return EXIT_USAGE;
    }
    let request = json!({ "request": "subscribe", "events": ["captures"] });
    let (line, reader) = match socket_path().and_then(|path| ask(&path, &request)) {
        Ok(answer) => answer,
        Err(err) => {
            eprintln!("pila msg: {err}");
            return EXIT_NO_PILA;
        }
    };
    let answer: Value = serde_json::from_str(&line).unwrap_or_default();
    if answer["ok"] != true {
        eprintln!("pila msg: {}", plain(&answer["error"]));
        return EXIT_REFUSED;
    }
    each_event(reader, |line| {
        let event: Value = serde_json::from_str(line).unwrap_or_default();
        if event["event"] != "captures" {
            return None;
        }
        let captures = event["captures"].as_array().cloned().unwrap_or_default();
        Some(format!("{}\n", waybar_line(&captures)))
    })
}

/// Each event pila sends, as `text` writes it (`None`: nothing), until pila
/// closes the connection or the output does (`| head`).
fn each_event(
    mut reader: BufReader<UnixStream>,
    mut text: impl FnMut(&str) -> Option<String>,
) -> i32 {
    // Events come when something changes: no timeout from here on.
    if let Err(err) = reader.get_ref().set_read_timeout(None) {
        eprintln!("pila msg: can't talk to pila: {err}");
        return EXIT_NO_PILA;
    }
    let mut out = std::io::stdout().lock();
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return 0,
            Ok(_) => {}
        }
        let Some(text) = text(&line) else {
            continue;
        };
        if out
            .write_all(text.as_bytes())
            .and_then(|()| out.flush())
            .is_err()
        {
            return 0;
        }
    }
}

/// `pila msg subscribe`, once pila said yes: each event as it comes, until
/// pila closes the connection.
fn follow(reader: BufReader<UnixStream>, json_output: bool) -> i32 {
    each_event(reader, |line| {
        if json_output {
            return Some(json_safe(line));
        }
        let event: Value = serde_json::from_str(line).unwrap_or_default();
        let kind = plain(&event["event"]);
        let mut text = format!("== {kind}\n");
        for item in text_of(&kind, &event) {
            text.push_str(&item);
            text.push('\n');
        }
        Some(text)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The man page lists every request `pila msg` takes, in its PILA MSG
    /// section.
    #[test]
    fn the_man_page_has_every_request() {
        let man = include_str!("../docs/pila.1").replace("\\-", "-");
        let section = man.split(".SH PILA MSG").nth(1).unwrap();
        let section = section.split("\n.SH ").next().unwrap();
        let words: Vec<&str> = section
            .split(|c: char| !(c.is_ascii_lowercase() || c == '-'))
            .collect();
        let requests = USAGE.split("Requests:").nth(1).unwrap();
        let requests = requests.split("Options:").next().unwrap();
        for line in requests.lines() {
            // A request starts its line, two spaces in.
            let Some(name) = line.strip_prefix("  ").and_then(|l| l.split(' ').next()) else {
                continue;
            };
            if !name.is_empty() {
                assert!(
                    words.contains(&name),
                    "docs/pila.1 doesn't list `pila msg {name}`"
                );
            }
        }
    }

    #[test]
    fn how_long_ago_is_said_in_its_unit() {
        assert_eq!(ago(0), "0 s ago");
        assert_eq!(ago(59), "59 s ago");
        assert_eq!(ago(60), "1 min ago");
        assert_eq!(ago(3599), "59 min ago");
        assert_eq!(ago(7200), "2 h ago");
        assert_eq!(ago(86400 * 3), "3 days ago");
    }

    /// Hidden with nothing captured; the app, or how many apps, with each
    /// capture in the tooltip; names can't bring controls in.
    #[test]
    fn the_waybar_module_names_who_captures() {
        let line = |captures: Value| -> Value {
            serde_json::from_str(&waybar_line(captures.as_array().unwrap())).unwrap()
        };
        assert_eq!(
            line(json!([])),
            json!({ "text": "", "tooltip": "", "class": "idle" })
        );
        let screen = json!({ "app": "grim", "protocol": "wlr-screencopy", "target": { "kind": "output", "output": "DP-1" } });
        let window = json!({ "app": "Web\u{1b}Browser", "protocol": "portal", "target": { "kind": "window", "id": "1", "app_id": "kitty", "output": "DP-1" } });
        assert_eq!(
            line(json!([screen])),
            json!({ "text": "● grim", "tooltip": "grim  screen DP-1  (wlr-screencopy)", "class": "capturing" })
        );
        let both = line(json!([screen, window, screen]));
        assert_eq!(both["text"], "● 2");
        assert_eq!(
            both["tooltip"],
            "grim  screen DP-1  (wlr-screencopy)\nWeb?Browser  window kitty  (portal)\ngrim  screen DP-1  (wlr-screencopy)"
        );
    }

    #[test]
    fn a_title_cant_send_a_terminal_escapes() {
        assert_eq!(plain(&json!("evil\u{1b}[2Jtitle\n")), "evil?[2Jtitle?");
    }

    #[test]
    fn subscribe_takes_the_events_named_or_all() {
        let args = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        assert_eq!(
            request(&args(&["subscribe"])).unwrap(),
            json!({ "request": "subscribe" })
        );
        assert_eq!(
            request(&args(&["subscribe", "windows", "outputs"])).unwrap(),
            json!({ "request": "subscribe", "events": ["windows", "outputs"] })
        );
    }

    #[test]
    fn a_screen_share_is_forgotten_by_id_or_all() {
        let args = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        assert_eq!(
            request(&args(&["forget-screen-share", "0a1b"])).unwrap(),
            json!({ "request": "forget-screen-share", "id": "0a1b" })
        );
        assert_eq!(
            request(&args(&["forget-screen-share", "all"])).unwrap(),
            json!({ "request": "forget-screen-share", "all": true })
        );
        assert!(request(&args(&["forget-screen-share"])).is_err());
        assert!(request(&args(&["screen-shares", "x"])).is_err());
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
