//! Screen shares remembered for apps: the portal's `persist_mode` and
//! `restore_data` (`portal.rs`). An app that asks for its share to be
//! remembered (Kooha and OBS always do) gets a Remember toggle in the
//! picker (`share_picker.rs`), off unless the app had one remembered; with
//! it on, `Start` answers `restore_data` naming a grant: a random id, a
//! secret only xdg-desktop-portal and Pila see (the portal keeps it for the
//! app behind a token of its own). The next time the app asks with it, the
//! same app is shared the same thing at once, without the picker, if it's
//! there: the output (by name), the region of that output, the window (by
//! app id and title); otherwise the picker asks as usual, the toggle on,
//! and what's picked takes the grant's place (or, with the toggle off,
//! the grant is forgotten). Never while the screen is locked.
//!
//! Grants an app asked to keep until revoked (`persist_mode` 2) are kept in
//! `$XDG_STATE_HOME/pila/screen-shares.json` (read by the portal's thread,
//! written by a thread of its own); those for as long as the app runs (1),
//! in memory alone. `pila msg screen-shares` lists them and
//! `pila msg forget-screen-share` forgets one, or all (`ipc.rs`).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use smithay::{
    desktop::Window,
    utils::{IsAlive, Rectangle},
};
use tracing::{info, warn};

use crate::{capture::Target, foreign_toplevel, Pila};

/// `persist_mode`: not remembered, for as long as the app runs, until
/// revoked.
pub const PERSIST_NONE: u32 = 0;
pub const PERSIST_TRANSIENT: u32 = 1;
pub const PERSIST_PERSISTENT: u32 = 2;

/// What a grant shares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Shared {
    /// An output, by name.
    Output { output: String },
    /// A region of an output, local to it, in logical pixels.
    Region {
        output: String,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
    /// A window, by its app id and title.
    Window { app_id: String, title: String },
}

/// A share remembered for an app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    /// The app, as the portal named it (empty for one it doesn't name).
    pub app_id: String,
    /// The app as the picker named it.
    pub app: String,
    pub shared: Shared,
    /// Kept until revoked (in the file), or for as long as the app runs.
    #[serde(skip, default = "kept")]
    pub persistent: bool,
}

fn kept() -> bool {
    true
}

/// The grants, by id.
pub type Store = HashMap<String, Grant>;

/// `$XDG_STATE_HOME/pila/screen-shares.json`.
pub fn store_path() -> Option<std::path::PathBuf> {
    crate::state_file::path("screen-shares.json")
}

/// The grants kept, read from their file (none or unreadable: none). Reads
/// a file: never on the event loop (the portal's thread reads it).
pub fn read_store() -> Store {
    let Some(path) = store_path() else {
        return Store::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Store::new();
    };
    parse_store(&text).unwrap_or_else(|err| {
        warn!(%err, path = %path.display(), "can't read the screen shares remembered");
        Store::new()
    })
}

/// The grants in `screen-shares.json`'s text, which anything may have
/// written.
pub fn parse_store(text: &str) -> serde_json::Result<Store> {
    let mut file: HashMap<String, Store> = serde_json::from_str(text)?;
    Ok(file.remove("grants").unwrap_or_default())
}

/// Write the grants kept until revoked to their file, on a thread, whole or
/// not at all.
fn save_store(store: &Store) {
    let Some(path) = store_path() else {
        return;
    };
    let kept: Store = store
        .iter()
        .filter(|(_, grant)| grant.persistent)
        .map(|(id, grant)| (id.clone(), grant.clone()))
        .collect();
    crate::state_file::save(
        "pila-screen-shares",
        path,
        "the screen shares remembered",
        move || {
            let file = HashMap::from([("grants".to_string(), kept)]);
            serde_json::to_string_pretty(&file).map_err(std::io::Error::other)
        },
    );
}

/// A share's grant, as `Start` answers it: its id, the `persist_mode`
/// answered, and the grant to keep once the share started (`None`: it's
/// kept already, restored).
#[derive(Debug, Clone)]
pub struct Remembered {
    pub id: String,
    pub mode: u32,
    pub grant: Option<Grant>,
}

/// The grants (`ScreenCasts::grants`): `None` until the portal's thread
/// read the file.
#[derive(Default)]
pub struct Grants {
    store: Option<Store>,
}

impl Pila {
    /// The grants kept were read: added to any made meanwhile.
    pub(crate) fn screen_grants_store(&mut self, store: Store) {
        let grants = self.screencast.grants.store.get_or_insert_with(Store::new);
        for (id, grant) in store {
            grants.entry(id).or_insert(grant);
        }
    }

    /// The grant `id`, if it's `app_id`'s.
    pub(crate) fn screen_grant(&self, id: &str, app_id: &str) -> Option<&Grant> {
        let grant = self.screencast.grants.store.as_ref()?.get(id)?;
        (grant.app_id == app_id).then_some(grant)
    }

    /// What `share` restores, if it may: the grant it names (its app's),
    /// what that shares there now, and how it's remembered. Never while
    /// locked: the picker cancels the request then.
    pub(crate) fn restored_share(
        &mut self,
        share: &crate::screencast::Share,
    ) -> Option<(Target, Option<Remembered>)> {
        if self.is_locked() {
            return None;
        }
        let id = share.restore.as_deref()?;
        let grant = self.screen_grant(id, &share.app_id)?;
        let target = self.grant_target(&grant.shared)?;
        let kept = if grant.persistent {
            PERSIST_PERSISTENT
        } else {
            PERSIST_TRANSIENT
        };
        let id = id.to_string();
        info!(app = grant.app, ?grant.shared, "portal: sharing as remembered");
        // Not to be remembered any more: the grant goes.
        let remembered = match share.persist.min(kept) {
            PERSIST_NONE => {
                self.forget_screen_grant(&id);
                None
            }
            mode => Some(Remembered {
                id,
                mode,
                grant: None,
            }),
        };
        Some((target, remembered))
    }

    /// What `shared` is now, if it's there.
    fn grant_target(&self, shared: &Shared) -> Option<Target> {
        let output = |name: &str| self.space.outputs().find(|o| o.name() == name).cloned();
        let target = match shared {
            Shared::Output { output: name } => Target::Output(output(name)?.downgrade()),
            Shared::Region {
                output: name,
                x,
                y,
                width,
                height,
            } => Target::Region(
                output(name)?.downgrade(),
                Rectangle::new((*x, *y).into(), (*width, *height).into()),
            ),
            Shared::Window { app_id, title } => {
                let window = self
                    .space
                    .elements()
                    .chain(self.off_space_windows())
                    .find(|w| {
                        shareable(w)
                            && foreign_toplevel::app_id(w) == *app_id
                            && foreign_toplevel::title(w) == *title
                    })?;
                Target::Toplevel(foreign_toplevel::identifier(window)?)
            }
        };
        // A region off its output, an output unmapped: not there.
        let there = match &target {
            Target::Output(_) | Target::Region(..) => self.target_area(&target).is_some(),
            _ => true,
        };
        there.then_some(target)
    }

    /// The grant for `share` picked as `target`, the Remember toggle on:
    /// the one it asked to restore, else a new one.
    pub(crate) fn new_screen_grant(
        &self,
        share: &crate::screencast::Share,
        app: Option<&str>,
        target: &Target,
    ) -> Option<Remembered> {
        let shared = match target {
            Target::Output(output) => Shared::Output {
                output: output.upgrade()?.name(),
            },
            Target::Region(output, region) => Shared::Region {
                output: output.upgrade()?.name(),
                x: region.loc.x,
                y: region.loc.y,
                width: region.size.w,
                height: region.size.h,
            },
            Target::Toplevel(identifier) => {
                let window = self.window_for_identifier(identifier)?;
                Shared::Window {
                    app_id: foreign_toplevel::app_id(&window),
                    title: foreign_toplevel::title(&window),
                }
            }
            Target::Gone => return None,
        };
        let id = share
            .restore
            .clone()
            .filter(|id| self.screen_grant(id, &share.app_id).is_some())
            .or_else(crate::state_file::random_id)?;
        let mode = share.persist.min(PERSIST_PERSISTENT);
        Some(Remembered {
            id,
            mode,
            grant: Some(Grant {
                app_id: share.app_id.clone(),
                app: app.unwrap_or(&share.app_id).to_string(),
                shared,
                persistent: mode == PERSIST_PERSISTENT,
            }),
        })
    }

    /// A share started: its grant, new, is kept.
    pub(crate) fn keep_screen_grant(&mut self, id: String, grant: Grant) {
        info!(app = grant.app, ?grant.shared, "portal: share remembered");
        let persistent = grant.persistent;
        let store = self.screencast.grants.store.get_or_insert_with(Store::new);
        let replaced = store.insert(id, grant);
        if persistent || replaced.is_some_and(|old| old.persistent) {
            save_store(store);
        }
    }

    /// Whether the remembered shares are read (when the portal starts
    /// serving): forgetting waits for it.
    pub(crate) fn screen_grants_read(&self) -> bool {
        self.screencast.grants.store.is_some()
    }

    /// Forget grant `id`; whether there was one.
    pub(crate) fn forget_screen_grant(&mut self, id: &str) -> bool {
        let Some(store) = self.screencast.grants.store.as_mut() else {
            return false;
        };
        let Some(grant) = store.remove(id) else {
            return false;
        };
        info!(app = grant.app, ?grant.shared, "portal: remembered share forgotten");
        if grant.persistent {
            save_store(store);
        }
        true
    }

    /// Forget every grant; how many there were.
    pub(crate) fn forget_screen_grants(&mut self) -> usize {
        let Some(store) = self.screencast.grants.store.as_mut() else {
            return 0;
        };
        let count = store.len();
        if count > 0 {
            info!(count, "portal: remembered shares forgotten");
            store.clear();
            save_store(store);
        }
        count
    }

    /// The grants, for `pila msg screen-shares`: by app, then id.
    pub(crate) fn ipc_screen_shares(&self) -> Vec<serde_json::Value> {
        let mut grants: Vec<_> = self.screencast.grants.store.iter().flatten().collect();
        grants.sort_by(|(a_id, a), (b_id, b)| (&a.app, a_id).cmp(&(&b.app, b_id)));
        grants
            .into_iter()
            .map(|(id, grant)| {
                serde_json::json!({
                    "id": id,
                    "app": grant.app,
                    "app_id": grant.app_id,
                    "shares": grant.shared,
                    "persistent": grant.persistent,
                })
            })
            .collect()
    }
}

/// Whether `window` can be shared: alive, named to taskbars, with a size
/// (as the picker offers them).
pub(crate) fn shareable(window: &Window) -> bool {
    window.alive()
        && foreign_toplevel::identifier(window).is_some()
        && !window.geometry().size.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_store_reads_back_what_it_writes() {
        let store = Store::from([(
            "0123".to_string(),
            Grant {
                app_id: "org.example.Recorder".into(),
                app: "Recorder".into(),
                shared: Shared::Region {
                    output: "DP-1".into(),
                    x: 10,
                    y: 20,
                    width: 300,
                    height: 150,
                },
                persistent: true,
            },
        )]);
        let file = HashMap::from([("grants".to_string(), store.clone())]);
        let text = serde_json::to_string(&file).unwrap();
        assert!(text.contains(r#""kind":"region""#), "{text}");
        assert_eq!(parse_store(&text).unwrap(), store);
    }

    #[test]
    fn a_store_written_by_anything_else_is_refused_whole() {
        assert!(parse_store(r#"{"grants": {"a": {"app_id": 1}}}"#).is_err());
        assert!(parse_store("not json").is_err());
        assert_eq!(parse_store("{}").unwrap(), Store::new());
    }
}
