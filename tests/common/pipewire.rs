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
        self.consume_as(node, false)
    }

    /// `consume`, taking dma-bufs: linear ones, which it can map to read.
    pub fn consume_dmabufs(&self, node: u32) -> Consumer {
        self.consume_as(node, true)
    }

    fn consume_as(&self, node: u32, dmabufs: bool) -> Consumer {
        let socket = self.socket();
        let (frames, receiver) = mpsc::channel();
        let (stop_sender, stop) = pw::channel::channel::<()>();
        let thread = std::thread::spawn(move || consume(&socket, node, frames, stop, dmabufs));
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

/// A buffer the consumer got: a frame (BGRx, rows `stride` bytes apart),
/// or none (`data` empty: the pointer alone), and the pointer's metadata if
/// it had some.
#[derive(Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub data: Vec<u8>,
    pub cursor: Option<CursorMeta>,
    /// It came in a dma-buf (mapped to read it).
    pub dmabuf: bool,
    /// Its `SPA_META_Header`, if it had one: what GStreamer's pipewiresrc
    /// takes its timestamps from.
    pub header: Option<Header>,
}

/// A buffer's `SPA_META_Header`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Header {
    /// Nanoseconds, of `CLOCK_MONOTONIC`.
    pub pts: i64,
    pub seq: u64,
    pub flags: u32,
}

/// A buffer's cursor metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorMeta {
    pub position: (i32, i32),
    pub hotspot: (i32, i32),
    /// A new image, if it came with one: width, height, BGRA pixels.
    pub bitmap: Option<(u32, u32, Vec<u8>)>,
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
    /// The next frame (with a picture) that `is` accepts, dropping the
    /// other buffers.
    #[track_caller]
    pub fn wait_frame(&self, what: &str, is: impl Fn(&Frame) -> bool) -> Frame {
        self.wait_buffer(what, |frame| !frame.data.is_empty() && is(frame))
    }

    /// The next buffer, with a picture or not, that `is` accepts.
    #[track_caller]
    pub fn wait_buffer(&self, what: &str, is: impl Fn(&Frame) -> bool) -> Frame {
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

fn consume(
    socket: &str,
    node: u32,
    frames: mpsc::Sender<Frame>,
    stop: pw::channel::Receiver<()>,
    dmabufs: bool,
) {
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
        .param_changed(move |stream, _, id, param| {
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Some(param) = param else {
                return;
            };
            let _ = parsed.borrow_mut().parse(param);
            // Room for the cursor's metadata, as browsers ask for it
            // (libwebrtc: a 64 to 384 px square image).
            // And for a header, as GStreamer's pipewiresrc asks for one.
            let mut params = vec![cursor_meta_param(), header_meta_param()];
            // The buffers it takes, as libwebrtc says it: dma-bufs too.
            if dmabufs {
                params.push(buffer_types_param(
                    1 << spa::sys::SPA_DATA_DmaBuf | 1 << spa::sys::SPA_DATA_MemFd,
                ));
            }
            let mut pods: Vec<&spa::pod::Pod> = params
                .iter()
                .filter_map(|bytes| spa::pod::Pod::from_bytes(bytes))
                .collect();
            let _ = stream.update_params(&mut pods);
        })
        .process(move |stream, _| {
            // SAFETY: the stream is alive; the buffer is ours until queued.
            let buffer = unsafe { stream.dequeue_raw_buffer() };
            if buffer.is_null() {
                return;
            }
            let info = *format.borrow();
            // SAFETY: a dequeued buffer of a stream mapped with MAP_BUFFERS.
            let frame = unsafe { read_buffer(buffer, info.size().width, info.size().height) };
            // SAFETY: dequeued from this stream.
            unsafe { stream.queue_raw_buffer(buffer) };
            if let Some(frame) = frame {
                let _ = frames.send(frame);
            }
        })
        .register()
        .unwrap();

    // Any raw video: what the node offers is fixed. Taking dma-bufs: linear
    // ones, for the app to pick from (as libwebrtc offers its modifiers).
    let mut object = spa::pod::object!(
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
    if dmabufs {
        const LINEAR: i64 = 0;
        object.properties.push(spa::pod::Property {
            key: spa::param::format::FormatProperties::VideoModifier.as_raw(),
            flags: spa::pod::PropertyFlags::from_bits_retain(
                spa::sys::SPA_POD_PROP_FLAG_MANDATORY | spa::sys::SPA_POD_PROP_FLAG_DONT_FIXATE,
            ),
            value: spa::pod::Value::Choice(spa::pod::ChoiceValue::Long(spa::utils::Choice(
                spa::utils::ChoiceFlags::empty(),
                spa::utils::ChoiceEnum::Enum {
                    default: LINEAR,
                    alternatives: vec![LINEAR],
                },
            ))),
        });
    }
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
            // Shared memory mapped by PipeWire; dma-bufs by `read_buffer`.
            if dmabufs {
                pw::stream::StreamFlags::empty()
            } else {
                pw::stream::StreamFlags::MAP_BUFFERS
            },
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
    thread: std::thread::JoinHandle<(u32, Vec<(u32, Vardict)>)>,
}

impl Started {
    /// The response (0 shared, 1 cancelled, 2 failed) and the streams'
    /// node ids.
    pub fn answer(self) -> (u32, Vec<u32>) {
        let (response, streams) = self.answer_streams();
        (
            response,
            streams.into_iter().map(|(node, _)| node).collect(),
        )
    }

    /// The response and the streams: each node id and its properties
    /// (`source_type`, `size`, a monitor's `position`).
    pub fn answer_streams(self) -> (u32, Vec<(u32, Vardict)>) {
        self.thread.join().expect("Start's thread")
    }

    /// Whether it was answered yet.
    pub fn answered(&self) -> bool {
        self.thread.is_finished()
    }
}

/// The cursor metadata a consumer asks for: room for an image 64 to 384
/// px square, as libwebrtc asks.
fn cursor_meta_param() -> Vec<u8> {
    use spa::pod::{ChoiceValue, Object, Property, Value};
    use spa::utils::{Choice, ChoiceEnum, ChoiceFlags};
    let size = |side: i32| {
        (std::mem::size_of::<spa::sys::spa_meta_cursor>()
            + std::mem::size_of::<spa::sys::spa_meta_bitmap>()) as i32
            + side * side * 4
    };
    let object = Object {
        type_: spa::utils::SpaTypes::ObjectParamMeta.as_raw(),
        id: spa::param::ParamType::Meta.as_raw(),
        properties: vec![
            Property::new(
                spa::sys::SPA_PARAM_META_type,
                Value::Id(spa::utils::Id(spa::sys::SPA_META_Cursor)),
            ),
            Property::new(
                spa::sys::SPA_PARAM_META_size,
                Value::Choice(ChoiceValue::Int(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Range {
                        default: size(64),
                        min: size(1),
                        max: size(384),
                    },
                ))),
            ),
        ],
    };
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &Value::Object(object),
    )
    .unwrap()
    .0
    .into_inner()
}

/// The `SPA_META_Header` a consumer asks for.
fn header_meta_param() -> Vec<u8> {
    use spa::pod::{Object, Property, Value};
    let object = Object {
        type_: spa::utils::SpaTypes::ObjectParamMeta.as_raw(),
        id: spa::param::ParamType::Meta.as_raw(),
        properties: vec![
            Property::new(
                spa::sys::SPA_PARAM_META_type,
                Value::Id(spa::utils::Id(spa::sys::SPA_META_Header)),
            ),
            Property::new(
                spa::sys::SPA_PARAM_META_size,
                Value::Int(std::mem::size_of::<spa::sys::spa_meta_header>() as i32),
            ),
        ],
    };
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &Value::Object(object),
    )
    .unwrap()
    .0
    .into_inner()
}

/// The `Buffers` a consumer asks for: of the data types in `types`.
fn buffer_types_param(types: i32) -> Vec<u8> {
    use spa::pod::{ChoiceValue, Object, Property, Value};
    use spa::utils::{Choice, ChoiceEnum, ChoiceFlags};
    let object = Object {
        type_: spa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
        id: spa::param::ParamType::Buffers.as_raw(),
        properties: vec![Property::new(
            spa::sys::SPA_PARAM_BUFFERS_dataType,
            Value::Choice(ChoiceValue::Int(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Flags {
                    default: types,
                    flags: vec![],
                },
            ))),
        )],
    };
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &Value::Object(object),
    )
    .unwrap()
    .0
    .into_inner()
}

/// What a dequeued `buffer` holds: its picture (if any) and its cursor
/// metadata (if any).
///
/// SAFETY: `buffer` is dequeued from a live stream mapped with MAP_BUFFERS.
unsafe fn read_buffer(buffer: *mut pw::sys::pw_buffer, width: u32, height: u32) -> Option<Frame> {
    use spa::sys::{spa_meta_bitmap, spa_meta_cursor};
    // SAFETY: per this function's contract; every pointer is checked and
    // every range read is within the buffer's own sizes.
    unsafe {
        let spa_buffer = (*buffer).buffer;
        if spa_buffer.is_null() || (*spa_buffer).n_datas == 0 {
            return None;
        }
        let data = &*(*spa_buffer).datas;
        let chunk = &*data.chunk;
        let size = (chunk.size as usize).min(data.maxsize as usize);
        let dmabuf = data.type_ == spa::sys::SPA_DATA_DmaBuf;
        let corrupted = chunk.flags & spa::sys::SPA_CHUNK_FLAG_CORRUPTED as i32 != 0;
        let pixels = if size == 0 || corrupted {
            Vec::new()
        } else if dmabuf {
            // Linear: its rows as they are, from the chunk's offset.
            let len = chunk.offset as usize + size;
            let map = libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                data.fd as i32,
                0,
            );
            if map == libc::MAP_FAILED {
                Vec::new()
            } else {
                let bytes =
                    std::slice::from_raw_parts(map.cast::<u8>().add(chunk.offset as usize), size)
                        .to_vec();
                libc::munmap(map, len);
                bytes
            }
        } else if data.data.is_null() {
            Vec::new()
        } else {
            std::slice::from_raw_parts(data.data.cast::<u8>(), size).to_vec()
        };
        let meta = spa::sys::spa_buffer_find_meta(spa_buffer, spa::sys::SPA_META_Cursor);
        let cursor = (!meta.is_null() && !(*meta).data.is_null()).then(|| {
            let cursor = &*(*meta).data.cast::<spa_meta_cursor>();
            let bitmap = (cursor.id != 0 && cursor.bitmap_offset != 0).then(|| {
                let bitmap = (*meta)
                    .data
                    .cast::<u8>()
                    .add(cursor.bitmap_offset as usize)
                    .cast::<spa_meta_bitmap>();
                let (w, h) = ((*bitmap).size.width, (*bitmap).size.height);
                let bytes = std::slice::from_raw_parts(
                    bitmap.cast::<u8>().add((*bitmap).offset as usize),
                    (w * h * 4) as usize,
                );
                (w, h, bytes.to_vec())
            });
            CursorMeta {
                position: (cursor.position.x, cursor.position.y),
                hotspot: (cursor.hotspot.x, cursor.hotspot.y),
                bitmap,
            }
        });
        if pixels.is_empty() && cursor.is_none() {
            return None;
        }
        let meta = spa::sys::spa_buffer_find_meta(spa_buffer, spa::sys::SPA_META_Header);
        let header = (!meta.is_null()
            && !(*meta).data.is_null()
            && (*meta).size as usize >= std::mem::size_of::<spa::sys::spa_meta_header>())
        .then(|| {
            let header = &*(*meta).data.cast::<spa::sys::spa_meta_header>();
            Header {
                pts: header.pts,
                seq: header.seq,
                flags: header.flags,
            }
        });
        Some(Frame {
            width,
            height,
            stride: chunk.stride as usize,
            data: pixels,
            cursor,
            dmabuf,
            header,
        })
    }
}

/// The portal Pila serves, called as xdg-desktop-portal calls it.
pub struct Portal {
    bus: Connection,
    serial: usize,
    /// The app's window its requests say they come from (`parent_window`,
    /// `wayland:<xdg-foreign handle>`); none by default.
    pub parent_window: String,
}

pub type Vardict = HashMap<String, OwnedVariant>;
/// A color as the portal answers it: red, green, blue, from 0 to 1.
pub type Rgb = (f64, f64, f64);

const NAME: &str = "org.freedesktop.impl.portal.desktop.pila";
const PATH: &str = "/org/freedesktop/portal/desktop";
const SCREENCAST: &str = "org.freedesktop.impl.portal.ScreenCast";
const SHORTCUTS: &str = "org.freedesktop.impl.portal.GlobalShortcuts";

/// `BindShortcuts`' and `ListShortcuts`' shortcuts: each id and trigger
/// description.
fn bound_shortcuts(results: &Vardict) -> Vec<(String, String)> {
    let Some(shortcuts) = results.get("shortcuts") else {
        return Vec::new();
    };
    let shortcuts: Vec<(String, Vardict)> = shortcuts.try_clone().unwrap().try_into().unwrap();
    shortcuts
        .into_iter()
        .map(|(id, properties)| {
            let trigger = properties
                .get("trigger_description")
                .map(|v| String::try_from(v.try_clone().unwrap()).unwrap())
                .unwrap_or_default();
            (id, trigger)
        })
        .collect()
}

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
        Portal {
            bus,
            serial: 0,
            parent_window: String::new(),
        }
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
        self.request_sources(name, app_id, cursor_mode, 1)
    }

    /// `request`, selecting the source `types` (1: monitors, 2: windows, 3:
    /// both).
    pub fn request_sources(
        &mut self,
        name: &str,
        app_id: &str,
        cursor_mode: u32,
        types: u32,
    ) -> Started {
        let session = OwnedObjectPath::try_from(format!("{PATH}/session/1_1/{name}")).unwrap();
        let handle = self.handle();
        let (response, _) = self.call("CreateSession", &(handle, &session, app_id, Vardict::new()));
        assert_eq!(response, 0, "CreateSession");
        let options = HashMap::from([
            ("types".to_string(), OwnedVariant::from(types)),
            ("cursor_mode".to_string(), OwnedVariant::from(cursor_mode)),
        ]);
        let handle = self.handle();
        let (response, _) = self.call("SelectSources", &(handle, &session, app_id, options));
        assert_eq!(response, 0, "SelectSources");
        let handle = self.handle();
        let bus = self.bus.clone();
        let app_id = app_id.to_string();
        let parent = self.parent_window.clone();
        let thread = std::thread::spawn(move || {
            let reply = zbus::block_on(bus.call_method(
                Some(NAME),
                PATH,
                Some(SCREENCAST),
                "Start",
                &(
                    handle,
                    &session,
                    app_id.as_str(),
                    parent.as_str(),
                    Vardict::new(),
                ),
            ))
            .unwrap_or_else(|err| panic!("Start: {err}"));
            let (response, results): (u32, Vardict) = reply.body().deserialize().unwrap();
            let streams = results
                .get("streams")
                .map(|streams| {
                    let streams: Vec<(u32, Vardict)> =
                        streams.try_clone().unwrap().try_into().unwrap();
                    streams
                })
                .unwrap_or_default();
            (response, streams)
        });
        Started { thread }
    }

    /// Ask for a screenshot as app `app_id`, as xdg-desktop-portal asks:
    /// `interactive` (the app wants to choose), `checked` (its permission
    /// store lets it take one unasked). Its answer comes on a thread: the
    /// response and the picture's URI.
    pub fn screenshot(
        &mut self,
        app_id: &str,
        interactive: bool,
        checked: bool,
    ) -> std::thread::JoinHandle<(u32, Option<String>)> {
        let handle = self.handle();
        let bus = self.bus.clone();
        let app_id = app_id.to_string();
        let parent = self.parent_window.clone();
        std::thread::spawn(move || {
            let options = HashMap::from([
                ("interactive".to_string(), OwnedVariant::from(interactive)),
                (
                    "permission_store_checked".to_string(),
                    OwnedVariant::from(checked),
                ),
            ]);
            let reply = zbus::block_on(bus.call_method(
                Some(NAME),
                PATH,
                Some("org.freedesktop.impl.portal.Screenshot"),
                "Screenshot",
                &(handle, app_id.as_str(), parent.as_str(), options),
            ))
            .unwrap_or_else(|err| panic!("Screenshot: {err}"));
            let (response, results): (u32, Vardict) = reply.body().deserialize().unwrap();
            let uri = results
                .get("uri")
                .map(|uri| String::try_from(uri.try_clone().unwrap()).unwrap());
            (response, uri)
        })
    }

    /// Ask for a color picked on the screen, as app `app_id`: the response
    /// and the color (each channel 0 to 1), on a thread.
    pub fn pick_color(&mut self, app_id: &str) -> std::thread::JoinHandle<(u32, Option<Rgb>)> {
        let handle = self.handle();
        let bus = self.bus.clone();
        let app_id = app_id.to_string();
        std::thread::spawn(move || {
            let reply = zbus::block_on(bus.call_method(
                Some(NAME),
                PATH,
                Some("org.freedesktop.impl.portal.Screenshot"),
                "PickColor",
                &(handle, app_id.as_str(), "", Vardict::new()),
            ))
            .unwrap_or_else(|err| panic!("PickColor: {err}"));
            let (response, results): (u32, Vardict) = reply.body().deserialize().unwrap();
            let color = results
                .get("color")
                .map(|color| Rgb::try_from(color.try_clone().unwrap()).unwrap());
            (response, color)
        })
    }

    /// Create a global shortcuts session `name` for app `app_id`.
    pub fn shortcuts_session(&mut self, name: &str, app_id: &str) {
        let session = OwnedObjectPath::try_from(format!("{PATH}/session/1_1/{name}")).unwrap();
        let handle = self.handle();
        let reply = zbus::block_on(self.bus.call_method(
            Some(NAME),
            PATH,
            Some(SHORTCUTS),
            "CreateSession",
            &(handle, &session, app_id, Vardict::new()),
        ))
        .unwrap_or_else(|err| panic!("CreateSession: {err}"));
        let (response, _): (u32, Vardict) = reply.body().deserialize().unwrap();
        assert_eq!(response, 0, "CreateSession");
    }

    /// Bind session `name`'s `shortcuts` (id, description, preferred
    /// trigger): the answer comes on a thread, the response and each bound
    /// shortcut's id and trigger description.
    pub fn bind_shortcuts(
        &mut self,
        name: &str,
        shortcuts: &[(&str, &str, &str)],
    ) -> std::thread::JoinHandle<(u32, Vec<(String, String)>)> {
        let session = OwnedObjectPath::try_from(format!("{PATH}/session/1_1/{name}")).unwrap();
        let handle = self.handle();
        let bus = self.bus.clone();
        let parent = self.parent_window.clone();
        let shortcuts: Vec<(String, Vardict)> = shortcuts
            .iter()
            .map(|(id, description, trigger)| {
                (
                    id.to_string(),
                    HashMap::from([
                        (
                            "description".to_string(),
                            OwnedVariant::try_from(zbus::Variant::from(*description)).unwrap(),
                        ),
                        (
                            "preferred_trigger".to_string(),
                            OwnedVariant::try_from(zbus::Variant::from(*trigger)).unwrap(),
                        ),
                    ]),
                )
            })
            .collect();
        std::thread::spawn(move || {
            let reply = zbus::block_on(bus.call_method(
                Some(NAME),
                PATH,
                Some(SHORTCUTS),
                "BindShortcuts",
                &(handle, &session, shortcuts, parent.as_str(), Vardict::new()),
            ))
            .unwrap_or_else(|err| panic!("BindShortcuts: {err}"));
            let (response, results): (u32, Vardict) = reply.body().deserialize().unwrap();
            (response, bound_shortcuts(&results))
        })
    }

    /// The shortcuts session `name` has bound: each id and trigger
    /// description.
    pub fn list_shortcuts(&mut self, name: &str) -> Vec<(String, String)> {
        let session = OwnedObjectPath::try_from(format!("{PATH}/session/1_1/{name}")).unwrap();
        let handle = self.handle();
        let reply = zbus::block_on(self.bus.call_method(
            Some(NAME),
            PATH,
            Some(SHORTCUTS),
            "ListShortcuts",
            &(handle, &session),
        ))
        .unwrap_or_else(|err| panic!("ListShortcuts: {err}"));
        let (response, results): (u32, Vardict) = reply.body().deserialize().unwrap();
        assert_eq!(response, 0, "ListShortcuts");
        bound_shortcuts(&results)
    }

    /// The shortcuts' signals Pila sends from now on, as they come: the
    /// signal's name, the session's object path and the shortcut's id.
    pub fn shortcut_signals(&self) -> mpsc::Receiver<(String, String, String)> {
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface(SHORTCUTS)
            .build()
            .unwrap();
        let stream =
            zbus::block_on(zbus::MessageStream::for_match_rule(rule, &self.bus, None)).unwrap();
        let (send, receive) = mpsc::channel();
        std::thread::spawn(move || {
            zbus::block_on(async move {
                use zbus::export::futures_core::Stream;
                let mut stream = stream;
                while let Some(Ok(message)) =
                    std::future::poll_fn(|cx| std::pin::Pin::new(&mut stream).poll_next(cx)).await
                {
                    let header = message.header();
                    let member = header.member().map(|m| m.to_string()).unwrap_or_default();
                    if member != "Activated" && member != "Deactivated" {
                        continue;
                    }
                    let Ok((session, id, _, _)) =
                        message
                            .body()
                            .deserialize::<(OwnedObjectPath, String, u64, Vardict)>()
                    else {
                        continue;
                    };
                    if send.send((member, session.to_string(), id)).is_err() {
                        break;
                    }
                }
            });
        });
        receive
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
