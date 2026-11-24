//! Files Pila keeps in `$XDG_STATE_HOME/pila` (`~/.local/state/pila`): what
//! apps' sessions placed (`session_management.rs`), the keys confirmed for
//! apps' shortcuts (`app_shortcuts.rs`), the screen shares remembered for apps
//! (`screen_grants.rs`). Each is written whole or not at all, for the user
//! alone: some hold secrets (ids an app keeps to get something back).

use std::{
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

use tracing::warn;

/// `$XDG_STATE_HOME/pila/<name>`; `None` with neither it (absolute) nor
/// `HOME` set.
pub fn path(name: &str) -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        })?;
    Some(state.join("pila").join(name))
}

/// Write `text` to `path` whole or not at all, readable by the user alone.
pub fn write(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temporary = path.with_extension("json.tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&temporary, path)
}

/// 128 random bits, in hex: an id that is a secret (`None`, logged, if
/// there's no randomness to be had).
pub fn random_id() -> Option<String> {
    let mut bytes = [0u8; 16];
    let read = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes));
    if let Err(err) = read {
        warn!(?err, "no random id");
        return None;
    }
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
