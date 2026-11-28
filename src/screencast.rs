//! Screen sharing for the desktop portal Pila serves (`portal.rs`): each
//! started session is a PipeWire video source node of Pila's, fed with the
//! frames it draws for the shared output.
//!
//! PipeWire runs on the event loop: its loop's fd is a calloop source,
//! iterated without waiting (`Pila::dispatch_pipewire`). Its callbacks can't
//! reach `Pila`, so they queue `CastEvent`s, handled right after. A frame is
//! drawn after the output's own (`after_frames`), when a cast's damage
//! tracker sees a change and the consumer has a buffer free: offscreen, read
//! back into the buffer (shm, `MemFd`) once the GPU is done, off the event
//! loop's back (`Captures::read_back`, `Pila::when_copied`), then queued.
//!
//! What's shared is picked by the user first (`share_picker.rs`): a request
//! waits in `ScreenCasts::waiting` until then. It's an output or a window
//! (`capture::Target`, as screen captures have it): a window's stream is its
//! surfaces alone, at its size, kept while it's covered, minimized or on
//! another workspace.

use std::{
    cell::{Cell, RefCell},
    os::fd::OwnedFd,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use pipewire::{
    self as pw,
    properties::properties,
    spa::{
        self,
        param::{
            format::{FormatProperties, MediaSubtype, MediaType},
            video::{VideoFormat, VideoInfoRaw},
            ParamType,
        },
        pod::{serialize::PodSerializer, ChoiceValue, Object, Pod, Property, Value},
        utils::{Choice, ChoiceEnum, ChoiceFlags, Fraction, Rectangle as SpaRectangle, SpaTypes},
    },
    stream::{StreamFlags, StreamListener, StreamRc, StreamState},
};
use smithay::{
    backend::{allocator::Fourcc, renderer::damage::OutputDamageTracker},
    reexports::calloop::{
        channel::Event, generic::Generic, Interest, Mode, PostAction, RegistrationToken,
    },
    utils::{Physical, Rectangle, Scale, Size, Transform},
};
use tracing::{debug, info, warn};

use crate::{
    capture::{CopyError, ReadBack, Source, Target, Write},
    cursor::CursorPixels,
    portal::{self, CursorMode, Refused, Reply, Request},
    render::{self, PilaRenderElement},
    screencast_buffers::{Allocator, Buffers, Kind, Memory},
    Pila,
};

/// Buffers a stream asks PipeWire for: one being drawn, one shown, one
/// queued, and a spare.
const BUFFERS: i32 = 4;

/// Screen sharing state (`Pila::screencast`).
#[derive(Default)]
pub struct ScreenCasts {
    /// Dropped before `pipewire`: streams before their core.
    casts: Vec<Cast>,
    pipewire: Option<PipeWire>,
    /// Pila's connection to the session bus, serving the portal.
    pub(crate) portal: Option<zbus::Connection>,
    /// Requests to share waiting for the picker, first come first asked:
    /// it shows one at a time (`share_picker.rs`).
    pub(crate) waiting: std::collections::VecDeque<crate::share_picker::Ask>,
    /// The shares remembered for apps (`screen_grants.rs`).
    pub(crate) grants: crate::screen_grants::Grants,
    next_id: u64,
}

impl ScreenCasts {
    /// Whether a cast is streaming: frames go out.
    pub fn streaming(&self) -> bool {
        self.casts.iter().any(|cast| cast.streaming)
    }

    /// What the casts of regions share (`share_region.rs` outlines them).
    pub(crate) fn region_targets(&self) -> impl Iterator<Item = &Target> {
        self.casts
            .iter()
            .map(|cast| &cast.target)
            .filter(|target| matches!(target, Target::Region(..)))
    }

    /// Who shares what: each cast's app and target (`capture_indicator.rs`).
    pub(crate) fn shares(&self) -> impl Iterator<Item = (&str, &Target)> {
        self.casts
            .iter()
            .map(|cast| (cast.app.as_str(), &cast.target))
    }

    /// How many casts there are, for tests (`Pila::state_json`).
    pub fn cast_count(&self) -> usize {
        self.casts.len()
    }

    /// Whether Pila is connected to PipeWire.
    pub fn connected(&self) -> bool {
        self.pipewire.is_some()
    }

    /// Whether there's a cast, streaming or not yet: captures' offscreen
    /// textures are kept (one is made with the cast, `start_cast`).
    pub fn active(&self) -> bool {
        !self.casts.is_empty()
    }
}

/// Load what a first connection to PipeWire needs (its libraries, plugins
/// and configuration) by making one and dropping it, on the calling thread:
/// the portal's, at startup. The event loop's own first connection then
/// finds them loaded or cached instead of reading them.
pub fn warm_pipewire() {
    let started = Instant::now();
    pw::init();
    let warmed = (|| -> Result<(), pw::Error> {
        let main_loop = pw::main_loop::MainLoopRc::new(None)?;
        let context = pw::context::ContextRc::new(&main_loop, None)?;
        let _core = context.connect_rc(None)?;
        Ok(())
    })();
    debug!(
        elapsed = ?started.elapsed(),
        connected = warmed.is_ok(),
        "portal: PipeWire warmed up"
    );
}

/// A request to share, from the portal's `Start`.
pub struct Share {
    /// The portal session, by its object path.
    pub session: String,
    /// How the pointer shows in the stream.
    pub cursor: CursorMode,
    /// Who asks (`portal::Request::Start`), and the window it asks from.
    pub app_id: String,
    pub app: Option<String>,
    pub parent: Option<crate::portal::ParentWindow>,
    /// What it may share: the picker offers these.
    pub sources: portal::Sources,
    /// How long it asks for the share to be remembered, and the grant it
    /// asks to restore (`screen_grants.rs`).
    pub persist: u32,
    pub restore: Option<String>,
    pub reply: Reply<Result<portal::Started, Refused>>,
}

/// Pila's connection to PipeWire, made at the first cast. Fields drop in
/// order: the listener, then the core, its context and the loop.
struct PipeWire {
    _core_listener: pw::core::Listener,
    core: pw::core::CoreRc,
    _context: pw::context::ContextRc,
    main_loop: pw::main_loop::MainLoopRc,
    /// What the callbacks saw, by cast id (`Pila::dispatch_pipewire`).
    events: Rc<RefCell<Vec<(u64, CastEvent)>>>,
    source: RegistrationToken,
    /// Dma-bufs for the streams, on the renderer's device; `None`: shared
    /// memory only.
    allocator: Rc<RefCell<Option<Allocator>>>,
}

/// What a callback saw.
enum CastEvent {
    State(StreamState),
    /// The stream's buffers changed: copies in flight into the old ones
    /// are dropped, and the next frame is whole.
    Buffers,
    /// The stream's buffers were negotiated, of this kind and size.
    Negotiated(Kind, (u32, u32)),
    /// The connection to PipeWire failed.
    Disconnected(String),
}

/// A started session's stream.
struct Cast {
    id: u64,
    /// Who shares: the app as the portal names it (`capture_indicator.rs`).
    app: String,
    /// The portal session, by its object path.
    session: String,
    /// What's shared: an output or a window.
    target: Target,
    source_type: portal::SourceType,
    cursor: CursorMode,
    /// The cursor metadata last sent (`CursorMode::Metadata`): where, and
    /// which image.
    cursor_sent: Option<CursorFrame>,
    /// The stream's buffers, allocated by Pila (`screencast_buffers.rs`).
    memory: Rc<RefCell<Buffers>>,
    /// What the stream was offered: the source's size and scale then.
    size: Size<i32, Physical>,
    scale: Scale<f64>,
    /// Dropped before `stream`.
    _listener: StreamListener<()>,
    stream: StreamRc,
    /// `Start`'s answer, once the node has an id, and the grant the share
    /// is remembered by (kept then, if new).
    reply: Option<Reply<Result<portal::Started, Refused>>>,
    remembered: Option<crate::screen_grants::Remembered>,
    position: (i32, i32),
    logical_size: (i32, i32),
    /// A consumer is linked: frames go out.
    streaming: bool,
    /// Changes since the last frame sent; `None` before the first.
    tracker: Option<OutputDamageTracker>,
    /// Bumped when the stream's buffers change and when the cast ends: a
    /// copy in flight into one of the old ones is dropped.
    buffers: Rc<Cell<u64>>,
    /// Ended by Pila (its output or window went away, an error): told to
    /// the app.
    ended: bool,
    /// The sequence number of the next buffer sent (`write_header`).
    seq: u64,
}

impl Drop for Cast {
    fn drop(&mut self) {
        self.buffers.set(self.buffers.get() + 1);
        if let Some(reply) = self.reply.take() {
            reply.send(Err(Refused::Failed));
        }
        let _ = self.stream.disconnect();
    }
}

impl Pila {
    /// Serve the portal on the session bus, if Pila is the session (or a
    /// headless test given a bus): whether it's being started, and
    /// `Pila::startup_step_done` follows once it serves (or failed).
    pub fn start_portal(&mut self) -> bool {
        if !self.backend.as_ref().is_some_and(|b| b.serves_portal()) {
            return false;
        }
        let Some(bus) = std::env::var_os("DBUS_SESSION_BUS_ADDRESS") else {
            return false;
        };
        let (send, channel) = calloop::channel::channel();
        let inserted = self.loop_handle.insert_source(channel, |event, _, state| {
            if let Event::Msg(request) = event {
                state.portal_request(request);
            }
        });
        if let Err(err) = inserted {
            warn!(?err, "portal: can't listen for requests");
            return false;
        }
        portal::serve(bus.to_string_lossy().into_owned(), send);
        true
    }

    fn portal_request(&mut self, request: Request) {
        match request {
            Request::Serving(connection) => {
                info!(name = portal::BUS_NAME, "portal: serving ScreenCast");
                self.screencast.portal = Some(connection);
                // Connected once, now, and kept: connecting holds the event
                // loop a few ms, a one-time cost paid at startup.
                if let Err(err) = self.connect_pipewire() {
                    warn!(
                        err,
                        "portal: can't connect to PipeWire; trying again at a request"
                    );
                }
                self.startup_step_done();
            }
            Request::Failed(err) => {
                warn!(err, "portal: can't serve the session bus");
                self.startup_step_done();
            }
            Request::Start {
                session,
                cursor,
                sources,
                app_id,
                app,
                parent,
                persist,
                restore,
                reply,
            } => {
                // Connected at startup; again if the connection went.
                if let Err(err) = self.connect_pipewire() {
                    warn!(err, "portal: can't connect to PipeWire");
                }
                let share = Share {
                    session,
                    cursor,
                    app_id,
                    app,
                    parent,
                    sources,
                    persist,
                    restore,
                    reply,
                };
                // Remembered for the app: shared at once, if it's there.
                if let Some((target, remembered)) = self.restored_share(&share) {
                    return self.start_cast(share, target, remembered);
                }
                self.screencast
                    .waiting
                    .push_back(crate::share_picker::Ask::Cast(share));
                self.show_share_picker();
            }
            Request::PickColor { reply } => self.start_color_pick(reply),
            Request::ShortcutsStore(store) => self.app_shortcuts_store(store),
            Request::ScreenGrants(store) => self.screen_grants_store(store),
            Request::ShortcutsSession { session, app_id } => {
                self.app_shortcuts_session(session, app_id)
            }
            Request::BindShortcuts {
                session,
                app,
                parent,
                shortcuts,
                reply,
            } => self.bind_app_shortcuts(session, app, parent, shortcuts, reply),
            Request::ListShortcuts { session, reply } => {
                reply.send(self.list_app_shortcuts(&session))
            }
            Request::ConfigureShortcuts { session } => self.configure_app_shortcuts(session),
            Request::DbusInhibitors(inhibitors) => self.set_dbus_inhibitors(inhibitors),
            // Told the idle notifier with this iteration's input.
            Request::UserActivity => self.input_activity = true,
            Request::ScreenSaverActive { reply } => reply.send(self.is_locked()),
            Request::Screenshot {
                ask,
                app,
                parent,
                reply,
            } => {
                self.portal_screenshot(ask, crate::screenshot::Shot { app, parent, reply });
            }
            Request::Close { session } => {
                debug!(session, "portal: session closed");
                self.screencast
                    .waiting
                    .retain(|ask| ask.session() != Some(session.as_str()));
                self.close_share_picker_of(&session);
                self.app_shortcuts_closed(&session);
                let closed: Vec<Target> = self
                    .screencast
                    .casts
                    .iter()
                    .filter(|cast| cast.session == session)
                    .map(|cast| cast.target.clone())
                    .collect();
                for target in &closed {
                    self.redraw_region_outline(target);
                }
                self.screencast.casts.retain(|cast| cast.session != session);
                self.trim_after_casts();
            }
        }
    }

    /// Start `share`'s stream of `target` (an output or a window), picked
    /// by the user (`share_picker.rs`) or remembered for the app
    /// (`screen_grants.rs`, by `remembered`), answered once its node has an
    /// id.
    pub(crate) fn start_cast(
        &mut self,
        share: Share,
        target: Target,
        remembered: Option<crate::screen_grants::Remembered>,
    ) {
        let Share {
            session,
            cursor,
            reply,
            app_id,
            app,
            ..
        } = share;
        let app = app
            .filter(|app| !app.is_empty())
            .or_else(|| (!app_id.is_empty()).then_some(app_id))
            .unwrap_or_else(|| "an app".to_string());
        // Only the user's pick (now, or remembered) starts a share, and
        // never while locked.
        if self.is_locked() {
            return reply.send(Err(Refused::Cancelled));
        }
        let Some((size, scale)) = self.capture_geometry(&target, None) else {
            return reply.send(Err(Refused::Cancelled));
        };
        if size.w <= 0 || size.h <= 0 {
            return reply.send(Err(Refused::Cancelled));
        }
        let (source_type, position, logical_size, name) = match &target {
            // A region goes as a monitor of its size, where it is (the
            // portal has no other kind for it).
            Target::Output(_) | Target::Region(..) => {
                let Some(crate::capture::TargetArea { output, area, .. }) =
                    self.target_area(&target)
                else {
                    return reply.send(Err(Refused::Cancelled));
                };
                let name = match target {
                    Target::Region(..) => format!(
                        "{} {}x{}+{}+{}",
                        output.name(),
                        area.size.w,
                        area.size.h,
                        area.loc.x,
                        area.loc.y
                    ),
                    _ => output.name(),
                };
                (
                    portal::SourceType::Monitor,
                    (area.loc.x, area.loc.y),
                    (area.size.w, area.size.h),
                    name,
                )
            }
            Target::Toplevel(identifier) => {
                let Some(window) = self.window_for_identifier(identifier) else {
                    return reply.send(Err(Refused::Cancelled));
                };
                let geometry = window.geometry();
                let size = (geometry.size.w, geometry.size.h);
                let name = crate::foreign_toplevel::app_id(&window);
                (portal::SourceType::Window, (0, 0), size, name)
            }
            Target::Gone => return reply.send(Err(Refused::Cancelled)),
        };
        let fps = self.cast_fps(&target);
        if let Err(err) = self.connect_pipewire() {
            warn!(err, "portal: can't connect to PipeWire");
            return reply.send(Err(Refused::Failed));
        }
        let Some(pipewire) = &self.screencast.pipewire else {
            return reply.send(Err(Refused::Failed));
        };
        let id = self.screencast.next_id;
        self.screencast.next_id += 1;
        let memory: Rc<RefCell<Buffers>> = Rc::default();
        let stream = match new_stream(pipewire, id, size, fps, cursor, memory.clone()) {
            Ok(stream) => stream,
            Err(err) => {
                warn!(err, "portal: can't make a stream");
                return reply.send(Err(Refused::Failed));
            }
        };
        let (stream, listener) = stream;
        info!(
            session,
            ?source_type,
            source = name,
            ?size,
            fps,
            "portal: sharing"
        );
        // A region shared is outlined on the screen.
        self.redraw_region_outline(&target);
        self.screencast.casts.push(Cast {
            id,
            app,
            session,
            target,
            source_type,
            cursor,
            cursor_sent: None,
            memory,
            size,
            scale,
            _listener: listener,
            stream,
            reply: Some(reply),
            remembered,
            position,
            logical_size,
            streaming: false,
            tracker: None,
            buffers: Rc::new(Cell::new(0)),
            ended: false,
            seq: 0,
        });
        self.capture_indicator_changed();
        // Callbacks may have run while connecting.
        self.handle_cast_events();
    }

    /// The rate a stream of `target` is offered at: its output's refresh
    /// (a window's: the output it's drawn for, else the first output's).
    fn cast_fps(&self, target: &Target) -> u32 {
        self.target_output(target)
            .or_else(|| self.space.outputs().next().cloned())
            .and_then(|o| o.current_mode())
            .map_or(60, fps_of)
    }

    /// Connect to PipeWire if not yet, its loop a source of the event loop.
    fn connect_pipewire(&mut self) -> Result<(), String> {
        if self.screencast.pipewire.is_some() {
            return Ok(());
        }
        let started = Instant::now();
        // Dma-bufs where the renderer can draw into them.
        let allocator = self.capture.dmabuf.as_ref().and_then(|target| {
            Allocator::open(
                target.device(),
                target.modifiers(crate::screencast_buffers::FOURCC),
            )
        });
        pw::init();
        let main_loop = pw::main_loop::MainLoopRc::new(None).map_err(|e| e.to_string())?;
        let context = pw::context::ContextRc::new(&main_loop, None).map_err(|e| e.to_string())?;
        let core = context.connect_rc(None).map_err(|e| e.to_string())?;
        let events: Rc<RefCell<Vec<(u64, CastEvent)>>> = Rc::default();
        let core_events = events.clone();
        let core_listener = core
            .add_listener_local()
            .error(move |id, _seq, res, message| {
                // The core's own errors end the connection; a node's are
                // its stream's (`state_changed`).
                if id == pw::core::PW_ID_CORE {
                    let err = format!("{message} ({res})");
                    core_events
                        .borrow_mut()
                        .push((u64::MAX, CastEvent::Disconnected(err)));
                }
            })
            .register();
        let fd: OwnedFd = main_loop
            .loop_()
            .fd()
            .try_clone_to_owned()
            .map_err(|e| e.to_string())?;
        let source = self
            .loop_handle
            .insert_source(
                Generic::new(fd, Interest::READ, Mode::Level),
                |_, _, state| {
                    state.dispatch_pipewire();
                    Ok(PostAction::Continue)
                },
            )
            .map_err(|e| e.error.to_string())?;
        self.screencast.pipewire = Some(PipeWire {
            _core_listener: core_listener,
            core,
            _context: context,
            main_loop,
            events,
            source,
            allocator: Rc::new(RefCell::new(allocator)),
        });
        info!(elapsed = ?started.elapsed(), "portal: connected to PipeWire");
        Ok(())
    }

    /// PipeWire's loop has something: run it without waiting, then handle
    /// what its callbacks saw.
    fn dispatch_pipewire(&mut self) {
        if let Some(pipewire) = &self.screencast.pipewire {
            pipewire
                .main_loop
                .loop_()
                .iterate(pw::loop_::Timeout::Finite(Duration::ZERO));
        }
        self.handle_cast_events();
    }

    fn handle_cast_events(&mut self) {
        let Some(pipewire) = &self.screencast.pipewire else {
            return;
        };
        let events = std::mem::take(&mut *pipewire.events.borrow_mut());
        let mut lost = false;
        // New grants of shares that started, kept once the casts are let go.
        let mut kept = Vec::new();
        for (id, event) in events {
            if let CastEvent::Disconnected(err) = event {
                warn!(err, "portal: PipeWire connection lost");
                for cast in &mut self.screencast.casts {
                    cast.ended = true;
                }
                lost = true;
                continue;
            }
            let Some(cast) = self.screencast.casts.iter_mut().find(|c| c.id == id) else {
                continue;
            };
            match event {
                CastEvent::State(StreamState::Paused) => {
                    cast.streaming = false;
                    if let Some(reply) = cast.reply.take() {
                        let node_id = cast.stream.node_id();
                        debug!(node_id, "portal: stream ready");
                        let remembered = cast.remembered.take();
                        reply.send(Ok(portal::Started {
                            streams: vec![portal::Stream {
                                node_id,
                                source_type: cast.source_type,
                                position: cast.position,
                                size: cast.logical_size,
                            }],
                            remembered: remembered.as_ref().map(|r| (r.id.clone(), r.mode)),
                        }));
                        if let Some(crate::screen_grants::Remembered {
                            id,
                            grant: Some(grant),
                            ..
                        }) = remembered
                        {
                            kept.push((id, grant));
                        }
                    }
                }
                CastEvent::State(StreamState::Streaming) => {
                    debug!(session = cast.session, "portal: streaming");
                    cast.streaming = true;
                    cast.tracker = None;
                    // The first frame now, whether or not the screen changes.
                    self.capture_due = true;
                    self.wake.ping();
                }
                CastEvent::State(StreamState::Error(err)) => {
                    warn!(err, session = cast.session, "portal: stream failed");
                    cast.ended = true;
                }
                CastEvent::State(StreamState::Unconnected) => cast.ended = true,
                CastEvent::State(StreamState::Connecting) => {}
                CastEvent::Buffers => {
                    cast.buffers.set(cast.buffers.get() + 1);
                    cast.tracker = None;
                }
                // Shared memory: the texture frames are read back through,
                // made now, ahead of the first frame (a one-time cost that
                // made it late). Dma-bufs are drawn into: none.
                CastEvent::Negotiated(Kind::Shm, (w, h)) => {
                    let size = Size::from((w as i32, h as i32));
                    if let Some(mut renderer) = self.backend.as_mut().and_then(|b| b.renderer()) {
                        self.capture.prepare_read_back(&mut renderer, size);
                    }
                }
                CastEvent::Negotiated(..) => {}
                CastEvent::Disconnected(_) => {}
            }
        }
        for (id, grant) in kept {
            self.keep_screen_grant(id, grant);
        }
        self.end_casts();
        if lost {
            self.disconnect_pipewire();
        }
    }

    /// End the casts of `app` sharing `target` (the tray's Stop,
    /// `Pila::stop_capture`), telling the app, as when its monitor goes.
    pub(crate) fn stop_casts(&mut self, app: &str, target: &crate::capture_indicator::Captured) {
        let stopped: Vec<u64> = self
            .screencast
            .casts
            .iter()
            .filter(|cast| cast.app == app && self.captured(&cast.target).as_ref() == Some(target))
            .map(|cast| cast.id)
            .collect();
        for cast in &mut self.screencast.casts {
            if stopped.contains(&cast.id) {
                cast.ended = true;
            }
        }
        self.end_casts();
    }

    /// Drop the casts Pila ended, telling their apps.
    fn end_casts(&mut self) {
        if !self.screencast.casts.iter().any(|cast| cast.ended) {
            return;
        }
        let (ended, kept) = std::mem::take(&mut self.screencast.casts)
            .into_iter()
            .partition(|cast| cast.ended);
        self.screencast.casts = kept;
        for cast in ended {
            info!(session = cast.session, "portal: sharing ended");
            self.redraw_region_outline(&cast.target);
            if let Some(connection) = &self.screencast.portal {
                portal::end_session(connection, cast.session.clone());
            }
        }
        self.trim_after_casts();
    }

    /// With no cast left, the offscreen textures go if no capture keeps
    /// them: now, not at the next frame drawn (a still screen has none).
    /// PipeWire's connection stays for the next.
    fn trim_after_casts(&mut self) {
        if self.screencast.casts.is_empty() {
            self.capture.trim_textures(false);
        }
    }

    /// PipeWire's connection is gone (the daemon went): dropped, made again
    /// at the next request.
    fn disconnect_pipewire(&mut self) {
        if let Some(pipewire) = self.screencast.pipewire.take() {
            self.loop_handle.remove(pipewire.source);
            debug!("portal: disconnected from PipeWire");
        }
    }

    /// Send a frame to each streaming cast whose output changed (or that
    /// has none yet), from `after_frames`. `input` is the scene of this
    /// pass, shared with the other captures.
    pub fn process_screencasts(&mut self, input: &mut Option<render::FrameInput>) {
        if !self.screencast.streaming() {
            return;
        }
        let mut casts = std::mem::take(&mut self.screencast.casts);
        for cast in casts.iter_mut().filter(|cast| cast.streaming) {
            let started = Instant::now();
            self.process_cast(cast, input);
            self.record_capture(started.elapsed());
        }
        casts.append(&mut self.screencast.casts);
        self.screencast.casts = casts;
        self.end_casts();
    }

    fn process_cast(&mut self, cast: &mut Cast, input: &mut Option<render::FrameInput>) {
        let Some((size, scale)) =
            self.capture_geometry(&cast.target, Some((cast.size, cast.scale)))
        else {
            cast.ended = true;
            return;
        };
        if size != cast.size {
            // A new size (a mode, a turn, a window resized): the stream's
            // format is offered again at it, and frames wait for buffers of
            // that size.
            info!(
                session = cast.session,
                ?size,
                "portal: what's shared changed size; negotiating again"
            );
            cast.size = size;
            cast.scale = scale;
            cast.tracker = None;
            cast.buffers.set(cast.buffers.get() + 1);
            let fps = self.cast_fps(&cast.target);
            cast.memory.borrow_mut().offered = ((size.w, size.h), fps);
            let offered = self
                .screencast
                .pipewire
                .as_ref()
                .map(modifiers_offered)
                .unwrap_or_default();
            let formats = format_params(size, fps, Modifiers::Offer(&offered));
            let mut pods: Vec<&Pod> = formats.iter().filter_map(|f| Pod::from_bytes(f)).collect();
            if let Err(err) = cast.stream.update_params(&mut pods) {
                warn!(%err, "portal: can't offer the new size");
                cast.ended = true;
            }
            return;
        }
        if scale != cast.scale {
            // The same pixels at another scale: drawn anew, nothing to
            // negotiate.
            cast.scale = scale;
            cast.tracker = None;
        }
        if size.w <= 0 || size.h <= 0 || cast.memory.borrow().size != (size.w as u32, size.h as u32)
        {
            // Still negotiating the new size.
            return;
        }
        // What's shared: the output at its place, or the window, its
        // geometry's top-left the buffer's (`capture::Target`, as screen
        // captures draw it). A window's contents stay private while the
        // screen is locked.
        let (origin, output, window) = match &cast.target {
            Target::Output(_) | Target::Region(..) => {
                let Some(crate::capture::TargetArea { output, area, geo }) =
                    self.target_area(&cast.target)
                else {
                    cast.ended = true;
                    return;
                };
                (area.loc, Some((output.name(), geo, area)), None)
            }
            Target::Toplevel(identifier) => {
                if self.is_locked() {
                    return;
                }
                let Some(window) = self.window_for_identifier(identifier) else {
                    cast.ended = true;
                    return;
                };
                let origin = self.space.element_location(&window).unwrap_or_default();
                (origin, None, Some(window))
            }
            Target::Gone => {
                cast.ended = true;
                return;
            }
        };
        let input = &*input.get_or_insert_with(|| self.frame_input());
        // The pointer shows over an output's stream, over a region's where
        // it's in it, and over a window's where it's over the window (and
        // the window on the screen).
        let pointer_over = match (&window, &output) {
            (None, Some((_, _, area))) => area.to_f64().contains(input.pointer),
            (None, None) => true,
            (Some(window), _) => self.space.element_location(window).is_some_and(|loc| {
                Rectangle::new(loc, window.geometry().size)
                    .to_f64()
                    .contains(input.pointer)
            }),
        };
        let clear = crate::capture::clear_for(window.as_ref(), &input.scene);
        let cursor_frame = (cast.cursor == CursorMode::Metadata).then(|| {
            let frame = self.cursor_frame(input.pointer, origin, scale);
            if pointer_over {
                frame
            } else {
                CursorFrame {
                    image: None,
                    ..frame
                }
            }
        });
        let Pila {
            backend,
            blur,
            capture,
            cursor_theme,
            ..
        } = self;
        let Some(backend) = backend.as_mut() else {
            return;
        };
        let Some(mut renderer) = backend.renderer() else {
            return;
        };
        let renderer = &mut *renderer;
        let mut elements: Vec<PilaRenderElement> = Vec::new();
        if cast.cursor == CursorMode::Embedded && pointer_over {
            elements = render::pointer_elements(renderer, input, cursor_theme, origin, scale);
        }
        match (&window, &output) {
            (Some(window), _) => {
                elements.extend(crate::capture::window_elements(renderer, window, scale))
            }
            (None, Some((name, geo, area))) => elements.extend(render::output_elements(
                renderer,
                blur,
                &input.scene,
                name,
                *geo,
                area.loc,
                scale,
            )),
            (None, None) => return,
        }

        let tracker = cast
            .tracker
            .get_or_insert_with(|| OutputDamageTracker::new(size, scale, Transform::Normal));
        let changed = match crate::capture::damage_of(tracker, &elements, "portal") {
            crate::capture::Damage::Rects(_) => true,
            crate::capture::Damage::Unchanged => false,
            crate::capture::Damage::Unknown => {
                cast.tracker = None;
                true
            }
        };
        // The pointer moved or changed (metadata): a buffer for that alone.
        let cursor_moved = cursor_frame.as_ref().is_some_and(|frame| {
            cast.cursor_sent
                .as_ref()
                .is_none_or(|sent| !sent.same_as(frame))
        });
        if !changed && !cursor_moved {
            return;
        }
        // SAFETY: the stream is alive; the buffer is ours until queued.
        let buffer = unsafe { cast.stream.dequeue_raw_buffer() };
        if buffer.is_null() {
            // The consumer holds them all: this change goes with the next.
            debug!(
                session = cast.session,
                "portal: no buffer free, frame skipped"
            );
            if changed {
                cast.tracker = None;
            }
            return;
        }
        let pts = Duration::from(self.clock.now()).as_nanos() as i64;
        // SAFETY: dequeued, so ours to write until queued.
        unsafe { write_header(buffer, pts, cast.seq) };
        cast.seq += 1;
        if let Some(frame) = cursor_frame {
            let new_image = cast
                .cursor_sent
                .as_ref()
                .is_none_or(|sent| !sent.same_image(&frame));
            // SAFETY: dequeued, so ours to write until queued.
            unsafe { write_cursor_meta(buffer, &frame, new_image) };
            cast.cursor_sent = Some(frame);
        }
        if !changed {
            // The pointer alone: no picture in it.
            // SAFETY: as above.
            unsafe {
                mark_cursor_only(buffer);
                cast.stream.queue_raw_buffer(buffer);
            }
            return;
        }
        let generation = cast.buffers.get();
        let buffers = cast.buffers.clone();
        let (w, h) = (size.w, size.h);
        let source = Source::Elements {
            elements: &elements,
            clear,
        };
        let dmabuf = match cast.memory.borrow().get(buffer) {
            Some(Memory::Dmabuf(dmabuf)) => Some(dmabuf.clone()),
            _ => None,
        };
        // A dma-buf is drawn into, no copy through the CPU; shared memory is
        // read back into.
        let drawn_into = dmabuf.is_some();
        let copied = match dmabuf {
            Some(mut dmabuf) => {
                crate::capture::render_into_dmabuf(renderer, &mut dmabuf, size, scale, source)
            }
            None => capture.read_back(
                renderer,
                size,
                scale,
                source,
                Fourcc::Argb8888,
                // The stream's buffers are mapped once and used again: no
                // pages to fault in.
                Write::OnLoop(Box::new(move |pixels| {
                    if buffers.get() != generation {
                        return Ok(());
                    }
                    // SAFETY: the buffer is still the stream's (same
                    // generation) and dequeued, so ours to write.
                    unsafe { fill(buffer, pixels, w, h) }
                })),
            ),
        };
        let stream = cast.stream.clone();
        let buffers = cast.buffers.clone();
        let queue = move |filled: bool| {
            if buffers.get() != generation {
                return;
            }
            if !filled {
                // SAFETY: as above.
                unsafe { mark_empty(buffer) };
            } else if drawn_into {
                // SAFETY: as above.
                unsafe { mark_whole(buffer, h) };
            }
            // SAFETY: dequeued from this stream, whose buffers are the same.
            unsafe { stream.queue_raw_buffer(buffer) };
        };
        // A window off the screen (or covered) draws for the share: frame
        // callbacks now, one window frame per frame sent
        // (`Pila::send_capture_frames`, as screen captures do).
        if let Some(window) = &window {
            self.send_capture_frames(window);
        }
        match copied {
            Ok(copied) => {
                let wanted = cast.buffers.clone();
                self.when_copied(
                    copied,
                    move || wanted.get() == generation,
                    move |_, result| queue(result.is_ok()),
                );
            }
            Err(err) => {
                if let CopyError::Render(err) = err {
                    warn!(err, "portal: frame failed");
                }
                cast.tracker = None;
                queue(false);
            }
        }
    }
}

/// The pointer as a stream's cursor metadata gives it: where its hotspot
/// is, in the stream's pixels, and its image (`None`: hidden).
pub(crate) struct CursorFrame {
    position: (i32, i32),
    image: Option<Arc<CursorPixels>>,
}

impl CursorFrame {
    fn same_image(&self, other: &CursorFrame) -> bool {
        match (&self.image, &other.image) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b) || a == b,
            (None, None) => true,
            _ => false,
        }
    }

    fn same_as(&self, other: &CursorFrame) -> bool {
        self.position == other.position && self.same_image(other)
    }
}

/// The largest cursor image sent as metadata, a side in pixels: a theme's
/// at twice its size is far from it.
const CURSOR_META_MAX: i32 = 256;

/// The room a buffer's cursor metadata takes: the cursor, a bitmap's
/// header and the largest image.
fn cursor_meta_size() -> usize {
    std::mem::size_of::<spa::sys::spa_meta_cursor>()
        + std::mem::size_of::<spa::sys::spa_meta_bitmap>()
        + (CURSOR_META_MAX * CURSOR_META_MAX * 4) as usize
}

impl Pila {
    /// The pointer for a stream of the output at `origin`, at `scale`.
    fn cursor_frame(
        &mut self,
        pointer: smithay::utils::Point<f64, smithay::utils::Logical>,
        origin: smithay::utils::Point<i32, smithay::utils::Logical>,
        scale: Scale<f64>,
    ) -> CursorFrame {
        use smithay::input::pointer::CursorImageStatus;
        let position = (pointer - origin.to_f64())
            .to_physical(scale)
            .to_i32_round();
        let image = match &self.cursor_status {
            CursorImageStatus::Hidden => None,
            CursorImageStatus::Named(icon) => Some(
                self.cursor_theme
                    .get(*icon, crate::scale::buffer_scale(scale))
                    .image,
            ),
            CursorImageStatus::Surface(surface) => {
                let hotspot = crate::cursor::surface_position(&self.cursor_status, pointer)
                    .map(|(_, top_left)| pointer.to_i32_round() - top_left)
                    .unwrap_or_default();
                shm_cursor(surface, (hotspot.x, hotspot.y)).map(Arc::new)
            }
        };
        CursorFrame {
            position: (position.x, position.y),
            image,
        }
    }
}

/// A client's cursor surface as an image, if its buffer is shm in a format
/// a cursor bitmap takes (most are; one that isn't goes without a picture).
fn shm_cursor(
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    hotspot: (i32, i32),
) -> Option<CursorPixels> {
    use smithay::{
        backend::renderer::utils::with_renderer_surface_state,
        reexports::wayland_server::protocol::wl_shm, wayland::shm::with_buffer_contents,
    };
    let buffer = with_renderer_surface_state(surface, |state| state.buffer().cloned()).flatten()?;
    with_buffer_contents(&buffer, |ptr, len, data| {
        let opaque = match data.format {
            wl_shm::Format::Argb8888 => false,
            wl_shm::Format::Xrgb8888 => true,
            _ => return None,
        };
        let (w, h, stride) = (data.width, data.height, data.stride as usize);
        if w <= 0 || h <= 0 || w > CURSOR_META_MAX || h > CURSOR_META_MAX {
            return None;
        }
        let row = w as usize * 4;
        let end = data.offset as usize + stride * (h as usize - 1) + row;
        if end > len || stride < row {
            return None;
        }
        // SAFETY: the pool is mapped, `len` bytes long, for this closure;
        // the range was checked.
        let src = unsafe { std::slice::from_raw_parts(ptr, len) };
        let mut pixels = Vec::with_capacity(row * h as usize);
        for y in 0..h as usize {
            let start = data.offset as usize + y * stride;
            pixels.extend_from_slice(&src[start..start + row]);
        }
        if opaque {
            for pixel in pixels.as_chunks_mut::<4>().0 {
                pixel[3] = 0xff;
            }
        }
        Some(CursorPixels {
            pixels,
            width: w,
            height: h,
            hotspot,
        })
    })
    .ok()
    .flatten()
}

/// Stamp a dequeued `buffer`'s header, if the consumer asked for one: when
/// it was made (`pts`, nanoseconds of `CLOCK_MONOTONIC`, as mutter's and
/// xdg-desktop-portal-wlr's) and its place in the stream (`seq`). GStreamer's
/// pipewiresrc (Kooha, OBS's GStreamer sources) times frames by it, and
/// without one by the graph's time, which a stream that drives itself
/// doesn't move: every frame at the same time, a recording 0 seconds long.
///
/// SAFETY: `buffer` is a dequeued buffer of a live stream.
unsafe fn write_header(buffer: *mut pw::sys::pw_buffer, pts: i64, seq: u64) {
    use spa::sys::spa_meta_header;
    // SAFETY: per this function's contract.
    unsafe {
        let spa_buffer = (*buffer).buffer;
        if spa_buffer.is_null() {
            return;
        }
        let meta = spa::sys::spa_buffer_find_meta(spa_buffer, spa::sys::SPA_META_Header);
        if meta.is_null() || ((*meta).size as usize) < std::mem::size_of::<spa_meta_header>() {
            return;
        }
        let header = (*meta).data.cast::<spa_meta_header>();
        if header.is_null() {
            return;
        }
        (*header).flags = 0;
        (*header).offset = 0;
        (*header).pts = pts;
        (*header).dts_offset = 0;
        (*header).seq = seq;
    }
}

/// Write the pointer into a dequeued `buffer`'s cursor metadata, with its
/// image if `new_image` (apps keep the last one). Nothing if the buffer has
/// none (the consumer didn't take it).
///
/// SAFETY: `buffer` is a dequeued buffer of a live stream.
unsafe fn write_cursor_meta(buffer: *mut pw::sys::pw_buffer, frame: &CursorFrame, new_image: bool) {
    use spa::sys::{spa_meta_bitmap, spa_meta_cursor};
    // SAFETY: per this function's contract.
    unsafe {
        let spa_buffer = (*buffer).buffer;
        if spa_buffer.is_null() {
            return;
        }
        let meta = spa::sys::spa_buffer_find_meta(spa_buffer, spa::sys::SPA_META_Cursor);
        if meta.is_null() || ((*meta).size as usize) < std::mem::size_of::<spa_meta_cursor>() {
            debug!(
                metas = (*spa_buffer).n_metas,
                "portal: a buffer without room for the cursor's metadata"
            );
            return;
        }
        let room = (*meta).size as usize;
        let cursor = (*meta).data.cast::<spa_meta_cursor>();
        if cursor.is_null() {
            return;
        }
        (*cursor).id = 1;
        (*cursor).flags = 0;
        (*cursor).position.x = frame.position.0;
        (*cursor).position.y = frame.position.1;
        (*cursor).bitmap_offset = 0;
        if !new_image {
            return;
        }
        // Hidden: an empty image.
        let (pixels, width, height, hotspot) = match &frame.image {
            Some(image) => (&image.pixels[..], image.width, image.height, image.hotspot),
            None => (&[0u8; 4][..], 1, 1, (0, 0)),
        };
        let header = std::mem::size_of::<spa_meta_cursor>();
        let bitmap_header = std::mem::size_of::<spa_meta_bitmap>();
        let bytes = (width * height * 4) as usize;
        if header + bitmap_header + bytes > room || pixels.len() < bytes {
            debug!(width, height, "portal: a cursor too big for its metadata");
            return;
        }
        (*cursor).hotspot.x = hotspot.0;
        (*cursor).hotspot.y = hotspot.1;
        (*cursor).bitmap_offset = header as u32;
        let bitmap = cursor.cast::<u8>().add(header).cast::<spa_meta_bitmap>();
        (*bitmap).format = spa::sys::SPA_VIDEO_FORMAT_BGRA;
        (*bitmap).size.width = width as u32;
        (*bitmap).size.height = height as u32;
        (*bitmap).stride = width * 4;
        (*bitmap).offset = bitmap_header as u32;
        let dst = bitmap.cast::<u8>().add(bitmap_header);
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), dst, bytes);
    }
}

/// Mark a dequeued `buffer` as bringing the pointer alone: no picture (its
/// data empty), so apps keep the last one.
///
/// SAFETY: as `fill`'s.
unsafe fn mark_cursor_only(buffer: *mut pw::sys::pw_buffer) {
    // SAFETY: per this function's contract.
    unsafe {
        for chunk in chunks(buffer) {
            // Empty, and corrupted for apps that would read a dma-buf
            // anyway (libwebrtc drops shared memory that's empty, any
            // buffer marked corrupted: after taking the pointer from it).
            chunk.size = 0;
            chunk.flags = spa::sys::SPA_CHUNK_FLAG_CORRUPTED as i32;
        }
    }
}

/// Mark a dequeued dma-buf `buffer` of `height` rows as holding a whole
/// frame: each plane's chunk its rows (after a pointer-only buffer emptied
/// them).
///
/// SAFETY: as `fill`'s.
unsafe fn mark_whole(buffer: *mut pw::sys::pw_buffer, height: i32) {
    // SAFETY: per this function's contract.
    unsafe {
        for chunk in chunks(buffer) {
            chunk.size = chunk.stride.max(0) as u32 * height.max(0) as u32;
            chunk.flags = 0;
        }
    }
}

/// The chunks of a dequeued `buffer`'s datas.
///
/// SAFETY: as `fill`'s; the chunks are used before the buffer is queued.
unsafe fn chunks<'a>(
    buffer: *mut pw::sys::pw_buffer,
) -> impl Iterator<Item = &'a mut spa::sys::spa_chunk> {
    // SAFETY: per this function's contract.
    unsafe {
        let spa_buffer = (*buffer).buffer;
        let datas: &mut [spa::sys::spa_data] =
            if spa_buffer.is_null() || (*spa_buffer).datas.is_null() {
                &mut []
            } else {
                std::slice::from_raw_parts_mut((*spa_buffer).datas, (*spa_buffer).n_datas as usize)
            };
        datas.iter_mut().filter_map(|data| data.chunk.as_mut())
    }
}

/// Write `pixels` (`w` by `h`) into a dequeued `buffer`'s memory and say so
/// in its chunk.
///
/// SAFETY: `buffer` is a dequeued buffer of a live stream mapped with
/// `MAP_BUFFERS`.
unsafe fn fill(
    buffer: *mut pw::sys::pw_buffer,
    pixels: &ReadBack<'_>,
    w: i32,
    h: i32,
) -> Result<(), CopyError> {
    // SAFETY: per this function's contract.
    let data = unsafe {
        let spa_buffer = (*buffer).buffer;
        if spa_buffer.is_null() || (*spa_buffer).datas.is_null() || (*spa_buffer).n_datas == 0 {
            return Err(CopyError::Constraints);
        }
        &mut *(*spa_buffer).datas
    };
    if data.data.is_null() || data.chunk.is_null() {
        return Err(CopyError::Constraints);
    }
    let stride = w as usize * 4;
    // SAFETY: mapped, `maxsize` bytes long; `write_into` checks the range.
    let dst =
        unsafe { std::slice::from_raw_parts_mut(data.data.cast::<u8>(), data.maxsize as usize) };
    pixels.write_into(dst, 0, stride)?;
    // SAFETY: checked non-null above; the buffer's.
    let chunk = unsafe { &mut *data.chunk };
    chunk.offset = 0;
    chunk.size = (stride * h as usize) as u32;
    chunk.stride = stride as i32;
    chunk.flags = 0;
    Ok(())
}

/// Mark a dequeued `buffer` as holding nothing, to queue it back.
///
/// SAFETY: as `fill`'s.
unsafe fn mark_empty(buffer: *mut pw::sys::pw_buffer) {
    // SAFETY: per this function's contract.
    unsafe {
        let spa_buffer = (*buffer).buffer;
        if spa_buffer.is_null() || (*spa_buffer).datas.is_null() || (*spa_buffer).n_datas == 0 {
            return;
        }
        let data = &mut *(*spa_buffer).datas;
        if let Some(chunk) = data.chunk.as_mut() {
            chunk.size = 0;
            chunk.flags = spa::sys::SPA_CHUNK_FLAG_CORRUPTED as i32;
        }
    }
}

/// A stream for cast `id`, offering frames of `size` up to `fps` a second,
/// connected as a driver: PipeWire takes its frames as they come.
fn new_stream(
    pipewire: &PipeWire,
    id: u64,
    size: Size<i32, Physical>,
    fps: u32,
    cursor: CursorMode,
    memory: Rc<RefCell<Buffers>>,
) -> Result<(StreamRc, StreamListener<()>), String> {
    let core = pipewire.core.clone();
    let stream = StreamRc::new(
        core,
        "pila-screencast",
        properties! {
            *pw::keys::MEDIA_CLASS => "Video/Source",
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
            *pw::keys::NODE_DESCRIPTION => "Pila screen share",
        },
    )
    .map_err(|e| e.to_string())?;
    let events = pipewire.events.clone();
    let state_events = events.clone();
    let format_events = events.clone();
    let allocator = pipewire.allocator.clone();
    let format_allocator = allocator.clone();
    let format_memory = memory.clone();
    let add_memory = memory.clone();
    let remove_memory = memory.clone();
    let listener = stream
        .add_local_listener_with_user_data(())
        .state_changed(move |_, _, _, new| {
            state_events.borrow_mut().push((id, CastEvent::State(new)));
        })
        .param_changed(move |stream, _, param_id, param| {
            if param_id != ParamType::Format.as_raw() {
                return;
            }
            format_events.borrow_mut().push((id, CastEvent::Buffers));
            let Some(param) = param else {
                return;
            };
            let mut info = VideoInfoRaw::new();
            if info.parse(param).is_err() {
                return;
            }
            let (width, height) = (info.size().width, info.size().height);
            let kind = match crate::screencast_buffers::format_modifiers(param) {
                None => Kind::Shm,
                // Several the app takes: one fixed, offered alone (with
                // shared memory still), and negotiated again.
                Some((false, candidates)) => {
                    let picked = format_allocator
                        .borrow_mut()
                        .as_mut()
                        .and_then(|a| a.pick_modifier((width, height), &candidates));
                    let modifiers = match picked {
                        Some((modifier, _)) => Modifiers::Fixed(modifier),
                        None => Modifiers::None,
                    };
                    let ((w, h), fps) = format_memory.borrow().offered;
                    let formats = format_params(Size::from((w, h)), fps, modifiers);
                    let mut pods: Vec<&Pod> =
                        formats.iter().filter_map(|f| Pod::from_bytes(f)).collect();
                    if let Err(err) = stream.update_params(&mut pods) {
                        warn!(%err, "portal: can't fix the stream's modifier");
                    }
                    return;
                }
                Some((true, picked)) => {
                    let planes = picked.first().and_then(|&m| {
                        format_allocator
                            .borrow_mut()
                            .as_mut()
                            .and_then(|a| a.pick_modifier((width, height), &[m]))
                    });
                    match planes {
                        Some((modifier, planes)) => Kind::Dmabuf { modifier, planes },
                        None => {
                            warn!("portal: can't allocate the dma-bufs negotiated");
                            return;
                        }
                    }
                }
            };
            {
                let mut memory = format_memory.borrow_mut();
                memory.kind = Some(kind);
                memory.size = (width, height);
            }
            debug!(?kind, "portal: stream buffers negotiated");
            format_events
                .borrow_mut()
                .push((id, CastEvent::Negotiated(kind, (width, height))));
            let Some(bytes) = buffers_param(width, height, kind) else {
                return;
            };
            let Some(pod) = Pod::from_bytes(&bytes) else {
                return;
            };
            // The pointer beside the frames, for the app to draw.
            let cursor_meta = (cursor == CursorMode::Metadata)
                .then(|| meta_param(spa::sys::SPA_META_Cursor, cursor_meta_size()))
                .flatten();
            // When each frame was made, which recorders time them by.
            let header_meta = meta_param(
                spa::sys::SPA_META_Header,
                std::mem::size_of::<spa::sys::spa_meta_header>(),
            );
            let mut params = vec![pod];
            for meta in [&cursor_meta, &header_meta] {
                if let Some(meta) = meta.as_deref().and_then(Pod::from_bytes) {
                    params.push(meta);
                }
            }
            if let Err(err) = stream.update_params(&mut params) {
                warn!(%err, "portal: can't set the stream's buffers");
            }
        })
        .add_buffer(move |_, _, buffer| {
            // SAFETY: PipeWire adds it, with the negotiated blocks.
            unsafe {
                add_memory
                    .borrow_mut()
                    .add(buffer, allocator.borrow_mut().as_mut())
            };
        })
        .remove_buffer(move |_, _, buffer| {
            remove_memory.borrow_mut().remove(buffer);
            events.borrow_mut().push((id, CastEvent::Buffers));
        })
        .register()
        .map_err(|e| e.to_string())?;
    memory.borrow_mut().offered = ((size.w, size.h), fps);
    let offered = modifiers_offered(pipewire);
    let formats = format_params(size, fps, Modifiers::Offer(&offered));
    let mut pods: Vec<&Pod> = formats.iter().filter_map(|f| Pod::from_bytes(f)).collect();
    if pods.is_empty() {
        return Err("can't describe the format".into());
    }
    stream
        .connect(
            spa::utils::Direction::Output,
            None,
            StreamFlags::DRIVER | StreamFlags::ALLOC_BUFFERS,
            &mut pods,
        )
        .map_err(|e| e.to_string())?;
    Ok((stream, listener))
}

/// The dma-buf modifiers streams offer: what the renderer draws into, if
/// there's an allocator for them.
fn modifiers_offered(pipewire: &PipeWire) -> Vec<u64> {
    pipewire
        .allocator
        .borrow()
        .as_ref()
        .map(|a| a.modifiers.clone())
        .unwrap_or_default()
}

/// A stream's refresh, as its max framerate: the output mode's, rounded.
fn fps_of(mode: smithay::output::Mode) -> u32 {
    ((mode.refresh + 500) / 1000).max(1) as u32
}

fn serialize(object: Object) -> Option<Vec<u8>> {
    PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(object))
        .ok()
        .map(|(cursor, _)| cursor.into_inner())
}

/// The format offered: raw video, BGRx (XRGB8888, as GL reads it back), of
/// the output's `size`, at a variable rate of at most `fps`.
fn format_param(size: Size<i32, Physical>, fps: u32, modifiers: Modifiers<'_>) -> Option<Vec<u8>> {
    let id = |raw: u32| Value::Id(spa::utils::Id(raw));
    let mut properties = vec![
        Property::new(
            FormatProperties::MediaType.as_raw(),
            id(MediaType::Video.as_raw()),
        ),
        Property::new(
            FormatProperties::MediaSubtype.as_raw(),
            id(MediaSubtype::Raw.as_raw()),
        ),
        Property::new(
            FormatProperties::VideoFormat.as_raw(),
            id(VideoFormat::BGRx.as_raw()),
        ),
        Property::new(
            FormatProperties::VideoSize.as_raw(),
            Value::Rectangle(SpaRectangle {
                width: size.w as u32,
                height: size.h as u32,
            }),
        ),
        Property::new(
            FormatProperties::VideoFramerate.as_raw(),
            Value::Fraction(Fraction { num: 0, denom: 1 }),
        ),
        Property::new(
            FormatProperties::VideoMaxFramerate.as_raw(),
            Value::Choice(ChoiceValue::Fraction(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Range {
                    default: Fraction { num: fps, denom: 1 },
                    min: Fraction { num: 1, denom: 1 },
                    max: Fraction { num: fps, denom: 1 },
                },
            ))),
        ),
    ];
    let modifier = |flags: u32, value: Value| Property {
        key: FormatProperties::VideoModifier.as_raw(),
        flags: spa::pod::PropertyFlags::from_bits_retain(flags),
        value,
    };
    match modifiers {
        Modifiers::None => {}
        Modifiers::Offer(all) => {
            let (&first, _) = all.split_first()?;
            properties.push(modifier(
                spa::sys::SPA_POD_PROP_FLAG_MANDATORY | spa::sys::SPA_POD_PROP_FLAG_DONT_FIXATE,
                Value::Choice(ChoiceValue::Long(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Enum {
                        default: first as i64,
                        alternatives: all.iter().map(|&m| m as i64).collect(),
                    },
                ))),
            ));
        }
        Modifiers::Fixed(one) => properties.push(modifier(
            spa::sys::SPA_POD_PROP_FLAG_MANDATORY,
            Value::Long(one as i64),
        )),
    }
    serialize(Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: ParamType::EnumFormat.as_raw(),
        properties,
    })
}

/// Which dma-buf modifiers a format offers.
#[derive(Clone, Copy)]
enum Modifiers<'a> {
    /// None: shared memory.
    None,
    /// These, for the app to pick from.
    Offer(&'a [u64]),
    /// This one, picked.
    Fixed(u64),
}

/// The formats a stream offers: with `modifiers` (dma-bufs) if there are
/// any, then without (shared memory).
fn format_params(size: Size<i32, Physical>, fps: u32, modifiers: Modifiers<'_>) -> Vec<Vec<u8>> {
    let with = match modifiers {
        Modifiers::Offer([]) | Modifiers::None => None,
        modifiers => format_param(size, fps, modifiers),
    };
    with.into_iter()
        .chain(format_param(size, fps, Modifiers::None))
        .collect()
}

/// A metadata block of `kind`, `size` bytes, on every buffer.
fn meta_param(kind: u32, size: usize) -> Option<Vec<u8>> {
    serialize(Object {
        type_: SpaTypes::ObjectParamMeta.as_raw(),
        id: ParamType::Meta.as_raw(),
        properties: vec![
            Property::new(
                spa::sys::SPA_PARAM_META_type,
                Value::Id(spa::utils::Id(kind)),
            ),
            Property::new(spa::sys::SPA_PARAM_META_size, Value::Int(size as i32)),
        ],
    })
}

/// The buffers a stream of `width` by `height` BGRx frames wants, `kind`:
/// shm (`MemFd`), one plane, tightly packed; or dma-bufs of so many planes.
fn buffers_param(width: u32, height: u32, kind: Kind) -> Option<Vec<u8>> {
    let stride = width as i32 * 4;
    let (blocks, data_type) = match kind {
        Kind::Shm => (1, spa::sys::SPA_DATA_MemFd),
        Kind::Dmabuf { planes, .. } => (planes as i32, spa::sys::SPA_DATA_DmaBuf),
    };
    serialize(Object {
        type_: SpaTypes::ObjectParamBuffers.as_raw(),
        id: ParamType::Buffers.as_raw(),
        properties: vec![
            Property::new(
                spa::sys::SPA_PARAM_BUFFERS_buffers,
                Value::Choice(ChoiceValue::Int(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Range {
                        default: BUFFERS,
                        min: 2,
                        max: 16,
                    },
                ))),
            ),
            Property::new(spa::sys::SPA_PARAM_BUFFERS_blocks, Value::Int(blocks)),
            Property::new(
                spa::sys::SPA_PARAM_BUFFERS_size,
                Value::Int(stride * height as i32),
            ),
            Property::new(spa::sys::SPA_PARAM_BUFFERS_stride, Value::Int(stride)),
            Property::new(
                spa::sys::SPA_PARAM_BUFFERS_dataType,
                Value::Choice(ChoiceValue::Int(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Flags {
                        default: 1 << data_type,
                        flags: vec![],
                    },
                ))),
            ),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_params_serialize() {
        for modifiers in [
            Modifiers::None,
            Modifiers::Offer(&[0, 1, 2]),
            Modifiers::Fixed(1),
        ] {
            let format = format_param((1920, 1080).into(), 60, modifiers).unwrap();
            let pod = Pod::from_bytes(&format).unwrap();
            let expected = match modifiers {
                Modifiers::None => None,
                Modifiers::Offer(all) => Some((false, all.to_vec())),
                Modifiers::Fixed(one) => Some((true, vec![one])),
            };
            assert_eq!(crate::screencast_buffers::format_modifiers(pod), expected);
        }
        let buffers = buffers_param(1920, 1080, Kind::Shm).unwrap();
        assert!(Pod::from_bytes(&buffers).is_some());
        let meta = meta_param(spa::sys::SPA_META_Cursor, cursor_meta_size()).unwrap();
        assert!(Pod::from_bytes(&meta).is_some());
    }
}
