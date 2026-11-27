//! A system tray on the test's own session bus (`notifications::Bus`), as
//! waybar's is: a StatusNotifierWatcher recording the items that register,
//! and a host's view of an item: its properties and its menu
//! (`com.canonical.dbusmenu`), which it can click (`src/tray.rs`).

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};

use zbus::{Connection, OwnedVariant};

use super::{notifications::Bus, TIMEOUT};

/// A tray's watcher, until it's dropped.
pub struct Watcher {
    _connection: Connection,
    registered: Arc<Mutex<Vec<String>>>,
}

impl Bus {
    /// A tray's watcher on this bus (`org.kde.StatusNotifierWatcher`).
    pub fn tray_watcher(&self) -> Watcher {
        let registered = Arc::new(Mutex::new(Vec::new()));
        let connection = zbus::block_on(
            zbus::connection::Builder::address(self.address.as_str())
                .name("org.kde.StatusNotifierWatcher")
                .serve_at(
                    "/StatusNotifierWatcher",
                    FakeWatcher {
                        registered: registered.clone(),
                    },
                )
                .build(),
        )
        .unwrap();
        Watcher {
            _connection: connection,
            registered,
        }
    }

    /// A host's connection, to look at items and click their menus.
    pub fn host(&self) -> Host {
        Host {
            connection: zbus::block_on(
                zbus::connection::Builder::address(self.address.as_str()).build(),
            )
            .unwrap(),
        }
    }
}

impl Watcher {
    /// The items registered so far, by their bus names.
    pub fn registered(&self) -> Vec<String> {
        self.registered.lock().unwrap().clone()
    }

    /// Wait for the `count`th registration; the item's name.
    pub fn wait_for(&self, count: usize) -> String {
        let started = Instant::now();
        loop {
            let registered = self.registered();
            if registered.len() >= count {
                return registered[count - 1].clone();
            }
            assert!(
                started.elapsed() < TIMEOUT,
                "{} items registered, not {count}: {registered:?}",
                registered.len()
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

struct FakeWatcher {
    registered: Arc<Mutex<Vec<String>>>,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl FakeWatcher {
    fn register_status_notifier_item(&self, service: String) {
        self.registered.lock().unwrap().push(service);
    }

    fn register_status_notifier_host(&self, _service: String) {}

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }
}

/// An icon as an item sends it: width, height, ARGB32 pixels.
pub type Pixmap = (i32, i32, Vec<u8>);

/// A tray host: what it reads of an item, and its clicks.
pub struct Host {
    connection: Connection,
}

/// A menu item: its id and label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub id: i32,
    pub label: String,
}

impl Host {
    /// Item `name`'s property `property` (`org.kde.StatusNotifierItem`).
    pub fn property(&self, name: &str, property: &str) -> OwnedVariant {
        let reply = zbus::block_on(self.connection.call_method(
            Some(name),
            "/StatusNotifierItem",
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.kde.StatusNotifierItem", property),
        ))
        .unwrap();
        reply.body().deserialize::<OwnedVariant>().unwrap()
    }

    /// The tooltip's title and description.
    pub fn tooltip(&self, name: &str) -> (String, String) {
        let tooltip = self.property(name, "ToolTip");
        let (_, _, title, description): (String, Vec<Pixmap>, String, String) =
            tooltip.try_into().unwrap();
        (title, description)
    }

    /// The menu's items, as a host lays it out (`GetLayout` of the root).
    pub fn menu(&self, name: &str) -> Vec<MenuItem> {
        type Layout = (i32, HashMap<String, OwnedVariant>, Vec<OwnedVariant>);
        let reply = zbus::block_on(self.connection.call_method(
            Some(name),
            "/MenuBar",
            Some("com.canonical.dbusmenu"),
            "GetLayout",
            &(0i32, -1i32, Vec::<String>::new()),
        ))
        .unwrap();
        let (_, (_, _, children)): (u32, Layout) = reply.body().deserialize().unwrap();
        children
            .into_iter()
            .map(|child| {
                let (id, properties, _): Layout = child.try_into().unwrap();
                let label = properties
                    .get("label")
                    .and_then(|label| String::try_from(label.try_clone().ok()?).ok())
                    .unwrap_or_default();
                MenuItem { id, label }
            })
            .collect()
    }

    /// Click menu item `id`, as a host does (`Event`, `clicked`).
    pub fn click(&self, name: &str, id: i32) {
        zbus::block_on(self.connection.call_method(
            Some(name),
            "/MenuBar",
            Some("com.canonical.dbusmenu"),
            "Event",
            &(id, "clicked", zbus::Variant::from(0i32), 0u32),
        ))
        .unwrap();
    }

    /// Whether `name` is on the bus: the item is in the tray.
    pub fn on_bus(&self, name: &str) -> bool {
        let reply = zbus::block_on(self.connection.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "NameHasOwner",
            &(name,),
        ))
        .unwrap();
        reply.body().deserialize::<bool>().unwrap()
    }
}
