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
//! waits in `ScreenCasts::waiting` until then.

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
    output::{Output, WeakOutput},
    reexports::calloop::{
        channel::Event, generic::Generic, Interest, Mode, PostAction, RegistrationToken,
    },
    utils::{Physical, Scale, Size, Transform},
};
use tracing::{debug, info, warn};

use crate::{
    capture::{CopyError, ReadBack, Source},
    cursor::CursorPixels,
    portal::{self, CursorMode, Refused, Reply, Request},
    render::{self, PilaRenderElement},
    scale::output_scale,
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
    portal: Option<zbus::Connection>,
    /// Requests to share waiting for the picker, first come first asked:
    /// it shows one at a time (`share_picker.rs`).
    pub(crate) waiting: std::collections::VecDeque<Share>,
    next_id: u64,
}

impl ScreenCasts {
    /// Whether a cast is streaming: frames go out.
    pub fn streaming(&self) -> bool {
        self.casts.iter().any(|cast| cast.streaming)
    }

    /// Whether there's a cast, streaming or not yet: captures' offscreen
    /// textures are kept (one is made with the cast, `start_cast`).
    pub fn active(&self) -> bool {
        !self.casts.is_empty()
    }
}

/// A request to share, from the portal's `Start`.
pub struct Share {
    /// The portal session, by its object path.
    pub session: String,
    /// How the pointer shows in the stream.
    pub cursor: CursorMode,
    /// Who asks (`portal::Request::Start`).
    pub app: Option<String>,
    pub reply: Reply<Result<Vec<portal::Stream>, Refused>>,
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
}

/// What a callback saw.
enum CastEvent {
    State(StreamState),
    /// The stream's buffers changed: copies in flight into the old ones
    /// are dropped, and the next frame is whole.
    Buffers,
    /// The connection to PipeWire failed.
    Disconnected(String),
}

/// A started session's stream.
struct Cast {
    id: u64,
    /// The portal session, by its object path.
    session: String,
    output: WeakOutput,
    cursor: CursorMode,
    /// The cursor metadata last sent (`CursorMode::Metadata`): where, and
    /// which image.
    cursor_sent: Option<CursorFrame>,
    /// What the stream was offered: the output's size and scale then.
    size: Size<i32, Physical>,
    scale: Scale<f64>,
    /// Dropped before `stream`.
    _listener: StreamListener<()>,
    stream: StreamRc,
    /// `Start`'s answer, once the node has an id.
    reply: Option<Reply<Result<Vec<portal::Stream>, Refused>>>,
    position: (i32, i32),
    logical_size: (i32, i32),
    /// A consumer is linked: frames go out.
    streaming: bool,
    /// Changes since the last frame sent; `None` before the first.
    tracker: Option<OutputDamageTracker>,
    /// Bumped when the stream's buffers change and when the cast ends: a
    /// copy in flight into one of the old ones is dropped.
    buffers: Rc<Cell<u64>>,
    /// Ended by Pila (its output went away, an error): told to the app.
    ended: bool,
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
                self.startup_step_done();
            }
            Request::Failed(err) => {
                warn!(err, "portal: can't serve the session bus");
                self.startup_step_done();
            }
            Request::Start {
                session,
                cursor,
                app,
                reply,
            } => {
                self.screencast.waiting.push_back(Share {
                    session,
                    cursor,
                    app,
                    reply,
                });
                self.show_share_picker();
            }
            Request::Close { session } => {
                debug!(session, "portal: session closed");
                self.screencast
                    .waiting
                    .retain(|share| share.session != session);
                self.close_share_picker_of(&session);
                self.screencast.casts.retain(|cast| cast.session != session);
                self.disconnect_pipewire_if_idle();
            }
        }
    }

    /// Start `share`'s stream of `output`, picked by the user
    /// (`share_picker.rs`), answered once its node has an id.
    pub(crate) fn start_cast(&mut self, share: Share, output: Output) {
        let Share {
            session,
            cursor,
            reply,
            ..
        } = share;
        let (Some(geo), Some(mode)) = (self.space.output_geometry(&output), output.current_mode())
        else {
            return reply.send(Err(Refused::Cancelled));
        };
        let size = output.current_transform().transform_size(mode.size);
        let scale = output_scale(&output);
        let fps = ((mode.refresh + 500) / 1000).max(1) as u32;
        if let Err(err) = self.connect_pipewire() {
            warn!(err, "portal: can't connect to PipeWire");
            return reply.send(Err(Refused::Failed));
        }
        let Some(pipewire) = &self.screencast.pipewire else {
            return reply.send(Err(Refused::Failed));
        };
        let id = self.screencast.next_id;
        self.screencast.next_id += 1;
        let stream = match new_stream(pipewire, id, size, fps, cursor) {
            Ok(stream) => stream,
            Err(err) => {
                warn!(err, "portal: can't make a stream");
                return reply.send(Err(Refused::Failed));
            }
        };
        let (stream, listener) = stream;
        // The first frame's texture now, while the app links to the stream.
        if let Some(mut renderer) = self.backend.as_mut().and_then(|b| b.renderer()) {
            self.capture.prepare_read_back(&mut renderer, size);
        }
        info!(
            session,
            output = output.name(),
            ?size,
            fps,
            "portal: sharing an output"
        );
        self.screencast.casts.push(Cast {
            id,
            session,
            output: output.downgrade(),
            cursor,
            cursor_sent: None,
            size,
            scale,
            _listener: listener,
            stream,
            reply: Some(reply),
            position: (geo.loc.x, geo.loc.y),
            logical_size: (geo.size.w, geo.size.h),
            streaming: false,
            tracker: None,
            buffers: Rc::new(Cell::new(0)),
            ended: false,
        });
        // Callbacks may have run while connecting.
        self.handle_cast_events();
    }

    /// Connect to PipeWire if not yet, its loop a source of the event loop.
    fn connect_pipewire(&mut self) -> Result<(), String> {
        if self.screencast.pipewire.is_some() {
            return Ok(());
        }
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
        });
        info!("portal: connected to PipeWire");
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
        for (id, event) in events {
            if let CastEvent::Disconnected(err) = event {
                warn!(err, "portal: PipeWire connection lost");
                for cast in &mut self.screencast.casts {
                    cast.ended = true;
                }
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
                        reply.send(Ok(vec![portal::Stream {
                            node_id,
                            position: cast.position,
                            size: cast.logical_size,
                        }]));
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
                CastEvent::Disconnected(_) => {}
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
            if let Some(connection) = &self.screencast.portal {
                portal::end_session(connection, cast.session.clone());
            }
        }
        self.disconnect_pipewire_if_idle();
    }

    /// With no cast left, PipeWire's connection goes (it's made again at
    /// the next), and the offscreen textures if no capture keeps them.
    fn disconnect_pipewire_if_idle(&mut self) {
        if !self.screencast.casts.is_empty() {
            return;
        }
        // Its texture goes now, not at the next frame drawn (a still
        // screen has none).
        self.capture.trim_textures(false);
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
        let Some(output) = cast.output.upgrade() else {
            cast.ended = true;
            return;
        };
        let (Some(geo), Some(mode)) = (self.space.output_geometry(&output), output.current_mode())
        else {
            cast.ended = true;
            return;
        };
        let size = output.current_transform().transform_size(mode.size);
        let scale = output_scale(&output);
        if (size, scale) != (cast.size, cast.scale) {
            // Prototype: a new size ends the share instead of renegotiating.
            info!(
                session = cast.session,
                "portal: the shared output changed size"
            );
            cast.ended = true;
            return;
        }
        let input = &*input.get_or_insert_with(|| self.frame_input());
        let clear = render::clear_color(&input.scene);
        let cursor_frame = (cast.cursor == CursorMode::Metadata)
            .then(|| self.cursor_frame(input.pointer, geo.loc, scale));
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
        if cast.cursor == CursorMode::Embedded {
            elements = render::pointer_elements(renderer, input, cursor_theme, geo.loc, scale);
        }
        elements.extend(render::output_elements(
            renderer,
            blur,
            &input.scene,
            &output.name(),
            geo,
            geo.loc,
            scale,
        ));

        let tracker = cast
            .tracker
            .get_or_insert_with(|| OutputDamageTracker::new(size, scale, Transform::Normal));
        let changed = match tracker.damage_output(1, &elements) {
            Ok((Some(damage), _)) => !damage.is_empty(),
            Ok(_) => false,
            Err(err) => {
                warn!(?err, "portal: damage tracking failed");
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
        let copied = capture.read_back(
            renderer,
            size,
            scale,
            source,
            Fourcc::Argb8888,
            move |pixels| {
                if buffers.get() != generation {
                    return Ok(());
                }
                // SAFETY: the buffer is still the stream's (same
                // generation) and dequeued, so ours to write.
                unsafe { fill(buffer, pixels, w, h) }
            },
        );
        let stream = cast.stream.clone();
        let buffers = cast.buffers.clone();
        let queue = move |filled: bool| {
            if buffers.get() != generation {
                return;
            }
            if !filled {
                // SAFETY: as above.
                unsafe { mark_empty(buffer) };
            }
            // SAFETY: dequeued from this stream, whose buffers are the same.
            unsafe { stream.queue_raw_buffer(buffer) };
        };
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
        let spa_buffer = (*buffer).buffer;
        if spa_buffer.is_null() || (*spa_buffer).n_datas == 0 {
            return;
        }
        if let Some(chunk) = (*(*spa_buffer).datas).chunk.as_mut() {
            chunk.size = 0;
            chunk.flags = 0;
        }
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
        if spa_buffer.is_null() || (*spa_buffer).n_datas == 0 {
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
        if spa_buffer.is_null() || (*spa_buffer).n_datas == 0 {
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
            let size = info.size();
            let Some(bytes) = buffers_param(size.width, size.height) else {
                return;
            };
            let Some(pod) = Pod::from_bytes(&bytes) else {
                return;
            };
            // The pointer beside the frames, for the app to draw.
            let cursor_meta = (cursor == CursorMode::Metadata)
                .then(|| meta_param(spa::sys::SPA_META_Cursor, cursor_meta_size()))
                .flatten();
            let mut params = vec![pod];
            if let Some(meta) = cursor_meta.as_deref().and_then(Pod::from_bytes) {
                params.push(meta);
            }
            if let Err(err) = stream.update_params(&mut params) {
                warn!(%err, "portal: can't set the stream's buffers");
            }
        })
        .remove_buffer(move |_, _, _| {
            events.borrow_mut().push((id, CastEvent::Buffers));
        })
        .register()
        .map_err(|e| e.to_string())?;
    let format = format_param(size, fps).ok_or("can't describe the format")?;
    let format = Pod::from_bytes(&format).ok_or("can't describe the format")?;
    stream
        .connect(
            spa::utils::Direction::Output,
            None,
            StreamFlags::DRIVER | StreamFlags::MAP_BUFFERS,
            &mut [format],
        )
        .map_err(|e| e.to_string())?;
    Ok((stream, listener))
}

fn serialize(object: Object) -> Option<Vec<u8>> {
    PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(object))
        .ok()
        .map(|(cursor, _)| cursor.into_inner())
}

/// The format offered: raw video, BGRx (XRGB8888, as GL reads it back), of
/// the output's `size`, at a variable rate of at most `fps`.
fn format_param(size: Size<i32, Physical>, fps: u32) -> Option<Vec<u8>> {
    let id = |raw: u32| Value::Id(spa::utils::Id(raw));
    serialize(Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: ParamType::EnumFormat.as_raw(),
        properties: vec![
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
        ],
    })
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

/// The buffers a stream of `width` by `height` BGRx frames wants: shm
/// (`MemFd`), one plane, tightly packed.
fn buffers_param(width: u32, height: u32) -> Option<Vec<u8>> {
    let stride = width as i32 * 4;
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
            Property::new(spa::sys::SPA_PARAM_BUFFERS_blocks, Value::Int(1)),
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
                        default: 1 << spa::sys::SPA_DATA_MemFd,
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
        let format = format_param((1920, 1080).into(), 60).unwrap();
        assert!(Pod::from_bytes(&format).is_some());
        let buffers = buffers_param(1920, 1080).unwrap();
        assert!(Pod::from_bytes(&buffers).is_some());
        let meta = meta_param(spa::sys::SPA_META_Cursor, cursor_meta_size()).unwrap();
        assert!(Pod::from_bytes(&meta).is_some());
    }
}
