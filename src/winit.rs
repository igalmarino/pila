//! Nested backend: pila runs as a window inside the host Wayland session.

use std::{os::unix::fs::MetadataExt, path::PathBuf, time::Duration};

use smithay::{
    backend::{
        egl::EGLDevice,
        renderer::{damage::OutputDamageTracker, gles::GlesRenderer, ImportDma},
        winit::{self, WinitEvent, WinitGraphicsBackend},
    },
    desktop::{layer_map_for_output, utils::send_frames_surface_tree},
    input::pointer::CursorImageStatus,
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::EventLoop,
        wayland_server::{DisplayHandle, Resource},
        winit::window::Window as WinitWindow,
    },
    utils::Transform,
    wayland::dmabuf::{DmabufFeedbackBuilder, DmabufState},
};
use tracing::{error, info, warn};

use crate::{cursor, render, Pila};

const REFRESH_MHZ: i32 = 60_000;
const BACKGROUND: [f32; 4] = [0.12, 0.13, 0.15, 1.0];

pub fn init_winit(
    event_loop: &mut EventLoop<'static, Pila>,
    state: &mut Pila,
) -> Result<(), Box<dyn std::error::Error>> {
    let display_handle = &state.display_handle.clone();

    let (mut backend, winit) = winit::init::<GlesRenderer>()?;
    init_dmabuf(&mut backend, &mut state.dmabuf_state, display_handle);

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
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    state.space.map_output(&output, (0, 0));

    let mut damage_tracker = OutputDamageTracker::from_output(&output);
    // Frames left to render in full: buffer ages are unreliable after a resize.
    let mut full_redraw = 0u8;
    state.backend = Some(backend);

    event_loop
        .handle()
        .insert_source(winit, move |event, _, state| {
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
                    redraw(state, &output, &mut damage_tracker, full_redraw > 0)
                }
                WinitEvent::CloseRequested => state.loop_signal.stop(),
                _ => (),
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

    let Some(backend) = state.backend.as_mut() else {
        return;
    };
    // Age 0 makes the damage tracker redraw everything. The frame that
    // confirms a session lock must not keep anything of the unlocked screen.
    let age = if full_redraw || lock_pending {
        0
    } else {
        backend.buffer_age().unwrap_or(0)
    };
    // Only what changed since the buffer was last drawn, or `None` if nothing did.
    let damage = match backend.bind() {
        Ok((renderer, mut framebuffer)) => {
            let mut elements = render::cursor_elements(renderer, cursor.as_ref(), output_location);
            elements.extend(render::scene_elements(renderer, &scene, output_location));
            let background = if scene.locked {
                render::LOCKED_BACKGROUND
            } else {
                BACKGROUND
            };
            damage_tracker
                .render_output(renderer, &mut framebuffer, age, &elements, background)
                .map_err(|err| error!(?err, "render failed"))
                .ok()
                .and_then(|result| result.damage.cloned())
        }
        Err(err) => {
            error!(?err, "failed to bind backend");
            None
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

    let time = state.start_time.elapsed();
    if !scene.locked {
        for window in state.space.elements() {
            window.send_frame(output, time, Some(Duration::ZERO), |_, _| {
                Some(output.clone())
            });
        }
    }
    for (surface, _) in &scene.lock_surfaces {
        send_frames_surface_tree(surface, output, time, Some(Duration::ZERO), |_, _| {
            Some(output.clone())
        });
    }
    for entry in scene.layers() {
        entry
            .layer
            .send_frame(output, time, Some(Duration::ZERO), |_, _| {
                Some(output.clone())
            });
    }
    if let Some((surface, _)) = &cursor {
        send_frames_surface_tree(surface, output, time, Some(Duration::ZERO), |_, _| {
            Some(output.clone())
        });
    }

    state.space.refresh();
    state.update_idle_inhibit();
    layer_map_for_output(output).cleanup();
    state.popups.cleanup();
    let _ = state.display_handle.flush_clients();

    if let Some(backend) = &state.backend {
        backend.window().request_redraw();
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
