//! The capture icon in the system tray: while anything captures the screen
//! (`capture_indicator.rs`), an item in the tray waybar and most bars host
//! (StatusNotifierItem), a square in the accent as the light is, its tooltip
//! naming who captures what, and its menu a Stop for each capture.
//! `[permissions] tray` turns it off.
//!
//! The event loop never waits for the bus: `Pila::update_tray` hands the item
//! to a thread of its own, started with the first capture, connected to the
//! session bus Pila's programs get (`Pila::session_bus_address`, as
//! notifications are). The item is on the bus only while something
//! captures: its name is taken and registered with the tray's watcher
//! (`org.kde.StatusNotifierWatcher`, the bar's), and given back when the last
//! capture ends, which takes it out of the tray. A watcher that starts later
//! (a bar restarted) gets it again (`NameOwnerChanged`, watched on a thread
//! of its own). The item and its menu (`com.canonical.dbusmenu`) are served
//! from zbus's thread; a Stop chosen comes back to the event loop, which ends
//! that capture (`Pila::stop_capture`).

use std::{
    collections::HashMap,
    sync::{mpsc, Arc, Mutex},
};

use tracing::{debug, info, warn};
use zbus::{fdo, object_server::SignalEmitter, Connection, OwnedObjectPath, OwnedVariant, Variant};

use crate::{capture_indicator::Capturer, sync::Locked, Pila};

const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";
const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
/// The icon's sizes, for the tray to pick from (logical pixels).
const ICON_SIZES: [i32; 4] = [16, 22, 32, 48];

/// An icon: width, height and ARGB32 pixels, in network byte order.
type Pixmap = (i32, i32, Vec<u8>);

/// A menu item as dbusmenu lays it out: id, properties, children.
type Layout = (i32, HashMap<String, OwnedVariant>, Vec<OwnedVariant>);

/// What the item shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Item {
    /// A line per capture.
    pub captures: Vec<String>,
    /// The menu: each Stop's id and label.
    pub stops: Vec<(i32, String)>,
    pub icon: Vec<Pixmap>,
}

enum Message {
    /// Show `item` on the bus at `bus` (`None`: there's none).
    Show { bus: Option<String>, item: Item },
    /// Take the item out of the tray.
    Hide,
    /// A tray's watcher started: register with it.
    WatcherStarted,
}

/// The tray icon's state on the event loop's side (`Pila::tray`).
#[derive(Default)]
pub struct Tray {
    /// The way to its thread, started with the first capture.
    sender: Option<mpsc::Sender<Message>>,
    /// The menu's Stops: the capture each ends, by id.
    stops: Vec<(i32, Capturer)>,
    last_id: i32,
    /// What was sent last; `None` while the item is hidden.
    sent: Option<Item>,
}

impl Pila {
    /// Show what's captured in the tray, or take the item away when nothing
    /// is: when the captures listed change (`update_capture_indicator`), and
    /// on a reload that turns it on or off or changes the accent.
    pub(crate) fn update_tray(&mut self) {
        let shown = &self.capture_indicator.shown;
        if !self.config.permissions.tray || shown.is_empty() {
            self.tray.stops.clear();
            if self.tray.sent.take().is_some() {
                self.send_tray(Message::Hide);
            }
            return;
        }
        // A capture keeps its id while it's listed: a menu open stays right.
        let mut stops = Vec::with_capacity(shown.len());
        for capturer in shown {
            let kept = self.tray.stops.iter().find(|(_, c)| c.same(capturer));
            let id = match kept {
                Some((id, _)) => *id,
                None => {
                    self.tray.last_id = self.tray.last_id.wrapping_add(1).max(1);
                    self.tray.last_id
                }
            };
            stops.push((id, capturer.clone()));
        }
        let item = Item {
            captures: stops.iter().map(|(_, c)| c.describe()).collect(),
            stops: stops
                .iter()
                .map(|(id, c)| (*id, format!("Stop {}", c.describe())))
                .collect(),
            icon: icon(crate::panel::accent(&self.config.colors)),
        };
        self.tray.stops = stops;
        if self.tray.sent.as_ref() == Some(&item) {
            return;
        }
        self.tray.sent = Some(item.clone());
        let bus = self.session_bus_address();
        self.send_tray(Message::Show { bus, item });
    }

    /// Hand `message` to the tray's thread, starting it with the first.
    fn send_tray(&mut self, message: Message) {
        let message = match &self.tray.sender {
            Some(sender) => match sender.send(message) {
                Ok(()) => return,
                // The thread is gone (it can't panic, but if it did): a new one.
                Err(mpsc::SendError(message)) => message,
            },
            None => message,
        };
        if matches!(message, Message::Hide) {
            return;
        }
        let (stops, channel) = calloop::channel::channel();
        let inserted = self.loop_handle.insert_source(channel, |event, _, state| {
            if let calloop::channel::Event::Msg(id) = event {
                state.tray_stop(id);
            }
        });
        let token = match inserted {
            Ok(token) => token,
            Err(err) => {
                warn!(?err, "tray: can't wait for its menu");
                return;
            }
        };
        let (sender, receiver) = mpsc::channel();
        let watcher = sender.clone();
        let spawned = std::thread::Builder::new()
            .name("pila-tray".into())
            .spawn(move || run(receiver, watcher, stops));
        if let Err(err) = spawned {
            warn!(?err, "tray: can't start its thread");
            self.loop_handle.remove(token);
            return;
        }
        // The receiver is alive: the thread just took it.
        let _ = sender.send(message);
        self.tray.sender = Some(sender);
    }

    /// Stop `id` was chosen in the menu: end that capture.
    fn tray_stop(&mut self, id: i32) {
        let Some((_, capturer)) = self.tray.stops.iter().find(|(stop, _)| *stop == id) else {
            return;
        };
        let capturer = capturer.clone();
        info!(
            app = capturer.app,
            protocol = capturer.protocol.name(),
            "tray: capture stopped"
        );
        self.stop_capture(&capturer);
    }
}

/// The icon at each of `ICON_SIZES`: a square in `accent`, as the light is.
fn icon(accent: [f32; 4]) -> Vec<Pixmap> {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let argb = [255, byte(accent[0]), byte(accent[1]), byte(accent[2])];
    ICON_SIZES
        .iter()
        .map(|&size| {
            let side = size * 5 / 8;
            let start = (size - side) / 2;
            let inside = start..start + side;
            let mut pixels = Vec::with_capacity((size * size * 4) as usize);
            for y in 0..size {
                for x in 0..size {
                    let lit = inside.contains(&x) && inside.contains(&y);
                    pixels.extend_from_slice(if lit { &argb } else { &[0; 4] });
                }
            }
            (size, size, pixels)
        })
        .collect()
}

/// What the item's interfaces serve, from zbus's thread.
#[derive(Default)]
struct Shared {
    item: Item,
    /// The menu's layout revision: one more with each new item.
    revision: u32,
}

/// The tray's thread: the item shown and hidden as the event loop says.
fn run(
    receiver: mpsc::Receiver<Message>,
    watcher: mpsc::Sender<Message>,
    stops: calloop::channel::Sender<i32>,
) {
    let shared = Arc::new(Mutex::new(Shared::default()));
    let name = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
    let mut bus: Option<(String, Connection)> = None;
    // The name is Pila's: the item is on the bus.
    let mut on_bus = false;
    let mut warned = false;
    for message in receiver {
        match message {
            Message::Show { bus: address, item } => {
                let Some(address) = address else {
                    if !std::mem::replace(&mut warned, true) {
                        info!("tray: no D-Bus session bus, so no capture icon in the tray");
                    }
                    continue;
                };
                if bus.as_ref().is_none_or(|(on, _)| *on != address) {
                    if let Some((_, old)) = bus.take() {
                        release(&old, &name);
                    }
                    on_bus = false;
                    match connect(&address, &shared, &stops, &watcher) {
                        Ok(connection) => bus = Some((address, connection)),
                        Err(err) => {
                            if !std::mem::replace(&mut warned, true) {
                                warn!(err, "tray: can't connect to the session bus");
                            }
                            continue;
                        }
                    }
                }
                let Some((_, connection)) = &bus else {
                    continue;
                };
                let revision = {
                    let mut shared = shared.locked();
                    shared.item = item;
                    shared.revision = shared.revision.wrapping_add(1);
                    shared.revision
                };
                if on_bus {
                    changed(connection, revision);
                } else if take_name(connection, &name) {
                    on_bus = true;
                    register(connection, &name);
                }
            }
            Message::Hide => {
                if let Some((_, connection)) = &bus {
                    if std::mem::take(&mut on_bus) {
                        release(connection, &name);
                    }
                }
            }
            Message::WatcherStarted => {
                if let (Some((_, connection)), true) = (&bus, on_bus) {
                    register(connection, &name);
                }
            }
        }
    }
}

/// Connect to the bus at `address`, serving the item and its menu, and
/// watch for a tray's watcher starting.
fn connect(
    address: &str,
    shared: &Arc<Mutex<Shared>>,
    stops: &calloop::channel::Sender<i32>,
    watcher: &mpsc::Sender<Message>,
) -> Result<Connection, String> {
    let connection = zbus::block_on(
        crate::session_bus::builder(address)
            .serve_at(
                ITEM_PATH,
                StatusNotifierItem {
                    shared: shared.clone(),
                },
            )
            .serve_at(
                MENU_PATH,
                Menu {
                    shared: shared.clone(),
                    stops: stops.clone(),
                },
            )
            .build(),
    )
    .map_err(|err| err.to_string())?;
    let starts = zbus::block_on(crate::session_bus::name_owner_changes(&connection));
    match starts {
        Ok(starts) => {
            let watcher = watcher.clone();
            let spawned = std::thread::Builder::new()
                .name("pila-tray-watch".into())
                .spawn(move || zbus::block_on(watch_watchers(starts, watcher)));
            if let Err(err) = spawned {
                warn!(?err, "tray: can't watch for a tray starting");
            }
        }
        Err(err) => warn!(%err, "tray: can't watch for a tray starting"),
    }
    Ok(connection)
}

/// Tell the tray's thread each time a tray's watcher starts.
async fn watch_watchers(mut starts: fdo::NameOwnerChangedStream, watcher: mpsc::Sender<Message>) {
    while let Some(signal) = crate::session_bus::next_change(&mut starts).await {
        let Ok(args) = signal.args() else {
            continue;
        };
        if args.name().as_str() != WATCHER || args.new_owner().is_none() {
            continue;
        }
        if watcher.send(Message::WatcherStarted).is_err() {
            return;
        }
    }
}

/// Take the item's name; whether it's Pila's.
fn take_name(connection: &Connection, name: &str) -> bool {
    match zbus::block_on(crate::session_bus::take_name(connection, name)) {
        Ok(()) => true,
        Err(err) => {
            warn!(err, name, "tray: can't take the item's name");
            false
        }
    }
}

/// Give the name back: the tray drops the item.
fn release(connection: &Connection, name: &str) {
    if let Err(err) = zbus::block_on(connection.release_name(name)) {
        debug!(%err, "tray: can't give the item's name back");
    }
}

/// Register the item with the tray's watcher, if one runs (else it does
/// when one starts: `watch_watchers`).
fn register(connection: &Connection, name: &str) {
    let registered = zbus::block_on(connection.call_method(
        Some(WATCHER),
        WATCHER_PATH,
        Some(WATCHER),
        "RegisterStatusNotifierItem",
        &(name,),
    ));
    if let Err(err) = registered {
        debug!(%err, "tray: no tray to show the capture icon in");
    }
}

/// Tell the tray the item changed: its tooltip, icon and menu.
fn changed(connection: &Connection, revision: u32) {
    let sent = zbus::block_on(async {
        let server = connection.object_server();
        let item = server.interface::<_, StatusNotifierItem>(ITEM_PATH).await?;
        StatusNotifierItem::new_tool_tip(item.signal_emitter()).await?;
        StatusNotifierItem::new_icon(item.signal_emitter()).await?;
        let menu = server.interface::<_, Menu>(MENU_PATH).await?;
        Menu::layout_updated(menu.signal_emitter(), revision, 0).await
    });
    if let Err(err) = sent {
        debug!(%err, "tray: can't tell the tray the item changed");
    }
}

/// Text for the tooltip's description, which trays read as markup.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// `org.kde.StatusNotifierItem`.
struct StatusNotifierItem {
    shared: Arc<Mutex<Shared>>,
}

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl StatusNotifierItem {
    #[zbus(property)]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }

    #[zbus(property)]
    fn id(&self) -> &str {
        "pila-captures"
    }

    #[zbus(property)]
    fn title(&self) -> &str {
        "Screen capture"
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        "Active"
    }

    #[zbus(property)]
    fn icon_name(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn icon_pixmap(&self) -> Vec<Pixmap> {
        self.shared.locked().item.icon.clone()
    }

    #[zbus(property)]
    fn overlay_icon_name(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn attention_icon_name(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn tool_tip(&self) -> (String, Vec<Pixmap>, String, String) {
        let captures = escape(&self.shared.locked().item.captures.join("\n"));
        (
            String::new(),
            Vec::new(),
            "Capturing the screen".to_string(),
            captures,
        )
    }

    /// A click shows the menu.
    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from(MENU_PATH).unwrap_or_default()
    }

    fn activate(&self, _x: i32, _y: i32) {}

    fn secondary_activate(&self, _x: i32, _y: i32) {}

    fn context_menu(&self, _x: i32, _y: i32) {}

    fn scroll(&self, _delta: i32, _orientation: String) {}

    #[zbus(signal)]
    async fn new_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn new_tool_tip(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

/// `com.canonical.dbusmenu`: the item's menu, a Stop per capture.
struct Menu {
    shared: Arc<Mutex<Shared>>,
    stops: calloop::channel::Sender<i32>,
}

impl Menu {
    /// Item `id`'s properties: the root's, or a Stop's.
    fn properties(&self, id: i32) -> Option<HashMap<String, OwnedVariant>> {
        let owned = |value: Variant<'_>| OwnedVariant::try_from(value).ok();
        let mut properties = HashMap::new();
        if id == 0 {
            properties.insert("children-display".into(), owned("submenu".into())?);
            return Some(properties);
        }
        let shared = self.shared.locked();
        let (_, label) = shared.item.stops.iter().find(|(stop, _)| *stop == id)?;
        // An underscore marks a mnemonic: written twice, it's shown.
        properties.insert("label".into(), owned(label.replace('_', "__").into())?);
        properties.insert("enabled".into(), owned(true.into())?);
        Some(properties)
    }

    fn is_stop(&self, id: i32) -> bool {
        self.shared
            .locked()
            .item
            .stops
            .iter()
            .any(|(stop, _)| *stop == id)
    }

    fn event(&self, id: i32, event_id: &str) -> bool {
        if !self.is_stop(id) {
            return false;
        }
        if event_id == "clicked" {
            let _ = self.stops.send(id);
        }
        true
    }
}

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl Menu {
    fn get_layout(
        &self,
        parent_id: i32,
        _recursion_depth: i32,
        _property_names: Vec<String>,
    ) -> fdo::Result<(u32, Layout)> {
        let unknown = || fdo::Error::InvalidArgs(format!("no menu item {parent_id}"));
        let properties = self.properties(parent_id).ok_or_else(unknown)?;
        let (revision, ids) = {
            let shared = self.shared.locked();
            let ids: Vec<i32> = shared.item.stops.iter().map(|(id, _)| *id).collect();
            (shared.revision, ids)
        };
        let children = if parent_id == 0 {
            ids.into_iter()
                .filter_map(|id| {
                    let properties: HashMap<String, Variant<'_>> = self
                        .properties(id)?
                        .into_iter()
                        .map(|(name, value)| (name, Variant::from(value)))
                        .collect();
                    let child = Variant::from((id, properties, Vec::<Variant<'_>>::new()));
                    OwnedVariant::try_from(child).ok()
                })
                .collect()
        } else {
            Vec::new()
        };
        Ok((revision, (parent_id, properties, children)))
    }

    fn get_group_properties(
        &self,
        ids: Vec<i32>,
        _property_names: Vec<String>,
    ) -> Vec<(i32, HashMap<String, OwnedVariant>)> {
        ids.into_iter()
            .filter_map(|id| Some((id, self.properties(id)?)))
            .collect()
    }

    fn get_property(&self, id: i32, name: String) -> fdo::Result<OwnedVariant> {
        self.properties(id)
            .and_then(|mut properties| properties.remove(&name))
            .ok_or_else(|| fdo::Error::InvalidArgs(format!("no {name} for menu item {id}")))
    }

    #[zbus(name = "Event")]
    fn event_call(
        &self,
        id: i32,
        event_id: String,
        _data: OwnedVariant,
        _timestamp: u32,
    ) -> fdo::Result<()> {
        match self.event(id, &event_id) {
            true => Ok(()),
            false => Err(fdo::Error::InvalidArgs(format!("no menu item {id}"))),
        }
    }

    /// The events' ids that name no item.
    fn event_group(&self, events: Vec<(i32, String, OwnedVariant, u32)>) -> Vec<i32> {
        events
            .into_iter()
            .filter(|(id, event_id, _, _)| !self.event(*id, event_id))
            .map(|(id, ..)| id)
            .collect()
    }

    fn about_to_show(&self, _id: i32) -> bool {
        false
    }

    fn about_to_show_group(&self, _ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        (Vec::new(), Vec::new())
    }

    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }

    #[zbus(property)]
    fn text_direction(&self) -> &str {
        "ltr"
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        "normal"
    }

    #[zbus(property)]
    fn icon_theme_path(&self) -> Vec<String> {
        Vec::new()
    }

    #[zbus(signal)]
    async fn layout_updated(
        emitter: &SignalEmitter<'_>,
        revision: u32,
        parent: i32,
    ) -> zbus::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_is_a_square_in_the_accent_at_each_size() {
        let icons = icon([1.0, 0.5, 0.0, 1.0]);
        assert_eq!(icons.len(), ICON_SIZES.len());
        for (size, (w, h, pixels)) in ICON_SIZES.iter().zip(&icons) {
            assert_eq!((w, h), (size, size));
            assert_eq!(pixels.len(), (size * size * 4) as usize);
            let at = |x: i32, y: i32| &pixels[((y * size + x) * 4) as usize..][..4];
            assert_eq!(at(size / 2, size / 2), [255, 255, 128, 0], "the middle");
            assert_eq!(at(0, 0), [0, 0, 0, 0], "the corner");
        }
    }

    #[test]
    fn the_tooltip_is_escaped() {
        assert_eq!(escape("a<b>&c"), "a&lt;b&gt;&amp;c");
    }
}
