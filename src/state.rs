use std::{cell::RefCell, collections::HashSet, ffi::OsString, process::Command, sync::Arc};

use smithay::{
    backend::renderer::utils::with_renderer_surface_state,
    desktop::{
        find_popup_root_surface, layer_map_for_output, LayerSurface, PopupManager, Space, Window,
        WindowSurfaceType,
    },
    input::{
        keyboard::Keycode,
        pointer::{CursorIcon, CursorImageStatus, Focus, GrabStartData as PointerGrabStartData},
        Seat, SeatHandler, SeatState,
    },
    output::Output,
    reexports::{
        calloop::{
            generic::Generic, EventLoop, Interest, LoopHandle, LoopSignal, Mode, PostAction,
        },
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
            Display, DisplayHandle, Resource,
        },
    },
    utils::{Clock, Logical, Monotonic, Point, Rectangle, Serial},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        cursor_shape::CursorShapeManagerState,
        dmabuf::DmabufState,
        output::OutputManagerState,
        presentation::PresentationState,
        selection::{data_device::DataDeviceState, primary_selection::PrimarySelectionState},
        session_lock::SessionLockManagerState,
        shell::{
            wlr_layer::{Layer, WlrLayerShellState},
            xdg::{decoration::XdgDecorationState, XdgShellState},
        },
        shm::ShmState,
        socket::ListeningSocketSource,
        xdg_activation::XdgActivationState,
    },
};
use tracing::{debug, info, warn};

use crate::{
    backend::Backend,
    config::{Action, Config},
    decoration::{self, Decoration},
    grabs::{resize_grab::ResizeEdge, MoveSurfaceGrab, ResizeSurfaceGrab},
    handlers::protocols::{self, Globals},
    idle::IdleInhibitors,
    render::{Scene, SceneLayer, SceneTitleBar, SceneWindow},
    session_lock::SessionLock,
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

impl Pila {
    pub fn new(
        event_loop: &mut EventLoop<'static, Pila>,
        display: Display<Self>,
        config: Config,
    ) -> Self {
        let start_time = std::time::Instant::now();
        let clock = Clock::new();
        let dh = display.handle();

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let layer_shell_state = WlrLayerShellState::new::<Self>(&dh);
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
        let session_lock_state = SessionLockManagerState::new::<Self, _>(&dh, |_| true);
        PresentationState::new::<Self>(&dh, clock.id() as u32);

        let mut seat_state = SeatState::new();
        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "seat0");
        // Nested: the host always gives us a keyboard and pointer.
        // A real-hardware backend should track hotplug instead.
        seat.add_keyboard(Default::default(), 200, 25)
            .expect("failed to initialize keyboard");
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
                .map(|(layer, location)| SceneLayer { layer, location })
                .collect()
        };
        Scene {
            upper: layers(upper),
            windows: self.scene_windows(),
            lower: layers(lower),
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
                let title_bar = decoration::title_bar_rect(window, loc).map(|rect| SceneTitleBar {
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
                })
            })
            .collect()
    }

    /// Find a window (mapped or not) by its toplevel surface.
    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<Window> {
        self.space
            .elements()
            .chain(self.unmapped.iter())
            .find(|w| w.toplevel().is_some_and(|t| t.wl_surface() == surface))
            .cloned()
    }

    /// The window with keyboard focus. While a menu is open the focus is on
    /// the popup, so this resolves it to the popup's parent window.
    pub fn focused_window(&self) -> Option<Window> {
        let focus = self.seat.get_keyboard()?.current_focus()?;
        let focus = self.popup_root(&focus).unwrap_or(focus);
        self.space
            .elements()
            .find(|w| w.toplevel().is_some_and(|t| *t.wl_surface() == focus))
            .cloned()
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
        let output = self.output_at(pos)?;
        let output_geo = self.space.output_geometry(&output)?;
        let zone = layer_map_for_output(&output).non_exclusive_zone();
        Some(Rectangle::new(output_geo.loc + zone.loc, zone.size))
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
            // Center the window and its title bar together.
            let size = window.geometry().size;
            let mut loc = output_geo.loc
                + Point::from((
                    (output_geo.size.w - size.w).max(0) / 2,
                    (output_geo.size.h - size.h - bar).max(0) / 2 + bar,
                ));
            while self
                .space
                .elements()
                .any(|w| self.space.element_location(w) == Some(loc))
            {
                loc += Point::from((CASCADE_STEP, CASCADE_STEP));
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
        let exclusive = self.exclusive_layer().map(|l| l.wl_surface().clone());
        match window {
            Some(window) => {
                self.space.raise_element(&window, true);
                let surface = window.toplevel().map(|t| t.wl_surface().clone());
                keyboard.set_focus(self, exclusive.or(surface), serial);
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
        if let Some(focus) = keyboard.current_focus().filter(|f| f.is_alive()) {
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

    /// Raise the bottom-most window: repeated presses walk through all windows.
    pub fn cycle_focus(&mut self, serial: Serial) {
        if self.space.elements().len() < 2 {
            return;
        }
        let bottom = self.space.elements().next().cloned();
        self.focus_window(bottom, serial);
    }

    pub fn close_focused(&mut self) {
        if let Some(toplevel) = self.focused_window().as_ref().and_then(|w| w.toplevel()) {
            toplevel.send_close();
        }
    }

    /// Set a window's maximized/fullscreen states. Entering either state from
    /// a normal window remembers its geometry; leaving both restores it.
    pub fn set_fill_state(&mut self, window: &Window, maximized: bool, fullscreen: bool) {
        let Some(toplevel) = window.toplevel() else {
            return;
        };
        let mapped_loc = self.space.element_location(window);
        let (was_maximized, was_fullscreen) = fill_state(window);
        window
            .user_data()
            .insert_if_missing(RestoreGeometry::default);
        let restore = &window.user_data().get::<RestoreGeometry>().unwrap().0;

        if maximized || fullscreen {
            let usable = !fullscreen;
            let Some(mut output_geo) = self.output_geometry_for_window(window, usable) else {
                return;
            };
            // A maximized window's title bar stays visible above it.
            let bar = decoration::layout_offset(window, fullscreen);
            output_geo.loc.y += bar;
            output_geo.size.h -= bar;
            if let (Some(loc), false) = (mapped_loc, was_maximized || was_fullscreen) {
                *restore.borrow_mut() = Some(Rectangle::new(loc, window.geometry().size));
            }
            toplevel.with_pending_state(|state| {
                set_state(&mut state.states, xdg_toplevel::State::Maximized, maximized);
                set_state(
                    &mut state.states,
                    xdg_toplevel::State::Fullscreen,
                    fullscreen,
                );
                state.size = Some(output_geo.size);
            });
            if mapped_loc.is_some() {
                self.space.map_element(window.clone(), output_geo.loc, true);
            }
        } else {
            let saved = restore.borrow_mut().take();
            toplevel.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Maximized);
                state.states.unset(xdg_toplevel::State::Fullscreen);
                // None lets the client pick its own size.
                state.size = saved.map(|r| r.size);
            });
            if let (Some(saved), Some(_)) = (saved, mapped_loc) {
                self.space.map_element(window.clone(), saved.loc, true);
            }
        }

        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
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
        }

        let Some(initial_window_location) = self.space.element_location(&window) else {
            return;
        };
        let grab = MoveSurfaceGrab {
            start_data,
            window,
            initial_window_location,
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
        let Some(location) = self.space.element_location(&window) else {
            return;
        };
        let Some(toplevel) = window.toplevel() else {
            return;
        };

        let size = window.geometry().size;
        toplevel.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Resizing);
        });
        toplevel.send_pending_configure();

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

    /// Housekeeping after `output` was drawn, for any backend: drop dead
    /// windows and popups, re-check idle inhibitors, and flush clients.
    pub fn post_render(&mut self, output: &Output) {
        self.space.refresh();
        self.update_idle_inhibit();
        layer_map_for_output(output).cleanup();
        self.popups.cleanup();
        let _ = self.display_handle.flush_clients();
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
            Action::CloseFocused => self.close_focused(),
            Action::CycleFocus => self.cycle_focus(serial),
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

    /// Run a shell command as a client of this compositor.
    pub fn spawn(&self, command: &str) {
        info!(command, "spawning");
        match Command::new("/bin/sh")
            .arg("-c")
            .arg(command)
            .env("WAYLAND_DISPLAY", &self.socket_name)
            .spawn()
        {
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

fn restore_geometry(window: &Window) -> Option<Rectangle<i32, Logical>> {
    window
        .user_data()
        .get::<RestoreGeometry>()
        .and_then(|r| *r.0.borrow())
}

/// (maximized, fullscreen) as last requested by the compositor.
pub fn fill_state(window: &Window) -> (bool, bool) {
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

/// Whether a surface has a buffer attached, i.e. is mapped.
pub fn has_buffer(surface: &WlSurface) -> bool {
    with_renderer_surface_state(surface, |state| state.buffer().is_some()).unwrap_or(false)
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}
