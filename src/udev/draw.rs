//! Drawing outputs, paced by their vblanks, and confirming a session lock.

use super::*;

use smithay::{
    backend::{
        allocator::Buffer as _,
        drm::compositor::{FrameError, PrimaryPlaneElement},
        renderer::{
            element::{
                Element as _, Id, RenderElement as _, RenderElementPresentationState,
                RenderElementStates, RenderingReason, UnderlyingStorage,
            },
            gles::GlesTarget,
            PresentationMode,
        },
    },
    utils::{Rectangle, Transform},
};

use crate::capture::{copy_frame, Source};
use crate::frame_stats::GpuFrame;

/// The idle callback queued by `UdevData::queue_redraw`.
pub(super) fn render_all(state: &mut Pila) {
    let Some(Backend::Udev(data)) = &mut state.backend else {
        return;
    };
    data.redraw_scheduled = false;
    let ids: Vec<_> = data.outputs.keys().copied().collect();
    // One scene for every output drawn in this pass, built once each one's
    // commit timers have applied; its cost counts in the first one's frame.
    let ready: Vec<_> = ids
        .into_iter()
        .filter_map(|id| prepare(state, id))
        .collect();
    if !ready.is_empty() {
        let started = std::time::Instant::now();
        let input = state.frame_input();
        let mut scene_cpu = started.elapsed();
        for ready in ready {
            draw(state, ready, false, &input, std::mem::take(&mut scene_cpu));
        }
    }
    // With every output powered off nothing is drawn, so nothing above
    // checks the lock: a locker (swaylock -f waits for it) would wait forever.
    if state.lock_pending() {
        maybe_confirm_lock(state);
    }
}

/// Draw one output, unless it has a frame in flight (then it's drawn at the
/// next vblank) or nothing changed. `at_vblank`: called right after one, so
/// the frame is due at the next (`frame_stats.rs`).
pub(super) fn render_output(state: &mut Pila, id: OutputId, at_vblank: bool) {
    let Some(ready) = prepare(state, id) else {
        return;
    };
    let started = std::time::Instant::now();
    let input = state.frame_input();
    let scene_cpu = started.elapsed();
    draw(state, ready, at_vblank, &input, scene_cpu);
}

/// What a frame may put on a plane of its own instead of drawing it: the
/// cursor and, with `direct-scanout`, a client's buffer that is all the
/// output shows (a fullscreen video or game), on the primary plane, whatever
/// its format. Smithay tries it on the kernel first and draws the frame if
/// it's refused. Not overlay planes: what they take differs by driver, and
/// they'd be for a window among others, which saves little.
fn frame_flags(direct_scanout: bool) -> FrameFlags {
    if direct_scanout {
        FrameFlags::ALLOW_CURSOR_PLANE_SCANOUT | FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT_ANY
    } else {
        FrameFlags::ALLOW_CURSOR_PLANE_SCANOUT
    }
}

/// The format and modifier of the client's dmabuf `element` shows, if it
/// shows one (logged when direct scanout starts or stops).
fn dmabuf_format(element: &PilaRenderElement, renderer: &mut GlesRenderer) -> Option<Format> {
    match element.underlying_storage(renderer)? {
        UnderlyingStorage::Wayland(buffer) => smithay::wayland::dmabuf::get_dmabuf(buffer)
            .ok()
            .map(|dmabuf| dmabuf.format()),
        UnderlyingStorage::Memory(_) | UnderlyingStorage::SolidColor(_) => None,
    }
}

/// A client's buffer Smithay tried to show as it is and couldn't, in front
/// to back order, the first one: its format and modifier (`None`: not a
/// dmabuf) and why (`ScanoutFailed`: the kernel or GBM refused it, which
/// sends the client the plane's formats, `frame::send_dmabuf_feedback`;
/// `FormatUnsupported`: not one the plane takes). Logged when it changes.
fn refused_scanout(
    elements: &[PilaRenderElement],
    states: &RenderElementStates,
    renderer: &mut GlesRenderer,
) -> Option<(Option<Format>, RenderingReason)> {
    elements.iter().find_map(|element| {
        let state = states.element_render_state(element.id().clone())?;
        match state.presentation_state {
            RenderElementPresentationState::Rendering {
                reason: Some(reason),
            } => Some((dmabuf_format(element, renderer), reason)),
            _ => None,
        }
    })
}

/// An output about to be drawn (`prepare`).
pub(super) struct Ready {
    id: OutputId,
    output: Output,
    output_geo: Rectangle<i32, smithay::utils::Logical>,
    /// When `prepare` decided to draw it (for `idle_until`).
    now: Duration,
    lock_pending: bool,
    /// The CPU time `prepare` took (its commit timers), counted in its frame.
    cpu: Duration,
}

/// Whether `id` is to be drawn now (nothing in flight, something asked for,
/// not held back: see `render_output`); if so, its commit timers applied, so
/// the scene built next has what they unblocked.
fn prepare(state: &mut Pila, id: OutputId) -> Option<Ready> {
    // A gamma change takes the CRTC first; drawing goes on at its vblank.
    if let Some(Backend::Udev(data)) = &mut state.backend {
        if commit_pending_gamma(data, id) {
            return None;
        }
    }
    let now: Duration = state.clock.now().into();
    let lock_pending = state.lock_pending();
    let Some(Backend::Udev(data)) = &mut state.backend else {
        return None;
    };
    let active = data.session.is_active();
    let surface = data.outputs.get_mut(&id)?;
    let content = surface.redraw?;
    if surface.crtc_state.frame_pending
        || surface.drm_output.is_none()
        || !active
        || surface.crtc_state.recovery.blocks_render()
    {
        return None;
    }
    // An output that had nothing new to show just now waits its turn
    // (`idle_wait`), unless what's asked surely shows or a lock waits.
    let wait = idle_wait(
        surface.crtc_state.idle_until,
        now,
        content,
        surface.crtc_state.idle_streak,
        lock_pending,
    );
    if let Some(delay) = wait {
        if let Some(request) = surface.crtc_state.request.as_mut() {
            request.held = true;
        }
        data.schedule_idle_redraw(id, delay);
        return None;
    }
    if hold_late(state, id, now, lock_pending) {
        return None;
    }
    let Some(Backend::Udev(data)) = &state.backend else {
        return None;
    };
    let output = data.outputs.get(&id)?.output.clone();
    let output_geo = state.space.output_geometry(&output)?;

    let started = std::time::Instant::now();
    // A destroyed cursor surface falls back to the default arrow.
    if matches!(&state.cursor_status, CursorImageStatus::Surface(s) if !s.alive()) {
        state.cursor_status = CursorImageStatus::default_named();
    }
    state.signal_commit_timers(&output);
    Some(Ready {
        id,
        output,
        output_geo,
        now,
        lock_pending,
        cpu: started.elapsed(),
    })
}

/// Whether `id`'s frame waits to be drawn its margin before the next vblank
/// (`render_late.rs`); if so, a timer draws it then. One held already waits
/// until its moment, and is let through from then on, or at once if the scene
/// stopped being steady meanwhile (`render_late::still_held`).
fn hold_late(state: &mut Pila, id: OutputId, now: Duration, lock_pending: bool) -> bool {
    let enabled = state.config.render_late;
    let animating = state.animating();
    let settling = render_late::settling(state.settling_since, now);
    let Some(Backend::Udev(data)) = &mut state.backend else {
        return false;
    };
    // The worst of the other outputs' frames: one drawn first holds this one
    // up (they share the event loop and the GPU).
    let others = data
        .outputs
        .iter()
        .filter(|(other, surface)| **other != id && surface.drm_output.is_some())
        .filter_map(|(_, surface)| surface.render_timing.worst())
        .max()
        .unwrap_or_default();
    let Some(surface) = data.outputs.get_mut(&id) else {
        return false;
    };
    // A game that tears wants its frames at once; with adaptive sync the
    // flip makes the vblank; a lock waits for its frame; the first frame
    // after a modeset starts a new grid; while Pila animates, or a window or
    // layer surface that appeared or changed size settles, apps are busy on
    // the GPU as recent frames don't foretell (and an animation of Pila's
    // shows at the same vblank either way).
    let steady = enabled
        && !lock_pending
        && !surface.crtc_state.first_frame
        && !surface.crtc_state.torn
        && !surface.crtc_state.presented_variable
        && !animating
        && !settling;
    if let Some(hold) = surface.crtc_state.render_at {
        if render_late::still_held(hold, now, steady) {
            return true;
        }
        // Its moment, or let go: drawn now (and not as held if let go, since
        // it isn't drawn its margin before the vblank).
        if now < hold.at {
            surface.crtc_state.render_at = None;
        }
        return false;
    }
    if !steady {
        return false;
    }
    let period = crate::frame_stats::refresh_period(&surface.output);
    let at = surface
        .render_timing
        .margin(now, others, period)
        .and_then(|margin| render_late::render_at(surface.last_vblank_time, now, period, margin));
    let Some(hold) = at else {
        return false;
    };
    surface.crtc_state.render_at = Some(hold);
    if let Some(request) = surface.crtc_state.request.as_mut() {
        request.held = true;
    }
    data.schedule_late_render(id, hold.at);
    true
}

/// Draw `ready` from `input` (`Pila::frame_input`, shared by the outputs of a
/// pass), whose building took `scene_cpu` (counted in this frame).
fn draw(
    state: &mut Pila,
    ready: Ready,
    at_vblank: bool,
    input: &crate::render::FrameInput,
    scene_cpu: Duration,
) {
    let Ready {
        id,
        output,
        output_geo,
        now,
        lock_pending,
        cpu: prepare_cpu,
    } = ready;
    // This frame's CPU time: its own drawing, `prepare`'s and the scene's
    // share (`frame_stats.rs`).
    let now_instant = std::time::Instant::now();
    let started = now_instant
        .checked_sub(prepare_cpu + scene_cpu)
        .unwrap_or(now_instant);
    let scene = &input.scene;
    let pointer_here = output_geo.to_f64().contains(input.pointer);
    let scale = crate::scale::output_scale(&output);

    let direct_scanout = state.config.direct_scanout;
    let flags = frame_flags(direct_scanout);
    let Pila {
        backend,
        blur,
        capture,
        cursor_theme,
        clock,
        hidden_frames_dirty,
        ..
    } = state;
    let Some(Backend::Udev(data)) = backend else {
        return;
    };
    let UdevData {
        gpus,
        primary,
        outputs,
        devices,
        dmabuf_feedback,
        ..
    } = &mut **data;
    // The device's other outputs with a commit on its way: a commit that
    // switches the primary plane between the swapchain and a client's buffer
    // may wait in the kernel for theirs (seen on amdgpu leaving direct
    // scanout: the `direct scanout` log below says how many).
    let others_pending = outputs
        .iter()
        .filter(|(other, surface)| {
            **other != id && other.node == id.node && surface.crtc_state.frame_pending
        })
        .count();
    // Checked above; the commits applied since (`signal_commit_timers`)
    // can't take an output away, but nothing is lost by not counting on it.
    let Some(surface) = outputs.get_mut(&id) else {
        return;
    };
    let Some(drm_output) = surface.drm_output.as_mut() else {
        return;
    };
    let Some(render_node) = devices.get(&id.node).map(|device| device.render_node) else {
        return;
    };
    // On another GPU than the primary: rendered there, copied to this one.
    let format = drm_output.with_compositor(|compositor| compositor.format());
    let Some(mut lent) = super::draw_renderer(gpus, primary, &render_node, format) else {
        return;
    };
    // Pila's elements are built on the primary's renderer either way.
    let renderer = match &mut lent {
        OutputRenderer::Primary(lent) | OutputRenderer::Secondary(lent) => lent.as_mut(),
    };
    surface.redraw = None;
    let request = surface.crtc_state.request.take();
    // Held to be drawn now, its margin before the vblank (`render_late.rs`).
    let held = surface.crtc_state.render_at.take();
    // How long before its vblank a held frame was ready, if it is already.
    let mut spare = None;

    let elements = render::frame_elements(
        renderer,
        blur,
        cursor_theme,
        input,
        &output.name(),
        output_geo,
        scale,
        pointer_here,
    );

    let cursor_wl_surface = input.cursor_wl_surface();
    // What failed at the DRM level, and how.
    let mut failure: Option<(&str, String)> = None;
    // Captures filled from this frame, and the time they took.
    let mut composited = Vec::new();
    let mut capture_cpu = Duration::ZERO;
    // Rendered, with nothing new to show: no frame (`frame_stats.rs`, idle).
    let mut nothing_new = false;
    // The frame is a client's buffer as it is, on the primary plane.
    let mut direct = false;
    // What the GPU drew, to measure its time (`frame_stats.rs`).
    let mut gpu = None;
    let clear = render::clear_color(scene);
    // A fullscreen game that asks for it gets its frames shown at once, torn
    // (`tearing.rs`): an asynchronous page flip, which Smithay makes only when
    // nothing but the primary plane changed, and else a normal one.
    let presentation = if scene.tears(output_geo) {
        PresentationMode::Async
    } else {
        PresentationMode::VSync
    };
    // The elements' states and whether nothing changed, or why it failed.
    let render_started = std::time::Instant::now();
    let rendered = match &mut lent {
        OutputRenderer::Primary(lent) => {
            let renderer = lent.as_mut();
            match drm_output.render_frame(renderer, &elements, clear, flags, presentation) {
                Ok(result) => {
                    direct = matches!(result.primary_element, PrimaryPlaneElement::Element(_));
                    match &result.primary_element {
                        PrimaryPlaneElement::Swapchain(element) => {
                            surface.swapchain_format = Some(element.buffer().format());
                            gpu = GpuFrame::new(&element.sync, now, clock.now().into());
                        }
                        // Looked up only when the plane changes to it.
                        PrimaryPlaneElement::Element(element) if !surface.direct => {
                            surface.client_format = dmabuf_format(element, renderer);
                        }
                        PrimaryPlaneElement::Element(_) => {}
                    }
                    // Why a client's buffer wasn't shown as it is: its format
                    // and modifier, once per change (a buffer the plane
                    // refuses, before and after its client is told the
                    // plane's formats).
                    let refused = if direct_scanout && !direct {
                        refused_scanout(&elements, &result.states, renderer)
                    } else {
                        None
                    };
                    if surface.refused_scanout != refused {
                        if let Some((format, reason)) = &refused {
                            tracing::debug!(
                                target: "pila::frames",
                                output = output.name(),
                                ?format,
                                ?reason,
                                "direct scanout refused"
                            );
                        }
                        surface.refused_scanout = refused;
                    }
                    if !result.is_empty {
                        // Captures of this output take the frame just
                        // composited, blitted, rather than drawing the scene
                        // again (`Captures::take_composited`). All of it:
                        // the buffer may hold anything. (On another GPU the
                        // frame is a copy there: they're drawn from the
                        // scene, `process_captures`.)
                        let capture_started = std::time::Instant::now();
                        for job in capture.take_composited(&output, &elements, pointer_here) {
                            let (size, scale) = (job.size, job.scale);
                            let mut blit =
                                |renderer: &mut GlesRenderer, target: &mut GlesTarget<'_>| {
                                    result
                                        .blit_frame_result(
                                            size,
                                            Transform::Normal,
                                            scale,
                                            renderer,
                                            target,
                                            [Rectangle::from_size(size)],
                                            std::iter::empty::<Id>(),
                                        )
                                        .map_err(|err| format!("{err:?}"))
                                };
                            let source = Source::Draw(&mut blit);
                            let copied =
                                copy_frame(renderer, &job.buffer, capture, size, scale, source);
                            composited.push((job, copied));
                        }
                        capture_cpu = capture_started.elapsed();
                    }
                    Ok((result.states, result.is_empty))
                }
                Err(err) => Err(format!("{err:?}")),
            }
        }
        OutputRenderer::Secondary(lent) => {
            let elements: Vec<_> = elements
                .into_iter()
                .map(super::secondary::PrimaryGpuElement)
                .collect();
            drm_output
                .render_frame(lent, &elements, clear, flags, presentation)
                .map(|result| {
                    if let PrimaryPlaneElement::Swapchain(element) = &result.primary_element {
                        gpu = GpuFrame::new(&element.sync, now, clock.now().into());
                    }
                    (result.states, result.is_empty)
                })
                .map_err(|err| format!("{err:?}"))
        }
    };
    drop(lent);
    let render_time = render_started.elapsed().saturating_sub(capture_cpu);
    match rendered {
        Ok((states, is_empty)) => {
            *hidden_frames_dirty |=
                frame::update_primary_outputs(scene, cursor_wl_surface, &output, &states);
            // The formats this output's primary plane takes, for a surface
            // it showed as it is or tried to; back to the usual ones for the
            // others, and for all with direct scanout off.
            if let Some(scanout) = &surface.scanout_feedback {
                let scanout = if direct_scanout {
                    scanout
                } else {
                    &*dmabuf_feedback
                };
                frame::send_dmabuf_feedback(
                    scene,
                    cursor_wl_surface,
                    &output,
                    &states,
                    dmabuf_feedback,
                    scanout,
                );
            }
            if !is_empty {
                let feedback =
                    frame::presentation_feedback(scene, cursor_wl_surface, &output, &states);
                let queue_started = std::time::Instant::now();
                match drm_output.queue_frame(Some(feedback)) {
                    Ok(()) => {
                        surface.crtc_state.frame_pending = true;
                        surface.crtc_state.queued_held = held.is_some();
                        // What it took to be ready, once its GPU work is
                        // done too (`render_late.rs`).
                        let waits = if direct {
                            render_late::Waits::Nothing
                        } else if gpu.is_some() {
                            render_late::Waits::Fence
                        } else {
                            render_late::Waits::Unknown
                        };
                        spare = surface
                            .render_timing
                            .queued(now, clock.now().into(), waits, held);
                        // Going to a client's buffer or back is a change of
                        // the plane's that the kernel checks anew, in the
                        // test Smithay makes while rendering and in the
                        // commit: where the time of such a frame goes.
                        if std::mem::replace(&mut surface.direct, direct) != direct {
                            tracing::debug!(
                                target: "pila::frames",
                                output = output.name(),
                                on = direct,
                                render_ms = render_time.as_secs_f64() * 1000.0,
                                queue_ms = queue_started.elapsed().as_secs_f64() * 1000.0,
                                others_pending,
                                client = ?surface.client_format,
                                swapchain = ?surface.swapchain_format,
                                "direct scanout"
                            );
                        }
                        // The first frame after a modeset starts a new grid
                        // of vblanks: nothing to measure it against.
                        surface.crtc_state.queued_request = request
                            .filter(|_| !surface.crtc_state.first_frame)
                            .map(|r| Request {
                                drawn: Some(now),
                                after: in_flight_shown(
                                    surface.last_vblank_time,
                                    r.in_flight,
                                    surface.crtc_state.torn,
                                ),
                                ..r
                            });
                        if scene.locked {
                            surface.crtc_state.lock_frame = LockFrame::Queued;
                        }
                    }
                    Err(err) => {
                        surface.needs_redraw();
                        failure = Some(("queue", format!("{err:?}")));
                    }
                }
            } else {
                nothing_new = true;
                if scene.locked {
                    // Nothing changed: the screen already shows the locked scene.
                    surface.crtc_state.lock_frame = LockFrame::Shown;
                }
            }
        }
        Err(err) => {
            surface.needs_redraw();
            failure = Some(("render", err));
        }
    }
    if !scene.locked {
        surface.crtc_state.lock_frame = LockFrame::None;
    }
    let queued = surface.crtc_state.frame_pending;
    if held.is_some() {
        surface.crtc_state.back_to_back =
            back_to_back_after_held(surface.crtc_state.back_to_back, queued);
    }
    let wait_over = surface
        .crtc_state
        .idle_until
        .is_none_or(|until| now >= until);
    surface.crtc_state.idle_streak = idle_streak(
        surface.crtc_state.idle_streak,
        nothing_new,
        queued,
        wait_over,
    );
    surface.crtc_state.idle_until = nothing_new.then(|| {
        let period = crate::frame_stats::refresh_period(&output);
        idle_until(surface.last_vblank_time, now, period)
    });
    let mut retry = None;
    if let Some((stage, err)) = failure {
        retry = surface.crtc_state.recovery.failed();
        if surface.crtc_state.recovery.should_log() {
            warn!(
                output = output.name(),
                stage,
                err,
                failures = surface.crtc_state.recovery.failures(),
                retry_ms = retry.map(|d| d.as_millis() as u64),
                "frame failed; trying again later"
            );
        }
    } else if queued {
        let failures = surface.crtc_state.recovery.succeeded();
        if failures > 0 {
            info!(output = output.name(), failures, "output recovered");
        }
    }
    if let Some(delay) = retry {
        data.schedule_recovery(id, delay);
    }

    if lock_pending {
        maybe_confirm_lock(state);
    }
    let captures = composited.len() as u32;
    for (job, copied) in composited {
        state.finish_composited(job, copied, &output);
    }
    for _ in 0..captures {
        state.record_capture(capture_cpu / captures);
    }
    if queued {
        // The captures are counted apart (`frame_stats.rs`).
        let cpu = started.elapsed().saturating_sub(capture_cpu);
        tracing::trace!(
            target: "pila::frames",
            output = output.name(),
            at_vblank,
            cpu_us = cpu.as_micros() as u64,
            animating = state.animating(),
            "queued"
        );
        let first = match &mut state.backend {
            Some(Backend::Udev(data)) => data
                .outputs
                .get_mut(&id)
                .is_some_and(|s| std::mem::take(&mut s.crtc_state.first_frame)),
            _ => false,
        };
        if first {
            tracing::debug!(
                target: "pila::frames",
                output = output.name(),
                cpu_ms = cpu.as_secs_f64() * 1000.0,
                "first frame after setting the output up"
            );
        } else {
            if direct {
                state.record_direct_frame(&output);
            }
            if let Some(hold) = held {
                state.record_held_frame(&output, hold.margin());
            }
            if let Some(spare) = spare {
                state.record_spare(&output, spare);
            }
            state.record_frame(&output, cpu);
            if let Some(gpu) = gpu {
                state.watch_gpu(&output, gpu);
            }
        }
    } else if nothing_new {
        state.record_idle_frame(&output, started.elapsed());
    }
    frame::send_frames(scene, cursor_wl_surface, &output, state.clock.now().into());
    state.post_render(&output);
}

/// A queued frame reached the screen: send presentation feedback, and draw
/// again if something changed meanwhile.
pub(super) fn frame_finish(
    state: &mut Pila,
    id: OutputId,
    metadata: &mut Option<DrmEventMetadata>,
) {
    let now = state.clock.now();
    let animating = state.animating();
    let Some(Backend::Udev(data)) = &mut state.backend else {
        return;
    };
    let Some(surface) = data.outputs.get_mut(&id) else {
        return;
    };
    // Every commit on its way sets it (a frame, `queue_frame`; a gamma
    // change): one that didn't is from before the CRTC was cleared and set
    // up again (an output turned off and on at once, `retrain_bad_links`),
    // delivered late. Taken for the new setup's, it would mark a frame not
    // yet queued as shown, or confirm a lock before its frame is.
    if !std::mem::take(&mut surface.crtc_state.frame_pending) {
        debug!(
            name = surface.output.name(),
            "a vblank from before the output was set up again: ignored"
        );
        return;
    }
    // A gamma change's vblank (`gamma.rs`), not a frame's: nothing new was
    // shown, and `last_vblank` stays the last frame's, so a running
    // animation counts this refresh as missed.
    if std::mem::take(&mut surface.crtc_state.gamma_in_flight) {
        render_output(state, id, true);
        return;
    }
    if surface.crtc_state.lock_frame == LockFrame::Queued {
        surface.crtc_state.lock_frame = LockFrame::Shown;
    }

    let frame_duration = crate::frame_stats::refresh_period(&surface.output);
    // The frame was flipped at once, not at a vblank (`tearing.rs`): it has
    // no place on the grid of refreshes, so it misses none and isn't late.
    let torn = surface.drm_output.as_ref().is_some_and(|drm_output| {
        drm_output.with_pending_frame(|frame| {
            frame.is_some_and(|frame| frame.presentation_mode == PresentationMode::Async)
        })
    });
    // The kernel's vblank timestamp, when it gives a monotonic one.
    let hw_time = metadata.as_ref().and_then(|m| match m.time {
        DrmEventTime::Monotonic(time) if !time.is_zero() => Some(time),
        _ => None,
    });
    let seq = metadata.as_ref().map_or(0, |m| m.sequence as u64);
    // While an animation runs every vblank should show a new frame: the
    // vblanks since the last frame shown, beyond one, were missed (if it's
    // still running: the pause after one ends doesn't count).
    let missed = match metadata
        .is_some()
        .then_some(surface.crtc_state.last_vblank)
        .flatten()
    {
        Some(last) if !torn => {
            missed_refreshes(last, seq, surface.crtc_state.back_to_back && animating)
        }
        _ => 0,
    };
    // A frame asked for in time to show at a vblank, shown at a later one
    // (`frame_stats::late_refreshes`). With adaptive sync vblanks have no
    // fixed grid to measure by.
    let shown = hw_time.unwrap_or_else(|| now.into());
    let request = surface.crtc_state.queued_request.take();
    let late = request
        .filter(|_| !surface.crtc_state.presented_variable && !torn)
        .map_or(0, |r| {
            crate::frame_stats::late_refreshes(r.at, r.after, r.vblank, shown, frame_duration)
        });
    // What made it late: held back by the wait, drawn late, or shown late
    // after being drawn in time (the GPU, the commit).
    if let Some(r) = request.filter(|_| late > 0) {
        let ms = |d: Duration| (d.as_secs_f64() * 1e5).round() / 100.0;
        debug!(
            target: "pila::frames",
            output = surface.output.name(),
            in_flight = r.in_flight,
            held = r.held,
            drawn_after_ms = r.drawn.map(|d| ms(d.saturating_sub(r.at))),
            shown_after_ms = ms(shown.saturating_sub(r.at)),
            period_ms = ms(frame_duration),
            "late frame: asked for, drawn, shown"
        );
    }
    // A frame held to be drawn late (`render_late.rs`) that still came late:
    // that output draws at once for a while.
    if std::mem::take(&mut surface.crtc_state.queued_held) && (late > 0 || missed > 0) {
        surface.render_timing.showed_late(now.into());
        debug!(
            target: "pila::frames",
            output = surface.output.name(),
            "render late: a held frame came late; drawing at once for a while"
        );
    }
    surface.crtc_state.torn = torn;
    // From the first redraw asked for with something new to the vblank that
    // showed it (`frame_stats.rs`, latency).
    let latency = request.map(|r| shown.saturating_sub(r.at));
    surface.crtc_state.last_vblank = metadata.is_some().then_some(seq);
    // The time of a torn flip isn't a vblank's: the grid the next vblank is
    // reckoned from (`idle_until`, `commit_pacing`) stays the last one's.
    if !torn || surface.last_vblank_time.is_none() {
        surface.last_vblank_time = Some(hw_time.unwrap_or_else(|| now.into()));
    }
    tracing::trace!(target: "pila::frames", output = surface.output.name(), seq, missed, torn, "shown");
    let output = surface.output.clone();
    let vsync = if torn {
        wp_presentation_feedback::Kind::empty()
    } else {
        wp_presentation_feedback::Kind::Vsync
    };
    let (time, flags) = match hw_time {
        Some(time) => (
            time.into(),
            vsync
                | wp_presentation_feedback::Kind::HwClock
                | wp_presentation_feedback::Kind::HwCompletion,
        ),
        None => (now, vsync),
    };
    // Powered off meanwhile: the vblank belongs to the frame that was
    // dropped with the surface.
    let Some(drm_output) = &mut surface.drm_output else {
        return;
    };
    // With adaptive sync the period varies: clients are given the shortest,
    // the mode's (wp-presentation's "fastest rate").
    let variable = drm_output.with_compositor(|c| c.vrr_enabled());
    let refresh = if variable {
        Refresh::Variable(frame_duration)
    } else {
        Refresh::fixed(frame_duration)
    };
    if variable != surface.crtc_state.presented_variable {
        surface.crtc_state.presented_variable = variable;
        debug!(
            target: "pila::frames",
            name = surface.output.name(),
            variable,
            "refresh reported to clients"
        );
    }
    match drm_output.frame_submitted() {
        Ok(Some(mut feedback)) => {
            feedback.presented(time, refresh, seq, flags);
        }
        Ok(None) | Err(FrameError::EmptyFrame) => {}
        Err(err) => warn!(?err, "frame submission failed"),
    }

    // A late frame during an animation is counted once.
    let missed = u32::try_from(missed).unwrap_or(u32::MAX).max(late);
    state.record_missed(&output, missed);
    state.record_late(&output, late);
    if let Some(latency) = latency {
        state.record_latency(&output, latency);
    }
    if torn {
        state.record_torn_frame(&output);
    }
    if state.lock_pending() {
        maybe_confirm_lock(state);
    }
    render_output(state, id, true);
    let animating = state.animating();
    if let Some(Backend::Udev(data)) = &mut state.backend {
        if let Some(surface) = data.outputs.get_mut(&id) {
            // A frame held to be drawn late (`render_late.rs`) is on its
            // way too.
            let on_its_way =
                surface.crtc_state.frame_pending || surface.crtc_state.render_at.is_some();
            surface.crtc_state.back_to_back = back_to_back(animating, on_its_way);
        }
    }
}

/// Until when an output that rendered nothing new at `now` waits before
/// rendering again: half a period before the next vblank (`last` is one
/// it showed, if any), or the one after if that's past. So it renders at
/// most once per refresh, however often other outputs' input asks, and a
/// change that comes meanwhile is drawn in time for the vblank it would
/// have made anyway, unless it comes in the last half of the period. Not
/// frame pacing: frames with something to show are paced by vblanks.
pub(super) fn idle_until(last: Option<Duration>, now: Duration, period: Duration) -> Duration {
    let next = match last {
        Some(last) => crate::commit_pacing::next_vblank(last, now, period),
        None => now + period,
    };
    let deadline = next.saturating_sub(period / 2);
    if deadline > now {
        deadline
    } else {
        deadline + period
    }
}

/// How long a redraw of an output waits, given when its wait after a redraw
/// with nothing new ends (`idle_until`), whether what was asked brings
/// something new to show (`content`) and how many redraws in a row found
/// nothing new (`idle_streak`). `None`: draw now.
fn idle_wait(
    idle_until: Option<Duration>,
    now: Duration,
    content: crate::redraw::Content,
    idle_streak: u32,
    lock_pending: bool,
) -> Option<Duration> {
    let until = idle_until.filter(|until| now < *until)?;
    use crate::redraw::Content;
    // What's said to be new doesn't wait, as long as it turns out new: a
    // client's picture once (it may show nowhere: a second redraw in a row
    // that finds nothing means it wasn't), Pila's own change up to
    // `SURE_STREAK` times; what may bring nothing waits.
    let surely_new = match content {
        Content::Sure => idle_streak < SURE_STREAK,
        Content::Client => idle_streak <= 1,
        Content::Maybe => false,
    };
    (!surely_new && !lock_pending).then(|| until - now)
}

/// How many redraws in a row may find nothing new before even a change of
/// Pila's own waits (`idle_wait`): a bound on a loop of requests for what
/// shows elsewhere, far above what a real change meets.
const SURE_STREAK: u32 = 8;

/// The redraws in a row with nothing new after this one (`idle_wait`): one
/// more if it found nothing (the first of a new wait if the last wait was
/// over: once per refresh, something said new may skip it again), none
/// once a frame is queued.
fn idle_streak(streak: u32, nothing_new: bool, queued: bool, wait_over: bool) -> u32 {
    match (nothing_new, queued) {
        (true, _) if wait_over => 1,
        (true, _) => streak.saturating_add(1),
        (false, true) => 0,
        (false, false) => streak,
    }
}

/// Whether the next frame on an output follows the one just shown with no pause
/// meant: an animation runs, and this output has a frame on its way (queued,
/// or held to be drawn late: `render_late.rs`). One that
/// has nothing new while another output animates just rests, and its idle
/// vblanks aren't missed refreshes.
fn back_to_back(animating: bool, frame_queued: bool) -> bool {
    animating && frame_queued
}

/// Whether an output is still back to back (`back_to_back`) once a frame held
/// to be drawn late (`render_late.rs`) was drawn, `queued` or not.
/// A hold isn't a frame: one that found nothing new leaves it resting.
fn back_to_back_after_held(back_to_back: bool, queued: bool) -> bool {
    back_to_back && queued
}

/// The vblank that showed the frame in flight when the one being drawn was
/// asked for, which that one is due a period after
/// (`frame_stats::late_refreshes`): `last`, the last vblank shown. Unless
/// that frame was `torn`: shown at once, at no vblank, it left `last` where
/// it was, maybe seconds before (`frame_finish`), and the frame after it is
/// due as if none had been in flight.
fn in_flight_shown(last: Option<Duration>, in_flight: bool, torn: bool) -> Option<Duration> {
    last.filter(|_| in_flight && !torn)
}

/// Refreshes missed from the vblank `last` that showed a frame to `seq` that
/// shows the next: all but one, if they were meant to be back to back.
fn missed_refreshes(last: u64, seq: u64, back_to_back: bool) -> u64 {
    if back_to_back {
        seq.saturating_sub(last + 1)
    } else {
        0
    }
}

/// Confirm a pending session lock once every output shows a locked frame.
pub(super) fn maybe_confirm_lock(state: &mut Pila) {
    let Some(Backend::Udev(data)) = &state.backend else {
        return;
    };
    let outputs = data
        .outputs
        .values()
        .map(|surface| (surface.drm_output.is_some(), surface.crtc_state.lock_frame));
    if lock_shown(outputs) {
        state.confirm_lock();
    }
}

/// Whether a pending lock can be confirmed, given each output's `(on, its
/// lock frame)`: every output that's on shows a locked frame. One that's off
/// shows nothing, locked or not; with none on, a locker would otherwise wait
/// forever.
fn lock_shown(outputs: impl IntoIterator<Item = (bool, LockFrame)>) -> bool {
    outputs
        .into_iter()
        .filter(|(on, _)| *on)
        .all(|(_, frame)| frame == LockFrame::Shown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redraw::Content;

    #[test]
    fn a_clients_buffer_takes_the_primary_plane_only_with_direct_scanout() {
        let primary =
            FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT | FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT_ANY;
        assert!(frame_flags(true).contains(FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT_ANY));
        assert!(!frame_flags(false).intersects(primary));
        // Either way the cursor has its plane, and nothing goes on an
        // overlay plane.
        for flags in [frame_flags(true), frame_flags(false)] {
            assert!(flags.contains(FrameFlags::ALLOW_CURSOR_PLANE_SCANOUT));
            assert!(!flags.contains(FrameFlags::ALLOW_OVERLAY_PLANE_SCANOUT));
        }
    }

    #[test]
    fn the_lock_waits_for_every_output_on_to_show_it() {
        use LockFrame::*;
        assert!(lock_shown([(true, Shown), (true, Shown)]));
        // Queued isn't shown yet, and an output not drawn yet isn't either.
        assert!(!lock_shown([(true, Shown), (true, Queued)]));
        assert!(!lock_shown([(true, None), (true, Shown)]));
        // An output that's off doesn't count, whatever it last showed.
        assert!(lock_shown([(true, Shown), (false, None)]));
        // None on (or none at all): nothing to wait for.
        assert!(lock_shown([(false, Queued)]));
        assert!(lock_shown([]));
    }

    #[test]
    fn an_output_with_nothing_new_waits_until_half_a_period_before_a_vblank() {
        let ms = Duration::from_millis;
        let period = ms(16);
        // A vblank at 100 ms: the next at 116, so until 108.
        assert_eq!(idle_until(Some(ms(100)), ms(102), period), ms(108));
        // Past 108 already: until half a period before the one after.
        assert_eq!(idle_until(Some(ms(100)), ms(110), period), ms(124));
        // Rendering right at the end of a wait waits a whole period more:
        // at most one render with nothing new per refresh.
        assert_eq!(idle_until(Some(ms(100)), ms(108), period), ms(124));
        // No vblank known yet: half a period.
        assert_eq!(idle_until(None, ms(50), period), ms(58));
    }

    #[test]
    fn a_clients_frame_after_a_redraw_with_nothing_new_isnt_held_back() {
        let ms = Duration::from_millis;
        let period = ms(16);
        // A redraw with nothing new at 110, in the second half of the period
        // before the vblank at 116: the next waits until 124.
        let until = Some(idle_until(Some(ms(100)), ms(110), period));
        // A client's new frame at 111 is drawn at once, in time for 116.
        assert_eq!(idle_wait(until, ms(111), Content::Client, 1, false), None);
        // A redraw that may bring nothing (a commit with no new picture) waits.
        assert_eq!(
            idle_wait(until, ms(111), Content::Maybe, 1, false),
            Some(ms(13))
        );
        // Nor does anything wait with a lock waiting for its frame (with a
        // frame in flight, `prepare` doesn't ask: its vblank draws it).
        assert_eq!(idle_wait(until, ms(111), Content::Maybe, 1, true), None);
    }

    #[test]
    fn redraws_said_to_bring_something_new_that_dont_wait_only_once() {
        let ms = Duration::from_millis;
        let until = Some(idle_until(Some(ms(100)), ms(110), ms(16)));
        // After one redraw with nothing new, something said new is drawn at
        // once; if that found nothing either, the next waits like the rest:
        // a client sending what shows nowhere (covered, say) in a loop of
        // fifo barriers can't make an output redraw as fast as it loops.
        assert_eq!(idle_wait(until, ms(111), Content::Client, 1, false), None);
        assert_eq!(
            idle_wait(until, ms(111), Content::Client, 2, false),
            Some(ms(13))
        );
        assert_eq!(
            idle_wait(until, ms(111), Content::Client, 5, false),
            Some(ms(13))
        );
        // The streak starts over with each wait (once per refresh), and
        // with a frame.
        assert_eq!(idle_streak(5, true, false, true), 1);
        assert_eq!(idle_streak(1, true, false, false), 2);
        assert_eq!(idle_streak(5, false, true, false), 0);
        assert_eq!(idle_streak(5, false, false, false), 5);
        // A change of Pila's own (an animation starting, a window moved)
        // never waits: it shows, whatever came before.
        assert_eq!(idle_wait(until, ms(111), Content::Sure, 5, false), None);
        // Unless it keeps finding nothing (something of Pila's asking in a
        // loop for what shows elsewhere): then even it waits, so no output
        // redraws with nothing new more than a few times a refresh.
        assert_eq!(
            idle_wait(until, ms(111), Content::Sure, SURE_STREAK, false),
            Some(ms(13))
        );
    }

    #[test]
    fn an_output_at_rest_during_another_outputs_animation_misses_nothing() {
        // Its last frame showed during the animation, and nothing followed.
        assert!(!back_to_back(true, false));
        // 240 vblanks later something changes, with another animation running.
        assert_eq!(missed_refreshes(1000, 1241, back_to_back(true, false)), 0);
    }

    /// Seen on bare metal: DP-1 had a redraw held to render late while eDP-1
    /// animated, the held redraw found nothing new, and DP-1 rested for 4 s:
    /// its next frame counted 240 missed refreshes.
    #[test]
    fn a_held_redraw_that_finds_nothing_new_leaves_the_output_resting() {
        // At its vblank, with a redraw held and an animation elsewhere.
        let held = back_to_back(true, true);
        assert!(held);
        // Drawn, nothing new: no frame queued, the output rests.
        let after = back_to_back_after_held(held, false);
        assert_eq!(missed_refreshes(1000, 1241, after), 0);
        // A frame queued: it stays back to back.
        assert!(back_to_back_after_held(held, true));
    }

    #[test]
    fn a_frame_late_during_an_animation_is_missed_refreshes() {
        assert!(back_to_back(true, true));
        assert_eq!(missed_refreshes(1000, 1001, back_to_back(true, true)), 0);
        assert_eq!(missed_refreshes(1000, 1003, back_to_back(true, true)), 2);
        // With no animation the pause is meant.
        assert_eq!(missed_refreshes(1000, 1003, back_to_back(false, true)), 0);
    }

    #[test]
    fn a_frame_after_a_torn_one_isnt_late_by_the_time_the_game_tore() {
        use crate::frame_stats::late_refreshes;
        // A game tore for 167 refreshes (a TTY run, 60.01 Hz): torn frames
        // leave the last vblank shown where it was before.
        let period = Duration::from_micros(16_664);
        let before_tearing = Duration::from_secs(100);
        let vblank = before_tearing + period * 167;
        // Asked for just after a vblank, with a torn frame in flight, and
        // shown at the next vblank: in time.
        let at = vblank + Duration::from_micros(280);
        let shown = vblank + period;
        let after = in_flight_shown(Some(before_tearing), true, true);
        assert_eq!(late_refreshes(at, after, before_tearing, shown, period), 0);
        // A frame in flight that was shown at a vblank is still the one the
        // next is due after: shown a vblank later, it's late.
        let after = in_flight_shown(Some(vblank), true, false);
        assert_eq!(after, Some(vblank));
        assert_eq!(late_refreshes(at, after, vblank, shown + period, period), 1);
    }
}
