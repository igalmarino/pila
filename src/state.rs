use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    ffi::OsString,
    os::{fd::AsFd, unix::process::CommandExt},
    process::{Command, Stdio},
    sync::Arc,
};

use smithay::{
    backend::renderer::{element::solid::SolidColorBuffer, utils::with_renderer_surface_state},
    desktop::{
        find_popup_root_surface, layer_map_for_output, LayerSurface, PopupManager, Space, Window,
        WindowSurfaceType,
    },
    input::{
        keyboard::{Keycode, XkbConfig},
        pointer::{CursorIcon, CursorImageStatus, Focus, GrabStartData as PointerGrabStartData},
        Seat, SeatHandler, SeatState,
    },
    output::Output,
    reexports::{
        calloop::{
            self,
            generic::Generic,
            timer::{TimeoutAction, Timer},
            EventLoop, Interest, LoopHandle, LoopSignal, Mode, PostAction,
        },
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
            Client, Display, DisplayHandle, Resource,
        },
    },
    utils::{Clock, Logical, Monotonic, Point, Rectangle, Serial},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        cursor_shape::CursorShapeManagerState,
        dmabuf::DmabufState,
        output::OutputManagerState,
        presentation::PresentationState,
        seat::WaylandFocus,
        security_context::SecurityContext,
        selection::{data_device::DataDeviceState, primary_selection::PrimarySelectionState},
        session_lock::SessionLockManagerState,
        shell::{
            wlr_layer::{Layer, WlrLayerShellState},
            xdg::{decoration::XdgDecorationState, XdgShellState},
        },
        shm::ShmState,
        socket::ListeningSocketSource,
        xdg_activation::XdgActivationState,
        xwayland_shell::XWaylandShellState,
    },
    xwayland::XWaylandClientData,
};
use tracing::{debug, error, info, warn};

use crate::{
    animation::SceneGhost,
    backend::Backend,
    config::{Action, Config},
    decoration::{self, Decoration, TitleBarPart},
    focus::KeyboardFocus,
    grabs::{resize_grab::ResizeEdge, MoveSurfaceGrab, ResizeSurfaceGrab},
    handlers::protocols::{self, Globals},
    idle::IdleInhibitors,
    render::{FrameInput, Scene, SceneLayer, SceneTitleBar, SceneWindow},
    session_lock::SessionLock,
    switcher::Switch,
};

/// How long after startup a problem found then is shown as a notification
/// (the session's notification daemon starts with the autostart programs).
const STARTUP_NOTICE_DELAY: std::time::Duration = std::time::Duration::from_secs(6);

/// Show a problem found at startup as a notification, once the session has had
/// time to start its notification daemon.
pub fn notify_after_startup(
    loop_handle: &LoopHandle<'static, Pila>,
    summary: &'static str,
    body: String,
) {
    let timer = Timer::from_duration(STARTUP_NOTICE_DELAY);
    let inserted = loop_handle.insert_source(timer, move |_, _, state: &mut Pila| {
        state.notify_error(summary, &body);
        TimeoutAction::Drop
    });
    if let Err(err) = inserted {
        warn!(?err, summary, "can't schedule the notification");
    }
}

/// Offset applied to each new window that would land exactly on top of another.
const CASCADE_STEP: i32 = 32;

pub struct Pila {
    /// The clock presentation-time timestamps use (CLOCK_MONOTONIC).
    pub clock: Clock<Monotonic>,
    pub socket_name: OsString,
    /// Where `pila msg` finds this pila (`ipc.rs`).
    pub ipc_socket: Option<std::path::PathBuf>,
    /// Clients of `pila msg` subscribed to events (`ipc.rs`).
    pub ipc_events: crate::ipc::Subscribers,
    pub display_handle: DisplayHandle,
    pub loop_signal: LoopSignal,
    pub loop_handle: LoopHandle<'static, Pila>,
    /// Wakes the event loop: calloop doesn't for pending idle callbacks, so
    /// one inserted outside an event source (e.g. from `after_frames`)
    /// would otherwise wait for the next unrelated event (`queue_redraw`).
    pub wake: smithay::reexports::calloop::ping::Ping,
    pub config: Config,

    /// Xwayland and its window manager (`xwayland.rs`).
    pub xwayland: crate::xwayland::XwaylandState,
    /// The startup commands, held back until Xwayland is ready.
    pub pending_startup: Vec<String>,
    /// Programs pila started, reaped on SIGCHLD (`reap_children`).
    pub children: Vec<std::process::Child>,

    /// Where a dragged window would snap to, shown above the windows
    /// (`snap.rs`), and the buffer it's drawn with.
    pub snap_preview: Option<Rectangle<i32, Logical>>,
    pub snap_preview_buffer: SolidColorBuffer,
    /// The config file, watched for changes (`config_reload.rs`).
    pub config_path: Option<std::path::PathBuf>,
    /// A reload is scheduled (changes are coalesced).
    pub config_reload_pending: bool,
    /// An output was drawn this event-loop iteration: `after_frames` has
    /// housekeeping to do.
    pub frame_drawn: bool,
    /// A redraw is scheduled for hidden surfaces waiting for a frame
    /// callback (`frame.rs`, `schedule_hidden_frames`).
    pub hidden_frame_timer: bool,
    /// Surfaces with fifo barriers to signal (`commit_pacing.rs`).
    pub commit_pacing: crate::commit_pacing::CommitPacing,
    /// Screen capture: ext-image-copy-capture and wlr-screencopy.
    pub capture: crate::capture::Captures,
    /// Taskbar listeners of wlr-foreign-toplevel-management (`toplevel_management.rs`).
    pub toplevel_management: crate::toplevel_management::ToplevelManagement,
    /// Workspaces (`workspaces.rs`) and their ext-workspace listeners
    /// (`ext_workspace.rs`).
    pub workspaces: crate::workspaces::Workspaces,
    pub ext_workspaces: crate::ext_workspace::ExtWorkspaces,
    /// Saved window states of clients' sessions (`session_management.rs`).
    pub session_management: crate::session_management::SessionManagement,
    /// Blurred backdrops of layer surfaces (`blur.rs`).
    pub blur: crate::blur::BlurCache,
    /// What's known and set about outputs beyond the `Space`: monitors,
    /// display tools' settings, gamma and power.
    pub outputs: crate::output_management::Outputs,
    /// Animations in progress (`animation.rs`), and the workspace switch's
    /// (`workspace_slide.rs`).
    pub animations: crate::animation::Animations,
    pub workspace_slide: Option<crate::workspace_slide::WorkspaceSlide>,
    /// Each output's frame statistics, by name (`frame_stats.rs`).
    pub frame_stats: HashMap<String, crate::frame_stats::FrameStats>,
    /// The screen captures' frames (`frame_stats.rs`, `Pila::record_capture`).
    pub capture_stats: crate::frame_stats::FrameStats,

    /// Mapped windows, in stacking order (back to front).
    pub space: Space<Window>,
    /// Windows off the `Space`, and the order they were used in.
    pub windows: Windows,
    /// Input state of pila's own beyond the seat's.
    pub input: InputState,

    /// The state of Smithay's protocols that handlers hand back.
    pub protocols: Protocols,
    pub popups: PopupManager,
    /// Whether the screen is locked, and by whom (`session_lock.rs`).
    pub session_lock: SessionLock,
    /// State of the simpler protocols (`handlers/protocols.rs`).
    pub globals: Globals,
    pub idle_inhibitors: IdleInhibitors,

    pub seat: Seat<Self>,
    /// Cursor most recently requested by the client under the pointer.
    pub cursor_status: CursorImageStatus,
    /// The icon of the drag in progress (`handlers/mod.rs`), drawn at the
    /// pointer plus its offset.
    pub dnd_icon: Option<crate::render::DndIcon>,
    /// The window menu, while open (`window_menu.rs`).
    pub window_menu: Option<crate::window_menu::WindowMenu>,

    /// Nested-only: set by `init_winit`. It lives here (not in the redraw closure) so
    /// `DmabufHandler` can test-import client buffers into the renderer. A DRM backend
    /// needs its own renderer handle here instead.
    pub backend: Option<Backend>,
}

// Every Smithay protocol's `Dispatch`, through its user data (`Dispatch2`):
// the one macro that replaced the per-protocol `delegate_*!`.
smithay::delegate_dispatch2!(Pila);

/// Windows off the `Space`, and the order they were used in (`Pila::windows`).
#[derive(Default)]
pub struct Windows {
    /// Toplevels that exist but haven't committed a buffer yet.
    pub unmapped: Vec<Window>,
    /// Minimized windows (`minimize.rs`).
    pub minimized: Vec<Window>,
    /// Windows most recently focused first, and the Mod+Tab in progress
    /// (`switcher.rs`).
    pub focus_history: Vec<Window>,
    pub switch: Option<Switch>,
    /// What `show-desktop` minimized, to bring back (`minimize.rs`).
    pub desktop_shown: Option<crate::minimize::DesktopShown>,
}

/// pila's own input state, beyond the seat's (`Pila::input`).
#[derive(Default)]
pub struct InputState {
    /// Keys whose press triggered a binding; their release is swallowed too.
    pub suppressed_keys: HashSet<Keycode>,
    /// The surface whose pointer constraint pila activated, if any.
    pub constrained: Option<WlSurface>,
    /// Nested-only: the last absolute pointer position from the host, to
    /// derive relative motion (it differs from pila's pointer while locked).
    pub host_pointer: Option<Point<f64, Logical>>,
    /// A touchpad swipe pila is handling itself (`gestures.rs`).
    pub swipe: Option<crate::gestures::Swipe>,
    /// A virtual keyboard's keymap the seat has now, in place of the
    /// configured one (`virtual_keyboard.rs`).
    pub virtual_keymap: Option<crate::virtual_keyboard::ActiveKeymap>,
    /// The configured keymap as text, kept while a virtual keyboard's is in
    /// place; reset when the keyboard config is applied.
    pub configured_keymap: Option<smithay::input::keyboard::KeymapFile>,
    /// The physical keyboard's layout names as last read, told while a
    /// virtual keyboard's keymap is in place (`keyboard_layout.rs`).
    pub layout_names: Vec<String>,
    /// Pointer buttons whose press only woke the monitors: their release
    /// isn't forwarded either.
    pub suppressed_buttons: HashSet<u32>,
    /// The last pointer button press: a client may ask for the window menu
    /// with its serial after the release (`window_menu::recent_press`).
    pub last_press: Option<crate::window_menu::LastPress>,
    /// A key pressed while the window menu is open, for it (`window_menu.rs`).
    pub menu_key: Option<smithay::input::keyboard::Keysym>,
}

/// The state of Smithay's protocols (`Pila::protocols`), which its handler
/// traits ask for.
pub struct Protocols {
    pub compositor: CompositorState,
    pub xdg_shell: XdgShellState,
    pub layer_shell: WlrLayerShellState,
    pub shm: ShmState,
    pub output_manager: OutputManagerState,
    pub seat: SeatState<Pila>,
    pub data_device: DataDeviceState,
    pub primary_selection: PrimarySelectionState,
    pub xdg_decoration: XdgDecorationState,
    pub xdg_activation: XdgActivationState,
    pub cursor_shape: CursorShapeManagerState,
    /// Its global is created by the backend, which knows the render device
    /// and formats.
    pub dmabuf: DmabufState,
    pub session_lock: SessionLockManagerState,
    /// linux-drm-syncobj, where the backend's DRM device can do it.
    pub syncobj: Option<smithay::wayland::drm_syncobj::DrmSyncobjState>,
}

/// `XDG_CURRENT_DESKTOP` for everything pila starts; xdg-desktop-portal
/// picks its backends from `pila-portals.conf` by it.
const DESKTOP_NAME: &str = "pila";
/// What `export_environment` hands the session bus.
const SESSION_ENV: [&str; 5] = [
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_TYPE",
    // For a shell started by systemd or D-Bus to find `pila msg`'s socket.
    "PILA_SOCKET",
];

/// Other compositors' IPC sockets and markers, which pila's children don't
/// inherit when it's started from inside another session (nested, or a TTY
/// run from one of its terminals): shells such as Noctalia and Dux Shell pick
/// their compositor backend by them, and would drive that compositor.
const OTHER_COMPOSITORS_ENV: [&str; 11] = [
    "NIRI_SOCKET",
    "SWAYSOCK",
    "I3SOCK",
    "HYPRLAND_INSTANCE_SIGNATURE",
    "MANGO_INSTANCE_SIGNATURE",
    "UMBRIEL_SOCKET",
    "LABWC_PID",
    "MIRACLESOCK",
    "AQUEOUS_SOCKET",
    "SCROLLSOCK",
    "KDE_FULL_SESSION",
];

/// How long the startup programs wait for the session bus to take pila's
/// environment (`export_environment`).
const EXPORT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// How long a global clients were told is gone can still be bound
/// (`retire_global`).
const GLOBAL_RETIRE_DELAY: std::time::Duration = std::time::Duration::from_secs(10);

/// Whether the D-Bus address `bus` is the systemd user bus of the runtime
/// directory `runtime` (`unix:path=$XDG_RUNTIME_DIR/bus`, maybe followed by
/// more `,key=value` pairs such as a guid).
fn is_user_bus(bus: &str, runtime: &str) -> bool {
    let Some(rest) = bus.strip_prefix("unix:path=") else {
        return false;
    };
    let path = rest.split(',').next().unwrap_or(rest);
    !runtime.is_empty() && path == format!("{runtime}/bus")
}

/// The limit of open files Pila was started with, which the programs it starts
/// get back (`session_env`, `raise_file_limit`).
static STARTING_FILE_LIMIT: std::sync::OnceLock<libc::rlimit> = std::sync::OnceLock::new();

/// Raise the soft limit of open files to the hard one: Pila holds a descriptor
/// or more per client, shm pool, dma-buf and fence, and a busy session goes past
/// the usual 1024. What it starts gets the old limit back (`session_env`), since
/// programs using `select()` break past 1024. Call once at startup.
pub fn raise_file_limit() {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a valid rlimit for the call to fill.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        warn!(err = %std::io::Error::last_os_error(), "can't read the open files limit");
        return;
    }
    let _ = STARTING_FILE_LIMIT.set(limit);
    if limit.rlim_cur >= limit.rlim_max {
        return;
    }
    let raised = libc::rlimit {
        rlim_cur: limit.rlim_max,
        rlim_max: limit.rlim_max,
    };
    // SAFETY: `raised` is a valid rlimit.
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raised) } != 0 {
        warn!(err = %std::io::Error::last_os_error(), "can't raise the open files limit");
        return;
    }
    info!(
        from = limit.rlim_cur,
        to = raised.rlim_cur,
        "raised the open files limit"
    );
}

/// xkbcommon's settings for `[keyboard]` (empty strings: its defaults).
pub fn xkb_config(kb: &crate::config::KeyboardConfig) -> XkbConfig<'_> {
    XkbConfig {
        rules: "",
        model: &kb.model,
        layout: &kb.layout,
        variant: &kb.variant,
        options: kb.options.clone(),
    }
}

impl Pila {
    pub fn new(
        event_loop: &mut EventLoop<'static, Pila>,
        display: Display<Self>,
        config: Config,
    ) -> Self {
        let clock = Clock::new();
        let dh = display.handle();

        let compositor_state = CompositorState::new_v6::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let layer_shell_state = WlrLayerShellState::new_with_filter::<Self, _>(&dh, is_privileged);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let primary_selection_state = PrimarySelectionState::new::<Self>(&dh);
        let xdg_decoration_state = XdgDecorationState::new::<Self>(&dh);
        let xdg_activation_state = XdgActivationState::new::<Self>(&dh);
        let cursor_shape_state = CursorShapeManagerState::new::<Self>(&dh);
        let dmabuf_state = DmabufState::new();
        let popups = PopupManager::default();
        let loop_handle = event_loop.handle();
        // Wakes the event loop (`queue_redraw`); there's nothing to do on
        // waking but run the idle callbacks that follow every dispatch.
        let (wake, wake_source) =
            smithay::reexports::calloop::ping::make_ping().expect("failed to create a ping");
        loop_handle
            .insert_source(wake_source, |_, _, _| {})
            .expect("failed to insert the wake source");
        let globals = protocols::init_globals(&dh, &loop_handle, &primary_selection_state);
        crate::capture::init_globals(&dh);
        crate::screencopy::init_global(&dh);
        crate::toplevel_management::init_global(&dh);
        crate::ext_workspace::init_global(&dh);
        crate::session_management::init_global(&dh);
        crate::gamma_control::init_global(&dh);
        crate::output_power::init_global(&dh);
        crate::output_management::init_global(&dh);
        let session_lock_state = SessionLockManagerState::new::<Self, _>(&dh, is_privileged);
        let xwayland_shell_state = XWaylandShellState::new::<Self>(&dh);
        PresentationState::new::<Self>(&dh, clock.id() as u32);

        let mut seat_state = SeatState::new();
        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "seat0");
        // pila always has a keyboard and pointer, whatever devices exist.
        // A keymap xkbcommon can't build (e.g. a typo in the layout) falls
        // back to the default one rather than leaving pila without keyboard.
        let kb = &config.keyboard;
        let mut keymap_failed = false;
        if let Err(err) = seat.add_keyboard(xkb_config(kb), kb.repeat_delay, kb.repeat_rate) {
            warn!(
                ?err,
                ?kb,
                "invalid keyboard config; using the default keymap"
            );
            seat.add_keyboard(XkbConfig::default(), kb.repeat_delay, kb.repeat_rate)
                .expect("failed to initialize keyboard");
            keymap_failed = true;
        }
        if keymap_failed {
            // Only in the log otherwise: say so on screen too.
            let what = crate::config::keymap_description(kb);
            notify_after_startup(
                &loop_handle,
                "Keyboard layout not found",
                format!("{what}: using the default keymap. Check [keyboard] in the config."),
            );
        }
        seat.add_pointer();

        let socket_name = Self::init_wayland_listener(display, event_loop);
        let loop_signal = event_loop.get_signal();

        Self {
            clock,
            socket_name,
            ipc_socket: None,
            ipc_events: Default::default(),
            display_handle: dh,
            loop_signal,
            loop_handle,
            wake,
            config,
            protocols: Protocols {
                compositor: compositor_state,
                xdg_shell: xdg_shell_state,
                layer_shell: layer_shell_state,
                shm: shm_state,
                output_manager: output_manager_state,
                seat: seat_state,
                data_device: data_device_state,
                primary_selection: primary_selection_state,
                xdg_decoration: xdg_decoration_state,
                xdg_activation: xdg_activation_state,
                cursor_shape: cursor_shape_state,
                dmabuf: dmabuf_state,
                session_lock: session_lock_state,
                syncobj: None,
            },
            windows: Windows::default(),
            input: InputState::default(),
            capture: Default::default(),
            outputs: Default::default(),
            xwayland: crate::xwayland::XwaylandState {
                wm: None,
                display: None,
                shell_state: xwayland_shell_state,
                token: None,
                restarts: Vec::new(),
            },
            pending_startup: Vec::new(),
            children: Vec::new(),
            snap_preview: None,
            snap_preview_buffer: SolidColorBuffer::default(),
            config_path: None,
            config_reload_pending: false,
            frame_drawn: false,
            hidden_frame_timer: false,
            commit_pacing: Default::default(),
            toplevel_management: Default::default(),
            workspaces: Default::default(),
            ext_workspaces: Default::default(),
            session_management: Default::default(),
            blur: Default::default(),
            animations: Default::default(),
            workspace_slide: None,
            frame_stats: HashMap::new(),
            capture_stats: Default::default(),
            space: Space::default(),
            popups,
            session_lock: SessionLock::default(),
            globals,
            idle_inhibitors: IdleInhibitors::default(),
            seat,
            cursor_status: CursorImageStatus::default_named(),
            dnd_icon: None,
            window_menu: None,
            backend: None,
        }
    }

    fn init_wayland_listener(
        display: Display<Pila>,
        event_loop: &mut EventLoop<'static, Pila>,
    ) -> OsString {
        // Picks the next free wayland-N name in $XDG_RUNTIME_DIR.
        let listening_socket =
            ListeningSocketSource::new_auto().expect("failed to create Wayland socket");
        let socket_name = listening_socket.socket_name().to_os_string();
        info!(socket = ?socket_name, "listening for Wayland clients");

        let loop_handle = event_loop.handle();
        loop_handle
            .insert_source(listening_socket, |client_stream, _, state| {
                let client_state = Arc::new(ClientState::for_stream(&client_stream));
                if let Err(err) = state
                    .display_handle
                    .insert_client(client_stream, client_state)
                {
                    warn!(?err, "failed to insert client");
                }
            })
            .expect("failed to init the Wayland listening source");

        loop_handle
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display, state| {
                    // SAFETY: the display is never dropped while the source is alive.
                    let dispatched = unsafe { display.get_mut().dispatch_clients(state) };
                    if let Err(err) = dispatched {
                        // Nothing works without it: out, with the usual cleanup.
                        error!(?err, "can't talk to the clients; quitting");
                        state.loop_signal.stop();
                    }
                    // Whatever the clients asked for may change the screen.
                    state.queue_redraw();
                    Ok(PostAction::Continue)
                },
            )
            .expect("failed to init the Wayland display source");

        socket_name
    }

    // ---------------------------------------------------------------------
    // Lookups
    // ---------------------------------------------------------------------

    /// The client surface under `pos` (with its origin), for pointer focus.
    /// `None` over pila's own decorations. While locked, only lock surfaces.
    pub fn surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        if self.is_locked() {
            return self.lock_surface_under(pos);
        }
        match self.under(pos)? {
            Under::Layer(_, surface, origin) | Under::Window(_, Hit::Surface(surface, origin)) => {
                Some((surface, origin))
            }
            Under::Window(_, Hit::Decoration(_)) => None,
        }
    }

    /// What's under `pos`, in stacking order: layer surfaces above the
    /// windows, the windows, then layer surfaces below them.
    pub fn under(&self, pos: Point<f64, Logical>) -> Option<Under> {
        let layer = |layers| {
            crate::layers::layer_surface_under(layers, pos)
                .map(|(layer, surface, origin)| Under::Layer(layer, surface, origin))
        };
        layer(self.upper_layers())
            .or_else(|| {
                self.window_under(pos)
                    .map(|(window, hit)| Under::Window(window, hit))
            })
            .or_else(|| layer(self.lower_layers()))
    }

    /// The topmost window under `pos` and what part of it was hit. Per window,
    /// front to back as it's drawn: client surfaces (incl. popups), title bar,
    /// resize border.
    /// Ignores layer surfaces; use `under` for anything input-related.
    fn window_under(&self, pos: Point<f64, Logical>) -> Option<(Window, Hit)> {
        self.space.elements().rev().find_map(|window| {
            let loc = self.space.element_location(window)?;
            let render_loc = loc - window.geometry().loc;
            if let Some((surface, origin)) =
                window.surface_under(pos - render_loc.to_f64(), WindowSurfaceType::ALL)
            {
                let origin = (origin + render_loc).to_f64();
                return Some((window.clone(), Hit::Surface(surface, origin)));
            }
            if let Some(bar) = decoration::title_bar_rect(window, loc) {
                if bar.to_f64().contains(pos) {
                    let part = decoration::title_bar_part(bar, pos);
                    return Some((window.clone(), Hit::Decoration(Decoration::TitleBar(part))));
                }
            }
            decoration::border_edges(window, loc, pos)
                .map(|edges| (window.clone(), Hit::Decoration(Decoration::Border(edges))))
        })
    }

    /// Where the pointer is, in global logical coordinates.
    pub fn pointer_location(&self) -> Point<f64, Logical> {
        self.seat
            .get_pointer()
            .map(|p| p.current_location())
            .unwrap_or_default()
    }

    /// The scene and the cursor, for a frame or a capture (`render::FrameInput`).
    pub fn frame_input(&self) -> FrameInput {
        let pointer = self.pointer_location();
        FrameInput {
            scene: self.scene(),
            pointer,
            cursor_surface: crate::cursor::surface_position(&self.cursor_status, pointer),
            named_cursor: match &self.cursor_status {
                CursorImageStatus::Named(icon) => Some(*icon),
                _ => None,
            },
        }
    }

    /// Layer surfaces and mapped windows front to back, with what's needed to
    /// draw title bars. Collected up front so a backend can render without
    /// borrowing `self`.
    pub fn scene(&self) -> Scene {
        if self.is_locked() {
            let lock_surfaces = self
                .space
                .outputs()
                .filter_map(|output| {
                    let surface = self.lock_surface(output)?;
                    let location = self.space.output_geometry(output)?.loc;
                    Some((surface.wl_surface().clone(), location))
                })
                .collect();
            return Scene {
                locked: true,
                lock_surfaces,
                ..Default::default()
            };
        }
        let now = self.animation_time();
        let (mut windows, ghosts) = self.scene_windows(now);
        let slide = self.scene_slide(now);
        if let Some(slide) = &slide {
            for entry in &mut windows {
                if !slide.carried.contains(&entry.window) {
                    entry.slid = Some(self.window_output(&entry.window));
                }
            }
        }
        let layers = |layers: Vec<(LayerSurface, Point<i32, Logical>)>| {
            layers
                .into_iter()
                .map(|(layer, location)| SceneLayer {
                    blur: crate::blur::blur_rects(&layer),
                    effect: self.layer_effect(&layer, location, now),
                    layer,
                    location,
                })
                .collect()
        };
        Scene {
            upper: layers(self.upper_layers()),
            layer_ghosts: self.scene_layer_ghosts(now),
            slide,
            snap_preview: self
                .snap_preview
                .map(|rect| (self.snap_preview_buffer.clone(), rect)),
            windows,
            ghosts,
            lower: layers(self.lower_layers()),
            dnd_icon: self.dnd_icon.as_ref().map(|icon| {
                let at = self.pointer_location().to_i32_round() + icon.offset;
                (icon.surface.clone(), at)
            }),
            window_menu: self.window_menu.as_ref().map(|menu| menu.scene()),
            corner_radius: self.config.corner_radius,
            colors: self.config.colors,
            ..Default::default()
        }
    }

    pub(crate) fn scene_windows(
        &self,
        now: std::time::Duration,
    ) -> (Vec<SceneWindow>, Vec<SceneGhost>) {
        let focused = self.focused_window();
        let pointer = self.pointer_location();
        let hovered = match self.under(pointer) {
            Some(Under::Window(window, Hit::Decoration(Decoration::TitleBar(part)))) => {
                Some((window, part))
            }
            _ => None,
        };
        // A window fading out as a ghost isn't drawn itself.
        let windows: Vec<Window> = self
            .space
            .elements()
            .rev()
            .filter(|w| !self.is_ghost(w))
            .cloned()
            .collect();
        let ghosts = self.scene_ghosts(&windows, now);
        let windows = windows
            .iter()
            .filter_map(|window| {
                self.scene_window(window, focused.as_ref(), hovered.as_ref(), Some(now))
            })
            .collect();
        (windows, ghosts)
    }

    /// How a mapped `window` is drawn: its title bar (`focused` and
    /// `hovered` decide its look), flash, corners and, given the time,
    /// its animation.
    pub(crate) fn scene_window(
        &self,
        window: &Window,
        focused: Option<&Window>,
        hovered: Option<&(Window, TitleBarPart)>,
        now: Option<std::time::Duration>,
    ) -> Option<SceneWindow> {
        let loc = self.space.element_location(window)?;
        let bar_rect = decoration::title_bar_rect(window, loc);
        let (maximized, fullscreen) = fill_state(window);
        let (radius, alpha, shadow_rule) = crate::rules::with_window_props(window, |props| {
            (
                props.corner_radius.unwrap_or(self.config.corner_radius),
                props.opacity.unwrap_or(1.0) as f32,
                props.shadow,
            )
        });
        let rounding = crate::rounded::Rounding::new(
            Rectangle::new(loc, window.geometry().size),
            bar_rect,
            radius,
            maximized || fullscreen,
        );
        let title_bar = bar_rect.map(|rect| SceneTitleBar {
            rect,
            focused: focused == Some(window),
            hover: hovered.filter(|(w, _)| w == window).map(|(_, part)| *part),
            colors: self.config.colors,
        });
        let render_location = loc - window.geometry().loc;
        let frame = bar_rect.map_or(Rectangle::new(loc, window.geometry().size), |bar| {
            bar.merge(Rectangle::new(loc, window.geometry().size))
        });
        let opaque = alpha >= 1.0 && crate::blur::is_opaque(window);
        let config = &self.config.shadow;
        // Past its geometry a client draws what a shadow would be.
        let wanted = shadow_rule
            .unwrap_or_else(|| config.enabled && window.geometry().contains_rect(window.bbox()));
        let shadow = (wanted
            && config.size > 0
            && !maximized
            && !fullscreen
            && !crate::workspaces::is_override_redirect(window))
        .then(|| crate::rounded::Shadow {
            frame,
            radius: rounding.map_or(0.0, |r| r.radius),
            size: config.size,
            offset: config.offset.into(),
            color: if focused == Some(window) {
                config.color
            } else {
                config.inactive_color
            },
        });
        Some(SceneWindow {
            window: window.clone(),
            render_location,
            blur: if opaque {
                Vec::new()
            } else {
                crate::blur::window_blur_rects(window, render_location, frame)
            },
            opaque,
            title_bar,
            flash: crate::bell::flash(window, loc),
            rounding,
            shadow,
            alpha,
            effect: now.and_then(|now| self.window_effect(window, now)),
            slid: None,
        })
    }

    /// Find a window (mapped or not) by its toplevel surface.
    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<Window> {
        self.space
            .elements()
            .chain(self.windows.unmapped.iter())
            .chain(self.off_space_windows())
            .find(|w| w.wl_surface().is_some_and(|s| *s == *surface))
            .cloned()
    }

    /// The window with keyboard focus. While a menu is open the focus is on
    /// the popup, so this resolves it to the popup's parent window.
    pub fn focused_window(&self) -> Option<Window> {
        let focus = self.keyboard_focus()?;
        let focus = self.popup_root(&focus).unwrap_or(focus);
        self.space
            .elements()
            .find(|w| w.wl_surface().is_some_and(|s| *s == focus))
            .cloned()
    }

    /// The surface with keyboard focus (an X11 window's Wayland surface for
    /// X11 focus), if any.
    pub fn keyboard_focus(&self) -> Option<WlSurface> {
        self.seat.get_keyboard()?.current_focus()?.surface()
    }

    /// The toplevel surface a popup belongs to, if `surface` is a popup.
    pub fn popup_root(&self, surface: &WlSurface) -> Option<WlSurface> {
        let popup = self.popups.find_popup(surface)?;
        find_popup_root_surface(&popup).ok()
    }

    /// The output under `pos`, or the first output.
    pub fn output_at(&self, pos: Point<f64, Logical>) -> Option<Output> {
        self.space
            .output_under(pos)
            .next()
            .or_else(|| self.space.outputs().next())
            .cloned()
    }

    /// Geometry of the output under `pos`, or the first output.
    pub fn output_geometry_at(&self, pos: Point<f64, Logical>) -> Option<Rectangle<i32, Logical>> {
        self.space.output_geometry(&self.output_at(pos)?)
    }

    /// The part of the output under `pos` that panels (layer surfaces with an
    /// exclusive zone) leave for windows, in global coordinates.
    pub fn usable_geometry_at(&self, pos: Point<f64, Logical>) -> Option<Rectangle<i32, Logical>> {
        self.output_area(&self.output_at(pos)?, true)
    }

    /// All of `output`, or only the `usable` part (see `usable_geometry_at`).
    pub fn output_area(&self, output: &Output, usable: bool) -> Option<Rectangle<i32, Logical>> {
        let output_geo = self.space.output_geometry(output)?;
        if !usable {
            return Some(output_geo);
        }
        let zone = layer_map_for_output(output).non_exclusive_zone();
        Some(Rectangle::new(output_geo.loc + zone.loc, zone.size))
    }

    /// An output went away (already unmapped from the space): close its
    /// panels so their clients can recreate them elsewhere, drop its lock
    /// surface, and bring windows and the pointer back onto the remaining
    /// outputs.
    pub fn output_removed(&mut self, output: &Output) {
        self.stop_captures_of(output);
        self.fail_screencopies_of(output);
        self.gamma_output_removed(output);
        self.output_power_removed(output);
        self.blur.forget_output(&output.name());
        let layers: Vec<_> = layer_map_for_output(output).layers().cloned().collect();
        for layer in layers {
            layer.layer_surface().send_close();
        }
        self.drop_lock_surface(output);

        if let Some(pointer) = self.seat.get_pointer() {
            let pos = pointer.current_location();
            if self.space.output_under(pos).next().is_none() {
                if let Some(geo) = self.output_geometry_at(pos) {
                    pointer.set_location((geo.loc + geo.size.downscale(2).to_point()).to_f64());
                }
            }
        }
        self.fixup_windows();
    }

    /// Move windows that aren't on any output anymore onto the pointer's
    /// output (centered), including where they return to after maximize or
    /// fullscreen, then refit maximized and fullscreen windows.
    pub fn fixup_windows(&mut self) {
        let pointer = self.pointer_location();
        let on_an_output = |space: &Space<Window>, rect: Rectangle<i32, Logical>| {
            space
                .outputs()
                .filter_map(|o| space.output_geometry(o))
                .any(|geo| geo.overlaps(rect))
        };
        // No output left: nothing to move windows onto.
        let area = self.usable_geometry_at(pointer);
        let windows: Vec<Window> = self.space.elements().cloned().collect();
        for window in windows {
            let Some(area) = area else {
                break;
            };
            let bar = decoration::layout_offset(&window, false);
            if let Some(geo) = self.space.element_geometry(&window) {
                if !on_an_output(&self.space, geo) {
                    let loc = centered_location(area, geo.size, bar);
                    debug!(?loc, "moving window off a removed output");
                    self.space.map_element(window.clone(), loc, false);
                    self.keep_transients_above(&window);
                    self.x11_window_moved(&window);
                }
            }
            if let Some(restore) = window.user_data().get::<RestoreGeometry>() {
                let mut saved = restore.0.borrow_mut();
                if let Some(rect) = saved.as_mut().filter(|r| !on_an_output(&self.space, **r)) {
                    rect.loc = centered_location(area, rect.size, bar);
                }
            }
        }
        self.refit_maximized();
    }

    /// The usable area (minus panels) of the output a window is mostly on.
    pub fn usable_area_for(&self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        self.output_geometry_for_window(window, true)
    }

    /// Drop maximized/fullscreen without restoring the old geometry (the
    /// caller places the window itself, e.g. snapping it).
    pub fn clear_fill_state(&mut self, window: &Window) {
        if let Some(toplevel) = window.toplevel() {
            toplevel.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Maximized);
                state.states.unset(xdg_toplevel::State::Fullscreen);
            });
        }
        if let Some(x11) = window.x11_surface() {
            let _ = x11.set_maximized(false);
            let _ = x11.set_fullscreen(false);
        }
        if let Some(restore) = window.user_data().get::<RestoreGeometry>() {
            restore.0.borrow_mut().take();
        }
    }

    /// Where a window is: its place in the `Space`, or for one off it
    /// (another workspace, minimized) where it comes back to. `None` for an
    /// unmapped window.
    pub fn window_location(&self, window: &Window) -> Option<Point<i32, Logical>> {
        self.space.element_location(window).or_else(|| {
            self.off_space_windows()
                .any(|w| w == window)
                .then(|| crate::workspaces::space_location(window))
        })
    }

    /// Put a window at `loc`: in the `Space` if it's there, else where it
    /// comes back to when shown again. Nothing for an unmapped window.
    fn place_window(&mut self, window: &Window, loc: Point<i32, Logical>) {
        if self.space.element_location(window).is_some() {
            self.space.map_element(window.clone(), loc, true);
            self.keep_transients_above(window);
        } else if self.off_space_windows().any(|w| w == window) {
            crate::workspaces::set_space_location(window, loc);
        }
    }

    /// Geometry of the output a window is mostly on, falling back to the
    /// pointer's output: all of it, or only the `usable` part.
    fn output_geometry_for_window(
        &self,
        window: &Window,
        usable: bool,
    ) -> Option<Rectangle<i32, Logical>> {
        let pos = match self.window_location(window) {
            Some(loc) => (loc + window.geometry().size.downscale(2).to_point()).to_f64(),
            // Not placed yet: where its session puts it, or at the pointer.
            None => match self
                .session_output(window)
                .and_then(|output| self.space.output_geometry(&output))
            {
                Some(geo) => geo.loc.to_f64(),
                None => self.seat.get_pointer()?.current_location(),
            },
        };
        if usable {
            self.usable_geometry_at(pos)
        } else {
            self.output_geometry_at(pos)
        }
    }

    // ---------------------------------------------------------------------
    // Window management
    // ---------------------------------------------------------------------

    /// Map a window the first time it has content: centered on the pointer's
    /// output, or where its session says if the client restored one
    /// (`session_management.rs`), cascaded if that spot is taken; or filling
    /// the output if the client asked for maximize/fullscreen before mapping
    /// or its session has it filled. Window rules may
    /// give it a position, or another workspace (it opens there, unfocused).
    pub fn place_new_window(&mut self, window: Window) {
        // Its app id and title are known by now.
        let props = self.resolve_window_rules(&window);
        self.place_new_window_with(window, props);
    }

    /// `place_new_window` with its window rules already resolved (an X11
    /// window's, whose size had to be set before).
    pub fn place_new_window_with(&mut self, window: Window, props: crate::rules::WindowProps) {
        self.windows.desktop_shown = None;
        self.mark_new_window(
            &window,
            props.always_on_top == Some(true),
            props.all_workspaces == Some(true),
        );
        let pointer_location = self.pointer_location();
        let (maximized, fullscreen) = fill_state(&window);
        // A position rule wins over the session, and is from the pointer's
        // output like any other.
        let restored = self
            .take_session_restore(&window)
            .filter(|_| props.position.is_none());
        let restored_rect = restored.as_ref().map(|(rect, _)| *rect);
        let anchor = restored
            .and_then(|(_, output)| self.space.output_geometry(&output))
            .map_or(pointer_location, |geo| geo.loc.to_f64());
        let output_geo = if fullscreen {
            self.output_geometry_at(anchor)
        } else {
            self.usable_geometry_at(anchor)
        }
        .unwrap_or_else(|| Rectangle::from_size((800, 600).into()));

        // Room for the title bar above the window, if pila draws one.
        let bar = decoration::layout_offset(&window, fullscreen);
        let location = if maximized || fullscreen {
            output_geo.loc + Point::from((0, bar))
        } else if let Some((x, y)) = props.position {
            debug!(target: "pila::rules", x, y, "placed by rule");
            output_geo.loc + Point::from((x, y + bar))
        } else {
            let size = window.geometry().size;
            let taken = |loc| {
                self.space
                    .elements()
                    .any(|w| self.space.element_location(w) == Some(loc))
            };
            let fits = |loc: Point<i32, Logical>| {
                let end = output_geo.loc + output_geo.size.to_point();
                loc.x + size.w <= end.x && loc.y + size.h <= end.y
            };
            let mut loc = match restored_rect {
                Some(rect) => {
                    crate::session_management::fit_in_area(rect.loc, size, bar, output_geo)
                }
                None => centered_location(output_geo, size, bar),
            };
            // Each window takes at most one spot, so this many steps find a
            // free one unless wrapping comes back to taken spots.
            for _ in 0..self.space.elements().count() {
                if !taken(loc) {
                    break;
                }
                loc += Point::from((CASCADE_STEP, CASCADE_STEP));
                // Off the bottom/right of the area: start again at its top left.
                if !fits(loc) {
                    loc = output_geo.loc + Point::from((0, bar));
                }
            }
            loc
        };

        if let (Some(rect), true) = (restored_rect, maximized || fullscreen) {
            set_restore_geometry(&window, rect);
        }

        // The app id, for window rules.
        let app_id = crate::foreign_toplevel::title_and_app_id(&window).1;
        debug!(app_id, ?location, size = ?window.geometry().size, maximized, fullscreen, "mapping window");
        self.space.map_element(window.clone(), location, true);
        self.announce_toplevel(&window);
        // A window set on another one opens where that one is.
        let parent_workspace = self
            .transient_parent(&window)
            .map(|parent| self.workspace_of(&parent));
        if let Some(index) = props
            .open_on_workspace
            .or(parent_workspace)
            .filter(|index| *index != self.workspaces.active)
            .filter(|_| !crate::stacking::is_on_all_workspaces(&window))
        {
            debug!(target: "pila::rules", app_id, workspace = index + 1, "opens on workspace");
            self.park(&window, index);
            self.refresh_pointer_focus();
            return;
        }
        // An X11 app that asked to start minimized (`WM_HINTS` IconicState,
        // which Smithay turns into hidden): no focus, straight to minimized.
        if window.x11_surface().is_some_and(|x11| x11.is_hidden()) {
            debug!(app_id, "starts minimized");
            self.minimize_window(&window);
            self.refresh_pointer_focus();
            return;
        }
        self.animate_open(&window);
        self.focus_window(Some(window), smithay::utils::SERIAL_COUNTER.next_serial());
    }

    /// Give keyboard focus to `window` and raise it, or clear focus. While a
    /// layer surface holds exclusive focus (`exclusive_layer`), the window is
    /// only raised and the keyboard stays with the layer surface.
    pub fn focus_window(&mut self, window: Option<Window>, serial: Serial) {
        if self.is_locked() {
            self.focus_lock_surface(serial);
            return;
        }
        // A popup grab ignores focus changes, so close the menu first.
        self.dismiss_popup_grab();
        let exclusive = self
            .exclusive_layer()
            .map(|l| KeyboardFocus::from(l.wl_surface().clone()));
        match window {
            Some(window) => {
                let window = self.focus_redirect(window);
                self.touch_focus_history(&window);
                self.raise_family(&window);
                let target = focus_target(&window);
                self.set_keyboard_focus(exclusive.or(target), serial);
            }
            None => {
                for w in self.space.elements() {
                    w.set_activated(false);
                }
                self.set_keyboard_focus(exclusive, serial);
            }
        }
        for w in self.space.elements() {
            if let Some(toplevel) = w.toplevel() {
                toplevel.send_pending_configure();
            }
        }
    }

    /// If a menu holds a popup grab, dismiss its popups and release the
    /// keyboard grab. The pointer grab ends itself once the client destroys
    /// the dismissed popups.
    pub fn dismiss_popup_grab(&mut self) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        // Popup grabs are the only keyboard grabs pila takes.
        if !keyboard.is_grabbed() {
            return;
        }
        // A dead focus (its window was just destroyed) has no popups left to close.
        if let Some(focus) = self.keyboard_focus().filter(|f| f.is_alive()) {
            let root = self.popup_root(&focus).unwrap_or(focus);
            for (popup, _) in PopupManager::popups_for_surface(&root) {
                let _ = PopupManager::dismiss_popup(&root, &popup);
            }
        }
        keyboard.unset_grab(self);
    }

    /// Fit maximized windows to the usable area again after it changed (a
    /// panel appeared, resized or went away, or the output was resized).
    pub fn refit_maximized(&mut self) {
        let windows: Vec<Window> = self
            .space
            .elements()
            .filter(|w| fill_state(w) != (false, false))
            .cloned()
            .collect();
        for window in windows {
            let (maximized, fullscreen) = fill_state(&window);
            self.set_fill_state(&window, maximized, fullscreen);
        }
    }

    pub fn close_focused(&mut self) {
        if let Some(window) = self.focused_window() {
            close_window(&window);
        }
    }

    /// Set a window's maximized/fullscreen states. Entering either state from
    /// a normal window remembers its geometry; leaving both restores it.
    pub fn set_fill_state(&mut self, window: &Window, maximized: bool, fullscreen: bool) {
        self.set_fill_state_on(window, maximized, fullscreen, None);
    }

    /// `set_fill_state`, filling `output` instead of the output the window
    /// is on (e.g. a client asked to go fullscreen on a specific monitor).
    pub fn set_fill_state_on(
        &mut self,
        window: &Window,
        maximized: bool,
        fullscreen: bool,
        output: Option<&Output>,
    ) {
        let toplevel = window.toplevel();
        let x11 = window.x11_surface();
        if toplevel.is_none() && x11.is_none() {
            return;
        }
        // On another workspace or minimized it counts as mapped too, at
        // the place it comes back to.
        let mapped_loc = self.window_location(window);
        let (was_maximized, was_fullscreen) = fill_state(window);
        // On screen and changing: move to the new rect, not jump (refits
        // keep the state and don't animate).
        if (maximized, fullscreen) != (was_maximized, was_fullscreen)
            && self.space.element_location(window).is_some()
        {
            self.animate_geometry(window);
        }
        window
            .user_data()
            .insert_if_missing(RestoreGeometry::default);
        let restore = &window.user_data().get::<RestoreGeometry>().unwrap().0;

        if maximized || fullscreen {
            let usable = !fullscreen;
            let output_geo = match output {
                Some(output) => self.output_area(output, usable),
                None => self.output_geometry_for_window(window, usable),
            };
            let Some(mut output_geo) = output_geo else {
                debug!("no output to fill");
                return;
            };
            // A maximized window's title bar stays visible above it.
            let bar = decoration::layout_offset(window, fullscreen);
            output_geo.loc.y += bar;
            output_geo.size.h -= bar;
            if let (Some(loc), false) = (mapped_loc, was_maximized || was_fullscreen) {
                *restore.borrow_mut() = Some(Rectangle::new(loc, window.geometry().size));
            }
            // A snapped window returns to where it was before snapping.
            if let Some(before) = self.forget_snap(window) {
                *restore.borrow_mut() = Some(before);
            }
            if let Some(toplevel) = toplevel {
                toplevel.with_pending_state(|state| {
                    set_state(&mut state.states, xdg_toplevel::State::Maximized, maximized);
                    set_state(
                        &mut state.states,
                        xdg_toplevel::State::Fullscreen,
                        fullscreen,
                    );
                    state.size = Some(output_geo.size);
                });
            }
            if let Some(x11) = x11 {
                // X applies the new geometry directly (no configure/ack cycle).
                let _ = x11.set_maximized(maximized);
                let _ = x11.set_fullscreen(fullscreen);
                let _ = x11.configure(output_geo);
            }
            self.place_window(window, output_geo.loc);
        } else {
            // The saved geometry, or (if the window started out filled, as
            // a terminal configured to open maximized) two thirds of its
            // output, centered: left where it was, at a size of its own
            // choosing, it could end up in a corner straddling two outputs.
            let saved = restore.borrow_mut().take().or_else(|| {
                self.output_geometry_for_window(window, true).map(|area| {
                    let size = (area.size.w * 2 / 3, area.size.h * 2 / 3).into();
                    let bar = decoration::layout_offset(window, false);
                    Rectangle::new(centered_location(area, size, bar), size)
                })
            });
            if let Some(toplevel) = toplevel {
                toplevel.with_pending_state(|state| {
                    state.states.unset(xdg_toplevel::State::Maximized);
                    state.states.unset(xdg_toplevel::State::Fullscreen);
                    state.size = saved.map(|r| r.size);
                });
            }
            if let Some(x11) = x11 {
                let _ = x11.set_maximized(false);
                let _ = x11.set_fullscreen(false);
                if let Some(saved) = saved {
                    let _ = x11.configure(saved);
                }
            }
            if let Some(saved) = saved {
                self.place_window(window, saved.loc);
            }
        }

        if let Some(toplevel) = toplevel.filter(|t| t.is_initial_configure_sent()) {
            toplevel.send_pending_configure();
        }
        self.queue_redraw();
    }

    pub fn begin_move(
        &mut self,
        window: Window,
        start_data: PointerGrabStartData<Self>,
        serial: Serial,
    ) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        if self.is_locked() {
            return;
        }

        // Dragging a maximized/fullscreen window restores it under the cursor.
        let (maximized, fullscreen) = fill_state(&window);
        if maximized || fullscreen {
            let width = restore_geometry(&window)
                .map(|r| r.size.w)
                .unwrap_or(window.geometry().size.w);
            // It follows the pointer at once, no animation in the way.
            self.without_geometry_animation(|state| state.set_fill_state(&window, false, false));
            // Keep the grab point on the title bar if pila draws one,
            // otherwise near the top of the client's own.
            let y_offset = if decoration::is_ssd(&window) {
                -decoration::TITLE_BAR_HEIGHT / 2
            } else {
                CASCADE_STEP / 2
            };
            let loc = Point::from((
                start_data.location.x as i32 - width / 2,
                start_data.location.y as i32 - y_offset,
            ));
            self.space.map_element(window.clone(), loc, true);
            self.keep_transients_above(&window);
            self.x11_window_moved(&window);
        } else if crate::snap::snapped_side(&window).is_some() {
            // Same for a snapped window: back to its old size under the cursor.
            let width = self
                .window_rect(&window)
                .map(|r| r.size.w)
                .unwrap_or(window.geometry().size.w);
            let before_w = crate::snap::snapped_before_width(&window).unwrap_or(width);
            let x = start_data.location.x as i32 - before_w / 2;
            let y = self.space.element_location(&window).map_or(0, |l| l.y);
            self.unsnap_window(&window, Some((x, y).into()));
        }

        let Some(initial_window_location) = self.space.element_location(&window) else {
            return;
        };
        let grab = MoveSurfaceGrab {
            start_data,
            window,
            initial_window_location,
            snap: None,
            moved: false,
        };
        pointer.set_grab(self, grab, serial, Focus::Clear);
        // After set_grab: clearing the focus resets the cursor to the default.
        self.set_cursor(CursorImageStatus::Named(CursorIcon::Grabbing));
    }

    pub fn begin_resize(
        &mut self,
        window: Window,
        edges: ResizeEdge,
        start_data: PointerGrabStartData<Self>,
        serial: Serial,
    ) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let (maximized, fullscreen) = fill_state(&window);
        if maximized || fullscreen || self.is_locked() {
            return;
        }
        // A resized window isn't snapped anymore.
        self.forget_snap(&window);
        let Some(location) = self.space.element_location(&window) else {
            return;
        };
        if let Some(toplevel) = window.toplevel() {
            toplevel.with_pending_state(|state| {
                state.states.set(xdg_toplevel::State::Resizing);
            });
            toplevel.send_pending_configure();
        } else if window.x11_surface().is_none() {
            return;
        }

        let size = window.geometry().size;

        let grab =
            ResizeSurfaceGrab::start(start_data, window, edges, Rectangle::new(location, size));
        pointer.set_grab(self, grab, serial, Focus::Clear);
        // After set_grab: clearing the focus resets the cursor to the default.
        self.set_cursor(CursorImageStatus::Named(edges.cursor_icon()));
    }

    /// Ask the backend for a redraw. pila only redraws when something may
    /// have changed: input, client requests, or its own state changes outside
    /// of those (e.g. a deferred unlock).
    pub fn queue_redraw(&mut self) {
        if let Some(backend) = &mut self.backend {
            backend.queue_redraw();
        }
        // On bare metal the redraw is an idle callback, which calloop only
        // runs after the next event; waking the loop makes that now.
        self.wake.ping();
    }

    /// The `[[output]]` section for the output called `name`, under that
    /// name whatever the section says (what's keyed on it, like
    /// `Outputs::overrides`, goes by the output's name).
    pub fn output_section(&self, name: &str) -> Option<crate::config::OutputConfig> {
        let section = self.config.output(name, self.outputs.monitors.get(name))?;
        let mut section = section.clone();
        section.name = name.to_string();
        Some(section)
    }

    /// Housekeeping after `output` was drawn, for any backend: what
    /// concerns that output now, the rest once per event-loop iteration
    /// however many outputs were drawn (`after_frames`).
    pub fn post_render(&mut self, output: &Output) {
        self.update_surface_scales(output);
        layer_map_for_output(output).cleanup();
        self.signal_barriers_on(output);
        self.frame_drawn = true;
    }

    /// Called after every event-loop iteration (`main.rs`), once all the
    /// outputs due were drawn: captures and screencopies, dead windows and
    /// popups, taskbar listeners and idle inhibitors. Nothing if no output
    /// was drawn.
    pub fn after_frames(&mut self) {
        // Every iteration: a window can close before its last move is drawn.
        self.refresh_sessions();
        // A layout can change with no frame to draw (an xkb option's keys).
        self.send_keyboard_events();
        if !std::mem::take(&mut self.frame_drawn) {
            // A change no frame follows (every monitor turned off).
            self.send_ipc_events_due();
            return;
        }
        self.apply_unblocked_commits();
        self.process_captures();
        self.process_screencopies();
        self.capture.trim_textures();
        // The space knows which outputs each window is on only after its
        // refresh, so that comes before the taskbar listeners are updated.
        self.space.refresh();
        self.refresh_toplevels();
        self.send_ipc_events();
        self.update_idle_inhibit();
        self.popups.cleanup();
        self.step_animations();
        self.schedule_hidden_frames();
    }

    /// Show a compositor-chosen cursor, e.g. during a move/resize grab. The
    /// grabs reset it to the default when they end; the client under the
    /// pointer then sets its own on re-enter.
    pub fn set_cursor(&mut self, status: CursorImageStatus) {
        let seat = self.seat.clone();
        self.cursor_image(&seat, status);
    }

    // ---------------------------------------------------------------------
    // Actions
    // ---------------------------------------------------------------------

    pub fn run_action(&mut self, action: Action, serial: Serial) {
        match action {
            Action::Spawn(command) => self.spawn(&command),
            Action::Terminal => {
                if let Some(terminal) = self.config.terminal_command() {
                    self.spawn(&terminal);
                }
            }
            Action::Launcher => {
                if let Some(launcher) = self.config.launcher.clone() {
                    self.spawn(&launcher);
                }
            }
            Action::CloseFocused => self.close_focused(),
            Action::CycleFocus => self.cycle_focus(true, serial),
            Action::CycleFocusBack => self.cycle_focus(false, serial),
            Action::Minimize => {
                if let Some(window) = self.focused_window() {
                    self.minimize_window(&window);
                }
            }
            Action::SnapLeft | Action::SnapRight => {
                if let Some(window) = self.focused_window() {
                    let side = if action == Action::SnapLeft {
                        crate::snap::Side::Left
                    } else {
                        crate::snap::Side::Right
                    };
                    self.snap_window(&window, side);
                }
            }
            Action::SnapTopLeft
            | Action::SnapTopRight
            | Action::SnapBottomLeft
            | Action::SnapBottomRight => {
                if let Some(window) = self.focused_window() {
                    use crate::snap::Side;
                    let side = match action {
                        Action::SnapTopLeft => Side::TopLeft,
                        Action::SnapTopRight => Side::TopRight,
                        Action::SnapBottomLeft => Side::BottomLeft,
                        _ => Side::BottomRight,
                    };
                    self.snap_window(&window, side);
                }
            }
            Action::ToggleAlwaysOnTop => {
                if let Some(window) = self.focused_window() {
                    self.toggle_always_on_top(&window);
                }
            }
            Action::ToggleAllWorkspaces => {
                if let Some(window) = self.focused_window() {
                    self.toggle_all_workspaces(&window);
                }
            }
            Action::ShowDesktop => self.show_desktop(),
            Action::FocusOutput(target) => self.focus_output(target),
            Action::MoveToOutput(target) => self.move_focused_to_output(target),
            Action::SwitchLayout(target) => self.switch_layout(target),
            Action::PowerOffMonitors => self.power_off_monitors(),
            Action::PowerOnMonitors => self.power_on_monitors(),
            Action::Restore => {
                if let Some(window) = self.focused_window() {
                    self.restore_window_size(&window);
                }
            }
            Action::MoveWindow(x, y) => {
                if let Some(window) = self.focused_window() {
                    self.move_window_by(&window, (x, y).into());
                }
            }
            Action::ResizeWindow(w, h) => {
                if let Some(window) = self.focused_window() {
                    self.resize_window_by(&window, (w, h));
                }
            }
            Action::Center => {
                if let Some(window) = self.focused_window() {
                    self.center_window(&window);
                }
            }
            Action::ToggleMaximize => {
                if let Some(window) = self.focused_window() {
                    let (maximized, fullscreen) = fill_state(&window);
                    self.set_fill_state(&window, !maximized, fullscreen);
                }
            }
            Action::ToggleFullscreen => {
                if let Some(window) = self.focused_window() {
                    let (maximized, fullscreen) = fill_state(&window);
                    self.set_fill_state(&window, maximized, !fullscreen);
                }
            }
            Action::Workspace(target) => self.go_to_workspace(target),
            Action::MoveToWorkspace(target) => self.move_focused_to_workspace(target),
            Action::Quit => {
                info!("quit requested");
                self.loop_signal.stop();
            }
            Action::ChangeVt(vt) => {
                if let Some(backend) = &mut self.backend {
                    backend.change_vt(vt);
                }
            }
        }
    }

    /// The startup programs: once Xwayland is ready (or failed), so they
    /// get its `DISPLAY`, and after the session bus knows pila's
    /// environment (for portals), or `EXPORT_TIMEOUT` if that hangs.
    pub fn start_session(&mut self, commands: Vec<String>) {
        self.pending_startup = commands;
        if !self.export_environment() {
            self.run_pending_startup();
        }
    }

    /// The startup programs still waiting, if any (`start_session`).
    fn run_pending_startup(&mut self) {
        let commands = std::mem::take(&mut self.pending_startup);
        self.spawn_all(&commands);
    }

    /// Give the D-Bus session bus pila's `WAYLAND_DISPLAY`, `DISPLAY` and
    /// desktop name, so services it activates (xdg-desktop-portal and its
    /// wlr backend, for screen sharing) talk to pila.
    ///
    /// Only on bare metal; nested, apps use the host's portals. On the
    /// systemd user bus (a login session, `scripts/install`) the variables go
    /// to `systemd --user` too, which is what starts the portals and pipewire
    /// there. A private bus (`scripts/tty` under `dbus-run-session`) has no
    /// systemd: that's how a test instance on another VT leaves the login
    /// session's services alone.
    ///
    /// The export runs in the background (a stuck bus mustn't freeze pila);
    /// `run_pending_startup` follows when it's done or after
    /// `EXPORT_TIMEOUT`. Returns false if nothing was started.
    fn export_environment(&mut self) -> bool {
        if !self.backend.as_ref().is_some_and(Backend::is_session) {
            return false;
        }
        let Some(bus) = std::env::var_os("DBUS_SESSION_BUS_ADDRESS") else {
            info!("no D-Bus session bus; screen sharing through portals won't work");
            return false;
        };
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").unwrap_or_default();
        let systemd = is_user_bus(&bus.to_string_lossy(), &runtime.to_string_lossy());
        let mut cmd = Command::new("dbus-update-activation-environment");
        if systemd {
            cmd.arg("--systemd");
        }
        cmd.args(SESSION_ENV);
        self.session_env(&mut cmd);
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                warn!(?err, "can't run dbus-update-activation-environment");
                return false;
            }
        };

        let (sender, channel) = calloop::channel::channel();
        let waiter = std::thread::Builder::new()
            .name("pila-dbus-env".into())
            .spawn(move || {
                let _ = sender.send(child.wait());
            });
        if let Err(err) = waiter {
            warn!(?err, "can't wait for dbus-update-activation-environment");
            return false;
        }
        let done = self
            .loop_handle
            .insert_source(channel, move |event, _, state| {
                if let calloop::channel::Event::Msg(result) = event {
                    match result {
                        Ok(status) if status.success() => {
                            info!(systemd, "exported the environment to the session bus")
                        }
                        Ok(status) => warn!(?status, "dbus-update-activation-environment failed"),
                        Err(err) => warn!(?err, "dbus-update-activation-environment failed"),
                    }
                    state.run_pending_startup();
                }
            });
        if let Err(err) = done {
            warn!(?err, "can't wait for dbus-update-activation-environment");
            return false;
        }
        let timeout = Timer::from_duration(EXPORT_TIMEOUT);
        let _ = self.loop_handle.insert_source(timeout, |_, _, state| {
            if !state.pending_startup.is_empty() {
                warn!("dbus-update-activation-environment hangs; starting the session anyway");
                state.run_pending_startup();
            }
            TimeoutAction::Drop
        });
        true
    }

    /// The environment of everything pila starts. Its output goes to pila's
    /// log (stderr) instead of the terminal pila was started from (on a TTY,
    /// the console), and it doesn't read that terminal. It starts with no
    /// signals blocked: pila blocks SIGINT, SIGTERM and SIGCHLD for its
    /// signal source (`main.rs`), and a blocked mask survives exec, so
    /// programs would otherwise ignore `kill` and Ctrl+C, and so would
    /// everything they start. It gets the limit of open files Pila was
    /// started with (`raise_file_limit`).
    pub(crate) fn session_env(&self, cmd: &mut Command) {
        let file_limit = STARTING_FILE_LIMIT.get().copied();
        // SAFETY: between fork and exec only async-signal-safe calls are
        // allowed; sigemptyset, sigprocmask and setrlimit are.
        unsafe {
            cmd.pre_exec(move || {
                let mut none = std::mem::zeroed::<libc::sigset_t>();
                libc::sigemptyset(&mut none);
                if libc::sigprocmask(libc::SIG_SETMASK, &none, std::ptr::null_mut()) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // Lowering a soft limit can't fail; if it did, the program
                // would still run.
                if let Some(limit) = &file_limit {
                    libc::setrlimit(libc::RLIMIT_NOFILE, limit);
                }
                Ok(())
            });
        }
        cmd.stdin(Stdio::null());
        if let Ok(stderr) = std::io::stderr().as_fd().try_clone_to_owned() {
            cmd.stdout(Stdio::from(stderr));
        }
        cmd.env("WAYLAND_DISPLAY", &self.socket_name)
            .env("XDG_CURRENT_DESKTOP", DESKTOP_NAME)
            .env("XDG_SESSION_TYPE", "wayland");
        for var in OTHER_COMPOSITORS_ENV {
            cmd.env_remove(var);
        }
        match &self.ipc_socket {
            Some(path) => cmd.env("PILA_SOCKET", path),
            None => cmd.env_remove("PILA_SOCKET"),
        };
        // X11 apps go to pila's Xwayland, never the host's X server.
        match self.xwayland.display {
            Some(display) => cmd.env("DISPLAY", format!(":{display}")),
            None => cmd.env_remove("DISPLAY"),
        };
    }

    pub fn spawn_all(&mut self, commands: &[String]) {
        for command in commands {
            self.spawn(command);
        }
    }

    /// Run a shell command as a client of this compositor.
    pub fn spawn(&mut self, command: &str) {
        info!(command, "spawning");
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg(command);
        self.session_env(&mut cmd);
        // The output under the pointer, for commands that act on "this
        // screen" (e.g. `grim -o "$PILA_OUTPUT"`).
        let pointer = self.seat.get_pointer().map(|p| p.current_location());
        match pointer.and_then(|pos| self.output_at(pos)) {
            Some(output) => cmd.env("PILA_OUTPUT", output.name()),
            None => cmd.env_remove("PILA_OUTPUT"),
        };
        match cmd.spawn() {
            Ok(child) => self.adopt_child(child),
            Err(err) => warn!(command, ?err, "failed to spawn"),
        }
    }

    /// Keep a program pila started until it exits, so it can be reaped
    /// (`reap_children`) instead of lingering as a zombie.
    pub(crate) fn adopt_child(&mut self, child: std::process::Child) {
        self.children.push(child);
        // A SIGCHLD may have come before this one was listed.
        self.reap_children();
    }

    /// Reap the programs pila started that exited (on SIGCHLD, `main.rs`).
    /// Only pila's own: Smithay waits for Xwayland itself.
    pub fn reap_children(&mut self) {
        self.children
            .retain_mut(|child| matches!(child.try_wait(), Ok(None)));
    }
}

/// What's under a point: a layer surface (with the surface hit and its origin
/// in global coordinates), or a window and the part of it that was hit.
pub enum Under {
    Layer(LayerSurface, WlSurface, Point<f64, Logical>),
    Window(Window, Hit),
}

impl Under {
    /// The layer it's on, for a layer surface.
    pub fn layer(&self) -> Option<Layer> {
        match self {
            Under::Layer(layer, ..) => Some(layer.layer()),
            Under::Window(..) => None,
        }
    }
}

/// What part of a window is under a point: a client surface (with its origin
/// in global coordinates), or a part that pila draws or handles itself.
pub enum Hit {
    Surface(WlSurface, Point<f64, Logical>),
    Decoration(Decoration),
}

/// Geometry to restore when a window leaves maximized/fullscreen.
#[derive(Default)]
struct RestoreGeometry(RefCell<Option<Rectangle<i32, Logical>>>);

/// Set where a window returns to when it leaves maximized/fullscreen.
pub fn set_restore_geometry(window: &Window, rect: Rectangle<i32, Logical>) {
    window
        .user_data()
        .insert_if_missing(RestoreGeometry::default);
    if let Some(restore) = window.user_data().get::<RestoreGeometry>() {
        *restore.0.borrow_mut() = Some(rect);
    }
}

pub fn restore_geometry(window: &Window) -> Option<Rectangle<i32, Logical>> {
    window
        .user_data()
        .get::<RestoreGeometry>()
        .and_then(|r| *r.0.borrow())
}

/// (maximized, fullscreen) as last requested by the compositor.
pub fn fill_state(window: &Window) -> (bool, bool) {
    if let Some(x11) = window.x11_surface() {
        return (x11.is_maximized(), x11.is_fullscreen());
    }
    window.toplevel().map_or((false, false), |toplevel| {
        toplevel.with_pending_state(|state| {
            (
                state.states.contains(xdg_toplevel::State::Maximized),
                state.states.contains(xdg_toplevel::State::Fullscreen),
            )
        })
    })
}

fn set_state(
    states: &mut smithay::wayland::shell::xdg::ToplevelStateSet,
    state: xdg_toplevel::State,
    on: bool,
) {
    if on {
        states.set(state);
    } else {
        states.unset(state);
    }
}

/// Ask a window to close (xdg `close`, or `WM_DELETE_WINDOW` for X11).
pub fn close_window(window: &Window) {
    if let Some(toplevel) = window.toplevel() {
        toplevel.send_close();
    } else if let Some(x11) = window.x11_surface() {
        let _ = x11.close();
    }
}

/// What to give keyboard focus to for `window`: its xdg toplevel surface, or
/// the X11 window (so Smithay also sets the X input focus).
fn focus_target(window: &Window) -> Option<KeyboardFocus> {
    if let Some(x11) = window.x11_surface() {
        return Some(x11.clone().into());
    }
    window
        .toplevel()
        .map(|toplevel| toplevel.wl_surface().clone().into())
}

/// Where a window of `size` goes to be centered in `area` together with its
/// title bar (`bar` pixels above it), staying inside `area` if it's larger.
pub fn centered_location(
    area: Rectangle<i32, Logical>,
    size: smithay::utils::Size<i32, Logical>,
    bar: i32,
) -> Point<i32, Logical> {
    area.loc
        + Point::from((
            (area.size.w - size.w).max(0) / 2,
            (area.size.h - size.h - bar).max(0) / 2 + bar,
        ))
}

/// Whether a surface has a buffer attached, i.e. is mapped.
pub fn has_buffer(surface: &WlSurface) -> bool {
    with_renderer_surface_state(surface, |state| state.buffer().is_some()).unwrap_or(false)
}

/// Whether a surface had a buffer at its last commit (`commit_map_change`).
struct WasMapped(std::sync::atomic::AtomicBool);

/// What a commit did to a layer surface or popup (`commit_map_change`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MapChange {
    /// Its first buffer, or the first after being hidden.
    Mapped,
    /// A null buffer after a buffer. Smithay resets the role in that very
    /// commit, initial configure included, but the configure that maps it
    /// again must answer the client's next commit, its new initial one.
    Unmapped,
    Unchanged,
}

/// Called once per commit of a layer surface or popup.
pub fn commit_map_change(surface: &WlSurface) -> MapChange {
    let mapped = has_buffer(surface);
    let was = smithay::wayland::compositor::with_states(surface, |states| {
        states
            .data_map
            .insert_if_missing_threadsafe(|| WasMapped(Default::default()));
        states
            .data_map
            .get::<WasMapped>()
            .unwrap()
            .0
            .swap(mapped, std::sync::atomic::Ordering::Relaxed)
    });
    match (was, mapped) {
        (false, true) => MapChange::Mapped,
        (true, false) => MapChange::Unmapped,
        _ => MapChange::Unchanged,
    }
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
    /// Set for a client that connected through a security context
    /// (`handlers/protocols.rs`): a sandboxed app (Flatpak).
    pub security_context: Option<SecurityContext>,
    /// The client's process (its socket's peer credentials), for the log.
    pub pid: Option<i32>,
}

impl ClientState {
    pub fn for_stream(stream: &std::os::unix::net::UnixStream) -> Self {
        Self {
            pid: peer_pid(stream),
            ..Default::default()
        }
    }
}

/// The process at the other end of a Unix socket (`SO_PEERCRED`).
fn peer_pid(stream: &std::os::unix::net::UnixStream) -> Option<i32> {
    use std::os::fd::AsRawFd;
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` and `len` describe a valid ucred buffer for the call.
    let ret = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut cred).cast(),
            &mut len,
        )
    };
    (ret == 0 && cred.pid > 0).then_some(cred.pid)
}

/// Take a global away without killing the client binding it right then, before
/// it heard: clients are told now, and it's removed some time later.
pub fn retire_global(
    display: &smithay::reexports::wayland_server::DisplayHandle,
    loop_handle: &LoopHandle<'static, Pila>,
    global: smithay::reexports::wayland_server::backend::GlobalId,
) {
    display.disable_global::<Pila>(global.clone());
    let timer = Timer::from_duration(GLOBAL_RETIRE_DELAY);
    let inserted = loop_handle.insert_source(timer, move |_, _, state| {
        state.display_handle.remove_global::<Pila>(global.clone());
        TimeoutAction::Drop
    });
    if let Err(err) = inserted {
        warn!(%err, "can't schedule removing a global");
    }
}

/// `GlobalDispatch::can_view` of a global not for sandboxed apps
/// (`is_privileged`).
macro_rules! privileged_global {
    () => {
        fn can_view(client: smithay::reexports::wayland_server::Client, _: &()) -> bool {
            $crate::state::is_privileged(&client)
        }
    };
}
pub(crate) use privileged_global;

/// Global data of a Smithay global that Smithay can't filter, re-created by
/// pila (`hide_from_sandboxes`) to be hidden like the privileged ones: binding
/// is Smithay's (`GlobalData`), only `can_view` is pila's.
pub struct Privileged;

impl<I: Resource> smithay::wayland::GlobalDispatch2<I, Pila> for Privileged
where
    smithay::wayland::GlobalData: smithay::wayland::GlobalDispatch2<I, Pila>,
{
    fn bind(
        &self,
        state: &mut Pila,
        handle: &DisplayHandle,
        client: &Client,
        resource: smithay::reexports::wayland_server::New<I>,
        data_init: &mut smithay::reexports::wayland_server::DataInit<'_, Pila>,
    ) {
        smithay::wayland::GlobalData.bind(state, handle, client, resource, data_init);
    }

    fn can_view(&self, client: &Client) -> bool {
        is_privileged(client)
    }
}

/// Replace the global `global` (of interface `I`, at `version`), which Smithay
/// made visible to every client, by one only privileged clients see.
pub fn hide_from_sandboxes<I: Resource + 'static>(
    dh: &DisplayHandle,
    global: smithay::reexports::wayland_server::backend::GlobalId,
    version: u32,
) where
    Pila: smithay::reexports::wayland_server::GlobalDispatch<I, Privileged>,
{
    dh.remove_global::<Pila>(global);
    dh.create_global::<Pila, I, _>(version, Privileged);
}

/// Whether `client` may use the privileged protocols (capturing the
/// screen, reading the clipboard, typing for others, locking, panels and
/// the like): any client but a sandboxed one, i.e. one that connected
/// through a security context. Xwayland is trusted (X11 has no such
/// boundary anyway).
pub fn is_privileged(client: &Client) -> bool {
    if client.get_data::<XWaylandClientData>().is_some() {
        return true;
    }
    client
        .get_data::<ClientState>()
        .is_some_and(|state| state.security_context.is_none())
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, reason: DisconnectReason) {
        let pid = self.pid.unwrap_or(0);
        match reason {
            DisconnectReason::ProtocolError(err) => {
                warn!(pid, %err, "client disconnected by a protocol error");
            }
            DisconnectReason::ConnectionClosed => debug!(pid, "client disconnected"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centered_location_centers_window_and_title_bar() {
        let area = Rectangle::new((100, 50).into(), (1000, 800).into());
        // 600x400 window with a 30px bar: 430 tall in total.
        let loc = centered_location(area, (600, 400).into(), 30);
        assert_eq!(loc, Point::from((100 + 200, 50 + 185 + 30)));
    }

    #[test]
    fn the_user_bus_is_recognized_with_or_without_a_guid() {
        let runtime = "/run/user/1000";
        assert!(is_user_bus("unix:path=/run/user/1000/bus", runtime));
        assert!(is_user_bus(
            "unix:path=/run/user/1000/bus,guid=abc",
            runtime
        ));
        // dbus-run-session's private bus, another user's, or no runtime dir.
        assert!(!is_user_bus("unix:path=/tmp/dbus-XYZ,guid=abc", runtime));
        assert!(!is_user_bus("unix:path=/run/user/1001/bus", runtime));
        assert!(!is_user_bus("unix:abstract=/tmp/dbus-XYZ", runtime));
        assert!(!is_user_bus("unix:path=/bus", ""));
    }

    #[test]
    fn centered_location_keeps_oversized_windows_in_the_area() {
        let area = Rectangle::new((1920, 0).into(), (1280, 720).into());
        let loc = centered_location(area, (2000, 1000).into(), 30);
        // Top-left corner (below the bar) instead of off the area.
        assert_eq!(loc, Point::from((1920, 30)));
    }
}
