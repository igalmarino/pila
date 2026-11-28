//! Desktop notifications from Pila (an invalid config, a program that isn't
//! installed): sent over D-Bus to the notification daemon (mako, dunst, a
//! shell's own), `org.freedesktop.Notifications.Notify`, so no notify-send.
//!
//! The event loop never waits for the bus: `Notifier::send` hands the
//! notification to a thread of its own, started with the first one, which
//! connects to the session bus Pila's programs get (`[environment]`'s
//! `DBUS_SESSION_BUS_ADDRESS`, else Pila's own) and keeps the connection.
//! Without a bus or a daemon they don't show; the log says why, once, and the
//! next notification tries again (a daemon started later, by `autostart`, or
//! by the bus when the first one asks for it).

use std::{collections::HashMap, sync::mpsc};

use tracing::warn;
use zbus::{Connection, Variant};

/// A notification to send, with the bus to send it on.
struct Notification {
    bus: Option<String>,
    summary: String,
    body: String,
    milliseconds: u32,
}

/// The way to the notifications' thread, started with the first one.
#[derive(Default)]
pub struct Notifier {
    sender: Option<mpsc::Sender<Notification>>,
}

impl Notifier {
    /// Send a notification on the session bus at `bus` (`None`: there's
    /// none), shown for `milliseconds`; it doesn't wait.
    pub fn send(&mut self, bus: Option<String>, summary: &str, body: &str, milliseconds: u32) {
        let notification = Notification {
            bus,
            summary: summary.to_string(),
            body: body.to_string(),
            milliseconds,
        };
        let notification = match &self.sender {
            Some(sender) => match sender.send(notification) {
                Ok(()) => return,
                // The thread is gone (it can't panic, but if it did): a new one.
                Err(mpsc::SendError(notification)) => notification,
            },
            None => notification,
        };
        let (sender, receiver) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("notifications".into())
            .spawn(move || run(receiver));
        if let Err(err) = spawned {
            warn!(?err, "can't start the notifications' thread");
            return;
        }
        // The receiver is alive: the thread just took it.
        let _ = sender.send(notification);
        self.sender = Some(sender);
    }
}

/// The notifications' thread: each one sent as it comes, on the bus it names.
fn run(receiver: mpsc::Receiver<Notification>) {
    let mut connection: Option<(String, Connection)> = None;
    let mut warned = false;
    for notification in receiver {
        let result = match &notification.bus {
            None => Err("no D-Bus session bus".to_string()),
            Some(bus) => {
                if connection
                    .as_ref()
                    .is_none_or(|(address, _)| address != bus)
                {
                    connection = connect(bus).map(|c| (bus.clone(), c)).ok();
                }
                match &connection {
                    None => Err(format!("can't connect to the session bus at {bus}")),
                    Some((_, bus)) => notify(bus, &notification),
                }
            }
        };
        if let Err(err) = result {
            // Connect again next time: the bus may have been restarted.
            connection = None;
            if !warned {
                warned = true;
                warn!(
                    err,
                    "no desktop notifications: none shows without a notification daemon \
                     (such as mako) on the session bus"
                );
            }
        }
    }
}

fn connect(address: &str) -> zbus::Result<Connection> {
    zbus::block_on(crate::session_bus::builder(address).build())
}

/// `Notify`, as notify-send calls it: app name `pila`, no icon or actions.
fn notify(bus: &Connection, notification: &Notification) -> Result<(), String> {
    let hints: HashMap<&str, Variant<'_>> = HashMap::new();
    let timeout = i32::try_from(notification.milliseconds).unwrap_or(i32::MAX);
    zbus::block_on(bus.call_method(
        Some("org.freedesktop.Notifications"),
        "/org/freedesktop/Notifications",
        Some("org.freedesktop.Notifications"),
        "Notify",
        &(
            "pila",
            0u32,
            "",
            notification.summary.as_str(),
            notification.body.as_str(),
            Vec::<&str>::new(),
            hints,
            timeout,
        ),
    ))
    .map(drop)
    .map_err(|err| err.to_string())
}
