//! wlr-screencopy: one-shot copies of an output, or a region of it, into a
//! client buffer. What screenshot tools and shells built before
//! ext-image-copy-capture use (Noctalia's region and fullscreen screenshots,
//! its blurred lock screen); to be dropped once they move to the standard
//! protocol, which `capture.rs` implements.
//!
//! A frame names its output and region at creation; pila answers with the
//! buffer it accepts (shm, and a dma-buf on the renderer's device), and the
//! client asks for the copy with `copy`, or `copy_with_damage` to wait until
//! something in the region changed since that manager's last copy of it.
//! Copies are rendered from the scene like `capture.rs`'s, by `copy_frame`;
//! plain copies run from an idle callback, damage ones after each rendered
//! frame (`Pila::after_frames`). A region is in the output's logical
//! coordinates, copied at the output's scale.

use crate::sync::Locked;
use std::{sync::Mutex, time::Instant};

use smithay::{
    backend::renderer::{buffer_type, damage::OutputDamageTracker, BufferType},
    output::{Output, WeakOutput},
    reexports::{
        wayland_protocols_wlr::screencopy::v1::server::{
            zwlr_screencopy_frame_v1::{self as frame, Flags, ZwlrScreencopyFrameV1},
            zwlr_screencopy_manager_v1::{self as manager, ZwlrScreencopyManagerV1},
        },
        wayland_server::{
            backend::ClientId,
            backend::ObjectId,
            protocol::{wl_buffer::WlBuffer, wl_output::WlOutput, wl_shm},
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
        },
    },
    utils::{Logical, Physical, Rectangle, Size, Transform},
    wayland::{dmabuf::get_dmabuf, shm::with_buffer_contents},
};
use tracing::{debug, warn};

use crate::{
    capture::{self, CopyError},
    render::{self, PilaRenderElement},
    scale::output_scale,
    Pila,
};

const VERSION: u32 = 3;

/// The shm format advertised (the protocol allows one).
const SHM_FORMAT: wl_shm::Format = wl_shm::Format::Xrgb8888;

/// A frame's target and progress.
pub struct FrameState {
    /// The manager it was created from (damage is tracked per manager).
    manager: ZwlrScreencopyManagerV1,
    output: WeakOutput,
    /// Output-local, logical.
    region: Rectangle<i32, Logical>,
    /// The region in physical pixels: the buffer size.
    size: Size<i32, Physical>,
    overlay_cursor: bool,
    buffer: Option<WlBuffer>,
    with_damage: bool,
    /// `copy` was sent (a frame copies once).
    copied: bool,
}

/// A frame's user data; `None` for one that failed at creation.
pub type FrameData = Mutex<Option<FrameState>>;

/// What distinguishes the damage trackers of `copy_with_damage`: the
/// manager (each client sees changes since *its* last copy), the output and
/// the region (as x, y, w, h: `Rectangle` isn't hashable).
pub type TrackerKey = (ObjectId, String, (i32, i32, i32, i32), u64);

/// Create the global. Called once from `Pila::new`.
pub fn init_global(dh: &DisplayHandle) {
    dh.create_global::<Pila, ZwlrScreencopyManagerV1, ()>(VERSION, ());
}

/// The region in physical pixels at the output's scale; a whole output is
/// exactly its mode size (in its orientation), like `capture.rs`.
fn physical_size(
    output: &Output,
    region: Rectangle<i32, Logical>,
    logical: Size<i32, Logical>,
) -> Option<Size<i32, Physical>> {
    if region == Rectangle::from_size(logical) {
        let mode = output.current_mode()?;
        return Some(output.current_transform().transform_size(mode.size));
    }
    Some(
        region
            .size
            .to_f64()
            .to_physical(output_scale(output))
            .to_i32_round(),
    )
}

fn log_failure(err: CopyError) {
    match err {
        CopyError::Constraints => debug!("screencopy: the buffer doesn't match"),
        CopyError::Render(err) => warn!(err, "screencopy failed"),
    }
}

/// Whether `buffer` is one this frame accepts: the advertised size, and a
/// format `copy_frame` can write.
fn buffer_matches(buffer: &WlBuffer, size: Size<i32, Physical>) -> bool {
    match buffer_type(buffer) {
        Some(BufferType::Shm) => with_buffer_contents(buffer, |_, _, data| {
            data.width == size.w
                && data.height == size.h
                && data.stride >= size.w * 4
                && capture::SHM_FORMATS.contains(&data.format)
        })
        .unwrap_or(false),
        Some(BufferType::Dma) => get_dmabuf(buffer).is_ok_and(|d| {
            use smithay::backend::allocator::Buffer as _;
            let s = d.size();
            (s.w, s.h) == (size.w, size.h) && capture::DMABUF_FORMATS.contains(&d.format().code)
        }),
        _ => false,
    }
}

impl Pila {
    /// The state of a new frame for `output` (the whole of it, or `region`),
    /// or `None` if it can't be captured.
    fn screencopy_frame_state(
        &self,
        manager: &ZwlrScreencopyManagerV1,
        output: &WlOutput,
        overlay_cursor: bool,
        region: Option<Rectangle<i32, Logical>>,
    ) -> Option<FrameState> {
        let output = Output::from_resource(output)?;
        let geo = self.space.output_geometry(&output)?;
        let full = Rectangle::from_size(geo.size);
        let region = match region {
            Some(region) => region
                .intersection(full)
                .filter(|r| r.size.w > 0 && r.size.h > 0)?,
            None => full,
        };
        let size = physical_size(&output, region, geo.size)?;
        Some(FrameState {
            manager: manager.clone(),
            output: output.downgrade(),
            region,
            size,
            overlay_cursor,
            buffer: None,
            with_damage: false,
            copied: false,
        })
    }

    /// Tell a new frame's client which buffers it takes.
    fn announce_screencopy_buffers(
        &self,
        frame: &ZwlrScreencopyFrameV1,
        size: Size<i32, Physical>,
    ) {
        let (w, h) = (size.w as u32, size.h as u32);
        frame.buffer(SHM_FORMAT, w, h, w * 4);
        if frame.version() >= 3 {
            if self.capture.dmabuf.as_ref().is_some_and(|d| d.offers()) {
                frame.linux_dmabuf(capture::DMABUF_FORMATS[1] as u32, w, h);
            }
            frame.buffer_done();
        }
    }

    /// Copy every frame whose client asked for it: all of them, or (with
    /// damage) the ones whose region changed since that client's last copy.
    /// Called after each rendered frame and when a plain copy is requested.
    /// `input`: the scene of this pass, built by the first copy that needs it
    /// (shared with the captures, `after_frames`).
    pub fn process_screencopies(&mut self, input: &mut Option<crate::render::FrameInput>) {
        if self.capture.screencopies.is_empty() {
            return;
        }
        let frames = std::mem::take(&mut self.capture.screencopies);
        for frame in frames {
            let done = self.process_screencopy(&frame, input);
            if let Some(started) = self.capture.building.take() {
                self.record_capture(started.elapsed());
            }
            if !done {
                self.capture.screencopies.push(frame);
            }
        }
    }

    /// Copy one frame; `false` if it's still waiting for a change.
    fn process_screencopy(
        &mut self,
        frame: &ZwlrScreencopyFrameV1,
        input: &mut Option<crate::render::FrameInput>,
    ) -> bool {
        let Some(data) = frame.data::<FrameData>() else {
            return true;
        };
        let (manager, output, region, size, overlay_cursor, buffer, with_damage) = {
            let state = data.locked();
            let Some(state) = state.as_ref() else {
                return true;
            };
            let Some(buffer) = state.buffer.clone() else {
                return true;
            };
            (
                state.manager.clone(),
                state.output.clone(),
                state.region,
                state.size,
                state.overlay_cursor,
                buffer,
                state.with_damage,
            )
        };
        let geometry = output
            .upgrade()
            .and_then(|o| self.space.output_geometry(&o).map(|geo| (o, geo)));
        let Some((output, geo)) = geometry else {
            frame.failed();
            return true;
        };
        // The output changed mode or scale since the frame was set up.
        if physical_size(&output, region, geo.size) != Some(size) {
            frame.failed();
            return true;
        }

        // Everything taken from `self` before borrowing the renderer.
        self.capture.building = Some(Instant::now());
        let input = &*input.get_or_insert_with(|| self.frame_input());
        let origin = geo.loc + region.loc;
        let scale = output_scale(&output);
        let clear = render::clear_color(&input.scene);

        let Pila {
            backend,
            blur,
            capture,
            ..
        } = self;
        let Some(backend) = backend.as_mut() else {
            frame.failed();
            return true;
        };
        let (renderer, cursor_theme) = backend.renderer_and_cursor_theme();
        let mut elements: Vec<PilaRenderElement> = Vec::new();
        if overlay_cursor {
            elements = render::pointer_elements(renderer, input, cursor_theme, origin, scale);
        }
        elements.extend(render::output_elements(
            renderer,
            blur,
            &input.scene,
            &output.name(),
            geo,
            origin,
            scale,
        ));

        let mut damage = Vec::new();
        let key = (
            manager.id(),
            output.name(),
            (region.loc.x, region.loc.y, region.size.w, region.size.h),
            // A tracker keeps its scale, and the size stays for another one.
            scale.x.to_bits(),
        );
        if with_damage {
            let tracker = capture
                .screencopy_trackers
                .entry(key.clone())
                .or_insert_with(|| OutputDamageTracker::new(size, scale, Transform::Normal));
            match tracker.damage_output(1, &elements) {
                Ok((Some(rects), _)) if !rects.is_empty() => damage = rects.clone(),
                Ok(_) => return false,
                Err(err) => {
                    warn!(?err, "screencopy: damage tracking failed");
                    frame.failed();
                    return true;
                }
            }
        }

        let source = capture::Source::Elements {
            elements: &elements,
            clear,
        };
        let copied = match capture::copy_frame(renderer, &buffer, capture, size, scale, source) {
            Ok(copied) => copied,
            Err(err) => {
                // The tracker saw changes that were never delivered: the
                // next frame starts over with a new one, all damaged.
                capture.screencopy_trackers.remove(&key);
                log_failure(err);
                frame.failed();
                return true;
            }
        };
        let shown = self.presented_at(Some(&output));
        let (hi, lo, nanos) = capture::split_timestamp(shown.into());
        let frame = frame.clone();
        let wanted = frame.clone();
        self.when_copied(
            copied,
            move || wanted.is_alive(),
            move |state, result| {
                if let Err(err) = result {
                    state.capture.screencopy_trackers.remove(&key);
                    log_failure(err);
                    frame.failed();
                    return;
                }
                // Rows come out top-down for both buffer kinds (`copy_frame`).
                frame.flags(Flags::empty());
                for rect in damage {
                    frame.damage(
                        rect.loc.x as u32,
                        rect.loc.y as u32,
                        rect.size.w as u32,
                        rect.size.h as u32,
                    );
                }
                frame.ready(hi, lo, nanos);
            },
        );
        true
    }

    /// Fail the pending frames of an output that went away.
    pub fn fail_screencopies_of(&mut self, output: &Output) {
        self.capture.screencopies.retain(|frame| {
            let gone = frame.data::<FrameData>().is_some_and(|d| {
                d.locked()
                    .as_ref()
                    .is_some_and(|s| s.output.upgrade().as_ref() == Some(output))
            });
            if gone {
                frame.failed();
            }
            !gone
        });
        self.capture
            .screencopy_trackers
            .retain(|(_, name, ..), _| name != output.name().as_str());
    }
}

impl GlobalDispatch<ZwlrScreencopyManagerV1, ()> for Pila {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ZwlrScreencopyManagerV1>,
        _: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }

    crate::state::privileged_global!();
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for Pila {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &ZwlrScreencopyManagerV1,
        request: manager::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let (frame, overlay_cursor, output, region) = match request {
            manager::Request::CaptureOutput {
                frame,
                overlay_cursor,
                output,
            } => (frame, overlay_cursor, output, None),
            manager::Request::CaptureOutputRegion {
                frame,
                overlay_cursor,
                output,
                x,
                y,
                width,
                height,
            } => {
                // A client's rectangle: a negative size or a place past the
                // limit fails the frame (the protocol has no error for it).
                let within = crate::surface_limits::position_ok(x.into(), y.into())
                    && crate::surface_limits::size_ok(width, height);
                if !within {
                    debug!(x, y, width, height, "screencopy: a region past the limit");
                    data_init.init(frame, Mutex::new(None)).failed();
                    return;
                }
                let region = Rectangle::new((x, y).into(), (width, height).into());
                (frame, overlay_cursor, output, Some(region))
            }
            _ => return,
        };
        let frame_state =
            state.screencopy_frame_state(resource, &output, overlay_cursor != 0, region);
        let size = frame_state.as_ref().map(|s| s.size);
        let frame = data_init.init(frame, Mutex::new(frame_state));
        match size {
            Some(size) => state.announce_screencopy_buffers(&frame, size),
            None => frame.failed(),
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &ZwlrScreencopyManagerV1, _: &()) {
        state
            .capture
            .screencopy_trackers
            .retain(|(m, ..), _| *m != resource.id());
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, FrameData> for Pila {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &ZwlrScreencopyFrameV1,
        request: frame::Request,
        data: &FrameData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let (buffer, with_damage) = match request {
            frame::Request::Copy { buffer } => (buffer, false),
            frame::Request::CopyWithDamage { buffer } => (buffer, true),
            _ => return,
        };
        let mut frame_state = data.locked();
        let Some(frame_state) = frame_state.as_mut() else {
            // Failed at creation; nothing to copy into.
            return;
        };
        if frame_state.copied {
            resource.post_error(frame::Error::AlreadyUsed, "copy sent twice");
            return;
        }
        if !buffer_matches(&buffer, frame_state.size) {
            resource.post_error(
                frame::Error::InvalidBuffer,
                "the buffer doesn't match the advertised size or formats",
            );
            return;
        }
        frame_state.copied = true;
        // An output unplugged since the frame was set up has no frame of its
        // own to come: fail now (`fail_screencopies_of` only fails the copies
        // already waiting when it goes).
        let output = frame_state
            .output
            .upgrade()
            .filter(|output| state.space.output_geometry(output).is_some());
        let Some(output) = output else {
            resource.failed();
            return;
        };
        frame_state.buffer = Some(buffer);
        frame_state.with_damage = with_damage;
        state.capture.screencopies.push(resource.clone());
        // Copied after the next frame (`after_frames`): at most once per
        // refresh however fast a client asks, as with damage. Its output's
        // frame, which may well have nothing new (`redraw::Content::Maybe`).
        state.request_redraw(&output, false);
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &ZwlrScreencopyFrameV1, _: &FrameData) {
        state.capture.screencopies.retain(|f| f != resource);
    }
}
