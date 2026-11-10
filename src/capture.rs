//! Screen capture: ext-image-capture-source-v1 (what to capture: an output
//! or a window) and ext-image-copy-capture-v1 (copying it into client
//! buffers). Screenshot tools (grim) and screen sharing
//! (xdg-desktop-portal-wlr, for browsers and OBS through PipeWire) use them.
//! Smithay doesn't implement either, so this is built on the bindings from
//! `wayland-protocols`.
//!
//! A session advertises its buffer constraints (the source's size in
//! physical pixels and the shm formats), then copies one frame per `capture`.
//! The first frame of a session is copied right away; later ones wait until
//! the source changes: after every event-loop iteration that drew a frame
//! (`Pila::after_frames`) pending captures are checked against a per-session damage tracker, and
//! only copied (with that damage) once something changed. A source that
//! changes size re-sends the constraints and fails the pending frame; a
//! source that goes away stops the session.
//!
//! Frames are rendered offscreen from the same scene the backends draw
//! (`render.rs`), at the source's scale: an output at its own scale, a window
//! at the scale of the output it's drawn for (`Pila::window_output`), or off
//! every output at the scale its client was last told, so a share keeps its
//! size when the window is minimized. A window is captured without pila's
//! title bar. While the session is locked, outputs capture the lock screen
//! and window captures wait. With `paint_cursors`, the cursor is included
//! (nested, only client-drawn cursors: the host draws named ones).
//!
//! Buffers may be shm (read back from an offscreen texture, rows reversed:
//! GL reads bottom-up) or dma-buf on the renderer's device (rendered into
//! directly; what xdg-desktop-portal-wlr uses for PipeWire). Cursor
//! sessions are created stopped. Any client but a sandboxed one
//! (`state::is_privileged`) may capture, as with other compositors.

use crate::sync::Locked;
use std::{
    cell::RefCell,
    collections::HashMap,
    os::fd::OwnedFd,
    rc::Rc,
    sync::Mutex,
    time::{Duration, Instant},
};

use smithay::{
    backend::{
        allocator::{Buffer as _, Fourcc},
        egl::fence::EGLFence,
        renderer::{
            buffer_type,
            damage::OutputDamageTracker,
            element::AsRenderElements,
            gles::{GlesMapping, GlesRenderer, GlesTarget, GlesTexture},
            sync::SyncPoint,
            Bind, BufferType, ExportMem, Offscreen, Texture, TextureMapping,
        },
    },
    desktop::{utils::surface_primary_scanout_output, Window},
    output::{Output, WeakOutput},
    reexports::{
        calloop::{generic::Generic, Interest, Mode, PostAction},
        wayland_protocols::ext::{
            image_capture_source::v1::server::{
                ext_foreign_toplevel_image_capture_source_manager_v1::{
                    self as toplevel_source_manager, ExtForeignToplevelImageCaptureSourceManagerV1,
                },
                ext_image_capture_source_v1::{self as source, ExtImageCaptureSourceV1},
                ext_output_image_capture_source_manager_v1::{
                    self as output_source_manager, ExtOutputImageCaptureSourceManagerV1,
                },
            },
            image_copy_capture::v1::server::{
                ext_image_copy_capture_cursor_session_v1::{
                    self as cursor_session, ExtImageCopyCaptureCursorSessionV1,
                },
                ext_image_copy_capture_frame_v1::{
                    self as frame, ExtImageCopyCaptureFrameV1, FailureReason,
                },
                ext_image_copy_capture_manager_v1::{
                    self as manager, ExtImageCopyCaptureManagerV1,
                },
                ext_image_copy_capture_session_v1::{
                    self as session, ExtImageCopyCaptureSessionV1,
                },
            },
        },
        wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
        wayland_server::{
            backend::ClientId,
            protocol::{wl_buffer::WlBuffer, wl_output, wl_shm},
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
        },
    },
    utils::{Monotonic, Physical, Point, Rectangle, Scale, Size, Time, Transform},
    wayland::{
        compositor::with_states,
        dmabuf::get_dmabuf,
        foreign_toplevel_list::ForeignToplevelHandle,
        fractional_scale::with_fractional_scale,
        seat::WaylandFocus,
        shm::{with_buffer_contents, with_buffer_contents_mut},
    },
};
use tracing::{debug, trace, warn};

use crate::{
    render::{self, PilaRenderElement},
    scale::output_scale,
    Pila,
};

/// The shm formats offered. Rendering reads back RGBA, swizzled per format.
pub(crate) const SHM_FORMATS: [wl_shm::Format; 4] = [
    wl_shm::Format::Argb8888,
    wl_shm::Format::Xrgb8888,
    wl_shm::Format::Abgr8888,
    wl_shm::Format::Xbgr8888,
];

/// The dma-buf formats offered: what clients (PipeWire consumers) expect.
pub(crate) const DMABUF_FORMATS: [Fourcc; 2] = [Fourcc::Argb8888, Fourcc::Xrgb8888];

/// Where and how clients may allocate dma-bufs for captures: the renderer's
/// DRM device and the modifiers it can render to, per format. Set by the
/// backend (`Pila::capture.dmabuf`); without it only shm is offered.
pub struct DmabufTarget {
    device: u64,
    formats: Vec<(Fourcc, Vec<u64>)>,
}

impl DmabufTarget {
    pub fn new(device: u64, renderer: &GlesRenderer) -> Self {
        let render_formats = renderer.egl_context().dmabuf_render_formats();
        let formats = DMABUF_FORMATS
            .iter()
            .map(|&code| {
                let modifiers = render_formats
                    .iter()
                    .filter(|f| f.code == code)
                    .map(|f| u64::from(f.modifier))
                    .collect::<Vec<_>>();
                (code, modifiers)
            })
            .filter(|(_, modifiers)| !modifiers.is_empty())
            .collect();
        Self { device, formats }
    }

    /// Whether any dma-buf format is offered.
    pub(crate) fn offers(&self) -> bool {
        !self.formats.is_empty()
    }
}

/// Behind a captured window's (possibly translucent) surfaces.
const TRANSPARENT: [f32; 4] = [0.0, 0.0, 0.0, 0.0];

/// What an `ext_image_capture_source_v1` captures.
#[derive(Debug, Clone)]
pub enum Target {
    Output(WeakOutput),
    /// A window, by its foreign-toplevel identifier (`foreign_toplevel.rs`).
    Toplevel(String),
    /// The output or window was already gone when the source was created.
    Gone,
}

/// Screen capture state (`Pila::capture`).
#[derive(Default)]
pub struct Captures {
    /// ext-image-copy-capture sessions.
    pub sessions: Vec<Session>,
    /// Where captures may use dma-bufs (set by the backend).
    pub dmabuf: Option<DmabufTarget>,
    /// wlr-screencopy frames waiting to be copied, and the damage trackers
    /// of `copy_with_damage` (`screencopy.rs`).
    pub screencopies: Vec<ZwlrScreencopyFrameV1>,
    pub screencopy_trackers: HashMap<crate::screencopy::TrackerKey, OutputDamageTracker>,
    /// When the frame being processed started to be built (its scene and
    /// elements, then the copy if it changed), for `Pila::record_capture`;
    /// `None` if it went no further than its checks.
    pub building: Option<Instant>,
    /// Offscreen textures shm copies were rendered into, by size, for the
    /// next ones (`MAX_SHM_TEXTURES`): kept while captures go on, dropped
    /// once none does (`trim_textures`), since a 4K one is 33 MB.
    shm_textures: Vec<GlesTexture>,
}

/// A pending output capture to fill from the frame its output just
/// composited (`Captures::take_composited`), then hand to
/// `Pila::finish_composited`.
pub(crate) struct Composited {
    pub buffer: WlBuffer,
    pub size: Size<i32, Physical>,
    pub scale: Scale<f64>,
    resource: ExtImageCopyCaptureSessionV1,
    frame: ExtImageCopyCaptureFrameV1,
    damage: Vec<Rectangle<i32, Physical>>,
}

/// How many sizes of offscreen textures `Captures` keeps: an output and a
/// window or two captured at once.
const MAX_SHM_TEXTURES: usize = 4;

impl Captures {
    /// Drop the offscreen textures when no capture may want them: no
    /// session, no screencopy waiting, no screencopy client keeping damage.
    pub fn trim_textures(&mut self) {
        if self.sessions.is_empty()
            && self.screencopies.is_empty()
            && self.screencopy_trackers.is_empty()
        {
            self.shm_textures.clear();
        }
    }

    /// The pending captures of `output` that the frame just composited for
    /// it from `elements` can fill, by a blit instead of drawing the scene
    /// again (`udev/draw.rs`), each with the damage its session's tracker
    /// sees in `elements`. Only those that would get the same picture: of
    /// an output that isn't turned (its scanout buffer is upright), with the
    /// cursor as that frame has it (painted, or not on this output), at the
    /// size and scale advertised. The rest are left to `process_captures`,
    /// and so are those without a change, once checked here.
    pub(crate) fn take_composited(
        &mut self,
        output: &Output,
        elements: &[PilaRenderElement],
        pointer_here: bool,
    ) -> Vec<Composited> {
        if output.current_transform() != Transform::Normal {
            return Vec::new();
        }
        let Some(mode) = output.current_mode() else {
            return Vec::new();
        };
        let geometry = (mode.size, output_scale(output));
        let mut jobs = Vec::new();
        for session in &mut self.sessions {
            let ours = matches!(&session.target, Target::Output(o) if o.upgrade().as_ref() == Some(output));
            if !ours
                || !session.pending
                || session.stopped
                || session.geometry != Some(geometry)
                || (pointer_here && !session.paint_cursors)
            {
                continue;
            }
            let Some(frame) = session.frame.clone() else {
                continue;
            };
            let buffer = frame
                .data::<FrameData>()
                .and_then(|data| data.state.locked().buffer.clone());
            let Some(buffer) = buffer else {
                continue;
            };
            let (size, scale) = geometry;
            let tracker = session
                .tracker
                .get_or_insert_with(|| OutputDamageTracker::new(size, scale, Transform::Normal));
            let damage = match tracker.damage_output(1, elements) {
                Ok((damage, _)) => Some(damage.cloned().unwrap_or_default()),
                Err(err) => {
                    warn!(?err, "capture: damage tracking failed");
                    None
                }
            };
            session.composited = true;
            let damage = match damage {
                Some(damage) if session.captured_once => {
                    if damage.is_empty() {
                        continue;
                    }
                    damage
                }
                Some(_) => vec![Rectangle::from_size(size)],
                None => {
                    session.tracker = None;
                    vec![Rectangle::from_size(size)]
                }
            };
            jobs.push(Composited {
                buffer,
                size,
                scale,
                resource: session.resource.clone(),
                frame,
                damage,
            });
        }
        jobs
    }

    /// Offscreen textures kept, for tests (`Pila::state_json`).
    pub fn texture_count(&self) -> usize {
        self.shm_textures.len()
    }
}

/// A capture session (`Captures::sessions`).
pub struct Session {
    resource: ExtImageCopyCaptureSessionV1,
    target: Target,
    paint_cursors: bool,
    /// The size and scale last advertised to the client.
    geometry: Option<(Size<i32, Physical>, Scale<f64>)>,
    /// Damage since the last copied frame.
    tracker: Option<OutputDamageTracker>,
    /// The session's frame object, if the client has one.
    frame: Option<ExtImageCopyCaptureFrameV1>,
    /// `frame` asked to be captured and hasn't been yet.
    pending: bool,
    captured_once: bool,
    stopped: bool,
    /// The frame its output just composited filled or checked its pending
    /// frame in this event-loop iteration (`Captures::take_composited`):
    /// `process_captures` leaves it.
    composited: bool,
}

/// A frame's buffer, damage and whether `capture` was sent.
#[derive(Default)]
pub struct FrameState {
    buffer: Option<WlBuffer>,
    captured: bool,
}

/// User data of a frame: its session and state.
pub struct FrameData {
    session: ExtImageCopyCaptureSessionV1,
    state: Mutex<FrameState>,
}

/// Create the globals. Called once from `Pila::new`.
pub fn init_globals(dh: &DisplayHandle) {
    dh.create_global::<Pila, ExtOutputImageCaptureSourceManagerV1, ()>(1, ());
    dh.create_global::<Pila, ExtForeignToplevelImageCaptureSourceManagerV1, ()>(1, ());
    dh.create_global::<Pila, ExtImageCopyCaptureManagerV1, ()>(1, ());
}

impl Session {
    fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        self.resource.stopped();
        if self.pending {
            self.fail(FailureReason::Stopped);
        }
    }

    /// A copy failed after the damage tracker saw its changes: the next
    /// frame is copied at once and whole, as a session's first is, or a
    /// still source would never send it.
    fn start_over(&mut self) {
        self.tracker = None;
        self.captured_once = false;
    }

    fn fail(&mut self, reason: FailureReason) {
        self.pending = false;
        if let Some(frame) = &self.frame {
            frame.failed(reason);
        }
    }

    /// Advertise the buffer constraints for `geometry`.
    fn send_constraints(
        &mut self,
        geometry: (Size<i32, Physical>, Scale<f64>),
        dmabuf: Option<&DmabufTarget>,
    ) {
        let size = geometry.0;
        self.resource.buffer_size(size.w as u32, size.h as u32);
        for format in SHM_FORMATS {
            self.resource.shm_format(format);
        }
        if let Some(dmabuf) = dmabuf.filter(|d| !d.formats.is_empty()) {
            self.resource
                .dmabuf_device(dmabuf.device.to_ne_bytes().to_vec());
            for (code, modifiers) in &dmabuf.formats {
                let modifiers = modifiers.iter().flat_map(|m| m.to_ne_bytes()).collect();
                self.resource.dmabuf_format(*code as u32, modifiers);
            }
        }
        self.resource.done();
        self.geometry = Some(geometry);
        self.tracker = None;
    }
}

impl Pila {
    /// The size (physical pixels) and scale `target` is captured at, or
    /// `None` if it's gone. `last`: the geometry advertised before, if any.
    fn capture_geometry(
        &self,
        target: &Target,
        last: Option<(Size<i32, Physical>, Scale<f64>)>,
    ) -> Option<(Size<i32, Physical>, Scale<f64>)> {
        match target {
            Target::Output(output) => {
                let output = output.upgrade()?;
                // Not mapped any more: unplugged.
                self.space.output_geometry(&output)?;
                // In the output's orientation: a monitor turned a quarter is
                // captured upright, taller than wide.
                let size = output
                    .current_transform()
                    .transform_size(output.current_mode()?.size);
                Some((size, output_scale(&output)))
            }
            Target::Toplevel(identifier) => {
                let window = self.window_for_identifier(identifier)?;
                let scale = self.window_capture_scale(&window, last.map(|(_, scale)| scale));
                let size = window
                    .geometry()
                    .size
                    .to_f64()
                    .to_physical(scale)
                    .to_i32_round();
                Some((size, scale))
            }
            Target::Gone => None,
        }
    }

    /// The scale a window is captured at: its output's (`window_output`, the
    /// one it's drawn for, so a window across two outputs doesn't flip
    /// between them), else, off every output (minimized, on another
    /// workspace), the scale its client was last told and still draws at,
    /// else the one captured at before. A share keeps its size when the
    /// window leaves the screen, instead of renegotiating its buffers.
    fn window_capture_scale(&self, window: &Window, last: Option<Scale<f64>>) -> Scale<f64> {
        if let Some(output) = self.window_output(window) {
            return output_scale(&output);
        }
        let told = window.wl_surface().and_then(|surface| {
            with_states(&surface, |states| {
                with_fractional_scale(states, |fractional| fractional.preferred_scale())
            })
        });
        told.map(Scale::from)
            .or(last)
            .unwrap_or_else(|| Scale::from(1.0))
    }

    /// Copy every pending frame whose source changed (or that is a
    /// session's first). Called after each rendered frame and when a first
    /// capture is requested.
    pub fn process_captures(&mut self) {
        if !self.capture.sessions.iter().any(|s| s.pending) {
            return;
        }
        let mut sessions = std::mem::take(&mut self.capture.sessions);
        for session in sessions.iter_mut() {
            if std::mem::take(&mut session.composited) || !session.pending {
                continue;
            }
            self.process_capture(session);
            if let Some(started) = self.capture.building.take() {
                self.record_capture(started.elapsed());
            }
        }
        // Sessions created meanwhile (none expected) are kept too.
        sessions.append(&mut self.capture.sessions);
        self.capture.sessions = sessions;
    }

    fn process_capture(&mut self, session: &mut Session) {
        let Some(geometry) = self.capture_geometry(&session.target, session.geometry) else {
            session.stop();
            return;
        };
        if session.geometry != Some(geometry) {
            session.send_constraints(geometry, self.capture.dmabuf.as_ref());
            session.fail(FailureReason::BufferConstraints);
            return;
        }
        let (size, scale) = geometry;
        if size.w <= 0 || size.h <= 0 {
            // A window that hasn't got a size yet: wait for one.
            return;
        }
        // Window contents stay private while the screen is locked.
        if self.is_locked() && matches!(session.target, Target::Toplevel(_)) {
            return;
        }
        let Some(frame) = session.frame.clone() else {
            session.pending = false;
            return;
        };
        let Some(frame_data) = frame.data::<FrameData>() else {
            return;
        };
        let Some(buffer) = frame_data.state.locked().buffer.clone() else {
            return;
        };

        // Everything taken from `self` before borrowing the renderer.
        self.capture.building = Some(Instant::now());
        let input = self.frame_input();
        let scene = &input.scene;
        let (origin, window) = match &session.target {
            Target::Output(output) => {
                let output = output.upgrade();
                let geo = output.and_then(|o| self.space.output_geometry(&o));
                (geo.map(|g| g.loc).unwrap_or_default(), None)
            }
            Target::Toplevel(identifier) => {
                let window = self.window_for_identifier(identifier);
                let loc = window
                    .as_ref()
                    .and_then(|w| self.space.element_location(w))
                    .unwrap_or_default();
                (loc, window)
            }
            Target::Gone => return,
        };
        // Where the frame's content shows, for its presentation time.
        let shown_on = match &session.target {
            Target::Output(output) => output.upgrade(),
            _ => window.as_ref().and_then(|w| self.window_output(w)),
        };
        let clear = match window {
            Some(_) => TRANSPARENT,
            None => render::clear_color(scene),
        };

        let output_for_blur = match &session.target {
            Target::Output(output) => output
                .upgrade()
                .and_then(|o| self.space.output_geometry(&o).map(|geo| (o.name(), geo))),
            _ => None,
        };
        let Pila {
            backend,
            blur,
            capture,
            ..
        } = self;
        let Some(backend) = backend.as_mut() else {
            return;
        };
        let (renderer, cursor_theme) = backend.renderer_and_cursor_theme();

        let mut elements: Vec<PilaRenderElement> = Vec::new();
        if session.paint_cursors {
            elements = render::pointer_elements(renderer, &input, cursor_theme, origin, scale);
        }
        match (&window, &output_for_blur) {
            (Some(window), _) => {
                // The buffer's origin is the window geometry's top-left.
                let loc: Point<i32, Physical> =
                    (Point::default() - window.geometry().loc).to_physical_precise_round(scale);
                elements.extend(AsRenderElements::<GlesRenderer>::render_elements::<
                    PilaRenderElement,
                >(window, renderer, loc, scale, 1.0));
            }
            // Blurred backdrops are part of the screen; they're prepared
            // for this output as the backends do.
            (None, Some((name, geo))) => elements.extend(render::output_elements(
                renderer, blur, scene, name, *geo, origin, scale,
            )),
            // The output went away since its geometry was checked.
            (None, None) => return,
        }

        // Changes since the last copied frame; later frames wait for one.
        let tracker = session
            .tracker
            .get_or_insert_with(|| OutputDamageTracker::new(size, scale, Transform::Normal));
        let damage: Option<Vec<Rectangle<i32, Physical>>> =
            match tracker.damage_output(1, &elements) {
                Ok((Some(damage), _)) => Some(damage.clone()),
                Ok((None, _)) => Some(Vec::new()),
                Err(err) => {
                    warn!(?err, "capture: damage tracking failed");
                    None
                }
            };
        let damage = match damage {
            Some(damage) if session.captured_once => {
                if damage.is_empty() {
                    return;
                }
                damage
            }
            Some(_) => vec![Rectangle::from_size(size)],
            // What changed is unknown: all of it, with a new tracker.
            None => {
                session.tracker = None;
                vec![Rectangle::from_size(size)]
            }
        };

        let source = Source::Elements {
            elements: &elements,
            clear,
        };
        let copied = match copy_frame(renderer, &buffer, capture, size, scale, source) {
            Ok(copied) => copied,
            Err(err) => {
                session.start_over();
                session.fail(copy_failure(err));
                return;
            }
        };
        session.pending = false;
        session.captured_once = true;
        let resource = session.resource.clone();
        self.send_copied(resource, frame, damage, copied, shown_on.as_ref());
    }

    /// Send a copied frame's events once the copy is complete: its damage,
    /// presentation time and `ready`; or, if the copy failed on the GPU,
    /// `failed`, starting its session over.
    fn send_copied(
        &mut self,
        resource: ExtImageCopyCaptureSessionV1,
        frame: ExtImageCopyCaptureFrameV1,
        damage: Vec<Rectangle<i32, Physical>>,
        copied: Copied,
        shown_on: Option<&Output>,
    ) {
        let (hi, lo, nanos) = split_timestamp(self.presented_at(shown_on).into());
        let wanted = frame.clone();
        self.when_copied(
            copied,
            move || wanted.is_alive(),
            move |state, result| {
                if let Err(err) = result {
                    let sessions = &mut state.capture.sessions;
                    if let Some(session) = sessions.iter_mut().find(|s| s.resource == resource) {
                        session.start_over();
                    }
                    frame.failed(copy_failure(err));
                    return;
                }
                frame.transform(wl_output::Transform::Normal);
                for rect in damage {
                    frame.damage(rect.loc.x, rect.loc.y, rect.size.w, rect.size.h);
                }
                frame.presentation_time(hi, lo, nanos);
                frame.ready();
            },
        );
    }

    /// A capture `take_composited` gave for `output` was copied (or not).
    pub(crate) fn finish_composited(
        &mut self,
        job: Composited,
        copied: Result<Copied, CopyError>,
        output: &Output,
    ) {
        let Some(session) = self
            .capture
            .sessions
            .iter_mut()
            .find(|s| s.resource == job.resource)
        else {
            return;
        };
        session.composited = false;
        let copied = match copied {
            Ok(copied) => copied,
            Err(err) => {
                session.start_over();
                session.fail(copy_failure(err));
                return;
            }
        };
        session.pending = false;
        session.captured_once = true;
        self.send_copied(job.resource, job.frame, job.damage, copied, Some(output));
    }

    /// When a frame copied now shows on `output`: copies follow the frame
    /// just queued there (`after_frames`), which shows at the next refresh
    /// (`Backend::next_frame_time`: the next vblank on bare metal, one period
    /// on nested). Shown on no output (a window off the screen): now. What
    /// captures send as the frame's presentation time, which recorders use
    /// to keep sound and picture in step.
    pub(crate) fn presented_at(&self, output: Option<&Output>) -> Time<Monotonic> {
        let now = self.clock.now();
        match (output, &self.backend) {
            (Some(output), Some(backend)) => backend.next_frame_time(output, now),
            _ => now,
        }
    }

    /// Frame callbacks for the surfaces of a captured `window` that aren't
    /// shown on any output (covered, minimized, on another workspace), sent
    /// when the next frame is asked for: the screen paces only the ones it
    /// shows (hidden ones get one a second, off-space ones none), and a
    /// client that draws on frame callbacks would stall the share. Shown
    /// surfaces keep the screen's pace. Not frame pacing by pila: the
    /// capture client sets the rate, one window frame per captured frame.
    fn send_capture_frames(&self, window: &Window) {
        let Some(output) = self.space.outputs().next().cloned() else {
            return;
        };
        // Off the space a surface keeps the primary output it last had:
        // nothing draws it to update that.
        let shown = self.space.element_location(window).is_some();
        let time = self.clock.now();
        window.send_frame(&output, time, None, |surface, data| {
            let hidden = !shown || surface_primary_scanout_output(surface, data).is_none();
            hidden.then(|| output.clone())
        });
    }

    /// The window with foreign-toplevel `identifier` is gone: stop its
    /// sessions now, whether or not a frame is waiting.
    pub fn stop_window_captures(&mut self, identifier: &str) {
        for session in &mut self.capture.sessions {
            if matches!(&session.target, Target::Toplevel(id) if id == identifier) {
                session.stop();
            }
        }
    }

    /// Stop the sessions of an output that went away.
    pub fn stop_captures_of(&mut self, output: &Output) {
        for session in &mut self.capture.sessions {
            if matches!(&session.target, Target::Output(o) if o.upgrade().as_ref() == Some(output))
            {
                session.stop();
            }
        }
    }
}

/// A copy into a client's buffer, done or still on the GPU.
pub(crate) enum Copied {
    Done,
    /// `finish` completes it once `fence` (a native fence fd) is signaled.
    InFlight {
        fence: OwnedFd,
        finish: Finish,
    },
}

/// What completes a copy once the GPU is done with it.
pub(crate) type Finish = Box<dyn FnOnce(&mut GlesRenderer) -> Result<(), CopyError>>;

impl Pila {
    /// Run `then` once `copied` is complete: now, or when the GPU is done with
    /// it, without waiting on the event loop. A copy still on the GPU never
    /// completes within this call, so `then` may look up the sessions
    /// `process_captures` holds meanwhile. If `wanted` says the client gave
    /// up on the frame by then (destroyed it), neither the rest of the copy
    /// (writing into its shm buffer, which it may have reused) nor `then`
    /// runs.
    pub(crate) fn when_copied(
        &mut self,
        copied: Copied,
        wanted: impl Fn() -> bool + 'static,
        then: impl FnOnce(&mut Pila, Result<(), CopyError>) + 'static,
    ) {
        let (fence, finish) = match copied {
            Copied::Done => return then(self, Ok(())),
            Copied::InFlight { fence, finish } => (fence, finish),
        };
        trace!("capture: waiting for the GPU's fence");
        // Taken by whichever runs it: the source, or this if it can't be added.
        let pending = Rc::new(RefCell::new(Some((finish, wanted, then))));
        fn run<W: Fn() -> bool, T: FnOnce(&mut Pila, Result<(), CopyError>)>(
            state: &mut Pila,
            pending: &RefCell<Option<(Finish, W, T)>>,
        ) {
            let taken = pending.borrow_mut().take();
            if let Some((finish, wanted, then)) = taken {
                if !wanted() {
                    trace!("capture: the frame is gone; its copy is dropped");
                    return;
                }
                let result = match state.backend.as_mut() {
                    Some(backend) => finish(backend.renderer()),
                    None => Err(CopyError::Render("no renderer".into())),
                };
                then(state, result);
            }
        }
        let in_source = pending.clone();
        let inserted = self.loop_handle.insert_source(
            Generic::new(fence, Interest::READ, Mode::OneShot),
            move |_, _, state| {
                run(state, &in_source);
                Ok(PostAction::Remove)
            },
        );
        if let Err(err) = inserted {
            // `finish` waits for the GPU by itself; after this iteration, as
            // from the source.
            warn!(err = %err.error, "can't wait for a capture's fence");
            self.loop_handle
                .insert_idle(move |state| run(state, &pending));
            self.wake.ping();
        }
    }
}

pub(crate) enum CopyError {
    /// The buffer isn't shm, or doesn't match the size or formats.
    Constraints,
    Render(String),
}

/// A timestamp as protocols send it: seconds (high and low 32 bits) and
/// nanoseconds.
pub(crate) fn split_timestamp(time: Duration) -> (u32, u32, u32) {
    let secs = time.as_secs();
    ((secs >> 32) as u32, secs as u32, time.subsec_nanos())
}

/// Copy `elements` into the client's `buffer`: rendered straight into a
/// dma-buf, or offscreen and read back into shm.
/// A dma-buf must be of a format and modifier `capture.dmabuf` offers; shm
/// copies reuse `capture`'s offscreen textures.
pub(crate) fn copy_frame(
    renderer: &mut GlesRenderer,
    buffer: &WlBuffer,
    capture: &mut Captures,
    size: Size<i32, Physical>,
    scale: Scale<f64>,
    source: Source<'_>,
) -> Result<Copied, CopyError> {
    match buffer_type(buffer) {
        Some(BufferType::Shm) => {
            let textures = &mut capture.shm_textures;
            copy_to_shm(renderer, textures, buffer, size, scale, source)
        }
        Some(BufferType::Dma) => {
            let dmabuf = capture.dmabuf.as_ref();
            render_to_dmabuf(renderer, buffer, dmabuf, size, scale, source)
        }
        _ => Err(CopyError::Constraints),
    }
}

/// How a copy draws its frame into the buffer it fills, all of it, top row
/// first (`Transform::Normal`).
pub(crate) enum Source<'a> {
    /// Render these elements over `clear`, as the backends draw an output.
    Elements {
        elements: &'a [PilaRenderElement],
        clear: [f32; 4],
    },
    /// Some other way: the frame a DRM output just composited, blitted
    /// (`udev/draw.rs`). The error is logged.
    Draw(&'a mut dyn FnMut(&mut GlesRenderer, &mut GlesTarget<'_>) -> Result<SyncPoint, String>),
}

/// Draw `source` into `target`, of `size` at `scale`.
fn draw(
    renderer: &mut GlesRenderer,
    target: &mut GlesTarget<'_>,
    size: Size<i32, Physical>,
    scale: Scale<f64>,
    source: Source<'_>,
) -> Result<SyncPoint, CopyError> {
    match source {
        Source::Elements { elements, clear } => {
            let mut tracker = OutputDamageTracker::new(size, scale, Transform::Normal);
            tracker
                .render_output(renderer, target, 0, elements, clear)
                .map(|result| result.sync)
                .map_err(|e| render_error(&e))
        }
        Source::Draw(draw) => draw(renderer, target).map_err(CopyError::Render),
    }
}

/// Whether `target` offers dma-bufs of format `code` with `modifier`: what
/// the renderer can draw into. linux-dmabuf accepts any format it can
/// texture from, more than that, and a buffer it can't draw into would
/// fail to bind with no hint for the client to allocate another.
fn dmabuf_offered(target: Option<&DmabufTarget>, code: Fourcc, modifier: u64) -> bool {
    target.is_some_and(|target| {
        target
            .formats
            .iter()
            .any(|(c, modifiers)| *c == code && modifiers.contains(&modifier))
    })
}

/// Why a frame failed, for a copy that did, logged.
fn copy_failure(err: CopyError) -> FailureReason {
    match err {
        CopyError::Constraints => {
            debug!("capture: buffer doesn't match the constraints");
            FailureReason::BufferConstraints
        }
        CopyError::Render(err) => {
            warn!(err, "capture failed");
            FailureReason::Unknown
        }
    }
}

fn render_error(err: &dyn std::fmt::Debug) -> CopyError {
    CopyError::Render(render::error_text(err))
}

/// Draw `source` into the client's dma-buf: no copy through the CPU.
fn render_to_dmabuf(
    renderer: &mut GlesRenderer,
    buffer: &WlBuffer,
    offered: Option<&DmabufTarget>,
    size: Size<i32, Physical>,
    scale: Scale<f64>,
    source: Source<'_>,
) -> Result<Copied, CopyError> {
    let mut dmabuf = get_dmabuf(buffer)
        .map_err(|_| CopyError::Constraints)?
        .clone();
    let buffer_size = dmabuf.size();
    let format = dmabuf.format();
    if (buffer_size.w, buffer_size.h) != (size.w, size.h)
        || !dmabuf_offered(offered, format.code, u64::from(format.modifier))
    {
        return Err(CopyError::Constraints);
    }
    let mut target = renderer.bind(&mut dmabuf).map_err(|e| render_error(&e))?;
    // Drawn like a DRM output's buffer: top row first, so `Normal`.
    let sync = draw(renderer, &mut target, size, scale, source)?;
    // The client reads it as soon as it gets `ready`: once the GPU is done.
    if let Some(fence) = sync.export() {
        return Ok(Copied::InFlight {
            fence,
            finish: Box::new(|_| Ok(())),
        });
    }
    sync.wait().map_err(|e| render_error(&e))?;
    Ok(Copied::Done)
}

/// Draw `source` offscreen and copy the pixels into the shm `buffer`.
fn copy_to_shm(
    renderer: &mut GlesRenderer,
    textures: &mut Vec<GlesTexture>,
    buffer: &WlBuffer,
    size: Size<i32, Physical>,
    scale: Scale<f64>,
    source: Source<'_>,
) -> Result<Copied, CopyError> {
    let (w, h) = (size.w, size.h);
    let format = with_buffer_contents(buffer, |_, _, data| {
        (data.width == w && data.height == h && data.stride >= w * 4).then_some(data.format)
    })
    .ok()
    .flatten()
    .ok_or(CopyError::Constraints)?;
    // The client's byte order (ARGB is B, G, R, A in memory), read back
    // directly where GL can; otherwise read RGBA and swap per pixel.
    let wanted = match format {
        wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888 => Fourcc::Argb8888,
        wl_shm::Format::Abgr8888 | wl_shm::Format::Xbgr8888 => Fourcc::Abgr8888,
        _ => return Err(CopyError::Constraints),
    };

    // One of this size from an earlier copy, or a new one.
    let mut texture = match textures.iter().position(|t| t.size() == (w, h).into()) {
        Some(i) => textures.remove(i),
        None => Offscreen::<GlesTexture>::create_buffer(renderer, Fourcc::Abgr8888, (w, h).into())
            .map_err(|e| render_error(&e))?,
    };
    let mut target = renderer.bind(&mut texture).map_err(|e| render_error(&e))?;
    // The read-back below comes after the drawing in the same context: no
    // need to wait for it.
    let _ = draw(renderer, &mut target, size, scale, source)?;
    let region = Rectangle::from_size((w, h).into());
    let (mapping, swap_rb) = match renderer.copy_framebuffer(&target, region, wanted) {
        Ok(mapping) => (mapping, false),
        Err(_) => (
            renderer
                .copy_framebuffer(&target, region, Fourcc::Abgr8888)
                .map_err(|e| render_error(&e))?,
            wanted != Fourcc::Abgr8888,
        ),
    };
    drop(target);
    // The read-back above is ordered before anything drawn into it next.
    textures.push(texture);
    if textures.len() > MAX_SHM_TEXTURES {
        textures.remove(0);
    }
    // Reading the pixels waits for the GPU: with a fence to wait on, that's
    // after it signals, off the event loop's back.
    let display = renderer.egl_context().display().clone();
    let fence = renderer
        .with_context(|gl| {
            let fence = EGLFence::create(&display).ok();
            // SAFETY: a plain flush in the renderer's current context.
            unsafe { gl.Flush() };
            fence
        })
        .ok()
        .flatten()
        .and_then(|fence| fence.export().ok());
    let buffer = buffer.clone();
    let write =
        move |renderer: &mut GlesRenderer| write_shm(renderer, &mapping, &buffer, (w, h), swap_rb);
    match fence {
        Some(fence) => Ok(Copied::InFlight {
            fence,
            finish: Box::new(write),
        }),
        None => write(renderer).map(|()| Copied::Done),
    }
}

/// Write the read-back pixels of `mapping` into the shm `buffer` of `size`,
/// swapping red and blue if asked.
fn write_shm(
    renderer: &mut GlesRenderer,
    mapping: &GlesMapping,
    buffer: &WlBuffer,
    (w, h): (i32, i32),
    swap_rb: bool,
) -> Result<(), CopyError> {
    // glReadPixels returns rows bottom-up: an offscreen texture drawn with
    // `Transform::Normal` reads back upside down unless the mapping says
    // it's already flipped.
    let bottom_up = !mapping.flipped();
    let pixels = renderer
        .map_texture(mapping)
        .map_err(|e| render_error(&e))?;

    let row = (w * 4) as usize;
    if pixels.len() < row * h as usize {
        return Err(CopyError::Render("short read-back".into()));
    }
    with_buffer_contents_mut(buffer, |ptr, len, data| {
        let end = data.offset as usize + data.stride as usize * (h as usize - 1) + row;
        if end > len {
            return Err(CopyError::Constraints);
        }
        // SAFETY: the range was checked against the pool's length above;
        // the pool stays mapped for the duration of this closure.
        let dst = unsafe { std::slice::from_raw_parts_mut(ptr, len) };
        for y in 0..h as usize {
            let src_y = if bottom_up { h as usize - 1 - y } else { y };
            let src = &pixels[src_y * row..][..row];
            let start = data.offset as usize + y * data.stride as usize;
            let out = &mut dst[start..start + row];
            if swap_rb {
                for (o, s) in out
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(src.as_chunks::<4>().0)
                {
                    *o = [s[2], s[1], s[0], s[3]];
                }
            } else {
                out.copy_from_slice(src);
            }
        }
        Ok(())
    })
    .map_err(|_| CopyError::Constraints)?
}

// --- Globals ---------------------------------------------------------------

impl GlobalDispatch<ExtOutputImageCaptureSourceManagerV1, ()> for Pila {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ExtOutputImageCaptureSourceManagerV1>,
        _: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }

    crate::state::privileged_global!();
}

impl GlobalDispatch<ExtForeignToplevelImageCaptureSourceManagerV1, ()> for Pila {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ExtForeignToplevelImageCaptureSourceManagerV1>,
        _: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }

    crate::state::privileged_global!();
}

impl GlobalDispatch<ExtImageCopyCaptureManagerV1, ()> for Pila {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ExtImageCopyCaptureManagerV1>,
        _: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }

    crate::state::privileged_global!();
}

// --- Sources ---------------------------------------------------------------

impl Dispatch<ExtOutputImageCaptureSourceManagerV1, ()> for Pila {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &ExtOutputImageCaptureSourceManagerV1,
        request: output_source_manager::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            output_source_manager::Request::CreateSource { source, output } => {
                let target = Output::from_resource(&output)
                    .map(|o| Target::Output(o.downgrade()))
                    .unwrap_or(Target::Gone);
                data_init.init(source, target);
            }
            output_source_manager::Request::Destroy => {}
            _ => {}
        }
    }
}

impl Dispatch<ExtForeignToplevelImageCaptureSourceManagerV1, ()> for Pila {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &ExtForeignToplevelImageCaptureSourceManagerV1,
        request: toplevel_source_manager::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            toplevel_source_manager::Request::CreateSource {
                source,
                toplevel_handle,
            } => {
                let target = ForeignToplevelHandle::from_resource(&toplevel_handle)
                    .map(|h| Target::Toplevel(h.identifier()))
                    .unwrap_or(Target::Gone);
                data_init.init(source, target);
            }
            toplevel_source_manager::Request::Destroy => {}
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCaptureSourceV1, Target> for Pila {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &ExtImageCaptureSourceV1,
        _request: source::Request,
        _: &Target,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        // Only `destroy`; sessions keep their own copy of the target.
    }
}

// --- Sessions --------------------------------------------------------------

impl Dispatch<ExtImageCopyCaptureManagerV1, ()> for Pila {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &ExtImageCopyCaptureManagerV1,
        request: manager::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            manager::Request::CreateSession {
                session,
                source,
                options,
            } => {
                let options = match options {
                    WEnum::Value(options) => options,
                    WEnum::Unknown(bits) => {
                        resource.post_error(
                            manager::Error::InvalidOption,
                            format!("invalid options {bits:#x}"),
                        );
                        return;
                    }
                };
                let target = source.data::<Target>().cloned().unwrap_or(Target::Gone);
                let resource = data_init.init(session, ());
                let mut session = Session {
                    resource,
                    target,
                    paint_cursors: options.contains(manager::Options::PaintCursors),
                    geometry: None,
                    tracker: None,
                    frame: None,
                    pending: false,
                    captured_once: false,
                    stopped: false,
                    composited: false,
                };
                match state.capture_geometry(&session.target, None) {
                    Some(geometry) => {
                        session.send_constraints(geometry, state.capture.dmabuf.as_ref())
                    }
                    None => session.stop(),
                }
                debug!(target = ?session.target, paint_cursors = session.paint_cursors, "capture session");
                state.capture.sessions.push(session);
            }
            manager::Request::CreatePointerCursorSession { session, .. } => {
                data_init.init(session, Mutex::new(false));
            }
            manager::Request::Destroy => {}
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureCursorSessionV1, Mutex<bool>> for Pila {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &ExtImageCopyCaptureCursorSessionV1,
        request: cursor_session::Request,
        got_session: &Mutex<bool>,
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            cursor_session::Request::GetCaptureSession { session } => {
                let mut got = got_session.locked();
                if *got {
                    resource.post_error(
                        cursor_session::Error::DuplicateSession,
                        "get_capture_session sent twice",
                    );
                    return;
                }
                *got = true;
                // Cursor capture isn't supported: the session is stopped
                // from the start (clients use `paint_cursors` instead).
                let resource = data_init.init(session, ());
                let mut session = Session {
                    resource,
                    target: Target::Gone,
                    paint_cursors: false,
                    geometry: None,
                    tracker: None,
                    frame: None,
                    pending: false,
                    captured_once: false,
                    stopped: false,
                    composited: false,
                };
                session.stop();
                state.capture.sessions.push(session);
            }
            cursor_session::Request::Destroy => {}
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, ()> for Pila {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &ExtImageCopyCaptureSessionV1,
        request: session::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            session::Request::CreateFrame { frame } => {
                let Some(session) = state
                    .capture
                    .sessions
                    .iter_mut()
                    .find(|s| s.resource == *resource)
                else {
                    data_init.init(
                        frame,
                        FrameData {
                            session: resource.clone(),
                            state: Default::default(),
                        },
                    );
                    return;
                };
                if session.frame.as_ref().is_some_and(|f| f.is_alive()) {
                    resource.post_error(
                        session::Error::DuplicateFrame,
                        "create_frame before the previous frame was destroyed",
                    );
                    return;
                }
                let frame = data_init.init(
                    frame,
                    FrameData {
                        session: resource.clone(),
                        state: Default::default(),
                    },
                );
                session.frame = Some(frame);
                session.pending = false;
            }
            session::Request::Destroy => {}
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &ExtImageCopyCaptureSessionV1, _: &()) {
        state.capture.sessions.retain(|s| s.resource != *resource);
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, FrameData> for Pila {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &ExtImageCopyCaptureFrameV1,
        request: frame::Request,
        data: &FrameData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let mut frame_state = data.state.locked();
        match request {
            frame::Request::AttachBuffer { buffer } => {
                if frame_state.captured {
                    resource.post_error(frame::Error::AlreadyCaptured, "attach after capture");
                    return;
                }
                frame_state.buffer = Some(buffer);
            }
            frame::Request::DamageBuffer {
                x,
                y,
                width,
                height,
            } => {
                if frame_state.captured {
                    resource.post_error(frame::Error::AlreadyCaptured, "damage after capture");
                } else if x < 0 || y < 0 || width <= 0 || height <= 0 {
                    resource.post_error(frame::Error::InvalidBufferDamage, "invalid damage");
                }
                // pila always copies the whole buffer, so the region itself
                // doesn't matter.
            }
            frame::Request::Capture => {
                if frame_state.captured {
                    resource.post_error(frame::Error::AlreadyCaptured, "capture sent twice");
                    return;
                }
                if frame_state.buffer.is_none() {
                    resource.post_error(frame::Error::NoBuffer, "capture without a buffer");
                    return;
                }
                frame_state.captured = true;
                drop(frame_state);
                let Some(session) = state
                    .capture
                    .sessions
                    .iter_mut()
                    .find(|s| s.resource == data.session)
                else {
                    resource.failed(FailureReason::Stopped);
                    return;
                };
                if session.stopped {
                    resource.failed(FailureReason::Stopped);
                    return;
                }
                session.pending = true;
                // Processed in `after_frames`, frame drawn or not.
                state.capture_due = true;
                if !session.captured_once {
                    // The first frame is copied right away.
                    state
                        .loop_handle
                        .insert_idle(|state| state.process_captures());
                }
                if let Target::Toplevel(identifier) = session.target.clone() {
                    if !state.is_locked() {
                        if let Some(window) = state.window_for_identifier(&identifier) {
                            state.send_capture_frames(&window);
                        }
                    }
                }
            }
            frame::Request::Destroy => {}
            _ => {}
        }
    }

    fn destroyed(
        state: &mut Self,
        _: ClientId,
        resource: &ExtImageCopyCaptureFrameV1,
        _: &FrameData,
    ) {
        for session in &mut state.capture.sessions {
            if session.frame.as_ref() == Some(resource) {
                session.frame = None;
                session.pending = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dmabuf_must_be_of_a_format_and_modifier_offered() {
        let target = DmabufTarget {
            device: 0,
            formats: vec![(Fourcc::Argb8888, vec![0, 42])],
        };
        assert!(dmabuf_offered(Some(&target), Fourcc::Argb8888, 0));
        assert!(dmabuf_offered(Some(&target), Fourcc::Argb8888, 42));
        // A modifier the GPU can only texture from (external-only).
        assert!(!dmabuf_offered(Some(&target), Fourcc::Argb8888, 7));
        assert!(!dmabuf_offered(Some(&target), Fourcc::Xrgb8888, 0));
        // No dma-bufs offered at all.
        assert!(!dmabuf_offered(None, Fourcc::Argb8888, 0));
    }
}
