//! Files Pila keeps in `$XDG_STATE_HOME/pila` (`~/.local/state/pila`): what
//! apps' sessions placed (`session_management.rs`), the keys confirmed for
//! apps' shortcuts (`app_shortcuts.rs`), the screen shares remembered for apps
//! (`screen_grants.rs`).
//! Each is written whole or not at all, in the order asked, for the user
//! alone: some hold secrets (ids an app keeps to get something back).

use std::{
    collections::HashMap,
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::{mpsc, Mutex},
};

use tracing::warn;

use crate::sync::Locked;

/// A save asked for: the file, its name for the log, and what makes its text.
type Save = (
    PathBuf,
    &'static str,
    Box<dyn FnOnce() -> std::io::Result<String> + Send>,
);

/// The thread that writes the state files, one save after another
/// (`save`); started at the first.
static WRITER: Mutex<Option<mpsc::Sender<Save>>> = Mutex::new(None);

/// Write what `text` makes to `path`, whole or not at all, on the state
/// files' thread: in the order the saves were asked for, so one asked for
/// later is never undone by an earlier one (a permission revoked mustn't
/// come back). `what` names the file in the log.
pub fn save(
    path: PathBuf,
    what: &'static str,
    text: impl FnOnce() -> std::io::Result<String> + Send + 'static,
) {
    let mut writer = WRITER.locked();
    let mut save: Save = (path, what, Box::new(text));
    if let Some(sender) = writer.as_ref() {
        match sender.send(save) {
            Ok(()) => return,
            // The thread went (it panicked): started again.
            Err(mpsc::SendError(unsent)) => save = unsent,
        }
    }
    let (sender, saves) = mpsc::channel::<Save>();
    let spawned = std::thread::Builder::new()
        .name("pila-state-files".into())
        .spawn(move || {
            for (path, what, text) in saves {
                if let Err(err) = text().and_then(|text| write(&path, &text)) {
                    warn!(%err, path = %path.display(), "can't save {what}");
                }
            }
        });
    if let Err(err) = spawned {
        warn!(?err, "can't save {}", save.1);
        return;
    }
    // Received: the thread holds the other end.
    let _ = sender.send(save);
    *writer = Some(sender);
}

/// What `key` holds in a state file's `text` (`{"<key>": ...}`), which
/// anything may have written; nothing there: the default.
pub fn parse_keyed<T: serde::de::DeserializeOwned + Default>(
    text: &str,
    key: &str,
) -> serde_json::Result<T> {
    let mut file: HashMap<String, T> = serde_json::from_str(text)?;
    Ok(file.remove(key).unwrap_or_default())
}

/// What `key` holds in the state file `name` (`parse_keyed`); none or
/// unreadable: the default, logged as `what`. Reads a file: never on the
/// event loop.
pub fn read_keyed<T: serde::de::DeserializeOwned + Default>(
    name: &str,
    key: &str,
    what: &str,
) -> T {
    let Some(path) = path(name) else {
        return T::default();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return T::default();
    };
    parse_keyed(&text, key).unwrap_or_else(|err| {
        warn!(%err, path = %path.display(), "can't read {what}");
        T::default()
    })
}

/// `value` written under `key` to the state file `name` (`save`).
pub fn save_keyed<T: serde::Serialize + Send + 'static>(
    name: &str,
    key: &'static str,
    what: &'static str,
    value: T,
) {
    let Some(path) = path(name) else {
        return;
    };
    save(path, what, move || {
        let file = HashMap::from([(key, value)]);
        serde_json::to_string_pretty(&file).map_err(std::io::Error::other)
    });
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

    /// Saves are written in the order asked for: the last one stays.
    #[test]
    fn the_last_save_asked_for_is_the_one_kept() {
        let dir = std::env::temp_dir().join(format!("pila-state-order-{}", std::process::id()));
        let path = dir.join("grants.json");
        for text in ["first", "second", "last"] {
            save(path.clone(), "a test file", move || Ok(text.to_string()));
        }
        let started = std::time::Instant::now();
        while std::fs::read_to_string(&path).ok().as_deref() != Some("last") {
            assert!(started.elapsed().as_secs() < 5, "never written");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
