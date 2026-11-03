//! Nested backend: pila runs as a window inside the host Wayland session.

use std::{os::unix::fs::MetadataExt, path::PathBuf, time::Duration};

use smithay::{
    backend::{
        egl::{EGLDevice, EGLSurface},
        renderer::{damage::OutputDamageTracker, gles::GlesRenderer, ImportDma},
        winit::{self, WinitEvent, WinitGraphicsBackend},
    },
    desktop::layer_map_for_output,
    input::pointer::CursorImageStatus,
    output::{Mode, Output, PhysicalProperties, Scale as OutputScale, Subpixel},
    reexports::{
        calloop::EventLoop,
        wayland_protocols::wp::presentation_time::server::wp_presentation_feedback,
        wayland_server::{DisplayHandle, Resource},
        winit::window::Window as WinitWindow,
    },
    utils::Transform,
    wayland::dmabuf::{DmabufFeedbackBuilder, DmabufState},
    wayland::presentation::Refresh,
};
use tracing::{error, info, warn};

use crate::{backend::Backend, cursor, frame, render, Pila};

const REFRESH_MHZ: i32 = 60_000;

pub fn init_winit(
    event_loop: &mut EventLoop<'static, Pila>,
    state: &mut Pila,
) -> Result<(), Box<dyn std::error::Error>> {
    let display_handle = &state.display_handle.clone();

    let (mut backend, winit) = winit::init::<GlesRenderer>()?;
    init_dmabuf(&mut backend, &mut state.dmabuf_state, display_handle);
    if let Ok((_, dev)) = main_device(&mut backend) {
        state.capture_dmabuf = Some(crate::capture::DmabufTarget::new(dev, backend.renderer()));
    }

    let mode = Mode {
        size: backend.window_size(),
        refresh: REFRESH_MHZ,
    };

    let output = Output::new(
        "winit".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "pila".into(),
            model: "nested".into(),
        },
    );
    let _global = output.create_global::<Pila>(display_handle);
    // Nested there's one output; only `scale` of its `[[output]]` applies.
    let output_config = state.config.output("winit");
    if output_config
        .is_some_and(|c| c.mode.is_some() || c.position.is_some() || c.transform.is_some())
    {
        warn!("nested, only the output's scale applies; mode, position and transform are ignored");
    }
    let scale = output_config.and_then(|c| c.scale).unwrap_or(1.0);
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        Some(OutputScale::Fractional(scale)),
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    state.space.map_output(&output, (0, 0));

    let mut damage_tracker = OutputDamageTracker::from_output(&output);
    // Frames left to render in full: buffer ages are unreliable after a resize.
    let mut full_redraw = 0u8;
    state.backend = Some(Backend::Winit(Box::new(backend)));

    event_loop
        .handle()
        .insert_source(winit, move |event, _, state| {
            // Redraws happen on demand: after input or a window event here,
            // after client requests (`Pila::queue_redraw` in the display
            // source), and for a few frames after a resize.
            let mut queue_redraw = true;
            match event {
                WinitEvent::Resized { size, .. } => {
                    output.change_current_state(
                        Some(Mode {
                            size,
                            refresh: REFRESH_MHZ,
                        }),
                        None,
                        None,
                        None,
                    );
                    layer_map_for_output(&output).arrange();
                    state.refit_maximized();
                    state.configure_lock_surfaces();
                    full_redraw = 4;
                }
                WinitEvent::Input(event) => state.process_input_event(event),
                // Keys held when the host takes focus away never get a release.
                WinitEvent::Focus(false) => state.release_all_keys(),
                WinitEvent::Redraw => {
                    full_redraw = full_redraw.saturating_sub(1);
                    redraw(state, &output, &mut damage_tracker, full_redraw > 0);
                    // Keep going while buffers need repainting in full, or a
                    // lock still waits for its first locked frame.
                    queue_redraw = full_redraw > 0 || state.lock_pending();
                }
                WinitEvent::CloseRequested => state.loop_signal.stop(),
                _ => (),
            }
            if queue_redraw {
                state.queue_redraw();
            }
        })?;

    Ok(())
}

fn redraw(
    state: &mut Pila,
    output: &Output,
    damage_tracker: &mut OutputDamageTracker,
    full_redraw: bool,
) {
    // A destroyed cursor surface falls back to the default arrow.
    if matches!(&state.cursor_status, CursorImageStatus::Surface(s) if !s.is_alive()) {
        state.set_cursor(CursorImageStatus::default_named());
    }

    // Everything taken from `state` is collected before borrowing the backend.
    let pointer_location = state
        .seat
        .get_pointer()
        .map(|p| p.current_location())
        .unwrap_or_default();
    let cursor = cursor::surface_position(&state.cursor_status, pointer_location);
    let scene = state.scene();
    let lock_pending = state.lock_pending();
    let output_location = state
        .space
        .output_geometry(output)
        .map(|geo| geo.loc)
        .unwrap_or_default();

    let scale = crate::scale::output_scale(output);

    let Some(Backend::Winit(backend)) = state.backend.as_mut() else {
        return;
    };
    // Age 0 makes the damage tracker redraw everything. The frame that
    // confirms a session lock must not keep anything of the unlocked screen.
    let age = if full_redraw || lock_pending || !make_surface_current(backend) {
        0
    } else {
        backend.buffer_age().unwrap_or(0)
    };
    // Only what changed since the buffer was last drawn (`None` if nothing
    // did), and how each element was drawn.
    let (damage, render_states) = match backend.bind() {
        Ok((renderer, mut framebuffer)) => {
            let mut elements =
                render::cursor_elements(renderer, cursor.as_ref(), output_location, scale);
            elements.extend(render::scene_elements(
                renderer,
                &scene,
                output_location,
                scale,
            ));
            match damage_tracker.render_output(
                renderer,
                &mut framebuffer,
                age,
                &elements,
                render::clear_color(&scene),
            ) {
                Ok(result) => (result.damage.cloned(), Some(result.states)),
                Err(err) => {
                    error!(?err, "render failed");
                    (None, None)
                }
            }
        }
        Err(err) => {
            error!(?err, "failed to bind backend");
            (None, None)
        }
    };
    let submitted = damage.is_some_and(|damage| {
        backend
            .submit(Some(&damage))
            .map_err(|err| error!(?err, "failed to submit frame"))
            .is_ok()
    });
    // Single output: one locked frame on screen means everything is covered.
    if lock_pending && submitted {
        state.confirm_lock();
    }

    let cursor_surface = cursor.as_ref().map(|(surface, _)| surface);
    if let Some(states) = &render_states {
        frame::update_primary_outputs(&scene, cursor_surface, output, states);
        if submitted {
            // Nested, the host decides when the frame really reaches the
            // screen; "now" at submit is the best estimate available.
            frame::presentation_feedback(&scene, cursor_surface, output, states).presented(
                state.clock.now(),
                Refresh::fixed(Duration::from_secs_f64(1000.0 / REFRESH_MHZ as f64)),
                0,
                wp_presentation_feedback::Kind::Vsync,
            );
        }
    }
    // Frame callback times use the presentation clock too, so clients can
    // compare them with presentation feedback.
    frame::send_frames(&scene, cursor_surface, output, state.clock.now().into());
    state.post_render(output);
}

/// Make the window's EGL surface current, so its buffer age can be queried.
///
/// Mesa only knows the back buffer's age once the surface has been made
/// current for the frame; otherwise the query fails with `EGL_BAD_SURFACE`
/// (every frame while a client animates). `WinitGraphicsBackend::bind` would
/// do it, but it's lazy: the surface only becomes current when rendering
/// starts, which is after the age is needed.
fn make_surface_current(backend: &mut WinitGraphicsBackend<GlesRenderer>) -> bool {
    let surface: *const EGLSurface = backend.egl_surface();
    // SAFETY: `surface` points at the backend's own EGL surface, a different
    // field than the renderer borrowed mutably here, and stays valid for the
    // call. Changing the thread's current surface is fine: the renderer makes
    // its context current again before every operation.
    unsafe {
        backend
            .renderer()
            .egl_context()
            .make_current_with_surface(&*surface)
            .is_ok()
    }
}

/// Show the client's requested cursor on the host window. Client-drawn cursor
/// surfaces are rendered by pila itself (see `cursor`), so the host's is hidden.
pub fn apply_cursor(window: &WinitWindow, status: &CursorImageStatus) {
    match status {
        CursorImageStatus::Hidden => window.set_cursor_visible(false),
        CursorImageStatus::Named(icon) => {
            window.set_cursor(*icon);
            window.set_cursor_visible(true);
        }
        CursorImageStatus::Surface(_) => window.set_cursor_visible(false),
    }
}

/// Advertise `zwp_linux_dmabuf_v1` so GPU clients can share buffers with us
/// instead of falling back to software rendering.
fn init_dmabuf(
    backend: &mut WinitGraphicsBackend<GlesRenderer>,
    dmabuf_state: &mut DmabufState,
    display_handle: &DisplayHandle,
) {
    let formats = backend.renderer().dmabuf_formats();

    // v4 with default feedback names the main device; Mesa's EGL needs that
    // (or wl_drm, which we don't offer) to pick a GPU, so v3 is only a fallback.
    let feedback = match main_device(backend) {
        Ok((path, dev)) => match DmabufFeedbackBuilder::new(dev, formats.clone()).build() {
            Ok(feedback) => Some((path, feedback)),
            Err(err) => {
                warn!(
                    ?err,
                    "failed to build dmabuf feedback, using linux-dmabuf v3"
                );
                None
            }
        },
        Err(err) => {
            warn!(%err, "no DRM device for the EGL display, using linux-dmabuf v3");
            None
        }
    };

    match feedback {
        Some((path, feedback)) => {
            dmabuf_state.create_global_with_default_feedback::<Pila>(display_handle, &feedback);
            info!(device = %path.display(), "linux-dmabuf v4 enabled");
        }
        None => {
            dmabuf_state.create_global::<Pila>(display_handle, formats);
        }
    }
}

/// The DRM device node the renderer runs on, preferring the render node.
/// (Smithay's `EGLDevice::try_get_render_node` does this but needs `backend_drm`.)
fn main_device(
    backend: &mut WinitGraphicsBackend<GlesRenderer>,
) -> Result<(PathBuf, u64), Box<dyn std::error::Error>> {
    let device = EGLDevice::device_for_display(backend.renderer().egl_context().display())?;
    let path = device
        .render_device_path()
        .or_else(|_| device.drm_device_path())?;
    let dev = std::fs::metadata(&path)?.rdev();
    Ok((path, dev))
}
