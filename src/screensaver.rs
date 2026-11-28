//! `org.freedesktop.ScreenSaver`'s `Inhibit` on the session bus: how
//! desktop apps (Firefox and Chromium for a video or a call, mpv, Electron
//! apps) and, through xdg-desktop-portal-gtk's Inhibit, Flatpaks ask to keep
//! the screen on. While a cookie is held, idle is inhibited (`idle.rs`) as it
//! is by a visible Wayland idle inhibitor: swayidle neither locks nor turns
//! the monitors off. `SimulateUserActivity` counts as input does, and
//! `GetActive` answers whether the session is locked; the rest of the
//! interface (`Lock`, `SetActive`, idle times) isn't served.
//!
//! Served on the portal's connection (`portal::serve`), from zbus's thread;
//! the event loop gets the apps that hold one
//! (`portal::Request::DbusInhibitors`) whenever that changes. A cookie lives
//! until its app gives it back (`UnInhibit`) or leaves the bus
//! (`NameOwnerChanged`, watched on a thread of its own), so an app that
//! crashes doesn't keep the session awake; one that leaves before its call
//! is served keeps none either (`still_on_the_bus`). A connection holds at
//! most `MAX_PER_APP` cookies. If another program has the
//! name, it's theirs, and nothing is left at the paths either.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use tracing::{info, warn};
use zbus::{fdo, message::Header};

use crate::{portal::Request, sync::Locked};

/// The most cookies one connection holds at once: an app keeps one or two
/// (a video, a call), and no app grows the map without end.
pub const MAX_PER_APP: usize = 32;

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
        #[zbus(connection)] connection: &zbus::Connection,
        application_name: String,
        reason_for_inhibit: String,
    ) -> fdo::Result<u32> {
        let Some(owner) = header.sender() else {
            return Err(fdo::Error::Failed("no sender".into()));
        };
        let cookie = {
            let mut cookies = self.cookies.locked();
            let its_own = cookies
                .held
                .values()
                .filter(|held| held.owner == owner.as_str())
                .count();
            if its_own >= MAX_PER_APP {
                warn!(
                    app = application_name,
                    max = MAX_PER_APP,
                    "idle: an app asked for too many inhibitors"
                );
                return Err(fdo::Error::LimitsExceeded(format!(
                    "at most {MAX_PER_APP} inhibitors per connection"
                )));
            }
            let cookie = cookies.next();
            cookies.held.insert(
                cookie,
                Held {
                    owner: owner.to_string(),
                    inhibitor: Inhibitor {
                        app: application_name.clone(),
                        reason: reason_for_inhibit.clone(),
                    },
                },
            );
            cookie
        };
        // An app that called and left at once: its leaving may have reached
        // `watch_departures` before the cookie was in. Still on the bus
        // now, its leaving is yet to come and finds the cookie.
        if !still_on_the_bus(connection, owner.as_str()).await {
            self.cookies.locked().held.remove(&cookie);
            return Err(fdo::Error::Failed("the caller left the bus".into()));
        }
        info!(
            app = application_name,
            reason = reason_for_inhibit,
            "idle: an app inhibits idling"
        );
        self.cookies.locked().changed();
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

    /// The user is active, as if they had touched a key (apps that drive
    /// the screen another way, such as a remote desktop, say so).
    async fn simulate_user_activity(&self) {
        let send = self.cookies.locked().send.clone();
        let _ = send.send(Request::UserActivity);
    }

    /// Whether the screen saver is on: the session is locked.
    async fn get_active(&self) -> bool {
        let send = self.cookies.locked().send.clone();
        let (reply, answer) = crate::portal::reply_channel();
        if send.send(Request::ScreenSaverActive { reply }).is_err() {
            return false;
        }
        answer.await.unwrap_or(false)
    }
}

/// Whether `name` (a connection's unique name) is on the bus; if the bus
/// can't say, it is.
async fn still_on_the_bus(connection: &zbus::Connection, name: &str) -> bool {
    let Ok(name) = zbus::names::BusName::try_from(name) else {
        return true;
    };
    let answer = match fdo::DBusProxy::new(connection).await {
        Ok(bus) => bus.name_has_owner(name).await,
        Err(err) => Err(err.into()),
    };
    answer.unwrap_or_else(|err| {
        warn!(
            ?err,
            "idle: can't ask the bus whether an app is still on it"
        );
        true
    })
}

/// Take the interface off `PATHS` again.
async fn withdraw(server: &zbus::ObjectServer) {
    for path in PATHS {
        let _ = server.remove::<ScreenSaver, _>(path).await;
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
    let departures = match set_up(connection, &cookies).await {
        Ok(departures) => departures,
        Err(err) => {
            warn!(err, "idle: not serving org.freedesktop.ScreenSaver");
            withdraw(connection.object_server()).await;
            return;
        }
    };
    info!(name = BUS_NAME, "idle: serving apps' inhibitors");
    let spawned = std::thread::Builder::new()
        .name("pila-screensaver".into())
        .spawn(move || zbus::block_on(watch_departures(departures, cookies)));
    if let Err(err) = spawned {
        warn!(?err, "idle: can't watch the session bus");
    }
}

/// Serve the interface at `PATHS` and take its name, watching apps leave
/// the bus from before (so no app's leaving is missed); why not.
async fn set_up(
    connection: &zbus::Connection,
    cookies: &Arc<Mutex<Cookies>>,
) -> Result<fdo::NameOwnerChangedStream, String> {
    let server = connection.object_server();
    for path in PATHS {
        let iface = ScreenSaver {
            cookies: cookies.clone(),
        };
        server
            .at(path, iface)
            .await
            .map_err(|err| format!("can't serve it at {path}: {err}"))?;
    }
    let departures = crate::session_bus::name_owner_changes(connection)
        .await
        .map_err(|err| format!("can't watch the session bus: {err}"))?;
    crate::session_bus::take_name(connection, BUS_NAME)
        .await
        .map_err(|err| format!("can't take the name {BUS_NAME}: {err}"))?;
    Ok(departures)
}

/// Drop the cookies of each app that leaves the bus.
async fn watch_departures(
    mut departures: fdo::NameOwnerChangedStream,
    cookies: Arc<Mutex<Cookies>>,
) {
    while let Some(signal) = crate::session_bus::next_change(&mut departures).await {
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
