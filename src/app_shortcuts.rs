//! Apps' global shortcuts, for the desktop portal's GlobalShortcuts that Pila
//! serves (`portal.rs`): push-to-talk in a voice chat, a recorder's start and
//! stop, whichever window has the keyboard.
//!
//! An app's session binds its shortcuts (`BindShortcuts`: an id, what it does
//! and the keys it would like, as the shortcuts XDG specification writes
//! them, `parse_trigger`). Those its keys were confirmed for before, by app,
//! are bound at once; otherwise a panel lists them, each with the keys it
//! asked for (or none, or none where they're Pila's or another app's): the
//! user picks one with the arrows and presses the keys for it, Backspace
//! clears it, Save (Enter) binds them all, Cancel (Escape) none. What's
//! saved is remembered by app (`Store`, in
//! `$XDG_STATE_HOME/pila/app-shortcuts.json`, read by the portal's thread
//! and written by a thread of its own), and `ConfigureShortcuts` shows the
//! panel again.
//!
//! A bound shortcut's press goes to no window: the app's session gets
//! `Activated`, and `Deactivated` at its release (`emit`, on a thread: the
//! bus may be slow). Pila's own bindings come first, and nothing fires while
//! the screen is locked, a client inhibits shortcuts or a panel of Pila's is
//! modal. The panel is modal while it's shown: every key and press is its,
//! and every output is dimmed under it.

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use smithay::{
    backend::renderer::gles::GlesRenderer,
    input::keyboard::{xkb, Keysym, ModifiersState},
    utils::{Logical, Point, Rectangle, Scale, Size},
};
use tracing::{debug, info, warn};

use crate::{
    config::{Colors, KeyCombo, Mods, Trigger},
    panel::{self, Align, Item, ModalBuffers, TextKey},
    portal::{Refused, Reply},
    render::PilaRenderElement,
    Pila,
};

/// The panel's width, where its rows start, a row's height, how many show
/// at once (the selected one's kept in view), and where what a row's keys
/// are goes.
const WIDTH: i32 = 720;
const ROWS_AT: i32 = 100;
const ROW: i32 = 36;
const ROWS_SHOWN: usize = 8;
const KEYS_AT: i32 = 420;
/// The footer: its buttons, as the share picker's.
const BUTTON_HEIGHT: i32 = 36;
const SAVE_WIDTH: i32 = 120;
const CANCEL_WIDTH: i32 = 110;
const BUTTON_GAP: i32 = 12;

/// A shortcut an app asks for: its id, what it does, the keys it would like
/// (the shortcuts XDG specification's text).
#[derive(Debug, Clone)]
pub struct Asked {
    pub id: String,
    pub description: String,
    pub preferred: Option<String>,
}

/// A shortcut as it's bound: its id, what it does, its keys (none: unset).
#[derive(Debug, Clone, PartialEq)]
pub struct Bound {
    pub id: String,
    pub description: String,
    pub keys: Option<KeyCombo>,
}

impl Bound {
    /// How the keys read, for the app to show (`trigger_description`).
    pub fn trigger_description(&self) -> Option<String> {
        self.keys.as_ref().map(crate::shortcuts_view::combo_label)
    }
}

/// The keys of a shortcut as the shortcuts XDG specification writes them
/// (`CTRL+SHIFT+F12`: modifiers SHIFT, CTRL, ALT, LOGO, then an xkb key
/// name), or `None` for what Pila can't bind (NUM, an unknown key, none).
pub fn parse_trigger(text: &str) -> Option<KeyCombo> {
    let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
    let key = parts.pop().filter(|key| !key.is_empty())?;
    let mut mods = Mods::default();
    for part in parts {
        match part.to_ascii_uppercase().as_str() {
            "SHIFT" => mods.shift = true,
            "CTRL" | "CONTROL" => mods.ctrl = true,
            "ALT" => mods.alt = true,
            "LOGO" | "SUPER" => mods.logo = true,
            _ => return None,
        }
    }
    let sym = crate::config::key_by_name(key);
    if sym.raw() == 0 || sym.is_modifier_key() {
        return None;
    }
    Some(KeyCombo {
        mods,
        trigger: Trigger::Key(sym),
    })
}

/// `combo` as the shortcuts XDG specification writes it (`parse_trigger`).
pub fn format_trigger(combo: &KeyCombo) -> Option<String> {
    let Trigger::Key(sym) = combo.trigger else {
        return None;
    };
    let mut text = String::new();
    for (on, name) in [
        (combo.mods.shift, "SHIFT"),
        (combo.mods.ctrl, "CTRL"),
        (combo.mods.alt, "ALT"),
        (combo.mods.logo, "LOGO"),
    ] {
        if on {
            text.push_str(name);
            text.push('+');
        }
    }
    text.push_str(&xkb::keysym_get_name(sym));
    Some(text)
}

/// Whether `combo` may be an app's global shortcut: with Ctrl, Alt or Super
/// (typing never is), or a key that types nothing (F1–F35, Pause, Print, a
/// media key ...).
pub fn assignable(combo: &KeyCombo) -> bool {
    let Trigger::Key(sym) = combo.trigger else {
        return false;
    };
    if sym.is_modifier_key() {
        return false;
    }
    let mods = combo.mods;
    if mods.ctrl || mods.alt || mods.logo {
        return true;
    }
    let raw = sym.raw();
    (Keysym::F1.raw()..=Keysym::F35.raw()).contains(&raw)
        || matches!(sym, Keysym::Pause | Keysym::Print | Keysym::Scroll_Lock | Keysym::Insert)
        // XF86's keys: media, volume, launch ...
        || (0x1008_ff00..=0x1008_ffff).contains(&raw)
}

/// Keys read back from the store: only those an app could be given.
fn remembered_keys(text: &str) -> Option<KeyCombo> {
    parse_trigger(text).filter(assignable)
}

/// The keys confirmed for apps' shortcuts, by app, then by shortcut id: the
/// trigger text, empty for one left unset.
pub type Store = HashMap<String, HashMap<String, String>>;

/// `$XDG_STATE_HOME/pila/app-shortcuts.json`.
pub fn store_path() -> Option<std::path::PathBuf> {
    crate::state_file::path("app-shortcuts.json")
}

/// The store, read from its file (none or unreadable: empty). Reads a file:
/// never on the event loop (the portal's thread reads it).
pub fn read_store() -> Store {
    let Some(path) = store_path() else {
        return Store::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Store::new();
    };
    parse_store(&text).unwrap_or_else(|err| {
        warn!(%err, path = %path.display(), "can't read the apps' shortcuts");
        Store::new()
    })
}

/// The store in `app-shortcuts.json`'s text, which anything may have
/// written (the `app_shortcuts` fuzz target reads it as this does).
pub fn parse_store(text: &str) -> serde_json::Result<Store> {
    let mut file: HashMap<String, Store> = serde_json::from_str(text)?;
    Ok(file.remove("apps").unwrap_or_default())
}

/// Write `store` to its file, on a thread, whole or not at all.
fn save_store(store: Store) {
    let Some(path) = store_path() else {
        return;
    };
    crate::state_file::save(
        "pila-app-shortcuts",
        path,
        "the apps' shortcuts",
        move || {
            let file = HashMap::from([("apps".to_string(), store)]);
            serde_json::to_string_pretty(&file).map_err(std::io::Error::other)
        },
    );
}

/// A signal to the app's session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Activated,
    Deactivated,
}

/// An app's session (`CreateSession`).
struct Session {
    path: String,
    /// The app, as the store knows it: its app id, else the app id of the
    /// window it binds from.
    app: String,
    shortcuts: Vec<Bound>,
}

/// Apps' global shortcuts (`Pila::app_shortcuts`).
#[derive(Default)]
pub struct AppShortcuts {
    sessions: Vec<Session>,
    /// What was confirmed before (`None` until the portal's thread read it).
    store: Option<Store>,
    /// Keys held down that activated a shortcut: its session and id, for
    /// the release.
    held: HashMap<u32, (String, String)>,
    /// The panel, while it asks.
    panel: Option<Panel>,
    pending: Option<TextKey>,
    buffers: Rc<RefCell<ModalBuffers>>,
    /// Where signals go to be emitted, on a thread of their own.
    signals: Option<std::sync::mpsc::Sender<(String, String, Signal, u64)>>,
}

/// The panel while it asks.
struct Panel {
    session: String,
    /// Who asks, as the panel names it.
    app: Option<String>,
    rows: Vec<Bound>,
    selected: usize,
    /// Why the last keys pressed weren't taken.
    refused: Option<String>,
    /// The output it's on, by name.
    output: String,
    /// `BindShortcuts`' answer, or `None` for `ConfigureShortcuts` (the app
    /// hears of a change through `ShortcutsChanged`).
    reply: Option<Reply<Result<Vec<Bound>, Refused>>>,
}

/// Where the panel and its parts are, global.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub panel: Rectangle<i32, Logical>,
    /// The first row shown, and each row shown's place.
    pub first: usize,
    pub rows: Vec<Rectangle<i32, Logical>>,
    pub save: Rectangle<i32, Logical>,
    pub cancel: Rectangle<i32, Logical>,
}

/// The panel for `rows` shortcuts, `selected` in view, in the middle of
/// `area`.
pub fn layout(area: Rectangle<i32, Logical>, rows: usize, selected: usize) -> Layout {
    let shown = rows.clamp(1, ROWS_SHOWN);
    let first = crate::switcher_view::first_shown(rows, ROWS_SHOWN, Some(selected));
    let height = ROWS_AT + shown as i32 * ROW + panel::FOOTER_GAP - (ROW - panel::CAP_HEIGHT)
        + BUTTON_HEIGHT
        + panel::PADDING;
    let panel = panel::centered(area, Size::from((WIDTH, height)));
    let rows = (0..rows.saturating_sub(first).min(ROWS_SHOWN))
        .map(|i| {
            Rectangle::new(
                panel.loc + Point::from((panel::PADDING - 10, ROWS_AT + i as i32 * ROW - 7)),
                Size::from((WIDTH - 2 * panel::PADDING + 20, ROW)),
            )
        })
        .collect();
    let (save, cancel) =
        panel::footer_buttons(panel, (SAVE_WIDTH, CANCEL_WIDTH), BUTTON_HEIGHT, BUTTON_GAP);
    Layout {
        panel,
        first,
        rows,
        save,
        cancel,
    }
}

/// What `Pila::scene` gives the renderer of the panel.
#[derive(Clone)]
pub struct ScenePanel {
    pub output: String,
    pub layout: Layout,
    pub selected: usize,
    pub rounding: panel::Shape,
    buffers: Rc<RefCell<ModalBuffers>>,
}

impl Pila {
    /// The portal's thread read what was confirmed before.
    pub(crate) fn app_shortcuts_store(&mut self, store: Store) {
        self.app_shortcuts.store = Some(store);
    }

    /// An app's session began.
    pub(crate) fn app_shortcuts_session(&mut self, session: String, app_id: String) {
        debug!(session, app_id, "portal: shortcuts session");
        self.app_shortcuts.sessions.push(Session {
            path: session,
            app: app_id,
            shortcuts: Vec::new(),
        });
    }

    /// The app (or xdg-desktop-portal) closed `session`: its shortcuts go.
    pub(crate) fn app_shortcuts_closed(&mut self, session: &str) {
        self.app_shortcuts.sessions.retain(|s| s.path != session);
        self.app_shortcuts.held.retain(|_, (s, _)| s != session);
        if self
            .app_shortcuts
            .panel
            .as_ref()
            .is_some_and(|panel| panel.session == session)
        {
            self.cancel_app_shortcuts();
        }
    }

    /// `session` binds `asked`: at once where its app's keys for them were
    /// confirmed before, else once the user saves the panel.
    pub(crate) fn bind_app_shortcuts(
        &mut self,
        session: String,
        app: Option<String>,
        parent: Option<crate::portal::ParentWindow>,
        asked: Vec<Asked>,
        reply: Reply<Result<Vec<Bound>, Refused>>,
    ) {
        let Some(index) = self
            .app_shortcuts
            .sessions
            .iter()
            .position(|s| s.path == session)
        else {
            return reply.send(Err(Refused::Failed));
        };
        // An app without an app id is known by the window it binds from.
        if self.app_shortcuts.sessions[index].app.is_empty() {
            let window = parent
                .as_ref()
                .and_then(|parent| self.window_of_parent(parent));
            if let Some(window) = window {
                self.app_shortcuts.sessions[index].app = crate::foreign_toplevel::app_id(&window);
            }
        }
        let app_key = self.app_shortcuts.sessions[index].app.clone();
        // The file may have been written by anything: a key no app could be
        // given isn't remembered, and the app is asked again.
        let remembered: Option<HashMap<String, String>> = (!app_key.is_empty())
            .then(|| self.app_shortcuts.store.as_ref()?.get(&app_key))
            .flatten()
            .map(|r| {
                r.iter()
                    .filter(|(_, text)| text.is_empty() || remembered_keys(text).is_some())
                    .map(|(id, text)| (id.clone(), text.clone()))
                    .collect()
            });
        let remembered = remembered.as_ref();
        let rows: Vec<Bound> = asked
            .iter()
            .map(|asked| {
                let keys = match remembered.and_then(|r| r.get(&asked.id)) {
                    Some(text) => remembered_keys(text),
                    None => asked
                        .preferred
                        .as_deref()
                        .and_then(parse_trigger)
                        .filter(assignable),
                };
                Bound {
                    id: asked.id.clone(),
                    description: asked.description.clone(),
                    keys,
                }
            })
            .collect();
        let known = remembered.is_some_and(|r| asked.iter().all(|a| r.contains_key(&a.id)));
        if known && !self.is_locked() {
            let rows = self.free_keys(rows, &session);
            info!(
                app = app_key,
                count = rows.len(),
                "portal: shortcuts bound as before"
            );
            self.app_shortcuts.sessions[index].shortcuts = rows.clone();
            return reply.send(Ok(rows));
        }
        self.ask_app_shortcuts(
            session,
            app.or(Some(app_key).filter(|a| !a.is_empty())),
            rows,
            Some(reply),
        );
    }

    /// The shortcuts `session` has bound (or would have, as confirmed
    /// before), for `ListShortcuts`.
    pub(crate) fn list_app_shortcuts(&self, session: &str) -> Vec<Bound> {
        let Some(found) = self
            .app_shortcuts
            .sessions
            .iter()
            .find(|s| s.path == session)
        else {
            return Vec::new();
        };
        if !found.shortcuts.is_empty() {
            return found.shortcuts.clone();
        }
        let Some(remembered) = self
            .app_shortcuts
            .store
            .as_ref()
            .and_then(|s| s.get(&found.app))
        else {
            return Vec::new();
        };
        remembered
            .iter()
            .map(|(id, text)| Bound {
                id: id.clone(),
                description: String::new(),
                keys: remembered_keys(text),
            })
            .collect()
    }

    /// `ConfigureShortcuts`: the panel again, for `session`'s shortcuts.
    pub(crate) fn configure_app_shortcuts(&mut self, session: String) {
        let Some(found) = self
            .app_shortcuts
            .sessions
            .iter()
            .find(|s| s.path == session)
        else {
            return;
        };
        if found.shortcuts.is_empty() {
            return;
        }
        let rows = found.shortcuts.clone();
        let app = Some(found.app.clone()).filter(|a| !a.is_empty());
        self.ask_app_shortcuts(session, app, rows, None);
    }

    /// `rows` with the keys Pila's bindings or another session's shortcuts
    /// have taken away (left unset).
    fn free_keys(&self, mut rows: Vec<Bound>, session: &str) -> Vec<Bound> {
        for row in &mut rows {
            if row
                .keys
                .is_some_and(|keys| self.keys_taken(&keys, session).is_some())
            {
                row.keys = None;
            }
        }
        rows
    }

    /// Who has `keys` already: Pila, or another app's session.
    fn keys_taken(&self, keys: &KeyCombo, session: &str) -> Option<String> {
        if self.config.bindings.iter().any(|b| b.combo == *keys) {
            return Some("Pila".to_string());
        }
        self.app_shortcuts
            .sessions
            .iter()
            .filter(|s| s.path != session)
            .find(|s| s.shortcuts.iter().any(|b| b.keys == Some(*keys)))
            .map(|s| {
                if s.app.is_empty() {
                    "another app".to_string()
                } else {
                    s.app.clone()
                }
            })
    }

    /// Show the panel for `session`'s `rows` (one at a time: another asking
    /// is refused; never while locked).
    fn ask_app_shortcuts(
        &mut self,
        session: String,
        app: Option<String>,
        rows: Vec<Bound>,
        reply: Option<Reply<Result<Vec<Bound>, Refused>>>,
    ) {
        if self.is_locked() || self.modal_panel_open() || rows.is_empty() {
            if let Some(reply) = reply {
                reply.send(Err(Refused::Cancelled));
            }
            return;
        }
        let rows = self.free_keys(rows, &session);
        let Some(output) = self.output_at(self.pointer_location()) else {
            if let Some(reply) = reply {
                reply.send(Err(Refused::Cancelled));
            }
            return;
        };
        self.close_transient_panels();
        info!(
            app = app.as_deref().unwrap_or("?"),
            "portal: asking for shortcuts' keys"
        );
        self.app_shortcuts.panel = Some(Panel {
            session,
            app,
            rows,
            selected: 0,
            refused: None,
            output: output.name(),
            reply,
        });
        self.app_shortcuts.pending = None;
        *self.app_shortcuts.buffers.borrow_mut() = ModalBuffers::default();
        self.queue_redraw();
        self.draw_app_shortcuts_text();
    }

    pub fn app_shortcuts_open(&self) -> bool {
        self.app_shortcuts.panel.is_some()
    }

    fn take_app_shortcuts_panel(&mut self) -> Option<Panel> {
        let panel = self.app_shortcuts.panel.take()?;
        self.app_shortcuts.pending = None;
        *self.app_shortcuts.buffers.borrow_mut() = ModalBuffers::default();
        self.queue_redraw();
        // A share or capture permission asked meanwhile waited for it.
        self.show_waiting_panels();
        Some(panel)
    }

    /// Cancel (Escape), the session closed or the screen locked: nothing
    /// is bound.
    pub fn cancel_app_shortcuts(&mut self) {
        let Some(panel) = self.take_app_shortcuts_panel() else {
            return;
        };
        info!("portal: shortcuts not bound");
        if let Some(reply) = panel.reply {
            reply.send(Err(Refused::Cancelled));
        }
    }

    /// Save (Enter): the keys are bound, and remembered for the app.
    fn save_app_shortcuts(&mut self) {
        let Some(panel) = self.take_app_shortcuts_panel() else {
            return;
        };
        let Some(session) = self
            .app_shortcuts
            .sessions
            .iter_mut()
            .find(|s| s.path == panel.session)
        else {
            if let Some(reply) = panel.reply {
                reply.send(Err(Refused::Failed));
            }
            return;
        };
        session.shortcuts = panel.rows.clone();
        let app = session.app.clone();
        info!(app, count = panel.rows.len(), "portal: shortcuts bound");
        if !app.is_empty() {
            let store = self.app_shortcuts.store.get_or_insert_with(Store::new);
            let entry = store.entry(app).or_default();
            for row in &panel.rows {
                let text = row
                    .keys
                    .as_ref()
                    .and_then(format_trigger)
                    .unwrap_or_default();
                entry.insert(row.id.clone(), text);
            }
            save_store(store.clone());
        }
        match panel.reply {
            Some(reply) => reply.send(Ok(panel.rows)),
            None => {
                if let Some(connection) = &self.screencast.portal {
                    crate::portal::shortcuts_changed(connection, panel.session, panel.rows);
                }
            }
        }
    }

    /// A key while the panel asks (it takes them all): the arrows pick a
    /// row, Backspace clears it, Enter saves, Escape cancels; other keys
    /// (with Ctrl, Alt or Super, or a key that types nothing) become the
    /// selected row's.
    pub fn app_shortcuts_key(&mut self, sym: Keysym, mods: &ModifiersState) {
        let Some(panel) = self.app_shortcuts.panel.as_mut() else {
            return;
        };
        let plain = !mods.ctrl && !mods.alt && !mods.logo && !mods.shift;
        let count = panel.rows.len();
        match sym {
            Keysym::Up if plain => panel.selected = (panel.selected + count - 1) % count,
            Keysym::Down | Keysym::Tab if plain => panel.selected = (panel.selected + 1) % count,
            Keysym::Return | Keysym::KP_Enter if plain => return self.save_app_shortcuts(),
            Keysym::Escape if plain => return self.cancel_app_shortcuts(),
            Keysym::BackSpace | Keysym::Delete if plain => {
                panel.rows[panel.selected].keys = None;
                panel.refused = None;
            }
            _ => {
                let keys = KeyCombo {
                    mods: mods.into(),
                    trigger: Trigger::Key(sym),
                };
                if !assignable(&keys) {
                    return;
                }
                let session = panel.session.clone();
                let taken = self.keys_taken(&keys, &session);
                let Some(panel) = self.app_shortcuts.panel.as_mut() else {
                    return;
                };
                match taken {
                    Some(who) => {
                        let label = crate::shortcuts_view::combo_label(&keys);
                        panel.refused = Some(format!("{label} is {who}'s"));
                    }
                    None => {
                        // A row that had them gives them up.
                        for row in &mut panel.rows {
                            if row.keys == Some(keys) {
                                row.keys = None;
                            }
                        }
                        panel.rows[panel.selected].keys = Some(keys);
                        panel.refused = None;
                    }
                }
            }
        }
        self.queue_redraw();
        self.draw_app_shortcuts_text();
    }

    /// A press at `location` while the panel asks (it takes them all): its
    /// buttons and rows act, elsewhere nothing.
    pub fn app_shortcuts_press(&mut self, location: Point<f64, Logical>) {
        let Some(view) = self.scene_app_shortcuts() else {
            return;
        };
        let layout = &view.layout;
        if layout.save.to_f64().contains(location) {
            self.save_app_shortcuts();
        } else if layout.cancel.to_f64().contains(location) {
            self.cancel_app_shortcuts();
        } else if let Some(i) = layout
            .rows
            .iter()
            .position(|r| r.to_f64().contains(location))
        {
            if let Some(panel) = self.app_shortcuts.panel.as_mut() {
                panel.selected = layout.first + i;
            }
            self.queue_redraw();
            self.draw_app_shortcuts_text();
        }
    }

    /// The shortcut `sym` with exactly `mods` pressed activates, if one does
    /// and may now: its session and id.
    pub(crate) fn app_shortcut_for(
        &self,
        sym: Keysym,
        mods: &ModifiersState,
    ) -> Option<(String, String)> {
        if self.is_locked() || self.modal_panel_open() {
            return None;
        }
        let keys = KeyCombo {
            mods: mods.into(),
            trigger: Trigger::Key(sym),
        };
        self.app_shortcuts.sessions.iter().find_map(|session| {
            session
                .shortcuts
                .iter()
                .find(|b| b.keys == Some(keys))
                .map(|b| (session.path.clone(), b.id.clone()))
        })
    }

    /// The key `keycode` pressed `shortcut` (session, id): `Activated`.
    pub(crate) fn app_shortcut_pressed(&mut self, keycode: u32, shortcut: (String, String)) {
        self.emit_app_shortcut(&shortcut, Signal::Activated);
        self.app_shortcuts.held.insert(keycode, shortcut);
    }

    /// The key `keycode` was released: `Deactivated`, if it held a
    /// shortcut. Whether it did.
    pub(crate) fn app_shortcut_released(&mut self, keycode: u32) -> bool {
        let Some(shortcut) = self.app_shortcuts.held.remove(&keycode) else {
            return false;
        };
        self.emit_app_shortcut(&shortcut, Signal::Deactivated);
        true
    }

    fn emit_app_shortcut(&mut self, (session, id): &(String, String), signal: Signal) {
        let Some(connection) = self.screencast.portal.clone() else {
            return;
        };
        debug!(session, id, ?signal, "portal: shortcut");
        let timestamp = self.clock.now();
        let millis = Into::<std::time::Duration>::into(timestamp).as_millis() as u64;
        let sender = self.app_shortcuts.signals.get_or_insert_with(|| {
            let (sender, receiver) = std::sync::mpsc::channel::<(String, String, Signal, u64)>();
            let spawned = std::thread::Builder::new()
                .name("pila-shortcut-signals".into())
                .spawn(move || {
                    for (session, id, signal, millis) in receiver {
                        crate::portal::emit_shortcut(&connection, &session, &id, signal, millis);
                    }
                });
            if let Err(err) = spawned {
                warn!(?err, "can't send apps' shortcuts");
            }
            sender
        });
        let _ = sender.send((session.clone(), id.clone(), signal, millis));
    }

    /// The panel for `Pila::scene`, while it asks.
    pub(crate) fn scene_app_shortcuts(&self) -> Option<ScenePanel> {
        let panel = self.app_shortcuts.panel.as_ref()?;
        let (output, area) = self.output_named_or_first(Some(&panel.output))?;
        Some(ScenePanel {
            output: output.name(),
            layout: layout(area, panel.rows.len(), panel.selected),
            selected: panel.selected,
            rounding: panel::rounding(&self.config),
            buffers: self.app_shortcuts.buffers.clone(),
        })
    }

    /// Draw the panel's texts on a thread if what they say changed.
    pub(crate) fn draw_app_shortcuts_text(&mut self) {
        let (Some(view), Some(panel)) = (
            self.scene_app_shortcuts(),
            self.app_shortcuts.panel.as_ref(),
        ) else {
            return;
        };
        let scale = self.panel_text_scale(&view.output);
        let key = TextKey {
            items: items(panel, &view.layout, &self.config.colors),
            caps: panel::key_caps(&self.config.colors),
            size: (view.layout.panel.size.w, view.layout.panel.size.h),
            scale,
        };
        let drawn = view.buffers.borrow().text.as_ref().map(|t| &t.key) == Some(&key);
        self.draw_panel_text(
            key,
            drawn,
            "pila-app-shortcuts-text",
            |state| &mut state.app_shortcuts.pending,
            Self::draw_app_shortcuts_text,
            |state, text| {
                state.app_shortcuts.buffers.borrow_mut().text = Some(text);
                state.queue_redraw();
            },
        );
    }

    /// For the state dump: the sessions and their shortcuts, and the panel.
    pub fn app_shortcuts_json(&self) -> serde_json::Value {
        let shortcut = |b: &Bound| {
            serde_json::json!({
                "id": b.id,
                "keys": b.keys.as_ref().map(crate::shortcuts_view::combo_label),
            })
        };
        let panel = self
            .app_shortcuts
            .panel
            .as_ref()
            .zip(self.scene_app_shortcuts())
            .map(|(panel, view)| {
                let rect = crate::introspect::rect;
                serde_json::json!({
                    "app": panel.app,
                    "rows": panel.rows.iter().map(shortcut).collect::<Vec<_>>(),
                    "selected": panel.selected,
                    "refused": panel.refused,
                    "panel": rect(view.layout.panel),
                    "save": rect(view.layout.save),
                    "cancel": rect(view.layout.cancel),
                    "text": view.buffers.borrow().text.is_some(),
                })
            });
        serde_json::json!({
            "sessions": self.app_shortcuts.sessions.iter().map(|s| serde_json::json!({
                "app": s.app,
                "shortcuts": s.shortcuts.iter().map(shortcut).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "panel": panel,
            "loaded": self.app_shortcuts.store.is_some(),
        })
    }
}

/// The panel's texts, at places from its top-left.
fn items(panel: &Panel, layout: &Layout, colors: &Colors) -> Vec<Item> {
    let (text, dimmed, _) = panel::text_colors(colors);
    let width = layout.panel.size.w;
    let title = match &panel.app {
        Some(app) => format!("Shortcuts for {app}"),
        None => "An app asks for shortcuts".to_string(),
    };
    let note = panel.refused.clone().unwrap_or_else(|| {
        "They work whichever window has the keyboard. Pick one and press its keys.".into()
    });
    let mut items = vec![
        panel::title(&title, width, colors),
        panel::note(&note, 56, width, colors),
    ];
    for (i, row) in panel
        .rows
        .iter()
        .enumerate()
        .skip(layout.first)
        .take(ROWS_SHOWN)
    {
        let y = ROWS_AT + (i - layout.first) as i32 * ROW;
        items.push(Item::Text {
            text: row.description.clone(),
            at: (panel::PADDING, y),
            w: KEYS_AT - panel::PADDING - 16,
            h: panel::CAP_HEIGHT,
            font: panel::BODY,
            align: Align::Left,
            color: text,
        });
        match &row.keys {
            Some(keys) => items.push(Item::Keys {
                caps: crate::shortcuts_view::combo_caps(keys),
                at: (KEYS_AT, y),
                w: width - KEYS_AT - panel::PADDING,
                size: panel::KEY,
            }),
            None => items.push(Item::Text {
                text: "Not set".into(),
                at: (KEYS_AT, y),
                w: width - KEYS_AT - panel::PADDING,
                h: panel::CAP_HEIGHT,
                font: panel::NOTE,
                align: Align::Left,
                color: dimmed,
            }),
        }
    }
    items.extend(panel::button_labels(
        layout.panel,
        (layout.save, "Save"),
        (layout.cancel, "Cancel"),
        colors,
    ));
    items
}

/// The panel's elements for the output `output_name`, front to back: the
/// panel on its output once its text is there, and the dim on every one.
pub fn elements(
    renderer: &mut GlesRenderer,
    view: &ScenePanel,
    colors: &Colors,
    output_name: &str,
    output_geo: Rectangle<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement> {
    let layout = &view.layout;
    let selected = view
        .selected
        .checked_sub(layout.first)
        .and_then(|slot| layout.rows.get(slot))
        .map(|row| ("selected", *row, panel::accent(colors), None));
    let solids = selected
        .into_iter()
        .chain(panel::buttons(layout.save, layout.cancel, colors));
    panel::modal_elements(
        renderer,
        &mut view.buffers.borrow_mut(),
        &view.output,
        layout.panel,
        view.rounding,
        solids,
        colors,
        output_name,
        output_geo,
        scale,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triggers_read_and_write_as_the_specification_has_them() {
        let combo = parse_trigger("CTRL+SHIFT+F12").unwrap();
        assert!(combo.mods.ctrl && combo.mods.shift && !combo.mods.alt);
        assert_eq!(combo.trigger, Trigger::Key(Keysym::F12));
        assert_eq!(format_trigger(&combo).as_deref(), Some("SHIFT+CTRL+F12"));
        assert_eq!(parse_trigger(&format_trigger(&combo).unwrap()), Some(combo));
        // A letter, any case, is the lowercase keysym the keyboard gives.
        assert_eq!(
            parse_trigger("LOGO+A").unwrap().trigger,
            Trigger::Key(Keysym::a)
        );
        // By its code too: saved and read back, it's the same key (found by
        // the `app_shortcuts` fuzz target's property).
        assert_eq!(
            parse_trigger("LOGO+0x41").unwrap().trigger,
            Trigger::Key(Keysym::a)
        );
        // What Pila can't bind: NUM, a key it doesn't know, a modifier alone.
        assert_eq!(parse_trigger("NUM+a"), None);
        assert_eq!(parse_trigger("CTRL+NoSuchKey"), None);
        assert_eq!(parse_trigger("CTRL+Shift_L"), None);
        assert_eq!(parse_trigger(""), None);
        // A NUL in a name: xkbcommon can't look it up (it panicked).
        assert_eq!(parse_trigger("CTRL+a\0"), None);
    }

    #[test]
    fn typing_is_never_a_global_shortcut() {
        let keys = |text: &str| parse_trigger(text).unwrap();
        assert!(assignable(&keys("CTRL+a")));
        assert!(assignable(&keys("F12")));
        assert!(assignable(&keys("XF86AudioPlay")));
        assert!(!assignable(&keys("a")));
        assert!(!assignable(&keys("SHIFT+a")));
        assert!(!assignable(&keys("space")));
    }

    #[test]
    fn many_rows_scroll_to_the_selected_one() {
        let area = Rectangle::new((0, 0).into(), (1280, 800).into());
        let few = layout(area, 3, 0);
        assert_eq!(few.rows.len(), 3);
        assert!(area.contains_rect(few.panel));
        let many = layout(area, 20, 15);
        assert_eq!(many.rows.len(), ROWS_SHOWN);
        assert!(many.first <= 15 && 15 < many.first + ROWS_SHOWN);
        for row in &many.rows {
            assert!(many.panel.contains_rect(*row));
            assert!(!row.overlaps(many.save));
        }
    }
}
