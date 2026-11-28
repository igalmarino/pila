//! Pila's environment for the D-Bus session bus and `systemd --user`
//! (`Pila::export_environment`): what they start (xdg-desktop-portal and its
//! wlr backend, pipewire) talks to Pila. Sent over D-Bus itself, as
//! `dbus-update-activation-environment --systemd` would send it, so that
//! program isn't needed: `UpdateActivationEnvironment` on the bus, and on the
//! systemd user bus `SetEnvironment` on systemd's manager too.
//!
//! Blocking: `export` runs on a thread of its own, never the event loop.
//!
//! Also what Pila's other connections to the bus share (the portal, the
//! tray, notifications): their timeout, taking a name, following names'
//! owners.

use std::{collections::HashMap, time::Duration};

use zbus::{
    fdo::{self, RequestNameFlags, RequestNameReply},
    Connection,
};

/// How long Pila's connections to the session bus wait for an answer: a
/// hung peer only holds up their thread.
pub const TIMEOUT: Duration = Duration::from_secs(5);

/// A connection to the bus at `address`, its calls answered within
/// `TIMEOUT`.
pub fn builder(address: &str) -> zbus::connection::Builder<'_> {
    zbus::connection::Builder::address(address).method_timeout(TIMEOUT)
}

/// Take `name` on `connection`, not queued for it: why not, when it isn't
/// Pila's (another program has it).
pub async fn take_name(connection: &Connection, name: &str) -> Result<(), String> {
    match connection
        .request_name_with_flags(name, RequestNameFlags::DO_NOT_QUEUE)
        .await
    {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => Ok(()),
        Ok(reply) => Err(format!("another program has it ({reply:?})")),
        Err(err) => Err(err.to_string()),
    }
}

/// The bus's `NameOwnerChanged` signals, from now on.
pub async fn name_owner_changes(
    connection: &Connection,
) -> zbus::Result<fdo::NameOwnerChangedStream> {
    fdo::DBusProxy::new(connection)
        .await?
        .receive_name_owner_changed()
        .await
}

/// The next of `changes` (`None`: the connection closed).
pub async fn next_change(
    changes: &mut fdo::NameOwnerChangedStream,
) -> Option<fdo::NameOwnerChanged> {
    use zbus::export::futures_core::Stream;
    std::future::poll_fn(|cx| std::pin::Pin::new(&mut *changes).poll_next(cx)).await
}

/// Give the bus at `address` the variables `vars` (name, value) for the
/// services it starts and, with `systemd`, `systemd --user` too, each
/// waiting at most `timeout` for an answer.
pub fn export(
    address: &str,
    systemd: bool,
    vars: &[(String, String)],
    timeout: Duration,
) -> Result<(), String> {
    let bus = zbus::block_on(
        zbus::connection::Builder::address(address)
            .method_timeout(timeout)
            .build(),
    )
    .map_err(|err| format!("can't connect to the session bus at {address}: {err}"))?;
    update_activation_environment(&bus, vars)?;
    if systemd {
        set_systemd_environment(&bus, vars)?;
    }
    Ok(())
}

fn update_activation_environment(
    bus: &Connection,
    vars: &[(String, String)],
) -> Result<(), String> {
    let vars: HashMap<&str, &str> = vars
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    zbus::block_on(bus.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "UpdateActivationEnvironment",
        &(vars,),
    ))
    .map(drop)
    .map_err(|err| format!("the session bus refused the environment: {err}"))
}

fn set_systemd_environment(bus: &Connection, vars: &[(String, String)]) -> Result<(), String> {
    let assignments: Vec<String> = vars
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    zbus::block_on(bus.call_method(
        Some("org.freedesktop.systemd1"),
        "/org/freedesktop/systemd1",
        Some("org.freedesktop.systemd1.Manager"),
        "SetEnvironment",
        &(assignments,),
    ))
    .map(drop)
    .map_err(|err| format!("systemd --user refused the environment: {err}"))
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader},
        path::Path,
        process::{Child, Command, Stdio},
        sync::{Arc, Mutex},
        time::Instant,
    };

    use super::*;

    const TIMEOUT: Duration = Duration::from_secs(5);

    /// A private bus whose one activatable service writes the environment
    /// the bus starts it with to `env-of-service` in `dir`.
    struct Bus {
        daemon: Child,
        address: String,
    }

    impl Bus {
        /// `None` without dbus-daemon (package dbus).
        fn start(dir: &Path) -> Option<Bus> {
            let services = dir.join("services");
            std::fs::create_dir_all(&services).unwrap();
            let out = dir.join("env-of-service");
            std::fs::write(
                services.join("org.pila.EnvTest.service"),
                format!(
                    "[D-BUS Service]\nName=org.pila.EnvTest\nExec=/bin/sh -c 'env > {out}.new && mv {out}.new {out}'\n",
                    out = out.display()
                ),
            )
            .unwrap();
            let config = dir.join("bus.conf");
            std::fs::write(
                &config,
                format!(
                    "<busconfig>\
                     <type>session</type>\
                     <listen>unix:dir={dir}</listen>\
                     <servicedir>{services}</servicedir>\
                     <policy context=\"default\">\
                     <allow send_destination=\"*\" eavesdrop=\"true\"/>\
                     <allow eavesdrop=\"true\"/>\
                     <allow own=\"*\"/>\
                     </policy>\
                     </busconfig>",
                    dir = dir.display(),
                    services = services.display(),
                ),
            )
            .unwrap();
            let Ok(mut daemon) = Command::new("dbus-daemon")
                .arg(format!("--config-file={}", config.display()))
                .args(["--nofork", "--print-address=1"])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
            else {
                eprintln!("skipped: no dbus-daemon");
                return None;
            };
            let mut address = String::new();
            BufReader::new(daemon.stdout.take().unwrap())
                .read_line(&mut address)
                .unwrap();
            Some(Bus {
                daemon,
                address: address.trim().to_string(),
            })
        }

        /// The environment the bus starts its service with.
        fn service_env(&self, dir: &Path) -> Vec<String> {
            zbus::block_on(async {
                let bus = zbus::connection::Builder::address(self.address.as_str())
                    .method_timeout(Duration::from_millis(100))
                    .build()
                    .await
                    .unwrap();
                // The service never takes its name: the answer is an error,
                // late, and the file is what counts.
                let _ = bus
                    .call_method(
                        Some("org.freedesktop.DBus"),
                        "/org/freedesktop/DBus",
                        Some("org.freedesktop.DBus"),
                        "StartServiceByName",
                        &("org.pila.EnvTest", 0u32),
                    )
                    .await;
            });
            let out = dir.join("env-of-service");
            let started = Instant::now();
            loop {
                // Written whole: the service moves it into place.
                if let Ok(text) = std::fs::read_to_string(&out) {
                    return text.lines().map(str::to_string).collect();
                }
                assert!(started.elapsed() < TIMEOUT, "the service never ran");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = self.daemon.kill();
            let _ = self.daemon.wait();
        }
    }

    /// systemd's manager as far as `SetEnvironment` goes, recording.
    struct FakeSystemd {
        set: Arc<Mutex<Vec<String>>>,
    }

    #[zbus::interface(name = "org.freedesktop.systemd1.Manager")]
    impl FakeSystemd {
        fn set_environment(&self, assignments: Vec<String>) {
            self.set.lock().unwrap().extend(assignments);
        }
    }

    fn vars() -> Vec<(String, String)> {
        vec![
            ("WAYLAND_DISPLAY".into(), "wayland-7".into()),
            ("XDG_CURRENT_DESKTOP".into(), "pila".into()),
            ("A_VALUE".into(), "with spaces = and signs".into()),
        ]
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pila-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn what_the_bus_starts_gets_the_environment() {
        let dir = temp_dir("session-bus-env");
        let Some(bus) = Bus::start(&dir) else { return };
        export(&bus.address, false, &vars(), TIMEOUT).unwrap();
        let env = bus.service_env(&dir);
        for (name, value) in vars() {
            assert!(
                env.contains(&format!("{name}={value}")),
                "{name} not in {env:#?}"
            );
        }
        drop(bus);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn on_the_systemd_user_bus_systemd_gets_the_environment_too() {
        let dir = temp_dir("session-bus-systemd");
        let Some(bus) = Bus::start(&dir) else { return };
        let set = Arc::new(Mutex::new(Vec::new()));
        let _systemd = zbus::block_on(
            zbus::connection::Builder::address(bus.address.as_str())
                .name("org.freedesktop.systemd1")
                .serve_at(
                    "/org/freedesktop/systemd1",
                    FakeSystemd { set: set.clone() },
                )
                .build(),
        )
        .unwrap();
        export(&bus.address, true, &vars(), TIMEOUT).unwrap();
        let expected: Vec<String> = vars()
            .into_iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        assert_eq!(*set.lock().unwrap(), expected);
        assert!(bus.service_env(&dir).contains(&expected[0]));
        drop(bus);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_systemd_on_its_bus_the_export_says_so() {
        let dir = temp_dir("session-bus-no-systemd");
        let Some(bus) = Bus::start(&dir) else { return };
        let err = export(&bus.address, true, &vars(), TIMEOUT).unwrap_err();
        assert!(err.contains("systemd --user"), "{err}");
        drop(bus);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_bus_is_an_error_not_a_hang() {
        let err = export("unix:path=/nonexistent/pila-bus", false, &vars(), TIMEOUT).unwrap_err();
        assert!(err.contains("can't connect"), "{err}");
    }
}
