//! Embeds `git describe` in the binary (`PILA_GIT_VERSION`) for `--version`
//! and the log. Empty outside a git checkout (e.g. a release tarball).

use std::process::Command;

fn main() {
    let describe = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    println!("cargo:rustc-env=PILA_GIT_VERSION={describe}");
    // A new commit or checkout moves HEAD or the index; staging changes the
    // index too (close enough for "dirty").
    for path in [".git/HEAD", ".git/index"] {
        if std::path::Path::new(path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}
