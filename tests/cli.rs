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
