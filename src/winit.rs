//! Nested backend: pila runs as a window inside the host Wayland session.

use std::{os::unix::fs::MetadataExt, path::PathBuf, time::Duration};

use smithay::{
    backend::{
        egl::EGLDevice,
        renderer::{
            damage::OutputDamageTracker, element::surface::WaylandSurfaceRenderElement,
            gles::GlesRenderer, ImportDma,
        },
        winit::{self, WinitEvent, WinitGraphicsBackend},
    },
    input::pointer::{CursorIcon, CursorImageStatus},
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::EventLoop, wayland_server::DisplayHandle, winit::window::Window as WinitWindow,
    },
    utils::{Rectangle, Transform},
    wayland::dmabuf::{DmabufFeedbackBuilder, DmabufState},
};
use tracing::{error, info, warn};

use crate::{CalloopData, Pila};

const REFRESH_MHZ: i32 = 60_000;
const BACKGROUND: [f32; 4] = [0.12, 0.13, 0.15, 1.0];

pub fn init_winit(
    event_loop: &mut EventLoop<CalloopData>,
    data: &mut CalloopData,
) -> Result<(), Box<dyn std::error::Error>> {
    let display_handle = &mut data.display_handle;
    let state = &mut data.state;

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
    state.backend = Some(backend);

    event_loop
        .handle()
        .insert_source(winit, move |event, _, data| {
            let display = &mut data.display_handle;
            let state = &mut data.state;
            let Some(backend) = state.backend.as_mut() else {
                return;
            };

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
                }
                WinitEvent::Input(event) => state.process_input_event(event),
                WinitEvent::Redraw => {
                    let size = backend.window_size();
                    let damage = Rectangle::from_size(size);

                    let rendered = match backend.bind() {
                        Ok((renderer, mut framebuffer)) => {
                            smithay::desktop::space::render_output::<
                                _,
                                WaylandSurfaceRenderElement<GlesRenderer>,
                                _,
                                _,
                            >(
                                &output,
                                renderer,
                                &mut framebuffer,
                                1.0,
                                0,
                                [&state.space],
                                &[],
                                &mut damage_tracker,
                                BACKGROUND,
                            )
                            .map_err(|err| error!(?err, "render failed"))
                            .is_ok()
                        }
                        Err(err) => {
                            error!(?err, "failed to bind backend");
                            false
                        }
                    };
                    if rendered {
                        if let Err(err) = backend.submit(Some(&[damage])) {
                            error!(?err, "failed to submit frame");
                        }
                    }

                    for window in state.space.elements() {
                        window.send_frame(
                            &output,
                            state.start_time.elapsed(),
                            Some(Duration::ZERO),
                            |_, _| Some(output.clone()),
                        );
                    }

                    state.space.refresh();
                    state.popups.cleanup();
                    let _ = display.flush_clients();

                    backend.window().request_redraw();
                }
                WinitEvent::CloseRequested => state.loop_signal.stop(),
                _ => (),
            }
        })?;

    Ok(())
}

/// Show the client's requested cursor on the host window. Client-drawn cursor
/// surfaces aren't rendered yet, so those fall back to the host's default arrow.
pub fn apply_cursor(window: &WinitWindow, status: &CursorImageStatus) {
    match status {
        CursorImageStatus::Hidden => window.set_cursor_visible(false),
        CursorImageStatus::Named(icon) => {
            window.set_cursor(*icon);
            window.set_cursor_visible(true);
        }
        CursorImageStatus::Surface(_) => {
            window.set_cursor(CursorIcon::Default);
            window.set_cursor_visible(true);
        }
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
