//! Embeds `git describe` in the binary (`PILA_GIT_VERSION`) for `--version`
//! and the log, or `PILA_BUILD_COMMIT` if that's set: a package built without
//! the repository (`package.nix`) says which commit it is that way. Empty
//! outside a git checkout otherwise (e.g. a release tarball).

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=PILA_BUILD_COMMIT");
    let describe = std::env::var("PILA_BUILD_COMMIT").unwrap_or_else(|_| {
        Command::new("git")
            .args(["describe", "--tags", "--always", "--dirty"])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    });
    println!("cargo:rustc-env=PILA_GIT_VERSION={describe}");
    // A new commit or checkout moves HEAD or the index; staging changes the
    // index too; an edit to the sources makes the tree dirty (so a build of
    // uncommitted changes doesn't say the last build's version, clean).
    let mut paths = vec![".git/HEAD".to_string(), ".git/index".to_string()];
    // A commit moves the branch HEAD names (`ref: refs/heads/...`), not HEAD.
    if let Ok(head) = std::fs::read_to_string(".git/HEAD") {
        if let Some(branch) = head.trim().strip_prefix("ref: ") {
            paths.push(format!(".git/{branch}"));
        }
    }
    paths.extend(["src".to_string(), "Cargo.toml".to_string()]);
    for path in paths {
        if std::path::Path::new(&path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}
