//! A PipeWire daemon of the test's own, and a consumer that takes frames
//! from a node on it, as an app sharing the screen would: give Pila the
//! daemon with `Options::env` (`Daemon::env`).
//!
//! No session manager runs, so the consumer links itself to the node
//! (`link-factory`), as WirePlumber would.

use std::{
    cell::RefCell,
    collections::HashMap,
    path::PathBuf,
    process::{Child, Command, Stdio},
    rc::Rc,
    sync::mpsc,
    time::Instant,
};

use pipewire as pw;
use pw::{properties::properties, spa};
use zbus::{Connection, OwnedObjectPath, OwnedVariant};

use super::TIMEOUT;

/// A private PipeWire daemon, gone when dropped.
pub struct Daemon {
    daemon: Child,
    /// Its runtime directory: short, since a socket's path must fit in 108
    /// bytes.
    pub dir: PathBuf,
}

impl Daemon {
    pub fn start() -> Daemon {
        let dir = std::env::temp_dir().join(format!(
            "pw-{}-{}",
            std::process::id(),
            super::COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let daemon = Command::new("pipewire")
            .env("PIPEWIRE_RUNTIME_DIR", &dir)
            .env("XDG_RUNTIME_DIR", &dir)
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("pipewire (package pipewire)");
        let socket = dir.join("pipewire-0");
        let started = Instant::now();
        while !socket.exists() {
            assert!(started.elapsed() < TIMEOUT, "pipewire didn't start");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Daemon { daemon, dir }
    }

    /// The daemon for Pila: `Options::env`'s entry.
    pub fn env(&self) -> (String, String) {
        (
            "PIPEWIRE_RUNTIME_DIR".into(),
            self.dir.to_string_lossy().into_owned(),
        )
    }

    fn socket(&self) -> String {
        self.dir.join("pipewire-0").to_string_lossy().into_owned()
    }

    /// Link a consumer to `node` and hand its frames to the returned
    /// receiver, until it's dropped.
    pub fn consume(&self, node: u32) -> Consumer {
        let socket = self.socket();
        let (frames, receiver) = mpsc::channel();
        let (stop_sender, stop) = pw::channel::channel::<()>();
        let thread = std::thread::spawn(move || consume(&socket, node, frames, stop));
        Consumer {
            frames: receiver,
            stop: Some(stop_sender),
            thread: Some(thread),
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A frame the consumer got: BGRx, rows `stride` bytes apart.
#[derive(Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub data: Vec<u8>,
}

impl Frame {
    /// The pixel at (x, y), as RGBA.
    pub fn rgba(&self, x: u32, y: u32) -> [u8; 4] {
        let i = y as usize * self.stride + x as usize * 4;
        let p = &self.data[i..i + 4];
        [p[2], p[1], p[0], 255]
    }
}

pub struct Consumer {
    frames: mpsc::Receiver<Frame>,
    stop: Option<pw::channel::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Consumer {
    /// The next frame that `is` accepts, dropping the others.
    #[track_caller]
    pub fn wait_frame(&self, what: &str, is: impl Fn(&Frame) -> bool) -> Frame {
        let started = Instant::now();
        loop {
            let left = TIMEOUT.saturating_sub(started.elapsed());
            match self.frames.recv_timeout(left) {
                Ok(frame) if is(&frame) => return frame,
                Ok(_) => {}
                Err(_) => panic!("no frame {what} within {TIMEOUT:?}"),
            }
        }
    }

    /// Frames that came and weren't taken yet.
    pub fn pending(&self) -> Vec<Frame> {
        self.frames.try_iter().collect()
    }
}

impl Drop for Consumer {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn consume(socket: &str, node: u32, frames: mpsc::Sender<Frame>, stop: pw::channel::Receiver<()>) {
    pw::init();
    let main_loop = pw::main_loop::MainLoopRc::new(None).unwrap();
    let context = pw::context::ContextRc::new(&main_loop, None).unwrap();
    let core = context
        .connect_rc(Some(properties! { *pw::keys::REMOTE_NAME => socket }))
        .expect("connect to the test's pipewire");
    let quit = main_loop.clone();
    let _stop = stop.attach(main_loop.loop_(), move |()| quit.quit());

    let stream = pw::stream::StreamRc::new(
        core.clone(),
        "test-consumer",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .unwrap();
    let format: Rc<RefCell<spa::param::video::VideoInfoRaw>> = Rc::default();
    // Once both nodes have their port, link them.
    let links = Rc::new(RefCell::new(Linker {
        core: core.clone(),
        source: node,
        sink: None,
        ports: Vec::new(),
        link: None,
    }));
    let registry = core.get_registry_rc().unwrap();
    let registry_links = links.clone();
    let _registry_listener = registry
        .add_listener_local()
        .global(move |global| {
            if global.type_ != pw::types::ObjectType::Port {
                return;
            }
            let Some(props) = global.props else {
                return;
            };
            let (Some(node), Some(direction)) = (props.get("node.id"), props.get("port.direction"))
            else {
                return;
            };
            let Ok(node) = node.parse() else {
                return;
            };
            let mut linker = registry_links.borrow_mut();
            linker.ports.push((node, direction == "out", global.id));
            linker.try_link();
        })
        .register();
    let parsed = format.clone();
    let _listener = stream
        .add_local_listener_with_user_data(())
        .state_changed(move |stream, _, _, new| {
            if matches!(new, pw::stream::StreamState::Paused) {
                let mut linker = links.borrow_mut();
                linker.sink = Some(stream.node_id());
                linker.try_link();
            }
        })
        .param_changed(move |_, _, id, param| {
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            if let Some(param) = param {
                let _ = parsed.borrow_mut().parse(param);
            }
        })
        .process(move |stream, _| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let info = *format.borrow();
            let datas = buffer.datas_mut();
            let Some(data) = datas.first_mut() else {
                return;
            };
            let (size, stride) = (data.chunk().size() as usize, data.chunk().stride());
            if size == 0 {
                return;
            }
            let Some(bytes) = data.data() else {
                return;
            };
            let _ = frames.send(Frame {
                width: info.size().width,
                height: info.size().height,
                stride: stride as usize,
                data: bytes[..size].to_vec(),
            });
        })
        .register()
        .unwrap();

    // Any raw video: what the node offers is fixed.
    let object = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaType,
            Id,
            spa::param::format::MediaType::Video
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaSubtype,
            Id,
            spa::param::format::MediaSubtype::Raw
        ),
    );
    let bytes = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )
    .unwrap()
    .0
    .into_inner();
    let mut params = [spa::pod::Pod::from_bytes(&bytes).unwrap()];
    stream
        .connect(
            spa::utils::Direction::Input,
            None,
            pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .unwrap();
    main_loop.run();
}

/// Links the shared node's output port to the consumer's input port, as a
/// session manager would.
struct Linker {
    core: pw::core::CoreRc,
    source: u32,
    sink: Option<u32>,
    /// Ports seen: node, whether an output, id.
    ports: Vec<(u32, bool, u32)>,
    link: Option<pw::link::Link>,
}

impl Linker {
    fn try_link(&mut self) {
        let Some(sink) = self.sink else {
            return;
        };
        if self.link.is_some() {
            return;
        }
        let port = |node: u32, output: bool| {
            self.ports
                .iter()
                .find(|&&(n, out, _)| n == node && out == output)
                .map(|&(_, _, id)| id)
        };
        let (Some(output), Some(input)) = (port(self.source, true), port(sink, false)) else {
            return;
        };
        self.link = self
            .core
            .create_object::<pw::link::Link>(
                "link-factory",
                &properties! {
                    "link.output.node" => self.source.to_string(),
                    "link.output.port" => output.to_string(),
                    "link.input.node" => sink.to_string(),
                    "link.input.port" => input.to_string(),
                    "object.linger" => "false",
                },
            )
            .ok();
    }
}

/// A `Start` waiting for its answer.
pub struct Started {
    thread: std::thread::JoinHandle<(u32, Vec<u32>)>,
}

impl Started {
    /// The response (0 shared, 1 cancelled, 2 failed) and the streams'
    /// node ids.
    pub fn answer(self) -> (u32, Vec<u32>) {
        self.thread.join().expect("Start's thread")
    }

    /// Whether it was answered yet.
    pub fn answered(&self) -> bool {
        self.thread.is_finished()
    }
}

/// The portal Pila serves, called as xdg-desktop-portal calls it.
pub struct Portal {
    bus: Connection,
    serial: usize,
}

type Vardict = HashMap<String, OwnedVariant>;

const NAME: &str = "org.freedesktop.impl.portal.desktop.pila";
const PATH: &str = "/org/freedesktop/portal/desktop";
const SCREENCAST: &str = "org.freedesktop.impl.portal.ScreenCast";

impl Portal {
    /// Connect to the bus at `address` once Pila serves the portal there.
    pub fn connect(address: &str) -> Portal {
        let bus = zbus::block_on(zbus::connection::Builder::address(address).build()).unwrap();
        let dbus = zbus::block_on(zbus::fdo::DBusProxy::new(&bus)).unwrap();
        let started = Instant::now();
        loop {
            let name = zbus::names::BusName::try_from(NAME).unwrap();
            if zbus::block_on(dbus.name_has_owner(name)).unwrap_or(false) {
                break;
            }
            assert!(started.elapsed() < TIMEOUT, "Pila didn't serve the portal");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Portal { bus, serial: 0 }
    }

    fn call(
        &self,
        method: &str,
        body: &(impl serde::Serialize + zbus::zvariant::DynamicType),
    ) -> (u32, Vardict) {
        let reply =
            zbus::block_on(
                self.bus
                    .call_method(Some(NAME), PATH, Some(SCREENCAST), method, body),
            )
            .unwrap_or_else(|err| panic!("{method}: {err}"));
        reply.body().deserialize().unwrap()
    }

    fn handle(&mut self) -> OwnedObjectPath {
        self.serial += 1;
        OwnedObjectPath::try_from(format!("{PATH}/request/1_1/t{}", self.serial)).unwrap()
    }

    /// Create a session and select a monitor (`cursor_mode` 1: hidden, 2:
    /// embedded), then start it, as app `app_id`: Pila shows its picker, so
    /// `Start`'s answer comes once the test picks (`Started::answer`).
    pub fn request(&mut self, name: &str, app_id: &str, cursor_mode: u32) -> Started {
        let session = OwnedObjectPath::try_from(format!("{PATH}/session/1_1/{name}")).unwrap();
        let handle = self.handle();
        let (response, _) = self.call("CreateSession", &(handle, &session, app_id, Vardict::new()));
        assert_eq!(response, 0, "CreateSession");
        let options = HashMap::from([
            ("types".to_string(), OwnedVariant::from(1u32)),
            ("cursor_mode".to_string(), OwnedVariant::from(cursor_mode)),
        ]);
        let handle = self.handle();
        let (response, _) = self.call("SelectSources", &(handle, &session, app_id, options));
        assert_eq!(response, 0, "SelectSources");
        let handle = self.handle();
        let bus = self.bus.clone();
        let app_id = app_id.to_string();
        let thread = std::thread::spawn(move || {
            let reply = zbus::block_on(bus.call_method(
                Some(NAME),
                PATH,
                Some(SCREENCAST),
                "Start",
                &(handle, &session, app_id.as_str(), "", Vardict::new()),
            ))
            .unwrap_or_else(|err| panic!("Start: {err}"));
            let (response, results): (u32, Vardict) = reply.body().deserialize().unwrap();
            let nodes = results
                .get("streams")
                .map(|streams| {
                    let streams: Vec<(u32, Vardict)> =
                        streams.try_clone().unwrap().try_into().unwrap();
                    streams.into_iter().map(|(node, _)| node).collect()
                })
                .unwrap_or_default();
            (response, nodes)
        });
        Started { thread }
    }

    /// Close the session `name`, as xdg-desktop-portal does when the app
    /// stops sharing.
    pub fn close(&self, name: &str) {
        let session = format!("{PATH}/session/1_1/{name}");
        zbus::block_on(self.bus.call_method(
            Some(NAME),
            session.as_str(),
            Some("org.freedesktop.impl.portal.Session"),
            "Close",
            &(),
        ))
        .unwrap();
    }
}
