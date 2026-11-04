use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    ffi::OsString,
    os::fd::AsFd,
    process::{Command, Stdio},
    sync::Arc,
};

use smithay::{
    backend::renderer::{
        damage::OutputDamageTracker, element::solid::SolidColorBuffer,
        utils::with_renderer_surface_state,
    },
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
            EventLoop, Interest, LoopHandle, LoopSignal, Mode, PostAction, RegistrationToken,
        },
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
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
    xwayland::{X11Wm, XWaylandClientData},
};
use tracing::{debug, info, warn};

use crate::{
    backend::Backend,
    config::{Action, Config},
    decoration::{self, Decoration},
    focus::KeyboardFocus,
    grabs::{resize_grab::ResizeEdge, MoveSurfaceGrab, ResizeSurfaceGrab},
    handlers::protocols::{self, Globals},
    idle::IdleInhibitors,
    render::{FrameInput, Scene, SceneLayer, SceneTitleBar, SceneWindow},
    session_lock::SessionLock,
    switcher::Switch,
};

/// Offset applied to each new window that would land exactly on top of another.
const CASCADE_STEP: i32 = 32;

pub struct Pila {
    pub start_time: std::time::Instant,
    /// The clock presentation-time timestamps use (CLOCK_MONOTONIC).
    pub clock: Clock<Monotonic>,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,
    pub loop_signal: LoopSignal,
    pub loop_handle: LoopHandle<'static, Pila>,
    pub config: Config,

    /// Xwayland's window manager and display number, once it's running
    /// (`xwayland.rs`).
    pub xwm: Option<X11Wm>,
    pub xdisplay: Option<u32>,
    pub xwayland_shell_state: XWaylandShellState,
    /// The startup commands, held back until Xwayland is ready.
    pub pending_startup: Vec<String>,
    /// Xwayland's event source, removed on exit (`stop_xwayland`).
    pub xwayland_token: Option<RegistrationToken>,

    /// Where a dragged window would snap to, shown above the windows
    /// (`snap.rs`), and the buffer it's drawn with.
    pub snap_preview: Option<Rectangle<i32, Logical>>,
    pub snap_preview_buffer: SolidColorBuffer,
    /// Minimized windows, out of the `Space` (`minimize.rs`).
    pub minimized: Vec<Window>,
    /// Windows most recently focused first, and the Mod+Tab in progress
    /// (`switcher.rs`).
    pub focus_history: Vec<Window>,
    /// The config file, watched for changes (`config_reload.rs`).
    pub config_path: Option<std::path::PathBuf>,
    /// A reload is scheduled (changes are coalesced).
    pub config_reload_pending: bool,
    /// An output was drawn this event-loop iteration: `after_frames` has
    /// housekeeping to do.
    pub frame_drawn: bool,
    /// Screen capture sessions (`capture.rs`).
    pub captures: Vec<crate::capture::Session>,
    /// Where captures may use dma-bufs (set by the backend).
    pub capture_dmabuf: Option<crate::capture::DmabufTarget>,
    /// wlr-screencopy frames waiting to be copied, and the damage trackers
    /// of `copy_with_damage` (`screencopy.rs`).
    pub screencopies: Vec<ZwlrScreencopyFrameV1>,
    pub screencopy_trackers: HashMap<crate::screencopy::TrackerKey, OutputDamageTracker>,
    /// Taskbar listeners of wlr-foreign-toplevel-management (`toplevel_management.rs`).
    pub toplevel_management: crate::toplevel_management::ToplevelManagement,
    /// Workspaces (`workspaces.rs`) and their ext-workspace listeners
    /// (`ext_workspace.rs`).
    pub workspaces: crate::workspaces::Workspaces,
    pub ext_workspaces: crate::ext_workspace::ExtWorkspaces,
    /// The client controlling each output's gamma (`gamma_control.rs`).
    pub gamma_controls: crate::gamma_control::GammaControls,
    /// Clients watching or setting outputs' power (`output_power.rs`).
    pub output_powers: crate::output_power::OutputPowers,
    /// Blurred backdrops of layer surfaces (`blur.rs`).
    pub blur: crate::blur::BlurCache,
    /// Display tools' view of the monitors (`output_management.rs`), and
    /// what they set, over `[[output]]`, by output name.
    pub output_management: crate::output_management::OutputManagement,
    pub output_overrides: HashMap<String, crate::config::OutputConfig>,
    pub switch: Option<Switch>,
    /// A touchpad swipe pila is handling itself (`gestures.rs`).
    pub swipe: Option<crate::gestures::Swipe>,

    /// Mapped windows, in stacking order (back to front).
    pub space: Space<Window>,
    /// Toplevels that exist but haven't committed a buffer yet.
    pub unmapped: Vec<Window>,
    /// Keys whose press triggered a binding; their release is swallowed too.
    pub suppressed_keys: HashSet<Keycode>,

    // Smithay protocol state
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub layer_shell_state: WlrLayerShellState,
    pub shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Pila>,
    pub data_device_state: DataDeviceState,
    pub primary_selection_state: PrimarySelectionState,
    pub xdg_decoration_state: XdgDecorationState,
    pub xdg_activation_state: XdgActivationState,
    pub cursor_shape_state: CursorShapeManagerState,
    /// Its global is created by the backend, which knows the render device and formats.
    pub dmabuf_state: DmabufState,
    pub popups: PopupManager,
    pub session_lock_state: SessionLockManagerState,
    /// Whether the screen is locked, and by whom (`session_lock.rs`).
    pub session_lock: SessionLock,
    /// State of the simpler protocols (`handlers/protocols.rs`).
    pub globals: Globals,
    pub idle_inhibitors: IdleInhibitors,
    /// The surface whose pointer constraint pila activated, if any.
    pub constrained: Option<WlSurface>,
    /// Nested-only: the last absolute pointer position from the host, to
    /// derive relative motion (it differs from pila's pointer while locked).
    pub host_pointer: Option<Point<f64, Logical>>,

    pub seat: Seat<Self>,
    /// Cursor most recently requested by the client under the pointer.
    pub cursor_status: CursorImageStatus,

    /// Nested-only: set by `init_winit`. It lives here (not in the redraw closure) so
    /// `DmabufHandler` can test-import client buffers into the renderer. A DRM backend
    /// needs its own renderer handle here instead.
    pub backend: Option<Backend>,
}

/// `XDG_CURRENT_DESKTOP` for everything pila starts; xdg-desktop-portal
/// picks its backends from `pila-portals.conf` by it.
const DESKTOP_NAME: &str = "pila";
/// What `export_environment` hands the session bus.
const SESSION_ENV: [&str; 4] = [
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_TYPE",
];

/// How long the startup programs wait for the session bus to take pila's
/// environment (`export_environment`).
const EXPORT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

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
        let start_time = std::time::Instant::now();
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
        let globals = protocols::init_globals(&dh, &loop_handle, &primary_selection_state);
        crate::capture::init_globals(&dh);
        crate::screencopy::init_global(&dh);
        crate::toplevel_management::init_global(&dh);
        crate::ext_workspace::init_global(&dh);
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
        if let Err(err) = seat.add_keyboard(xkb_config(kb), kb.repeat_delay, kb.repeat_rate) {
            warn!(
                ?err,
                ?kb,
                "invalid keyboard config; using the default keymap"
            );
            seat.add_keyboard(XkbConfig::default(), kb.repeat_delay, kb.repeat_rate)
                .expect("failed to initialize keyboard");
        }
        seat.add_pointer();

        let socket_name = Self::init_wayland_listener(display, event_loop);
        let loop_signal = event_loop.get_signal();

        Self {
            start_time,
            clock,
            socket_name,
            display_handle: dh,
            loop_signal,
            loop_handle,
            config,
            xwm: None,
            xdisplay: None,
            xwayland_shell_state,
            pending_startup: Vec::new(),
            minimized: Vec::new(),
            snap_preview: None,
            snap_preview_buffer: SolidColorBuffer::default(),
            focus_history: Vec::new(),
            captures: Vec::new(),
            config_path: None,
            config_reload_pending: false,
            frame_drawn: false,
            capture_dmabuf: None,
            screencopies: Vec::new(),
            screencopy_trackers: HashMap::new(),
            toplevel_management: Default::default(),
            workspaces: Default::default(),
            ext_workspaces: Default::default(),
            gamma_controls: Default::default(),
            output_powers: Default::default(),
            output_management: Default::default(),
            blur: Default::default(),
            output_overrides: HashMap::new(),
            switch: None,
            swipe: None,
            xwayland_token: None,
            space: Space::default(),
            unmapped: Vec::new(),
            suppressed_keys: HashSet::new(),
            compositor_state,
            xdg_shell_state,
            layer_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            primary_selection_state,
            xdg_decoration_state,
            xdg_activation_state,
            cursor_shape_state,
            dmabuf_state,
            popups,
            session_lock_state,
            session_lock: SessionLock::default(),
            globals,
            idle_inhibitors: IdleInhibitors::default(),
            constrained: None,
            host_pointer: None,
            seat,
            cursor_status: CursorImageStatus::default_named(),
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
                if let Err(err) = state
                    .display_handle
                    .insert_client(client_stream, Arc::new(ClientState::default()))
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
                    unsafe {
                        display.get_mut().dispatch_clients(state).unwrap();
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
        let (upper, lower) = self.layer_order();
        let layer = |layers| {
            self.layer_surface_under(layers, pos)
                .map(|(layer, surface, origin)| Under::Layer(layer, surface, origin))
        };
        layer(upper)
            .or_else(|| {
                self.window_under(pos)
                    .map(|(window, hit)| Under::Window(window, hit))
            })
            .or_else(|| layer(lower))
    }

    /// The topmost window under `pos` and what part of it was hit. Per window,
    /// front to back: title bar, client surfaces (incl. popups), resize border.
    /// Ignores layer surfaces; use `under` for anything input-related.
    fn window_under(&self, pos: Point<f64, Logical>) -> Option<(Window, Hit)> {
        self.space.elements().rev().find_map(|window| {
            let loc = self.space.element_location(window)?;
            if let Some(bar) = decoration::title_bar_rect(window, loc) {
                if bar.to_f64().contains(pos) {
                    let part = decoration::title_bar_part(bar, pos);
                    return Some((window.clone(), Hit::Decoration(Decoration::TitleBar(part))));
                }
            }
            let render_loc = loc - window.geometry().loc;
            if let Some((surface, origin)) =
                window.surface_under(pos - render_loc.to_f64(), WindowSurfaceType::ALL)
            {
                let origin = (origin + render_loc).to_f64();
                return Some((window.clone(), Hit::Surface(surface, origin)));
            }
            decoration::border_edges(window, loc, pos)
                .map(|edges| (window.clone(), Hit::Decoration(Decoration::Border(edges))))
        })
    }

    /// The scene and the cursor, for a frame or a capture (`render::FrameInput`).
    pub fn frame_input(&self) -> FrameInput {
        let pointer = self
            .seat
            .get_pointer()
            .map(|p| p.current_location())
            .unwrap_or_default();
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
        let (upper, lower) = self.layer_order();
        let layers = |layers| {
            self.layer_surfaces(layers)
                .into_iter()
                .map(|(layer, location)| SceneLayer {
                    blur: crate::blur::blur_rects(&layer),
                    layer,
                    location,
                })
                .collect()
        };
        Scene {
            upper: layers(upper),
            snap_preview: self
                .snap_preview
                .map(|rect| (self.snap_preview_buffer.clone(), rect)),
            windows: self.scene_windows(),
            lower: layers(lower),
            corner_radius: self.config.corner_radius,
            ..Default::default()
        }
    }

    fn scene_windows(&self) -> Vec<SceneWindow> {
        let focused = self.focused_window();
        let pointer = self
            .seat
            .get_pointer()
            .map(|p| p.current_location())
            .unwrap_or_default();
        let hovered = match self.under(pointer) {
            Some(Under::Window(window, Hit::Decoration(Decoration::TitleBar(part)))) => {
                Some((window, part))
            }
            _ => None,
        };
        self.space
            .elements()
            .rev()
            .filter_map(|window| {
                let loc = self.space.element_location(window)?;
                let bar_rect = decoration::title_bar_rect(window, loc);
                let (maximized, fullscreen) = fill_state(window);
                let rounding = crate::rounded::Rounding::new(
                    Rectangle::new(loc, window.geometry().size),
                    bar_rect,
                    self.config.corner_radius,
                    maximized || fullscreen,
                );
                let title_bar = bar_rect.map(|rect| SceneTitleBar {
                    rect,
                    focused: focused.as_ref() == Some(window),
                    hover: hovered
                        .as_ref()
                        .filter(|(w, _)| w == window)
                        .map(|(_, part)| *part),
                });
                Some(SceneWindow {
                    window: window.clone(),
                    render_location: loc - window.geometry().loc,
                    title_bar,
                    flash: crate::bell::flash(window, loc),
                    rounding,
                })
            })
            .collect()
    }

    /// Find a window (mapped or not) by its toplevel surface.
    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<Window> {
        self.space
            .elements()
            .chain(self.unmapped.iter())
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
    fn output_area(&self, output: &Output, usable: bool) -> Option<Rectangle<i32, Logical>> {
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
        let pointer = self
            .seat
            .get_pointer()
            .map(|p| p.current_location())
            .unwrap_or_default();
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

    /// Geometry of the output a window is mostly on, falling back to the
    /// pointer's output: all of it, or only the `usable` part.
    fn output_geometry_for_window(
        &self,
        window: &Window,
        usable: bool,
    ) -> Option<Rectangle<i32, Logical>> {
        let pos = match self.space.element_geometry(window) {
            Some(geo) => (geo.loc + geo.size.downscale(2).to_point()).to_f64(),
            None => self.seat.get_pointer()?.current_location(),
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
    /// output (cascaded if that spot is taken), or filling the output if the
    /// client asked for maximize/fullscreen before mapping.
    pub fn place_new_window(&mut self, window: Window) {
        let pointer_location = self
            .seat
            .get_pointer()
            .map(|p| p.current_location())
            .unwrap_or_default();
        let (maximized, fullscreen) = fill_state(&window);
        let output_geo = if fullscreen {
            self.output_geometry_at(pointer_location)
        } else {
            self.usable_geometry_at(pointer_location)
        }
        .unwrap_or_else(|| Rectangle::from_size((800, 600).into()));

        // Room for the title bar above the window, if pila draws one.
        let bar = decoration::layout_offset(&window, fullscreen);
        let location = if maximized || fullscreen {
            output_geo.loc + Point::from((0, bar))
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
            let mut loc = centered_location(output_geo, size, bar);
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

        debug!(?location, size = ?window.geometry().size, maximized, fullscreen, "mapping window");
        self.space.map_element(window.clone(), location, true);
        self.announce_toplevel(&window);
        self.focus_window(Some(window), smithay::utils::SERIAL_COUNTER.next_serial());
    }

    /// Give keyboard focus to `window` and raise it, or clear focus. While a
    /// layer surface holds exclusive focus (`exclusive_layer`), the window is
    /// only raised and the keyboard stays with the layer surface.
    pub fn focus_window(&mut self, window: Option<Window>, serial: Serial) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
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
                self.touch_focus_history(&window);
                self.space.raise_element(&window, true);
                self.x11_window_raised(&window);
                let target = focus_target(&window);
                keyboard.set_focus(self, exclusive.or(target), serial);
            }
            None => {
                for w in self.space.elements() {
                    w.set_activated(false);
                }
                keyboard.set_focus(self, exclusive, serial);
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

    /// Focus the topmost window, e.g. after the focused surface went away.
    pub fn focus_top_window(&mut self, serial: Serial) {
        let top = self.space.elements().next_back().cloned();
        self.focus_window(top, serial);
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
        let mapped_loc = self.space.element_location(window);
        let (was_maximized, was_fullscreen) = fill_state(window);
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
            if mapped_loc.is_some() {
                self.space.map_element(window.clone(), output_geo.loc, true);
            }
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
            if let (Some(saved), Some(_)) = (saved, mapped_loc) {
                self.space.map_element(window.clone(), saved.loc, true);
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
            self.set_fill_state(&window, false, false);
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
    }

    /// Housekeeping after `output` was drawn, for any backend: what
    /// concerns that output now, the rest once per event-loop iteration
    /// however many outputs were drawn (`after_frames`).
    pub fn post_render(&mut self, output: &Output) {
        self.update_surface_scales(output);
        layer_map_for_output(output).cleanup();
        self.frame_drawn = true;
    }

    /// Called after every event-loop iteration (`main.rs`), once all the
    /// outputs due were drawn: captures and screencopies, dead windows and
    /// popups, taskbar listeners and idle inhibitors. Nothing if no output
    /// was drawn.
    pub fn after_frames(&mut self) {
        if !std::mem::take(&mut self.frame_drawn) {
            return;
        }
        self.process_captures();
        self.process_screencopies();
        // The space knows which outputs each window is on only after its
        // refresh, so that comes before the taskbar listeners are updated.
        self.space.refresh();
        self.refresh_toplevels();
        self.update_idle_inhibit();
        self.popups.cleanup();
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
                    self.resize_window_by(&window, (w, h).into());
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
        if !matches!(self.backend, Some(Backend::Udev(_))) {
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
    /// the console), and it doesn't read that terminal.
    pub(crate) fn session_env(&self, cmd: &mut Command) {
        cmd.stdin(Stdio::null());
        if let Ok(stderr) = std::io::stderr().as_fd().try_clone_to_owned() {
            cmd.stdout(Stdio::from(stderr));
        }
        cmd.env("WAYLAND_DISPLAY", &self.socket_name)
            .env("XDG_CURRENT_DESKTOP", DESKTOP_NAME)
            .env("XDG_SESSION_TYPE", "wayland");
        // X11 apps go to pila's Xwayland, never the host's X server.
        match self.xdisplay {
            Some(display) => cmd.env("DISPLAY", format!(":{display}")),
            None => cmd.env_remove("DISPLAY"),
        };
    }

    pub fn spawn_all(&self, commands: &[String]) {
        for command in commands {
            self.spawn(command);
        }
    }

    /// Run a shell command as a client of this compositor.
    pub fn spawn(&self, command: &str) {
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
            // Reap the child so exited clients don't linger as zombies.
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(err) => warn!(command, ?err, "failed to spawn"),
        }
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

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
    /// Set for a client that connected through a security context
    /// (`handlers/protocols.rs`): a sandboxed app (Flatpak).
    pub security_context: Option<SecurityContext>,
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
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
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
