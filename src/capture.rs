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
//! the source changes: after every rendered frame (`Pila::post_render`)
//! pending captures are checked against a per-session damage tracker, and
//! only copied (with that damage) once something changed. A source that
//! changes size re-sends the constraints and fails the pending frame; a
//! source that goes away stops the session.
//!
//! Frames are rendered offscreen from the same scene the backends draw
//! (`render.rs`), at the source's scale: an output at its own scale, a window
//! at the scale of the output it's on. A window is captured without pila's
//! title bar. While the session is locked, outputs capture the lock screen
//! and window captures wait. With `paint_cursors`, the cursor is included
//! (nested, only client-drawn cursors: the host draws named ones).
//!
//! Buffers may be shm (read back from an offscreen texture, rows reversed:
//! GL reads bottom-up) or dma-buf on the renderer's device (rendered into
//! directly; what xdg-desktop-portal-wlr uses for PipeWire). Cursor
//! sessions are created stopped. Any client may capture, as with other
//! compositors.

use std::{sync::Mutex, time::Duration};

use smithay::{
    backend::{
        allocator::{Buffer as _, Fourcc},
        renderer::{
            buffer_type,
            damage::OutputDamageTracker,
            element::AsRenderElements,
            gles::{GlesRenderer, GlesTexture},
            Bind, BufferType, ExportMem, Offscreen, TextureMapping,
        },
    },
    input::pointer::CursorImageStatus,
    output::{Output, WeakOutput},
    reexports::{
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
        wayland_server::{
            backend::ClientId,
            protocol::{wl_buffer::WlBuffer, wl_output, wl_shm},
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
        },
    },
    utils::{Physical, Point, Rectangle, Scale, Size, Transform},
    wayland::{
        dmabuf::get_dmabuf,
        foreign_toplevel_list::ForeignToplevelHandle,
        shm::{with_buffer_contents, with_buffer_contents_mut},
    },
};
use tracing::{debug, warn};

use crate::{
    cursor,
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
/// backend (`Pila::capture_dmabuf`); without it only shm is offered.
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

/// A capture session (`Pila::captures`).
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
    /// `None` if it's gone.
    fn capture_geometry(&self, target: &Target) -> Option<(Size<i32, Physical>, Scale<f64>)> {
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
                let scale = self
                    .space
                    .outputs_for_element(&window)
                    .first()
                    .map(output_scale)
                    .unwrap_or_else(|| Scale::from(1.0));
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

    /// Copy every pending frame whose source changed (or that is a
    /// session's first). Called after each rendered frame and when a first
    /// capture is requested.
    pub fn process_captures(&mut self) {
        if !self.captures.iter().any(|s| s.pending) {
            return;
        }
        let mut sessions = std::mem::take(&mut self.captures);
        for session in sessions.iter_mut().filter(|s| s.pending) {
            self.process_capture(session);
        }
        // Sessions created meanwhile (none expected) are kept too.
        sessions.append(&mut self.captures);
        self.captures = sessions;
    }

    fn process_capture(&mut self, session: &mut Session) {
        let Some(geometry) = self.capture_geometry(&session.target) else {
            session.stop();
            return;
        };
        if session.geometry != Some(geometry) {
            session.send_constraints(geometry, self.capture_dmabuf.as_ref());
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
        let Some(buffer) = frame_data.state.lock().unwrap().buffer.clone() else {
            return;
        };

        // Everything taken from `self` before borrowing the renderer.
        let scene = self.scene();
        let pointer = self
            .seat
            .get_pointer()
            .map(|p| p.current_location())
            .unwrap_or_default();
        let cursor_surface = cursor::surface_position(&self.cursor_status, pointer);
        let named_cursor = match &self.cursor_status {
            CursorImageStatus::Named(icon) => Some(*icon),
            _ => None,
        };
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
        let clear = match window {
            Some(_) => TRANSPARENT,
            None => render::clear_color(&scene),
        };

        let output_for_blur = match &session.target {
            Target::Output(output) => output
                .upgrade()
                .and_then(|o| self.space.output_geometry(&o).map(|geo| (o.name(), geo))),
            _ => None,
        };
        let Pila { backend, blur, .. } = self;
        let Some(backend) = backend.as_mut() else {
            return;
        };
        let (renderer, cursor_theme) = backend.renderer_and_cursor_theme();
        // Blurred backdrops are part of the screen; prepare them for this
        // output as the backends do.
        if let Some((name, geo)) = &output_for_blur {
            blur.prepare(renderer, &scene, name, geo.loc, geo.size, scale);
        }

        let mut elements: Vec<PilaRenderElement<GlesRenderer>> = Vec::new();
        if session.paint_cursors {
            elements.extend(render::cursor_elements(
                renderer,
                cursor_surface.as_ref(),
                origin,
                scale,
            ));
            if let (Some(icon), Some(theme)) = (named_cursor, cursor_theme) {
                let image = theme.get(icon, crate::scale::buffer_scale(scale));
                elements.extend(render::named_cursor_elements(
                    renderer, &image, pointer, origin, scale,
                ));
            }
        }
        match &window {
            Some(window) => {
                // The buffer's origin is the window geometry's top-left.
                let loc: Point<i32, Physical> =
                    (Point::default() - window.geometry().loc).to_physical_precise_round(scale);
                elements.extend(AsRenderElements::<GlesRenderer>::render_elements::<
                    PilaRenderElement<GlesRenderer>,
                >(window, renderer, loc, scale, 1.0));
            }
            None => elements.extend(render::scene_elements(
                renderer,
                &scene,
                origin,
                scale,
                &mut |i| blur.element(i, scale),
            )),
        }

        // Changes since the last copied frame; later frames wait for one.
        let tracker = session
            .tracker
            .get_or_insert_with(|| OutputDamageTracker::new(size, scale, Transform::Normal));
        let damage: Vec<Rectangle<i32, Physical>> = match tracker.damage_output(1, &elements) {
            Ok((Some(damage), _)) => damage.clone(),
            Ok((None, _)) => Vec::new(),
            Err(err) => {
                warn!(?err, "capture: damage tracking failed");
                Vec::new()
            }
        };
        let damage = if session.captured_once {
            if damage.is_empty() {
                return;
            }
            damage
        } else {
            vec![Rectangle::from_size(size)]
        };

        match copy_frame(renderer, &buffer, size, scale, &elements, clear) {
            Ok(()) => {}
            Err(CopyError::Constraints) => {
                debug!("capture: buffer doesn't match the constraints");
                session.fail(FailureReason::BufferConstraints);
                return;
            }
            Err(CopyError::Render(err)) => {
                warn!(err, "capture failed");
                session.fail(FailureReason::Unknown);
                return;
            }
        }

        frame.transform(wl_output::Transform::Normal);
        for rect in damage {
            frame.damage(rect.loc.x, rect.loc.y, rect.size.w, rect.size.h);
        }
        let time: Duration = self.clock.now().into();
        let secs = time.as_secs();
        frame.presentation_time((secs >> 32) as u32, secs as u32, time.subsec_nanos());
        frame.ready();
        session.pending = false;
        session.captured_once = true;
    }

    /// Stop the sessions of an output that went away.
    pub fn stop_captures_of(&mut self, output: &Output) {
        for session in &mut self.captures {
            if matches!(&session.target, Target::Output(o) if o.upgrade().as_ref() == Some(output))
            {
                session.stop();
            }
        }
    }
}

pub(crate) enum CopyError {
    /// The buffer isn't shm, or doesn't match the size or formats.
    Constraints,
    Render(String),
}

/// Copy `elements` into the client's `buffer`: rendered straight into a
/// dma-buf, or offscreen and read back into shm.
pub(crate) fn copy_frame(
    renderer: &mut GlesRenderer,
    buffer: &WlBuffer,
    size: Size<i32, Physical>,
    scale: Scale<f64>,
    elements: &[PilaRenderElement<GlesRenderer>],
    clear: [f32; 4],
) -> Result<(), CopyError> {
    match buffer_type(buffer) {
        Some(BufferType::Shm) => copy_to_shm(renderer, buffer, size, scale, elements, clear),
        Some(BufferType::Dma) => render_to_dmabuf(renderer, buffer, size, scale, elements, clear),
        _ => Err(CopyError::Constraints),
    }
}

fn render_error(err: &dyn std::fmt::Debug) -> CopyError {
    CopyError::Render(format!("{err:?}"))
}

/// Render `elements` into the client's dma-buf: no copy through the CPU.
fn render_to_dmabuf(
    renderer: &mut GlesRenderer,
    buffer: &WlBuffer,
    size: Size<i32, Physical>,
    scale: Scale<f64>,
    elements: &[PilaRenderElement<GlesRenderer>],
    clear: [f32; 4],
) -> Result<(), CopyError> {
    let mut dmabuf = get_dmabuf(buffer)
        .map_err(|_| CopyError::Constraints)?
        .clone();
    let buffer_size = dmabuf.size();
    if (buffer_size.w, buffer_size.h) != (size.w, size.h)
        || !DMABUF_FORMATS.contains(&dmabuf.format().code)
    {
        return Err(CopyError::Constraints);
    }
    let mut target = renderer.bind(&mut dmabuf).map_err(|e| render_error(&e))?;
    // Drawn like a DRM output's buffer: top row first, so `Normal`.
    let mut tracker = OutputDamageTracker::new(size, scale, Transform::Normal);
    let result = tracker
        .render_output(renderer, &mut target, 0, elements, clear)
        .map_err(|e| render_error(&e))?;
    // The client reads it as soon as it gets `ready`.
    result.sync.wait().map_err(|e| render_error(&e))?;
    Ok(())
}

/// Render `elements` offscreen and copy the pixels into the shm `buffer`.
fn copy_to_shm(
    renderer: &mut GlesRenderer,
    buffer: &WlBuffer,
    size: Size<i32, Physical>,
    scale: Scale<f64>,
    elements: &[PilaRenderElement<GlesRenderer>],
    clear: [f32; 4],
) -> Result<(), CopyError> {
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

    let mut texture =
        Offscreen::<GlesTexture>::create_buffer(renderer, Fourcc::Abgr8888, (w, h).into())
            .map_err(|e| render_error(&e))?;
    let mut target = renderer.bind(&mut texture).map_err(|e| render_error(&e))?;
    let mut tracker = OutputDamageTracker::new(size, scale, Transform::Normal);
    let result = tracker
        .render_output(renderer, &mut target, 0, elements, clear)
        .map_err(|e| render_error(&e))?;
    result.sync.wait().map_err(|e| render_error(&e))?;
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
    // glReadPixels returns rows bottom-up: an offscreen texture drawn with
    // `Transform::Normal` reads back upside down unless the mapping says
    // it's already flipped.
    let bottom_up = !mapping.flipped();
    let pixels = renderer
        .map_texture(&mapping)
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
                };
                match state.capture_geometry(&session.target) {
                    Some(geometry) => {
                        session.send_constraints(geometry, state.capture_dmabuf.as_ref())
                    }
                    None => session.stop(),
                }
                debug!(target = ?session.target, paint_cursors = session.paint_cursors, "capture session");
                state.captures.push(session);
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
                let mut got = got_session.lock().unwrap();
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
                };
                session.stop();
                state.captures.push(session);
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
                let Some(session) = state.captures.iter_mut().find(|s| s.resource == *resource)
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
        state.captures.retain(|s| s.resource != *resource);
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
        let mut frame_state = data.state.lock().unwrap();
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
                    .captures
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
                if !session.captured_once {
                    // The first frame is copied right away.
                    state
                        .loop_handle
                        .insert_idle(|state| state.process_captures());
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
        for session in &mut state.captures {
            if session.frame.as_ref() == Some(resource) {
                session.frame = None;
                session.pending = false;
            }
        }
    }
}
