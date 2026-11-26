//! `org.freedesktop.ScreenSaver`'s `Inhibit` on the session bus: how
//! desktop apps (Firefox and Chromium for a video or a call, mpv, Electron
//! apps) and, through xdg-desktop-portal-gtk's Inhibit, Flatpaks ask to keep
//! the screen on. While a cookie is held, idle is inhibited (`idle.rs`) as it
//! is by a visible Wayland idle inhibitor: swayidle neither locks nor turns
//! the monitors off.
//!
//! Served on the portal's connection (`portal::serve`), from zbus's thread;
//! the event loop gets the apps that hold one
//! (`portal::Request::DbusInhibitors`) whenever that changes. A cookie lives
//! until its app gives it back (`UnInhibit`) or leaves the bus
//! (`NameOwnerChanged`, watched on a thread of its own), so an app that
//! crashes doesn't keep the session awake.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use tracing::{info, warn};
use zbus::{fdo, fdo::RequestNameFlags, fdo::RequestNameReply, message::Header};

use crate::{portal::Request, sync::Locked};

/// The name apps call.
pub const BUS_NAME: &str = "org.freedesktop.ScreenSaver";
/// Where apps call it: the full path, and the short one older apps use
/// (KDE serves both too).
const PATHS: [&str; 2] = ["/org/freedesktop/ScreenSaver", "/ScreenSaver"];

/// An app that asked to keep the screen on, as it named itself and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inhibitor {
    pub app: String,
    pub reason: String,
}

/// A cookie: whose (its connection's unique name) and what for.
struct Held {
    owner: String,
    inhibitor: Inhibitor,
}

/// The cookies held, shared by both paths and the watch on departures.
struct Cookies {
    held: BTreeMap<u32, Held>,
    /// The last cookie given.
    last: u32,
    send: calloop::channel::Sender<Request>,
}

impl Cookies {
    /// A cookie no one holds: never 0, which apps may take for none.
    fn next(&mut self) -> u32 {
        loop {
            self.last = self.last.wrapping_add(1);
            if self.last != 0 && !self.held.contains_key(&self.last) {
                return self.last;
            }
        }
    }

    /// Tell the event loop who holds one now.
    fn changed(&self) {
        let inhibitors = self.held.values().map(|h| h.inhibitor.clone()).collect();
        let _ = self.send.send(Request::DbusInhibitors(inhibitors));
    }
}

#[derive(Clone)]
struct ScreenSaver {
    cookies: Arc<Mutex<Cookies>>,
}

#[zbus::interface(name = "org.freedesktop.ScreenSaver")]
impl ScreenSaver {
    async fn inhibit(
        &self,
        #[zbus(header)] header: Header<'_>,
        application_name: String,
        reason_for_inhibit: String,
    ) -> fdo::Result<u32> {
        let Some(owner) = header.sender() else {
            return Err(fdo::Error::Failed("no sender".into()));
        };
        info!(
            app = application_name,
            reason = reason_for_inhibit,
            "idle: an app inhibits idling"
        );
        let mut cookies = self.cookies.locked();
        let cookie = cookies.next();
        cookies.held.insert(
            cookie,
            Held {
                owner: owner.to_string(),
                inhibitor: Inhibitor {
                    app: application_name,
                    reason: reason_for_inhibit,
                },
            },
        );
        cookies.changed();
        Ok(cookie)
    }

    /// Give a cookie back: only its own app can.
    async fn un_inhibit(&self, #[zbus(header)] header: Header<'_>, cookie: u32) {
        let Some(owner) = header.sender() else {
            return;
        };
        let mut cookies = self.cookies.locked();
        if cookies
            .held
            .get(&cookie)
            .is_some_and(|held| held.owner == owner.as_str())
        {
            if let Some(held) = cookies.held.remove(&cookie) {
                info!(
                    app = held.inhibitor.app,
                    "idle: an app stops inhibiting idling"
                );
            }
            cookies.changed();
        }
    }
}

/// Serve the interface on `connection` (the portal's, on its thread) and
/// take the name; a name someone else has already is left to them, logged.
pub async fn serve(connection: &zbus::Connection, send: calloop::channel::Sender<Request>) {
    let cookies = Arc::new(Mutex::new(Cookies {
        held: BTreeMap::new(),
        last: 0,
        send,
    }));
    let server = connection.object_server();
    for path in PATHS {
        let iface = ScreenSaver {
            cookies: cookies.clone(),
        };
        if let Err(err) = server.at(path, iface).await {
            warn!(%err, path, "idle: can't serve org.freedesktop.ScreenSaver");
            return;
        }
    }
    // Watched before the name is taken, so no app's leaving is missed.
    let proxy = match fdo::DBusProxy::new(connection).await {
        Ok(proxy) => proxy,
        Err(err) => {
            warn!(%err, "idle: can't watch the session bus");
            return;
        }
    };
    let departures = match proxy.receive_name_owner_changed().await {
        Ok(stream) => stream,
        Err(err) => {
            warn!(%err, "idle: can't watch the session bus");
            return;
        }
    };
    match connection
        .request_name_with_flags(BUS_NAME, RequestNameFlags::DO_NOT_QUEUE)
        .await
    {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => {
            info!(name = BUS_NAME, "idle: serving apps' inhibitors");
        }
        Ok(reply) => {
            warn!(
                ?reply,
                name = BUS_NAME,
                "idle: another program has the name"
            );
            return;
        }
        Err(err) => {
            warn!(%err, name = BUS_NAME, "idle: can't take the name");
            return;
        }
    }
    let spawned = std::thread::Builder::new()
        .name("pila-screensaver".into())
        .spawn(move || zbus::block_on(watch_departures(departures, cookies)));
    if let Err(err) = spawned {
        warn!(?err, "idle: can't watch the session bus");
    }
}

/// Drop the cookies of each app that leaves the bus.
async fn watch_departures(
    mut departures: fdo::NameOwnerChangedStream,
    cookies: Arc<Mutex<Cookies>>,
) {
    use zbus::export::futures_core::Stream;
    while let Some(signal) =
        std::future::poll_fn(|cx| std::pin::Pin::new(&mut departures).poll_next(cx)).await
    {
        let Ok(args) = signal.args() else {
            continue;
        };
        if args.new_owner().is_some() || !args.name().starts_with(':') {
            continue;
        }
        let gone = args.name().as_str();
        let mut cookies = cookies.locked();
        let before = cookies.held.len();
        cookies.held.retain(|_, held| {
            let stays = held.owner != gone;
            if !stays {
                info!(app = held.inhibitor.app, "idle: an inhibiting app left");
            }
            stays
        });
        if cookies.held.len() != before {
            cookies.changed();
        }
    }
}
