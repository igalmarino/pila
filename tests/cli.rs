//! The command line: --version, --help, bad arguments, --check-config.

use std::process::Command;

fn pila(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_pila"))
        .args(args)
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .output()
        .expect("run pila")
}

fn temp_config(name: &str, text: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("pila-cli-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(&path, text).unwrap();
    path
}

#[test]
fn version_and_help() {
    let out = pila(&["--version"]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.starts_with(&format!("pila {}", env!("CARGO_PKG_VERSION"))),
        "{text}"
    );
    let out = pila(&["-h"]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    for flag in [
        "--command",
        "--config",
        "--backend",
        "--headless-outputs",
        "--check-config",
        "PILA_MOD",
    ] {
        assert!(text.contains(flag), "--help doesn't mention {flag}");
    }
}

#[test]
fn bad_arguments_exit_2() {
    for args in [
        &["--bogus"][..],
        &["--backend", "nope"],
        &["--backend"],
        &["-c"],
        &["--config"],
        &["--headless-outputs", "nonsense"],
        &["--headless-outputs"],
    ] {
        let out = pila(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("Usage"),
            "{args:?}"
        );
    }
}

#[test]
fn check_config_accepts_a_valid_file() {
    let path = temp_config("valid", "workspaces = 3\n[keyboard]\nlayout = \"us\"\n");
    let out = pila(&["--check-config", "--config", path.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains(": OK"));
}

#[test]
fn check_config_refuses_an_invalid_file_and_says_why() {
    let path = temp_config("invalid", "workspaces = 99\n");
    let out = pila(&["--check-config", "--config", path.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("invalid") && text.contains("workspaces"),
        "{text}"
    );
}

#[test]
fn check_config_refuses_a_bad_keymap() {
    let path = temp_config("keymap", "[keyboard]\nlayout = \"no-such-layout\"\n");
    let out = pila(&["--check-config", "--config", path.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn check_config_of_a_missing_file_is_fine() {
    let out = pila(&["--check-config", "--config", "/nonexistent/pila.toml"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("no such file"));
}

#[test]
fn the_example_config_checks() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/config.example.toml");
    let out = pila(&["--check-config", "--config", path]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn the_udev_backend_without_a_seat_fails_cleanly() {
    // In a container or CI there's no seat: an error, not a crash.
    let out = Command::new(env!("CARGO_BIN_EXE_pila"))
        .args(["--backend", "udev"])
        .env("XDG_RUNTIME_DIR", std::env::temp_dir())
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .env_remove("XDG_SEAT")
        .env("LIBSEAT_BACKEND", "noop-for-tests")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
}

/// A directory of `pila-report`'s own, as a tester's home.
fn report_home(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("pila-report-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("state/pila")).unwrap();
    dir
}

/// Run scripts/report in `home`, with this build first on the `PATH`.
fn report(home: &std::path::Path) -> std::process::Output {
    let path = std::env::var("PATH").unwrap();
    report_on_path(home, &path)
}

/// Run scripts/report in `home`, with this build first on `path`.
fn report_on_path(home: &std::path::Path, path: &str) -> std::process::Output {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/report");
    let bin = std::path::Path::new(env!("CARGO_BIN_EXE_pila"))
        .parent()
        .unwrap();
    let path = format!("{}:{path}", bin.display());
    Command::new(script)
        .current_dir(home)
        .env("PATH", path)
        .env("HOME", home)
        .env("USER", "somebody")
        .env("XDG_STATE_HOME", home.join("state"))
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .expect("run scripts/report")
}

#[test]
fn the_report_has_the_system_and_the_log_without_private_names() {
    use std::os::unix::fs::PermissionsExt;
    let home = report_home("full");
    std::fs::write(
        home.join("state/pila/pila.log"),
        format!(
            "first line\nopened {}/secret.txt for somebody\nthe last line\n",
            home.display()
        ),
    )
    .unwrap();
    let out = report(&home);
    assert!(out.status.success(), "{out:?}");

    let file = home.join("pila-report.txt");
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(
        text.contains(&format!("pila {}", env!("CARGO_PKG_VERSION"))),
        "{text}"
    );
    for section in ["## System", "## GPU", "## Monitors", "## Config", "## Log"] {
        assert!(text.contains(section), "no {section}: {text}");
    }
    assert!(text.contains("the last line"), "{text}");
    assert!(text.contains("~/secret.txt"), "{text}");
    assert!(!text.contains(&home.display().to_string()), "{text}");
    assert!(!text.contains("somebody"), "{text}");
    // The user decides what leaves their machine.
    let mode = std::fs::metadata(&file).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(said.contains("pila-report.txt"), "{said}");
    assert!(
        said.contains("before"),
        "no warning to read it first: {said}"
    );
}

#[test]
fn the_report_says_which_gpu_pila_uses_however_long_the_log() {
    let home = report_home("gpu");
    // More than the report takes from the end of the log.
    let mut log =
        String::from("2026-11-06T16:18:39Z INFO pila::udev: using GPU gpu=/dev/dri/card1\n");
    log.push_str(&"something else\n".repeat(1000));
    std::fs::write(home.join("state/pila/pila.log"), log).unwrap();
    let out = report(&home);
    assert!(out.status.success(), "{out:?}");
    let text = std::fs::read_to_string(home.join("pila-report.txt")).unwrap();
    assert!(text.contains("In use by pila: /dev/dri/card1"), "{text}");
}

#[test]
fn the_report_says_whether_the_tools_bindings_and_apps_need_are_installed() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    for tool in ["xdg-open", "wl-copy"] {
        // Every program on the PATH but this one, whatever this machine has.
        let home = report_home(tool);
        let bin = home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap()) {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                if entry.file_name() != tool {
                    let _ = symlink(entry.path(), bin.join(entry.file_name()));
                }
            }
        }
        let report_text = || {
            let out = report_on_path(&home, &bin.display().to_string());
            assert!(out.status.success(), "{out:?}");
            std::fs::read_to_string(home.join("pila-report.txt")).unwrap()
        };
        let text = report_text();
        assert!(text.contains(&format!("{tool}: missing")), "{text}");

        let program = bin.join(tool);
        std::fs::write(&program, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let text = report_text();
        assert!(text.contains(&format!("{tool}: installed")), "{text}");
    }
}

#[test]
fn the_report_hides_a_name_but_leaves_words_that_hold_it() {
    let home = report_home("words");
    std::fs::write(
        home.join("state/pila/pila.log"),
        "Failed to unsomebodyish a thing\n\
         somebody opened it, as somebody-else and somebody.old\n\
         the end for somebody\n",
    )
    .unwrap();
    let out = report(&home);
    assert!(out.status.success(), "{out:?}");
    let text = std::fs::read_to_string(home.join("pila-report.txt")).unwrap();
    // USER is somebody: an ordinary word that holds it stays as it is, and
    // doesn't show which part of it was the name.
    assert!(text.contains("Failed to unsomebodyish a thing"), "{text}");
    assert!(
        text.contains("<user> opened it, as <user>-else and <user>.old"),
        "{text}"
    );
    assert!(text.contains("the end for <user>"), "{text}");
}

#[test]
fn the_report_works_with_no_log() {
    let home = report_home("nolog");
    let out = report(&home);
    assert!(out.status.success(), "{out:?}");
    let text = std::fs::read_to_string(home.join("pila-report.txt")).unwrap();
    assert!(text.contains("no log found"), "{text}");
}

#[test]
fn a_missing_egl_library_is_an_error_not_a_crash() {
    // A broken library found first fails to load just like a missing one.
    let dir = std::env::temp_dir().join(format!("pila-no-egl-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("libEGL.so.1"), "not a library").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pila"))
        .args(["--backend", "headless"])
        .env("LD_LIBRARY_PATH", &dir)
        .env("XDG_RUNTIME_DIR", &dir)
        .env("XDG_STATE_HOME", &dir)
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("libEGL.so.1"), "{stderr}");
    assert!(stderr.contains("LD_LIBRARY_PATH"), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A copy of scripts/check and scripts/tty, which work from where they are and
/// leave their logs there.
fn scripts_copy(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("pila-scripts-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    for script in ["check", "tty"] {
        let from = format!("{}/scripts/{script}", env!("CARGO_MANIFEST_DIR"));
        std::fs::copy(from, dir.join("scripts").join(script)).unwrap();
    }
    dir
}

/// Run a script of a `scripts_copy` through bash, which only reads it. Executing
/// a file just written fails now and then with "Text file busy": another thread
/// forking at that moment holds it open for writing until it execs.
fn copied_script(dir: &std::path::Path, script: &str) -> Command {
    let mut command = Command::new("bash");
    command.arg(dir.join("scripts").join(script));
    command
}

/// Wait until `path`, just written, can be executed (see `copied_script`).
fn wait_until_executable(path: &std::path::Path) {
    while let Err(err) = Command::new(path).output() {
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::ExecutableFileBusy,
            "{}: {err}",
            path.display()
        );
        std::thread::yield_now();
    }
}

#[test]
fn scripts_check_runs_the_binary_it_is_given() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scripts_copy("given");
    let fake = dir.join("fake-pila");
    let ran = dir.join("ran");
    std::fs::write(
        &fake,
        format!("#!/bin/sh\necho \"$@\" > {}\n", ran.display()),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    wait_until_executable(&fake);
    let _ = std::fs::remove_file(&ran);
    // It finds nothing to judge afterwards: only what it started matters here.
    copied_script(&dir, "check")
        .arg("--nested")
        .env("PILA_BIN", &fake)
        .env("XDG_CONFIG_HOME", &dir)
        .output()
        .unwrap();
    let args = std::fs::read_to_string(&ran).expect("it didn't run the given binary");
    assert!(args.contains("--config"), "{args}");
}

#[test]
fn scripts_check_and_tty_say_when_the_given_binary_is_missing() {
    let dir = scripts_copy("missing");
    for args in [&["check", "--nested"][..], &["tty"][..]] {
        let out = copied_script(&dir, args[0])
            .args(&args[1..])
            .env("PILA_BIN", "/no/such/pila")
            .env("XDG_CONFIG_HOME", &dir)
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{args:?}: {stderr}");
        assert!(stderr.contains("/no/such/pila"), "{args:?}: {stderr}");
    }
}

#[test]
fn scripts_check_refuses_a_build_older_than_the_sources() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scripts_copy("stale");
    let release = dir.join("target/release");
    std::fs::create_dir_all(&release).unwrap();
    let bin = release.join("pila");
    let ran = dir.join("ran");
    std::fs::write(&bin, format!("#!/bin/sh\ntouch {}\n", ran.display())).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Newer than the build, whatever the clock's granularity.
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/main.rs"), "").unwrap();
    let out = copied_script(&dir, "check")
        .arg("--nested")
        .env_remove("PILA_BIN")
        .env("XDG_CONFIG_HOME", &dir)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    assert!(stderr.contains("older than the sources"), "{stderr}");
    assert!(!ran.exists(), "it ran the old build");
}

#[test]
fn scripts_check_finds_the_monitors_in_a_bare_metal_log() {
    // As the log says it, from where the code logs it: moving that code once
    // made every bare-metal run fail this check.
    let source = concat!(env!("CARGO_MANIFEST_DIR"), "/src/udev/outputs.rs");
    let code = std::fs::read_to_string(source).unwrap();
    assert!(
        code.contains("\"monitor\""),
        "it's not logged there anymore"
    );
    let dir = scripts_copy("monitors");
    let log = dir.join("bare-metal.log");
    std::fs::write(
        &log,
        "INFO pila: starting pila backend=Udev\n\
         INFO pila::udev::outputs: monitor name=\"eDP-1\" make=\"AUO\" model=\"B140HTN02.4\"\n\
         check: done\n",
    )
    .unwrap();
    let out = copied_script(&dir, "check")
        .arg("--judge")
        .arg(&log)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("PASS  monitors were identified (1)"),
        "{stdout}"
    );
}

#[test]
fn scripts_check_gives_a_verdict_on_a_log_that_stops_early() {
    // pila crashed before the scenarios began: it says so, not nothing.
    let dir = scripts_copy("early");
    let log = dir.join("early.log");
    std::fs::write(&log, "INFO pila: starting pila backend=Winit\n").unwrap();
    let out = copied_script(&dir, "check")
        .arg("--judge")
        .arg(&log)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("FAIL  the scenarios finished"), "{stdout}");
    assert!(stdout.contains("Some checks failed"), "{stdout}");
    assert_eq!(out.status.code(), Some(1), "{stdout}");
}

/// A copy of `pila-session` next to a fake pila running `body`, with no real
/// system log: (the session script, the directory it logs to).
fn session_copy(name: &str, body: &str) -> (Command, std::path::PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("pila-session-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = dir.join("pila-session");
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/resources/session/pila-session"
        ),
        &script,
    )
    .unwrap();
    for (path, text) in [
        // Only the session's run does anything: see `wait_until_executable`.
        (
            dir.join("pila"),
            format!("#!/bin/sh\n[ \"$1\" = --backend ] || exit 0\n{body}\n"),
        ),
        (
            bin.join("systemd-cat"),
            "#!/bin/sh\nexec cat >/dev/null\n".to_string(),
        ),
    ] {
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        wait_until_executable(&path);
    }
    let mut command = Command::new("bash");
    command
        .arg(&script)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("XDG_STATE_HOME", dir.join("state"))
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    (command, dir.join("state/pila"))
}

/// Wait for `text` in the session log; whether it came in time.
fn session_log_has(log: &std::path::Path, text: &str, timeout: std::time::Duration) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed() < timeout {
        if std::fs::read(log).is_ok_and(|log| String::from_utf8_lossy(&log).contains(text)) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    false
}

#[test]
fn the_session_log_is_written_as_it_happens() {
    let (mut command, dir) = session_copy("live", "echo the first line >&2\nexec sleep 30");
    let mut session = command.spawn().unwrap();
    // While the session runs, or a crash that takes the logger along loses it.
    let written = session_log_has(
        &dir.join("pila.log"),
        "the first line",
        std::time::Duration::from_secs(5),
    );
    session.kill().unwrap();
    session.wait().unwrap();
    let _ = std::fs::remove_dir_all(dir.parent().unwrap().parent().unwrap());
    assert!(written, "the log is held back in a buffer");
}

#[test]
fn the_session_log_stops_at_its_limit() {
    const LIMIT: u64 = 50 * 1024 * 1024;
    let (mut command, dir) = session_copy(
        "limit",
        &format!(
            "yes \"$(printf %1000s)\" | head -c {} >&2",
            LIMIT + 1024 * 1024
        ),
    );
    assert!(command.status().unwrap().success());
    let log = dir.join("pila.log");
    let said = session_log_has(
        &log,
        "the rest is only in the system log",
        std::time::Duration::from_secs(30),
    );
    let size = std::fs::metadata(&log).unwrap().len();
    let _ = std::fs::remove_dir_all(dir.parent().unwrap().parent().unwrap());
    assert!(said, "no word of the limit in a log of {size} bytes");
    assert!((LIMIT..LIMIT + 4096).contains(&size), "{size} bytes");
}
