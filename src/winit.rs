//! Nested backend: pila runs as a window inside the host Wayland session.

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use smithay::{
    backend::{
        drm::DrmNode,
        egl::{EGLContext, EGLDevice},
        renderer::{damage::OutputDamageTracker, gles::GlesRenderer, ImportDma},
        winit::{self, WinitEvent, WinitEventLoop, WinitGraphicsBackend},
    },
    desktop::layer_map_for_output,
    input::pointer::CursorImageStatus,
    output::{Mode, Output, PhysicalProperties, Scale as OutputScale, Subpixel},
    reexports::{
        calloop::{
            EventIterator, EventLoop, EventSource, Poll, PostAction, Readiness, Token, TokenFactory,
        },
        wayland_protocols::wp::presentation_time::server::wp_presentation_feedback,
        wayland_server::{DisplayHandle, Resource},
        winit::{
            dpi::LogicalSize,
            platform::wayland::WindowAttributesWayland,
            window::{Window as WinitWindow, WindowAttributes},
        },
    },
    utils::Transform,
    wayland::dmabuf::{DmabufFeedbackBuilder, DmabufState},
    wayland::presentation::Refresh,
};
use tracing::{error, info, warn};

use crate::{backend::Backend, frame, render, Pila};

const REFRESH_MHZ: i32 = 60_000;

/// Set when the host went away under Pila (`Nested::Ended`): `main` then
/// fails rather than quitting as if asked to.
static HOST_LOST: AtomicBool = AtomicBool::new(false);

/// Whether the nested session ended because its host went away.
pub fn host_lost() -> bool {
    HOST_LOST.load(Ordering::Relaxed)
}

/// What the host window gives the event loop.
enum Nested {
    Winit(WinitEvent),
    /// winit's loop ended: the host's connection broke (the host session
    /// ended or crashed) or failed.
    Ended,
}

/// Smithay's winit source, saying when it ends. winit ends its loop when
/// the host's connection fails, and Smithay's source then removes itself
/// without a word: Pila ran on with no window, unreachable but for its
/// clients, while winit's thread that watches the host's socket spun on the
/// dead connection, printing `Io error: Broken pipe` nonstop.
struct HostWindow(WinitEventLoop);

impl EventSource for HostWindow {
    type Event = Nested;
    type Metadata = ();
    type Ret = ();
    type Error = <WinitEventLoop as EventSource>::Error;

    const NEEDS_EXTRA_LIFECYCLE_EVENTS: bool = WinitEventLoop::NEEDS_EXTRA_LIFECYCLE_EVENTS;

    fn process_events<F>(
        &mut self,
        readiness: Readiness,
        token: Token,
        mut callback: F,
    ) -> Result<PostAction, Self::Error>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        let action = self.0.process_events(readiness, token, |event, meta| {
            callback(Nested::Winit(event), meta)
        })?;
        if action == PostAction::Remove {
            callback(Nested::Ended, &mut ());
        }
        Ok(action)
    }

    fn register(
        &mut self,
        poll: &mut Poll,
        token_factory: &mut TokenFactory,
    ) -> smithay::reexports::calloop::Result<()> {
        self.0.register(poll, token_factory)
    }

    fn reregister(
        &mut self,
        poll: &mut Poll,
        token_factory: &mut TokenFactory,
    ) -> smithay::reexports::calloop::Result<()> {
        self.0.reregister(poll, token_factory)
    }

    fn unregister(&mut self, poll: &mut Poll) -> smithay::reexports::calloop::Result<()> {
        self.0.unregister(poll)
    }

    fn before_sleep(&mut self) -> smithay::reexports::calloop::Result<Option<(Readiness, Token)>> {
        self.0.before_sleep()
    }

    fn before_handle_events(&mut self, events: EventIterator<'_>) {
        self.0.before_handle_events(events)
    }
}

/// The host window: Smithay's defaults (`winit::init`), with Pila's title
/// and the app id `pila`, which the host's window rules can match (a Pila
/// host can open it on another output, unfocused: `open-on-output`,
/// `open-focused`). Nested on X11 the Wayland attributes are ignored.
fn window_attributes() -> WindowAttributes {
    WindowAttributes::default()
        .with_surface_size(LogicalSize::new(1280.0, 800.0))
        .with_title("Pila")
        .with_visible(true)
        .with_platform_attributes(Box::new(
            WindowAttributesWayland::default().with_name("pila", "pila"),
        ))
}

pub fn init_winit(
    event_loop: &mut EventLoop<'static, Pila>,
    state: &mut Pila,
) -> Result<(), Box<dyn std::error::Error>> {
    let display_handle = &state.display_handle.clone();

    let (mut backend, winit) = winit::init_from_attributes::<GlesRenderer>(window_attributes())?;
    crate::render::warm_up(backend.renderer());
    let node = render_node(&mut backend);
    init_dmabuf(
        &mut backend,
        node,
        &mut state.protocols.dmabuf,
        display_handle,
    );
    if let Some(node) = node {
        state.capture.dmabuf = crate::capture::DmabufTarget::new(node.dev_id(), backend.renderer());
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
            serial_number: String::new(),
        },
    );
    let _global = output.create_global::<Pila>(display_handle);
    // Nested there's one output; only `scale` of its `[[output]]` applies.
    let output_config = state.config.output("winit", None);
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
    // The refreshes missed between frames, by when they start
    // (`frame_stats::NestedPacing`).
    let mut pacing = crate::frame_stats::NestedPacing::default();
    state.backend = Some(Backend::Winit(Box::new(backend)));

    event_loop
        .handle()
        .insert_source(HostWindow(winit), move |event, _, state| {
            let event = match event {
                Nested::Winit(event) => event,
                Nested::Ended => {
                    error!("lost the host compositor: quitting");
                    HOST_LOST.store(true, Ordering::Relaxed);
                    state.loop_signal.stop();
                    return;
                }
            };
            // Redraws happen on demand: after a window event here, and as
            // input and client requests ask (`redraw.rs`). A resize is
            // redrawn in full: the damage tracker sees the new size.
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
                    state.refit_to_usable_area();
                    state.configure_lock_surfaces();
                    state.refresh_output_heads();
                }
                WinitEvent::Input(event) => {
                    state.process_input_event(event);
                    queue_redraw = false;
                }
                // Keys held when the host takes focus away never get a release.
                WinitEvent::Focus(false) => state.release_all_keys(),
                WinitEvent::Redraw => {
                    let started = std::time::Instant::now();
                    let at = state.clock.now().into();
                    if redraw(state, &output, &mut damage_tracker) {
                        let period = crate::frame_stats::refresh_period(&output);
                        // Only animations: other redraws go at the host's
                        // pace, which may well pause.
                        let missed = pacing.frame(at, state.animating(), period);
                        state.record_missed(&output, missed);
                        state.record_frame(&output, started.elapsed());
                    }
                    // Again while a lock still waits for its first locked frame.
                    queue_redraw = state.lock_pending();
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

/// Draw the frame; whether one was submitted.
fn redraw(state: &mut Pila, output: &Output, damage_tracker: &mut OutputDamageTracker) -> bool {
    // A destroyed cursor surface falls back to the default arrow.
    if matches!(&state.cursor_status, CursorImageStatus::Surface(s) if !s.is_alive()) {
        state.set_cursor(CursorImageStatus::default_named());
    }

    state.signal_commit_timers(output);
    // Everything taken from `state` is collected before borrowing the backend.
    let input = state.frame_input();
    let scene = &input.scene;
    let lock_pending = state.lock_pending();
    let output_geo = state.space.output_geometry(output).unwrap_or_default();

    let scale = crate::scale::output_scale(output);

    let Pila {
        backend,
        blur,
        cursor_theme,
        clock,
        ..
    } = state;
    let Some(Backend::Winit(backend)) = backend.as_mut() else {
        return false;
    };
    let started = clock.now().into();
    // What the GPU drew, to measure its time (`frame_stats.rs`).
    let mut gpu = None;
    // Age 0 makes the damage tracker redraw everything. The frame that
    // confirms a session lock must not keep anything of the unlocked screen.
    let age = if lock_pending || !make_surface_current(backend) {
        0
    } else {
        backend.buffer_age().unwrap_or(0)
    };
    // Only what changed since the buffer was last drawn (`None` if nothing
    // did), and how each element was drawn.
    let (damage, render_states) = match backend.bind() {
        Ok((renderer, mut framebuffer)) => {
            // Nested, the host draws named cursors unless they're magnified.
            let elements = render::frame_elements(
                renderer,
                blur,
                cursor_theme,
                &input,
                &output.name(),
                output_geo,
                scale,
                true,
            );
            match damage_tracker.render_output(
                renderer,
                &mut framebuffer,
                age,
                &elements,
                render::clear_color(scene),
            ) {
                Ok(result) => {
                    gpu = crate::frame_stats::GpuFrame::new(
                        &result.sync,
                        started,
                        clock.now().into(),
                    );
                    (result.damage.cloned(), Some(result.states))
                }
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
    if let Some(gpu) = gpu.filter(|_| submitted) {
        state.watch_gpu(output, gpu);
    }

    let cursor_surface = input.cursor_wl_surface();
    if let Some(states) = &render_states {
        state.hidden_frames_dirty |=
            frame::update_primary_outputs(scene, cursor_surface, output, states);
        if submitted {
            // Nested, the host decides when the frame really reaches the
            // screen; "now" at submit is the best estimate available.
            frame::presentation_feedback(scene, cursor_surface, output, states).presented(
                state.clock.now(),
                Refresh::fixed(Duration::from_secs_f64(1000.0 / REFRESH_MHZ as f64)),
                0,
                wp_presentation_feedback::Kind::Vsync,
            );
        }
    }
    // Frame callback times use the presentation clock too, so clients can
    // compare them with presentation feedback.
    frame::send_frames(scene, cursor_surface, output, state.clock.now().into());
    state.post_render(output);
    submitted
}

/// Make the window's EGL surface current, so its buffer age can be queried.
///
/// Mesa only knows the back buffer's age once the surface has been made
/// current for the frame; otherwise the query fails with `EGL_BAD_SURFACE`
/// (every frame while a client animates). `WinitGraphicsBackend::bind` would
/// do it, but it's lazy: the surface only becomes current when rendering
/// starts, which is after the age is needed.
fn make_surface_current(backend: &mut WinitGraphicsBackend<GlesRenderer>) -> bool {
    let context: *const EGLContext = backend.renderer().egl_context();
    let surface = backend.egl_surface();
    // SAFETY: `context` points into the backend's renderer, and once
    // `renderer()`'s mutable borrow has ended only shared borrows touch the
    // backend, so it stays valid for the call (taking the surface first and the
    // renderer after would read the surface through a pointer that `&mut self`
    // invalidated). Changing the thread's current surface is fine: the renderer
    // makes its context current again before every operation.
    unsafe { (*context).make_current_with_surface(surface).is_ok() }
}

/// Show the client's requested cursor on the host window. Client-drawn cursor
/// surfaces, and named shapes while `magnified` (`shake_cursor.rs`), are
/// rendered by pila itself (see `cursor`), so the host's is hidden.
pub fn apply_cursor(window: &dyn WinitWindow, status: &CursorImageStatus, magnified: bool) {
    match status {
        CursorImageStatus::Hidden => window.set_cursor_visible(false),
        CursorImageStatus::Named(_) if magnified => window.set_cursor_visible(false),
        CursorImageStatus::Named(icon) => {
            window.set_cursor((*icon).into());
            window.set_cursor_visible(true);
        }
        CursorImageStatus::Surface(_) => window.set_cursor_visible(false),
    }
}

/// Advertise `zwp_linux_dmabuf_v1` so GPU clients can share buffers with us
/// instead of falling back to software rendering.
fn init_dmabuf(
    backend: &mut WinitGraphicsBackend<GlesRenderer>,
    node: Option<DrmNode>,
    dmabuf_state: &mut DmabufState,
    display_handle: &DisplayHandle,
) {
    let formats = backend.renderer().dmabuf_formats();

    // v4 with default feedback names the main device; Mesa's EGL needs that
    // (or wl_drm, which we don't offer) to pick a GPU, so v3 is only a fallback.
    let feedback = node.and_then(|node| {
        match DmabufFeedbackBuilder::new(node.dev_id(), formats.clone()).build() {
            Ok(feedback) => Some((node, feedback)),
            Err(err) => {
                warn!(
                    ?err,
                    "failed to build dmabuf feedback, using linux-dmabuf v3"
                );
                None
            }
        }
    });

    match feedback {
        Some((node, feedback)) => {
            dmabuf_state.create_global_with_default_feedback::<Pila>(display_handle, &feedback);
            let device = node.dev_path().unwrap_or_default();
            info!(device = %device.display(), "linux-dmabuf v4 enabled");
        }
        None => {
            dmabuf_state.create_global::<Pila>(display_handle, formats);
        }
    }
}

/// The DRM node the renderer runs on, preferring the render node; `None`
/// without one (llvmpipe: no dmabufs then).
fn render_node(backend: &mut WinitGraphicsBackend<GlesRenderer>) -> Option<DrmNode> {
    let display = backend.renderer().egl_context().display();
    let found = EGLDevice::device_for_display(display).and_then(|d| d.try_get_render_node());
    if let Ok(Some(node)) = found {
        return Some(node);
    }
    // No device, or no sysfs to tell it's a DRM node (`DrmNode::from_path`).
    let err = found.err().map(|err| err.to_string());
    warn!(
        err,
        "no DRM device for the EGL display, using linux-dmabuf v3"
    );
    None
}
