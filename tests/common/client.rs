//! A synthetic Wayland client for the integration tests: it dispatches on a
//! thread of its own, behaves like a well-mannered app (acks every
//! configure and draws a buffer of the configured size in its color), and
//! records what it was told (configures, input events) for the tests to
//! check. Requests are sent from the test's thread.

use std::{
    io::Write,
    os::{
        fd::{AsFd, AsRawFd, FromRawFd, OwnedFd},
        unix::net::UnixStream,
    },
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use wayland_client::backend::WaylandError;
use wayland_client::{
    delegate_noop,
    protocol::{
        wl_buffer::{self, WlBuffer},
        wl_callback::{self, WlCallback},
        wl_compositor::WlCompositor,
        wl_keyboard::{self, WlKeyboard},
        wl_output::{self, WlOutput},
        wl_pointer::{self, WlPointer},
        wl_registry::{self, WlRegistry},
        wl_seat::{self, WlSeat},
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
        wl_surface::{self, WlSurface},
    },
    Connection, Dispatch, EventQueue, QueueHandle, WEnum,
};
use wayland_protocols::xdg::{
    decoration::zv1::client::{
        zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
        zxdg_toplevel_decoration_v1::{self, ZxdgToplevelDecorationV1},
    },
    shell::client::{
        xdg_surface::{self, XdgSurface},
        xdg_toplevel::{self, XdgToplevel},
        xdg_wm_base::{self, XdgWmBase},
    },
};

use super::TIMEOUT;

/// xdg_toplevel states, as numbers in configures.
pub mod states {
    pub const MAXIMIZED: u32 = 1;
    pub const FULLSCREEN: u32 = 2;
    pub const RESIZING: u32 = 3;
    pub const ACTIVATED: u32 = 4;
    pub const TILED_LEFT: u32 = 5;
    pub const TILED_RIGHT: u32 = 6;
    pub const TILED_TOP: u32 = 7;
    pub const TILED_BOTTOM: u32 = 8;
    pub const SUSPENDED: u32 = 9;
}

/// An xdg_toplevel configure, as received.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Configure {
    pub size: (i32, i32),
    pub states: Vec<u32>,
}

impl Configure {
    pub fn has(&self, state: u32) -> bool {
        self.states.contains(&state)
    }
}

/// An input event, with the window it concerns where there is one.
#[derive(Debug, Clone, PartialEq)]
pub enum Ev {
    KeyboardEnter(Option<usize>),
    KeyboardLeave(Option<usize>),
    Key {
        key: u32,
        pressed: bool,
    },
    Modifiers {
        depressed: u32,
        locked: u32,
        group: u32,
    },
    PointerEnter {
        window: Option<usize>,
        x: f64,
        y: f64,
        serial: u32,
    },
    PointerLeave(Option<usize>),
    Motion {
        x: f64,
        y: f64,
    },
    Button {
        button: u32,
        pressed: bool,
        serial: u32,
    },
    Axis {
        horizontal: bool,
        value: f64,
    },
    AxisV120 {
        horizontal: bool,
        value: i32,
    },
}

/// Server-side decorations (asked through xdg-decoration) or the client's
/// own (no xdg-decoration at all, like GTK).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Decorations {
    Server,
    Client,
    /// Binds xdg-decoration and asks for client-side.
    AskClient,
}

pub struct WindowData {
    pub app_id: String,
    pub surface: WlSurface,
    pub xdg_surface: XdgSurface,
    pub toplevel: Option<XdgToplevel>,
    pub decoration: Option<ZxdgToplevelDecorationV1>,
    /// The mode xdg-decoration settled on (true: server side).
    pub server_side: Option<bool>,
    /// ARGB.
    pub color: [u8; 4],
    /// The size drawn when a configure leaves it to the client.
    pub default_size: (i32, i32),
    /// The size of the last buffer drawn.
    pub size: (i32, i32),
    pub configures: Vec<Configure>,
    pending: Configure,
    /// Acked configures (xdg_surface).
    pub acked: usize,
    /// The compositor asked it to close.
    pub close_requested: bool,
    /// Draw on every configure (else the test draws).
    pub auto_draw: bool,
    pub outputs: Vec<String>,
    pub preferred_scale: Option<i32>,
    pub bounds: Option<(i32, i32)>,
}

#[derive(Default)]
pub struct Shared {
    pub globals: Vec<(u32, String, u32)>,
    pub compositor: Option<WlCompositor>,
    pub shm: Option<WlShm>,
    pub wm_base: Option<XdgWmBase>,
    pub seat: Option<WlSeat>,
    pub decoration_manager: Option<ZxdgDecorationManagerV1>,
    pub keyboard: Option<WlKeyboard>,
    pub pointer: Option<WlPointer>,
    pub windows: Vec<WindowData>,
    pub events: Vec<Ev>,
    pub outputs: Vec<(WlOutput, String)>,
    syncs_done: Vec<u32>,
    next_sync: u32,
    /// The connection failed (protocol error, or pila went away).
    pub error: Option<String>,
    /// See `Client::expect_error`.
    pub expecting_error: bool,
    pub last_serial: u32,
    pub registry: Option<WlRegistry>,
    /// Events of the other protocols (`protocols.rs`), in order.
    pub log: Vec<Logged>,
    /// Layer, popup, lock and other surfaces (`protocols.rs`).
    pub surfaces: Vec<SurfaceData>,
    /// The name of the surface with keyboard / pointer focus: a window's
    /// app id or another surface's name.
    pub keyboard_name: Option<String>,
    pub pointer_name: Option<String>,
    /// The keymap pila sent (format, text).
    pub keymap: Option<(u32, String)>,
    /// The serial of the last pointer enter (for set_cursor).
    pub pointer_enter_serial: u32,
    /// The current selection offers: clipboard, primary, data-control's
    /// clipboard and primary.
    pub data_offer: Option<wayland_client::protocol::wl_data_offer::WlDataOffer>,
    /// The offer of the drag over this client, and its enter serial.
    pub dnd_offer: Option<(wayland_client::protocol::wl_data_offer::WlDataOffer, u32)>,
    pub primary_offer: Option<
        wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1,
    >,
    pub control_offer: Option<
        wayland_protocols::ext::data_control::v1::client::ext_data_control_offer_v1::ExtDataControlOfferV1,
    >,
    pub control_primary_offer: Option<
        wayland_protocols::ext::data_control::v1::client::ext_data_control_offer_v1::ExtDataControlOfferV1,
    >,
    /// Popups' xdg_surfaces and layer surfaces, by surface index (parents
    /// of popups).
    pub popup_xdg_surfaces: Vec<(usize, XdgSurface)>,
    pub layer_surfaces: Vec<
        (
            usize,
            wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        ),
    >,
}

/// An event of a protocol logged generically: the tag the object was
/// created with, its protocol id and the event, as `Debug` prints it.
#[derive(Debug, Clone)]
pub struct Logged {
    pub tag: String,
    pub id: u32,
    pub object: wayland_client::backend::ObjectId,
    pub event: String,
}

/// A surface that isn't a toplevel: a layer surface, popup, lock surface.
pub struct SurfaceData {
    pub name: String,
    pub surface: WlSurface,
    /// ARGB.
    pub color: [u8; 4],
    pub default_size: (i32, i32),
    pub size: (i32, i32),
    /// Sizes configured (popups: x, y too), in order.
    pub configures: Vec<(i32, i32, i32, i32)>,
    pub pending: (i32, i32, i32, i32),
    pub auto_draw: bool,
    /// Closed (layer surface), done (popup), or finished.
    pub closed: bool,
    /// The outputs it entered (wl_surface.enter, less leave), by name.
    pub outputs: Vec<String>,
}

impl Shared {
    fn window_of(&self, surface: &WlSurface) -> Option<usize> {
        self.windows.iter().position(|w| w.surface == *surface)
    }

    /// A window's app id or a surface's name.
    pub fn name_of(&self, surface: &WlSurface) -> Option<String> {
        if let Some(w) = self.windows.iter().find(|w| w.surface == *surface) {
            return Some(w.app_id.clone());
        }
        self.surfaces
            .iter()
            .find(|s| s.surface == *surface)
            .map(|s| s.name.clone())
    }

    /// The logged events with `tag`.
    pub fn logged(&self, tag: &str) -> Vec<String> {
        self.log
            .iter()
            .filter(|l| l.tag == tag)
            .map(|l| l.event.clone())
            .collect()
    }

    /// Whether an event with `tag` contains `text`.
    pub fn has_logged(&self, tag: &str, text: &str) -> bool {
        self.log
            .iter()
            .any(|l| l.tag == tag && l.event.contains(text))
    }
}

pub struct Handler {
    pub(crate) shared: Arc<Mutex<Shared>>,
}

pub struct Client {
    pub conn: Connection,
    pub qh: QueueHandle<Handler>,
    shared: Arc<Mutex<Shared>>,
    socket: UnixStream,
    thread: Option<JoinHandle<()>>,
}

/// A window of a `Client`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window(pub usize);

type BeforeCommit<'a> = Box<dyn FnOnce(&XdgToplevel) + 'a>;

pub struct WindowBuilder<'a> {
    client: &'a Client,
    app_id: String,
    title: String,
    size: (i32, i32),
    color: [u8; 4],
    decorations: Decorations,
    min_size: Option<(i32, i32)>,
    max_size: Option<(i32, i32)>,
    maximized: bool,
    fullscreen: bool,
    before_commit: Option<BeforeCommit<'a>>,
}

impl<'a> WindowBuilder<'a> {
    /// Run requests on the new toplevel before its first commit. (It must
    /// not call the client: it holds its lock.)
    pub fn before_commit(mut self, f: impl FnOnce(&XdgToplevel) + 'a) -> Self {
        self.before_commit = Some(Box::new(f));
        self
    }
    pub fn size(mut self, w: i32, h: i32) -> Self {
        self.size = (w, h);
        self
    }
    /// ARGB.
    pub fn color(mut self, color: [u8; 4]) -> Self {
        self.color = color;
        self
    }
    pub fn title(mut self, title: &str) -> Self {
        self.title = title.to_string();
        self
    }
    pub fn decorations(mut self, decorations: Decorations) -> Self {
        self.decorations = decorations;
        self
    }
    pub fn min_size(mut self, w: i32, h: i32) -> Self {
        self.min_size = Some((w, h));
        self
    }
    pub fn max_size(mut self, w: i32, h: i32) -> Self {
        self.max_size = Some((w, h));
        self
    }
    pub fn maximized(mut self) -> Self {
        self.maximized = true;
        self
    }
    pub fn fullscreen(mut self) -> Self {
        self.fullscreen = true;
        self
    }

    /// Create the window; it maps once it gets its first configure.
    pub fn create(self) -> Window {
        let c = self.client;
        let mut shared = c.lock();
        let index = shared.windows.len();
        let surface = shared
            .compositor
            .as_ref()
            .expect("wl_compositor")
            .create_surface(&c.qh, ());
        let xdg_surface = shared
            .wm_base
            .as_ref()
            .expect("xdg_wm_base")
            .get_xdg_surface(&surface, &c.qh, index);
        let toplevel = xdg_surface.get_toplevel(&c.qh, index);
        toplevel.set_app_id(self.app_id.clone());
        toplevel.set_title(self.title.clone());
        if let Some((w, h)) = self.min_size {
            toplevel.set_min_size(w, h);
        }
        if let Some((w, h)) = self.max_size {
            toplevel.set_max_size(w, h);
        }
        if self.maximized {
            toplevel.set_maximized();
        }
        if self.fullscreen {
            toplevel.set_fullscreen(None);
        }
        let decoration = match self.decorations {
            Decorations::Client => None,
            mode => {
                let manager = shared
                    .decoration_manager
                    .as_ref()
                    .expect("zxdg_decoration_manager_v1");
                let decoration = manager.get_toplevel_decoration(&toplevel, &c.qh, index);
                decoration.set_mode(if mode == Decorations::Server {
                    zxdg_toplevel_decoration_v1::Mode::ServerSide
                } else {
                    zxdg_toplevel_decoration_v1::Mode::ClientSide
                });
                Some(decoration)
            }
        };
        if let Some(before_commit) = self.before_commit {
            before_commit(&toplevel);
        }
        surface.commit();
        shared.windows.push(WindowData {
            app_id: self.app_id.clone(),
            surface,
            xdg_surface,
            toplevel: Some(toplevel),
            decoration,
            server_side: None,
            color: self.color,
            default_size: self.size,
            size: (0, 0),
            configures: Vec::new(),
            pending: Configure::default(),
            acked: 0,
            close_requested: false,
            auto_draw: true,
            outputs: Vec::new(),
            preferred_scale: None,
            bounds: None,
        });
        drop(shared);
        c.flush();
        Window(index)
    }
}

impl Client {
    pub fn connect(path: &Path) -> Client {
        let socket = UnixStream::connect(path).expect("connect to pila");
        let conn = Connection::from_socket(socket.try_clone().unwrap()).expect("a connection");
        let mut queue: EventQueue<Handler> = conn.new_event_queue();
        let qh = queue.handle();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let mut handler = Handler {
            shared: shared.clone(),
        };
        let registry = conn.display().get_registry(&qh, ());
        queue.roundtrip(&mut handler).expect("registry roundtrip");
        {
            let mut s = shared.lock().unwrap();
            s.registry = Some(registry.clone());
            let globals = s.globals.clone();
            for (name, interface, version) in globals {
                match interface.as_str() {
                    "wl_compositor" => {
                        s.compositor = Some(registry.bind(name, version.min(6), &qh, ()))
                    }
                    "wl_shm" => s.shm = Some(registry.bind(name, 1, &qh, ())),
                    "xdg_wm_base" => s.wm_base = Some(registry.bind(name, version.min(6), &qh, ())),
                    "wl_seat" => s.seat = Some(registry.bind(name, version.min(9), &qh, ())),
                    "zxdg_decoration_manager_v1" => {
                        s.decoration_manager = Some(registry.bind(name, 1, &qh, ()))
                    }
                    "wl_output" => {
                        let output: WlOutput = registry.bind(name, version.min(4), &qh, ());
                        s.outputs.push((output, String::new()));
                    }
                    _ => {}
                }
            }
        }
        // Seat capabilities, output names.
        queue.roundtrip(&mut handler).expect("roundtrip");
        queue.roundtrip(&mut handler).expect("roundtrip");
        let thread = std::thread::spawn(move || loop {
            if let Err(err) = dispatch(&mut queue, &mut handler) {
                handler.shared.lock().unwrap().error = Some(err);
                return;
            }
        });
        Client {
            conn,
            qh,
            shared,
            socket,
            thread: Some(thread),
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn flush(&self) {
        let _ = self.conn.flush();
    }

    /// The next requests are a protocol error, which `wait_error` will read.
    /// From here on the client's thread sends nothing of its own (acks,
    /// redraws): writing to a connection pila closed after the error would
    /// fail, and the backend keeps that I/O error for good, without reading
    /// the protocol error waiting in the socket. Requests flushed here still go.
    pub fn expect_error(&self) {
        self.lock().expecting_error = true;
    }

    /// Whether pila offers a global.
    pub fn has_global(&self, interface: &str) -> bool {
        self.lock().globals.iter().any(|(_, i, _)| i == interface)
    }

    /// A new window builder; `app_id` is also its title unless set.
    pub fn window(&self, app_id: &str) -> WindowBuilder<'_> {
        WindowBuilder {
            client: self,
            app_id: app_id.to_string(),
            title: app_id.to_string(),
            size: (400, 300),
            color: [255, 200, 50, 50],
            decorations: Decorations::Server,
            min_size: None,
            max_size: None,
            maximized: false,
            fullscreen: false,
            before_commit: None,
        }
    }

    /// Wait until every request sent so far was handled and its events
    /// were dispatched.
    pub fn roundtrip(&self) {
        let id = {
            let mut s = self.lock();
            s.next_sync += 1;
            let id = s.next_sync;
            self.conn.display().sync(&self.qh, id);
            id
        };
        self.flush();
        self.wait("a roundtrip", |s| s.syncs_done.contains(&id));
    }

    /// Wait until `cond` holds for the client's state. `cond` runs with the
    /// client's lock held: it must read the `Shared` it's given, never call
    /// the client's methods (that deadlocks).
    #[track_caller]
    pub fn wait(&self, what: &str, cond: impl Fn(&Shared) -> bool) {
        let started = Instant::now();
        loop {
            {
                let s = self.lock();
                if cond(&s) {
                    return;
                }
                if let Some(err) = &s.error {
                    panic!("client connection failed while waiting for {what}: {err}");
                }
            }
            if started.elapsed() > TIMEOUT {
                panic!("client: timed out waiting for {what}");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The connection's error, if it failed.
    pub fn error(&self) -> Option<String> {
        self.lock().error.clone()
    }

    /// Wait until the connection fails (e.g. a protocol error); its error.
    #[track_caller]
    pub fn wait_error(&self) -> String {
        assert!(
            self.lock().expecting_error,
            "call expect_error before the request that fails"
        );
        self.wait("the connection to fail", |s| s.error.is_some());
        self.error().unwrap()
    }

    /// The input events received so far.
    pub fn events(&self) -> Vec<Ev> {
        self.lock().events.clone()
    }

    pub fn clear_events(&self) {
        self.lock().events.clear();
    }

    /// Close the connection, as a crashing app would.
    pub fn disconnect(&mut self) {
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    // --- Windows ----------------------------------------------------------

    pub fn with_window<R>(&self, window: Window, f: impl FnOnce(&mut WindowData) -> R) -> R {
        f(&mut self.lock().windows[window.0])
    }

    pub fn toplevel(&self, window: Window) -> XdgToplevel {
        self.with_window(window, |w| w.toplevel.clone().expect("a toplevel"))
    }

    pub fn surface(&self, window: Window) -> WlSurface {
        self.with_window(window, |w| w.surface.clone())
    }

    pub fn configures(&self, window: Window) -> Vec<Configure> {
        self.with_window(window, |w| w.configures.clone())
    }

    pub fn last_configure(&self, window: Window) -> Configure {
        self.with_window(window, |w| w.configures.last().cloned().unwrap_or_default())
    }

    /// The size of the last buffer drawn.
    pub fn size(&self, window: Window) -> (i32, i32) {
        self.with_window(window, |w| w.size)
    }

    /// Wait until the window's last configure satisfies `cond`.
    #[track_caller]
    pub fn wait_configure(
        &self,
        window: Window,
        what: &str,
        cond: impl Fn(&Configure) -> bool,
    ) -> Configure {
        self.wait(what, |s| {
            s.windows[window.0].configures.last().is_some_and(&cond)
        });
        self.last_configure(window)
    }

    /// Stop drawing on configures (the test draws with `draw`).
    pub fn set_auto_draw(&self, window: Window, on: bool) {
        self.with_window(window, |w| w.auto_draw = on);
    }

    /// Draw a buffer of `size` (`None`: the last configured or default size).
    pub fn draw(&self, window: Window, size: Option<(i32, i32)>) {
        let mut s = self.lock();
        let shm = s.shm.clone().expect("wl_shm");
        let w = &mut s.windows[window.0];
        let size = size.unwrap_or_else(|| configured_size(w));
        draw(&shm, &self.qh, w, size);
        drop(s);
        self.flush();
    }

    /// Unmap the window (null buffer), keeping its objects.
    pub fn unmap(&self, window: Window) {
        self.with_window(window, |w| {
            w.surface.attach(None, 0, 0);
            w.surface.commit();
            w.size = (0, 0);
        });
        self.flush();
    }

    /// Map an unmapped window again, as a client following xdg-shell does:
    /// unmapping discarded its attributes, so it sets its app id again, then
    /// makes a new initial commit (it draws on the configure that answers).
    pub fn remap(&self, window: Window) {
        self.with_window(window, |w| {
            w.auto_draw = true;
            if let Some(toplevel) = &w.toplevel {
                toplevel.set_app_id(w.app_id.clone());
            }
            w.surface.commit();
        });
        self.flush();
    }

    /// Destroy the toplevel (as an app closing the window).
    pub fn destroy(&self, window: Window) {
        self.with_window(window, |w| {
            if let Some(d) = w.decoration.take() {
                d.destroy();
            }
            if let Some(t) = w.toplevel.take() {
                t.destroy();
            }
            w.xdg_surface.destroy();
            w.surface.destroy();
        });
        self.flush();
    }

    /// Run a request on the window's toplevel and flush.
    pub fn request(&self, window: Window, f: impl FnOnce(&XdgToplevel)) {
        let toplevel = self.toplevel(window);
        f(&toplevel);
        self.flush();
    }

    /// The serial of the last input event (for move/resize/popup grabs).
    pub fn last_serial(&self) -> u32 {
        self.lock().last_serial
    }

    pub fn seat(&self) -> WlSeat {
        self.lock().seat.clone().expect("a seat")
    }

    /// Which window has keyboard focus, from the client's events.
    pub fn keyboard_focus(&self) -> Option<usize> {
        let mut focus = None;
        for ev in self.lock().events.iter() {
            match ev {
                Ev::KeyboardEnter(w) => focus = *w,
                Ev::KeyboardLeave(_) => focus = None,
                _ => {}
            }
        }
        focus
    }

    /// The keys (evdev codes) received, pressed and released.
    pub fn keys(&self) -> Vec<(u32, bool)> {
        self.lock()
            .events
            .iter()
            .filter_map(|ev| match ev {
                Ev::Key { key, pressed } => Some((*key, *pressed)),
                _ => None,
            })
            .collect()
    }

    pub fn buttons(&self) -> Vec<(u32, bool)> {
        self.lock()
            .events
            .iter()
            .filter_map(|ev| match ev {
                Ev::Button {
                    button, pressed, ..
                } => Some((*button, *pressed)),
                _ => None,
            })
            .collect()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.disconnect();
    }
}

fn configured_size(w: &WindowData) -> (i32, i32) {
    let (cw, ch) = w.configures.last().map(|c| c.size).unwrap_or((0, 0));
    let base = if w.size.0 > 0 { w.size } else { w.default_size };
    (
        if cw > 0 { cw } else { base.0 },
        if ch > 0 { ch } else { base.1 },
    )
}

/// Draw `w` in its color at `size` and commit.
/// `EventQueue::blocking_dispatch`, without its flush once a protocol error
/// is expected (`Client::expect_error`).
fn dispatch(queue: &mut EventQueue<Handler>, handler: &mut Handler) -> Result<(), String> {
    let text = |err: &dyn std::fmt::Display| format!("{err}");
    if queue.dispatch_pending(handler).map_err(|e| text(&e))? > 0 {
        return Ok(());
    }
    if !handler.shared.lock().unwrap().expecting_error {
        queue.flush().map_err(|e| text(&e))?;
    }
    if let Some(guard) = queue.prepare_read() {
        let mut fds = [libc::pollfd {
            fd: guard.connection_fd().as_raw_fd(),
            events: libc::POLLIN | libc::POLLERR,
            revents: 0,
        }];
        // SAFETY: a valid array of one pollfd, on a fd `guard` keeps open.
        while unsafe { libc::poll(fds.as_mut_ptr(), 1, -1) } < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() != std::io::ErrorKind::Interrupted {
                return Err(text(&err));
            }
        }
        match guard.read() {
            Ok(_) => {}
            Err(WaylandError::Io(err)) if err.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(err) => return Err(text(&err)),
        }
    }
    queue.dispatch_pending(handler).map_err(|e| text(&e))?;
    Ok(())
}

fn draw(shm: &WlShm, qh: &QueueHandle<Handler>, w: &mut WindowData, size: (i32, i32)) {
    paint(shm, qh, &w.surface, w.color, size);
    w.size = size;
}

/// Attach a buffer of `size` in `color` (ARGB) to `surface` and commit.
pub fn paint(
    shm: &WlShm,
    qh: &QueueHandle<Handler>,
    surface: &WlSurface,
    color: [u8; 4],
    size: (i32, i32),
) {
    let (width, height) = size;
    let stride = width * 4;
    let len = (stride * height) as usize;
    // SAFETY: a new fd owned by `file`.
    let fd = unsafe { libc::memfd_create(c"pila-test".as_ptr(), libc::MFD_CLOEXEC) };
    assert!(fd >= 0, "memfd_create");
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    let [a, r, g, b] = color;
    let pixel = [b, g, r, a];
    let mut data = Vec::with_capacity(len);
    for _ in 0..(width * height) {
        data.extend_from_slice(&pixel);
    }
    file.write_all(&data).unwrap();
    let pool = shm.create_pool(file.as_fd(), len as i32, qh, ());
    let format = if a == 255 {
        wl_shm::Format::Xrgb8888
    } else {
        wl_shm::Format::Argb8888
    };
    let buffer = pool.create_buffer(0, width, height, stride, format, qh, ());
    pool.destroy();
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, width, height);
    surface.commit();
}

/// Read a file a compositor sent (e.g. a keymap) from its start: as real
/// clients map it, its offset is shared with every other client that got the
/// same file, and one that read it first must not leave this one at its end.
fn read_fd(fd: OwnedFd, size: usize) -> String {
    use std::os::unix::fs::FileExt;
    let file = std::fs::File::from(fd);
    let mut data = vec![0; size];
    let read = file.read_at(&mut data, 0).unwrap_or(0);
    data.truncate(read);
    String::from_utf8_lossy(&data)
        .trim_end_matches('\0')
        .to_string()
}

// --- Dispatch ---------------------------------------------------------------

impl Handler {
    pub(crate) fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Dispatch<WlRegistry, ()> for Handler {
    fn event(
        state: &mut Self,
        _: &WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut s = state.lock();
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => s.globals.push((name, interface, version)),
            wl_registry::Event::GlobalRemove { name } => s.globals.retain(|(n, _, _)| *n != name),
            _ => {}
        }
    }
}

impl Dispatch<WlCallback, u32> for Handler {
    fn event(
        state: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        id: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.lock().syncs_done.push(*id);
        }
    }
}

delegate_noop!(Handler: ignore WlCompositor);
delegate_noop!(Handler: ignore WlShm);
delegate_noop!(Handler: ignore WlShmPool);
delegate_noop!(Handler: ignore ZxdgDecorationManagerV1);

impl Dispatch<WlBuffer, ()> for Handler {
    fn event(
        _: &mut Self,
        buffer: &WlBuffer,
        event: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            buffer.destroy();
        }
    }
}

impl Dispatch<WlSurface, ()> for Handler {
    fn event(
        state: &mut Self,
        surface: &WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut s = state.lock();
        let name_of = |s: &Shared, output: &WlOutput| {
            s.outputs
                .iter()
                .find(|(o, _)| o == output)
                .map(|(_, n)| n.clone())
                .unwrap_or_default()
        };
        if let Some(i) = s.surfaces.iter().position(|d| d.surface == *surface) {
            match &event {
                wl_surface::Event::Enter { output } => {
                    let name = name_of(&s, output);
                    s.surfaces[i].outputs.push(name);
                }
                wl_surface::Event::Leave { output } => {
                    let name = name_of(&s, output);
                    s.surfaces[i].outputs.retain(|n| *n != name);
                }
                _ => {}
            }
        }
        let Some(index) = s.window_of(surface) else {
            return;
        };
        match event {
            wl_surface::Event::Enter { output } => {
                let name = name_of(&s, &output);
                s.windows[index].outputs.push(name);
            }
            wl_surface::Event::Leave { output } => {
                let name = name_of(&s, &output);
                s.windows[index].outputs.retain(|n| *n != name);
            }
            wl_surface::Event::PreferredBufferScale { factor } => {
                s.windows[index].preferred_scale = Some(factor);
            }
            _ => {}
        }
    }
}

impl Dispatch<WlOutput, ()> for Handler {
    fn event(
        state: &mut Self,
        output: &WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            let mut s = state.lock();
            if let Some(entry) = s.outputs.iter_mut().find(|(o, _)| o == output) {
                entry.1 = name;
            }
        }
    }
}

impl Dispatch<XdgWmBase, ()> for Handler {
    fn event(
        _: &mut Self,
        wm_base: &XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<XdgSurface, usize> for Handler {
    fn event(
        state: &mut Self,
        xdg_surface: &XdgSurface,
        event: xdg_surface::Event,
        index: &usize,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            let s = &mut *state.lock();
            let shm = s.shm.clone();
            let w = &mut s.windows[*index];
            xdg_surface.ack_configure(serial);
            w.acked += 1;
            let configure = std::mem::take(&mut w.pending);
            w.configures.push(configure);
            if w.auto_draw && w.toplevel.is_some() {
                let size = configured_size(w);
                draw(shm.as_ref().expect("wl_shm"), qh, w, size);
            }
        }
    }
}

impl Dispatch<XdgToplevel, usize> for Handler {
    fn event(
        state: &mut Self,
        _: &XdgToplevel,
        event: xdg_toplevel::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut s = state.lock();
        let w = &mut s.windows[*index];
        match event {
            xdg_toplevel::Event::Configure {
                width,
                height,
                states,
            } => {
                w.pending = Configure {
                    size: (width, height),
                    states: states
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|c| u32::from_ne_bytes(*c))
                        .collect(),
                };
            }
            xdg_toplevel::Event::Close => w.close_requested = true,
            xdg_toplevel::Event::ConfigureBounds { width, height } => {
                w.bounds = Some((width, height));
            }
            _ => {}
        }
    }
}

impl Dispatch<ZxdgToplevelDecorationV1, usize> for Handler {
    fn event(
        state: &mut Self,
        _: &ZxdgToplevelDecorationV1,
        event: zxdg_toplevel_decoration_v1::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_toplevel_decoration_v1::Event::Configure { mode } = event {
            state.lock().windows[*index].server_side =
                Some(mode == WEnum::Value(zxdg_toplevel_decoration_v1::Mode::ServerSide));
        }
    }
}

impl Dispatch<WlSeat, ()> for Handler {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            let mut s = state.lock();
            if caps.contains(wl_seat::Capability::Keyboard) && s.keyboard.is_none() {
                s.keyboard = Some(seat.get_keyboard(qh, ()));
            }
            if caps.contains(wl_seat::Capability::Pointer) && s.pointer.is_none() {
                s.pointer = Some(seat.get_pointer(qh, ()));
            }
        }
    }
}

impl Dispatch<WlKeyboard, ()> for Handler {
    fn event(
        state: &mut Self,
        _: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut s = state.lock();
        let ev = match event {
            wl_keyboard::Event::Keymap { fd, format, size } => {
                let format = match format {
                    WEnum::Value(f) => f as u32,
                    WEnum::Unknown(f) => f,
                };
                s.keymap = Some((format, read_fd(fd, size as usize)));
                return;
            }
            wl_keyboard::Event::Enter {
                surface, serial, ..
            } => {
                s.last_serial = serial;
                s.keyboard_name = s.name_of(&surface);
                Ev::KeyboardEnter(s.window_of(&surface))
            }
            wl_keyboard::Event::Leave { surface, .. } => {
                s.keyboard_name = None;
                Ev::KeyboardLeave(s.window_of(&surface))
            }
            wl_keyboard::Event::Key {
                key,
                state: key_state,
                serial,
                ..
            } => {
                s.last_serial = serial;
                Ev::Key {
                    key,
                    pressed: key_state == WEnum::Value(wl_keyboard::KeyState::Pressed),
                }
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_locked,
                group,
                ..
            } => Ev::Modifiers {
                depressed: mods_depressed,
                locked: mods_locked,
                group,
            },
            _ => return,
        };
        s.events.push(ev);
    }
}

impl Dispatch<WlPointer, ()> for Handler {
    fn event(
        state: &mut Self,
        _: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut s = state.lock();
        let ev = match event {
            wl_pointer::Event::Enter {
                surface,
                surface_x,
                surface_y,
                serial,
            } => {
                s.last_serial = serial;
                s.pointer_enter_serial = serial;
                s.pointer_name = s.name_of(&surface);
                Ev::PointerEnter {
                    window: s.window_of(&surface),
                    x: surface_x,
                    y: surface_y,
                    serial,
                }
            }
            wl_pointer::Event::Leave { surface, .. } => {
                s.pointer_name = None;
                Ev::PointerLeave(s.window_of(&surface))
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => Ev::Motion {
                x: surface_x,
                y: surface_y,
            },
            wl_pointer::Event::Button {
                button,
                state: button_state,
                serial,
                ..
            } => {
                s.last_serial = serial;
                Ev::Button {
                    button,
                    pressed: button_state == WEnum::Value(wl_pointer::ButtonState::Pressed),
                    serial,
                }
            }
            wl_pointer::Event::Axis { axis, value, .. } => Ev::Axis {
                horizontal: axis == WEnum::Value(wl_pointer::Axis::HorizontalScroll),
                value,
            },
            wl_pointer::Event::AxisValue120 { axis, value120 } => Ev::AxisV120 {
                horizontal: axis == WEnum::Value(wl_pointer::Axis::HorizontalScroll),
                value: value120,
            },
            _ => return,
        };
        s.events.push(ev);
    }
}
