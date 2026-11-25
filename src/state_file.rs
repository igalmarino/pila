//! Files Pila keeps in `$XDG_STATE_HOME/pila` (`~/.local/state/pila`): what
//! apps' sessions placed (`session_management.rs`), the keys confirmed for
//! apps' shortcuts (`app_shortcuts.rs`), the screen shares remembered for apps
//! (`screen_grants.rs`), the capture permissions saved (`permissions.rs`).
//! Each is written whole or not at all, in the order asked, for the user
//! alone: some hold secrets (ids an app keeps to get something back).

use std::{
    collections::HashMap,
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};

use tracing::warn;

use crate::sync::Locked;

/// The number of the next save asked for (`save`).
static NEXT_SAVE: AtomicU64 = AtomicU64::new(1);

/// The number of the last save written to each file (`write_in_order`).
/// Held while writing, so no two threads write at once.
static WRITTEN: Mutex<Option<HashMap<PathBuf, u64>>> = Mutex::new(None);

/// Write what `text` makes to `path` on a thread of its own (`name`), whole
/// or not at all, and in the order the saves were asked for: one asked for
/// later and written first isn't undone by this one (a permission revoked
/// mustn't come back). `what` names the file in the log.
pub fn save(
    name: &str,
    path: PathBuf,
    what: &'static str,
    text: impl FnOnce() -> std::io::Result<String> + Send + 'static,
) {
    let number = NEXT_SAVE.fetch_add(1, Ordering::Relaxed);
    let spawned = std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            if let Err(err) = text().and_then(|text| write_in_order(&path, &text, number)) {
                warn!(%err, path = %path.display(), "can't save {what}");
            }
        });
    if let Err(err) = spawned {
        warn!(?err, "can't save {what}");
    }
}

/// `write`, unless a save numbered after `number` was written to `path`
/// already; whether it wrote.
fn write_in_order(path: &Path, text: &str, number: u64) -> std::io::Result<bool> {
    let mut written = WRITTEN.locked();
    let written = written.get_or_insert_with(HashMap::new);
    if written.get(path).is_some_and(|&last| last > number) {
        return Ok(false);
    }
    write(path, text)?;
    written.insert(path.to_path_buf(), number);
    Ok(true)
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Saves written out of order leave the last one asked for.
    #[test]
    fn a_save_asked_for_earlier_and_written_later_is_skipped() {
        let dir = std::env::temp_dir().join(format!("pila-state-order-{}", std::process::id()));
        let path = dir.join("grants.json");
        assert!(write_in_order(&path, "newer", 11).unwrap());
        assert!(!write_in_order(&path, "older", 10).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "newer");
        assert!(write_in_order(&path, "newest", 12).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "newest");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
