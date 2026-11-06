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
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/report");
    let bin = std::path::Path::new(env!("CARGO_BIN_EXE_pila"))
        .parent()
        .unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
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
    // It finds nothing to judge afterwards: only what it started matters here.
    Command::new(dir.join("scripts/check"))
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
        let out = Command::new(dir.join("scripts").join(args[0]))
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
