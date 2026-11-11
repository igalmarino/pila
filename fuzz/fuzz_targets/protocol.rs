//! Sequences of protocol requests to a running Pila (the class of crash
//! 0.4.2 fixed: sizes, offsets, scales, buffers, subsurfaces, viewports,
//! windows and popups past what's sane). One headless Pila runs for the whole
//! fuzzing run (`PILA_BIN`, else `target/debug/pila` of this checkout; build
//! it first), each input is a new client sending its requests, and the input
//! fails if Pila died meanwhile. A client killed for a protocol error is fine:
//! that's Pila refusing it.
#![no_main]

use std::{
    io::{BufRead, BufReader, Write},
    os::{
        fd::AsFd,
        unix::{net::UnixStream, process::CommandExt},
    },
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use wayland_client::{
    delegate_noop,
    globals::{registry_queue_init, GlobalListContents},
    protocol::{
        wl_buffer::WlBuffer, wl_compositor::WlCompositor, wl_output, wl_registry::WlRegistry,
        wl_shm, wl_shm::WlShm, wl_shm_pool::WlShmPool, wl_subcompositor::WlSubcompositor,
        wl_subsurface::WlSubsurface, wl_surface::WlSurface,
    },
    Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_protocols::{
    wp::viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
    xdg::shell::client::{
        xdg_popup::XdgPopup, xdg_positioner, xdg_positioner::XdgPositioner, xdg_surface,
        xdg_surface::XdgSurface, xdg_toplevel::XdgToplevel, xdg_wm_base, xdg_wm_base::XdgWmBase,
    },
};

/// The most a pool may hold: requests may ask for any size, the memory
/// behind them stays small.
const POOL_MAX: i32 = 1 << 20;

#[derive(Debug, Arbitrary)]
enum Op {
    Surface,
    Pool { size: i32 },
    Buffer { pool: u8, offset: i32, w: i32, h: i32, stride: i32, format: u32 },
    Attach { surface: u8, buffer: Option<u8>, x: i32, y: i32 },
    Damage { surface: u8, x: i32, y: i32, w: i32, h: i32, buffer: bool },
    Scale { surface: u8, scale: i32 },
    Transform { surface: u8, transform: u32 },
    Offset { surface: u8, x: i32, y: i32 },
    Commit { surface: u8 },
    Subsurface { surface: u8, parent: u8 },
    Position { sub: u8, x: i32, y: i32 },
    Place { sub: u8, sibling: u8, above: bool },
    Sync { sub: u8, sync: bool },
    Viewport { surface: u8 },
    Source { viewport: u8, x: f64, y: f64, w: f64, h: f64 },
    Destination { viewport: u8, w: i32, h: i32 },
    Toplevel { surface: u8 },
    Geometry { xdg: u8, x: i32, y: i32, w: i32, h: i32 },
    MinMax { toplevel: u8, w: i32, h: i32, max: bool },
    Maximize { toplevel: u8, on: bool },
    Fullscreen { toplevel: u8, on: bool },
    Positioner { w: i32, h: i32, rect: (i32, i32, i32, i32), anchor: u32, gravity: u32, adjust: u32, offset: (i32, i32), reactive: bool },
    Popup { surface: u8, parent: u8, positioner: u8 },
    Reposition { popup: u8, positioner: u8, token: u32 },
    DestroySurface { surface: u8 },
    DestroySub { sub: u8 },
    DestroyXdg { xdg: u8 },
}

#[derive(Default)]
struct Client {
    surfaces: Vec<WlSurface>,
    pools: Vec<WlShmPool>,
    buffers: Vec<WlBuffer>,
    subs: Vec<WlSubsurface>,
    viewports: Vec<WpViewport>,
    xdgs: Vec<XdgSurface>,
    toplevels: Vec<XdgToplevel>,
    positioners: Vec<XdgPositioner>,
    popups: Vec<XdgPopup>,
}

/// The one Pila of this run, and the path of its Wayland socket.
struct Server {
    child: Child,
    socket: PathBuf,
    control: BufReader<UnixStream>,
    log: PathBuf,
}

static SERVER: OnceLock<Mutex<Server>> = OnceLock::new();

fn server() -> &'static Mutex<Server> {
    SERVER.get_or_init(|| {
        let bin = std::env::var_os("PILA_BIN").map(PathBuf::from).unwrap_or_else(|| {
            PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/debug/pila"))
        });
        // Short: a socket path must fit in 108 bytes.
        let dir = PathBuf::from(format!("/tmp/pila-fuzz-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config.toml");
        std::fs::write(&config, "[animations]\nenabled = false\n").unwrap();
        let log = dir.join("pila.log");
        let file = std::fs::File::create(&log).unwrap();
        let mut command = Command::new(&bin);
        command
            .args(["--backend", "headless", "--config"])
            .arg(&config)
            // Fast frames: what a commit leads to is drawn within the input.
            .args(["--headless-outputs", "HEADLESS-1:1280x800@1000"])
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .env("XDG_RUNTIME_DIR", &dir)
            .env("RUST_LOG", "warn")
            .stdin(Stdio::null())
            .stdout(file.try_clone().unwrap())
            .stderr(file);
        // Pila goes when the fuzzer does.
        // SAFETY: prctl is async-signal-safe.
        unsafe {
            command.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                Ok(())
            });
        }
        let child = command
            .spawn()
            .unwrap_or_else(|err| panic!("can't start {}: {err} (build it, or set PILA_BIN)", bin.display()));
        let started = Instant::now();
        let (socket, control) = loop {
            let found = std::fs::read_dir(&dir).ok().and_then(|entries| {
                entries.flatten().find_map(|e| {
                    let name = e.file_name().into_string().ok()?;
                    let display = name.strip_suffix(".control")?;
                    Some((dir.join(display), e.path()))
                })
            });
            if let Some((socket, control)) = found {
                if let Ok(control) = UnixStream::connect(&control) {
                    break (socket, control);
                }
            }
            assert!(started.elapsed() < Duration::from_secs(30), "pila didn't start: see {}", log.display());
            std::thread::sleep(Duration::from_millis(10));
        };
        control.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        Mutex::new(Server {
            child,
            socket,
            control: BufReader::new(control),
            log,
        })
    })
}

impl Server {
    /// Panics (the input failed) if Pila is gone: the control socket answers
    /// once what came before was handled.
    fn check_alive(&mut self) {
        let mut answer = String::new();
        let alive = self.control.get_mut().write_all(b"{\"cmd\":\"state\"}\n").is_ok()
            && self.control.read_line(&mut answer).is_ok_and(|n| n > 0);
        if alive && self.child.try_wait().ok().flatten().is_none() {
            return;
        }
        let log = std::fs::read_to_string(&self.log).unwrap_or_default();
        let tail: Vec<&str> = log.lines().rev().take(60).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        panic!("pila died:\n{}", tail.join("\n"));
    }
}

fuzz_target!(|ops: Vec<Op>| {
    let mut server = server().lock().unwrap_or_else(|e| e.into_inner());
    if let Ok(stream) = UnixStream::connect(&server.socket) {
        run(stream, &ops);
    }
    // What a commit leads to is drawn within a frame or two.
    std::thread::sleep(Duration::from_millis(3));
    server.check_alive();
});

/// Connect, bind, send `ops`, and wait for Pila to have handled them (or to
/// have killed this client for a protocol error).
fn run(stream: UnixStream, ops: &[Op]) {
    let conn = Connection::from_socket(stream).unwrap();
    let Ok((globals, mut queue)) = registry_queue_init::<State>(&conn) else {
        return;
    };
    let qh = queue.handle();
    let (Ok(compositor), Ok(subcompositor), Ok(shm), Ok(wm_base)) = (
        globals.bind::<WlCompositor, _, _>(&qh, 1..=6, ()),
        globals.bind::<WlSubcompositor, _, _>(&qh, 1..=1, ()),
        globals.bind::<WlShm, _, _>(&qh, 1..=1, ()),
        globals.bind::<XdgWmBase, _, _>(&qh, 1..=6, ()),
    ) else {
        return;
    };
    let viewporter = globals.bind::<WpViewporter, _, _>(&qh, 1..=1, ()).ok();
    let mut c = Client::default();
    let mut state = State;
    for op in ops {
        apply(op, &mut c, &qh, &compositor, &subcompositor, &shm, &wm_base, viewporter.as_ref());
        // Events (configures to ack, pings) along the way.
        if conn.flush().is_err() || queue.dispatch_pending(&mut state).is_err() {
            return;
        }
    }
    let _ = queue.roundtrip(&mut state);
    std::thread::sleep(Duration::from_millis(2));
    let _ = queue.roundtrip(&mut state);
}

fn pick<T>(items: &[T], index: u8) -> Option<&T> {
    (!items.is_empty()).then(|| &items[usize::from(index) % items.len()])
}

#[allow(clippy::too_many_arguments)]
fn apply(
    op: &Op,
    c: &mut Client,
    qh: &QueueHandle<State>,
    compositor: &WlCompositor,
    subcompositor: &WlSubcompositor,
    shm: &WlShm,
    wm_base: &XdgWmBase,
    viewporter: Option<&WpViewporter>,
) {
    match *op {
        Op::Surface => c.surfaces.push(compositor.create_surface(qh, ())),
        Op::Pool { size } => {
            let size = size.clamp(1, POOL_MAX);
            // SAFETY: a name and flags.
            let fd = unsafe { libc::memfd_create(c"pool".as_ptr(), libc::MFD_CLOEXEC) };
            if fd < 0 {
                return;
            }
            // SAFETY: just made and owned here.
            let file = unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(fd) };
            if file.set_len(size as u64).is_err() {
                return;
            }
            c.pools.push(shm.create_pool(file.as_fd(), size, qh, ()));
        }
        Op::Buffer { pool, offset, w, h, stride, format } => {
            let Some(pool) = pick(&c.pools, pool) else { return };
            let format = wl_shm::Format::try_from(format % 2).unwrap_or(wl_shm::Format::Argb8888);
            c.buffers.push(pool.create_buffer(offset, w, h, stride, format, qh, ()));
        }
        Op::Attach { surface, buffer, x, y } => {
            let Some(surface) = pick(&c.surfaces, surface) else { return };
            let buffer = buffer.and_then(|b| pick(&c.buffers, b));
            surface.attach(buffer, x, y);
        }
        Op::Damage { surface, x, y, w, h, buffer } => {
            let Some(surface) = pick(&c.surfaces, surface) else { return };
            if buffer && surface.version() >= 4 {
                surface.damage_buffer(x, y, w, h);
            } else {
                surface.damage(x, y, w, h);
            }
        }
        Op::Scale { surface, scale } => {
            if let Some(surface) = pick(&c.surfaces, surface) {
                surface.set_buffer_scale(scale);
            }
        }
        Op::Transform { surface, transform } => {
            if let Some(surface) = pick(&c.surfaces, surface) {
                if let Ok(t) = wl_output::Transform::try_from(transform % 8) {
                    surface.set_buffer_transform(t);
                }
            }
        }
        Op::Offset { surface, x, y } => {
            if let Some(surface) = pick(&c.surfaces, surface).filter(|s| s.version() >= 5) {
                surface.offset(x, y);
            }
        }
        Op::Commit { surface } => {
            if let Some(surface) = pick(&c.surfaces, surface) {
                surface.commit();
            }
        }
        Op::Subsurface { surface, parent } => {
            let (Some(surface), Some(parent)) = (pick(&c.surfaces, surface), pick(&c.surfaces, parent)) else {
                return;
            };
            c.subs.push(subcompositor.get_subsurface(surface, parent, qh, ()));
        }
        Op::Position { sub, x, y } => {
            if let Some(sub) = pick(&c.subs, sub) {
                sub.set_position(x, y);
            }
        }
        Op::Place { sub, sibling, above } => {
            let (Some(sub), Some(sibling)) = (pick(&c.subs, sub), pick(&c.surfaces, sibling)) else {
                return;
            };
            if above {
                sub.place_above(sibling);
            } else {
                sub.place_below(sibling);
            }
        }
        Op::Sync { sub, sync } => {
            if let Some(sub) = pick(&c.subs, sub) {
                if sync {
                    sub.set_sync();
                } else {
                    sub.set_desync();
                }
            }
        }
        Op::Viewport { surface } => {
            if let (Some(surface), Some(viewporter)) = (pick(&c.surfaces, surface), viewporter) {
                c.viewports.push(viewporter.get_viewport(surface, qh, ()));
            }
        }
        Op::Source { viewport, x, y, w, h } => {
            if let Some(viewport) = pick(&c.viewports, viewport) {
                viewport.set_source(x, y, w, h);
            }
        }
        Op::Destination { viewport, w, h } => {
            if let Some(viewport) = pick(&c.viewports, viewport) {
                viewport.set_destination(w, h);
            }
        }
        Op::Toplevel { surface } => {
            let Some(surface) = pick(&c.surfaces, surface) else { return };
            let xdg = wm_base.get_xdg_surface(surface, qh, ());
            c.toplevels.push(xdg.get_toplevel(qh, ()));
            c.xdgs.push(xdg);
        }
        Op::Geometry { xdg, x, y, w, h } => {
            if let Some(xdg) = pick(&c.xdgs, xdg) {
                xdg.set_window_geometry(x, y, w, h);
            }
        }
        Op::MinMax { toplevel, w, h, max } => {
            if let Some(toplevel) = pick(&c.toplevels, toplevel) {
                if max {
                    toplevel.set_max_size(w, h);
                } else {
                    toplevel.set_min_size(w, h);
                }
            }
        }
        Op::Maximize { toplevel, on } => {
            if let Some(toplevel) = pick(&c.toplevels, toplevel) {
                if on {
                    toplevel.set_maximized();
                } else {
                    toplevel.unset_maximized();
                }
            }
        }
        Op::Fullscreen { toplevel, on } => {
            if let Some(toplevel) = pick(&c.toplevels, toplevel) {
                if on {
                    toplevel.set_fullscreen(None);
                } else {
                    toplevel.unset_fullscreen();
                }
            }
        }
        Op::Positioner { w, h, rect, anchor, gravity, adjust, offset, reactive } => {
            let positioner = wm_base.create_positioner(qh, ());
            positioner.set_size(w, h);
            positioner.set_anchor_rect(rect.0, rect.1, rect.2, rect.3);
            if let Ok(anchor) = xdg_positioner::Anchor::try_from(anchor % 9) {
                positioner.set_anchor(anchor);
            }
            if let Ok(gravity) = xdg_positioner::Gravity::try_from(gravity % 9) {
                positioner.set_gravity(gravity);
            }
            positioner.set_constraint_adjustment(xdg_positioner::ConstraintAdjustment::from_bits_truncate(adjust));
            positioner.set_offset(offset.0, offset.1);
            if reactive && positioner.version() >= 3 {
                positioner.set_reactive();
            }
            c.positioners.push(positioner);
        }
        Op::Popup { surface, parent, positioner } => {
            let (Some(surface), Some(positioner)) = (pick(&c.surfaces, surface), pick(&c.positioners, positioner)) else {
                return;
            };
            let parent = pick(&c.xdgs, parent).cloned();
            let xdg = wm_base.get_xdg_surface(surface, qh, ());
            c.popups.push(xdg.get_popup(parent.as_ref(), positioner, qh, ()));
            c.xdgs.push(xdg);
        }
        Op::Reposition { popup, positioner, token } => {
            let (Some(popup), Some(positioner)) = (pick(&c.popups, popup), pick(&c.positioners, positioner)) else {
                return;
            };
            if popup.version() >= 3 {
                popup.reposition(positioner, token);
            }
        }
        Op::DestroySurface { surface } => {
            if !c.surfaces.is_empty() {
                let i = usize::from(surface) % c.surfaces.len();
                c.surfaces.swap_remove(i).destroy();
            }
        }
        Op::DestroySub { sub } => {
            if !c.subs.is_empty() {
                let i = usize::from(sub) % c.subs.len();
                c.subs.swap_remove(i).destroy();
            }
        }
        Op::DestroyXdg { xdg } => {
            if !c.xdgs.is_empty() {
                let i = usize::from(xdg) % c.xdgs.len();
                c.xdgs.swap_remove(i).destroy();
            }
        }
    }
}

struct State;

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<XdgWmBase, ()> for State {
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

/// Configures are acked, so windows and popups can map.
impl Dispatch<XdgSurface, ()> for State {
    fn event(
        _: &mut Self,
        xdg: &XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg.ack_configure(serial);
        }
    }
}

delegate_noop!(State: ignore WlCompositor);
delegate_noop!(State: ignore WlSurface);
delegate_noop!(State: ignore WlSubcompositor);
delegate_noop!(State: ignore WlSubsurface);
delegate_noop!(State: ignore WlShm);
delegate_noop!(State: ignore WlShmPool);
delegate_noop!(State: ignore WlBuffer);
delegate_noop!(State: ignore WpViewporter);
delegate_noop!(State: ignore WpViewport);
delegate_noop!(State: ignore XdgToplevel);
delegate_noop!(State: ignore XdgPositioner);
delegate_noop!(State: ignore XdgPopup);
