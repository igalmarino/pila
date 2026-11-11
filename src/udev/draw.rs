//! Drawing outputs, paced by their vblanks, and confirming a session lock.

use super::*;

use smithay::{
    backend::{
        drm::compositor::PrimaryPlaneElement,
        renderer::{element::Id, gles::GlesTarget},
    },
    utils::{Rectangle, Transform},
};

use crate::capture::{copy_frame, Source};

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
    // An output that had nothing new to show just now waits its turn
    // (`idle_wait`), unless what's asked surely shows or a lock waits.
    let now: Duration = state.clock.now().into();
    let lock_pending = state.lock_pending();
    if let Some(Backend::Udev(data)) = &mut state.backend {
        let wait = data.outputs.get(&id).and_then(|surface| {
            let wait = idle_wait(
                surface.idle_until,
                now,
                surface.redraw_content,
                surface.idle_streak,
                surface.frame_pending,
                lock_pending,
            );
            wait.filter(|_| surface.redraw_needed)
        });
        if let Some(delay) = wait {
            if let Some(request) = data
                .outputs
                .get_mut(&id)
                .and_then(|surface| surface.request.as_mut())
            {
                request.held = true;
            }
            data.schedule_idle_redraw(id, delay);
            return None;
        }
    }
    let Some(Backend::Udev(data)) = &state.backend else {
        return None;
    };
    let surface = data.outputs.get(&id)?;
    if surface.frame_pending
        || !surface.redraw_needed
        || surface.drm_output.is_none()
        || !data.session.is_active()
        || surface.recovery.blocks_render()
    {
        return None;
    }
    let output = surface.output.clone();
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
        cursor_theme,
        dmabuf_feedback,
        ..
    } = &mut **data;
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
    surface.redraw_needed = false;
    surface.redraw_content = crate::redraw::Content::Maybe;
    let request = surface.request.take();

    let mut elements = Vec::new();
    if pointer_here {
        elements =
            render::pointer_elements(renderer, input, Some(cursor_theme), output_geo.loc, scale);
    }
    elements.extend(render::output_elements(
        renderer,
        blur,
        scene,
        &output.name(),
        output_geo,
        output_geo.loc,
        scale,
    ));

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
    let clear = render::clear_color(scene);
    // The elements' states and whether nothing changed, or why it failed.
    let render_started = std::time::Instant::now();
    let rendered = match &mut lent {
        OutputRenderer::Primary(lent) => {
            let renderer = lent.as_mut();
            match drm_output.render_frame(renderer, &elements, clear, flags) {
                Ok(result) => {
                    direct = matches!(result.primary_element, PrimaryPlaneElement::Element(_));
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
                .render_frame(lent, &elements, clear, flags)
                .map(|result| (result.states, result.is_empty))
                .map_err(|err| format!("{err:?}"))
        }
    };
    drop(lent);
    let render_time = render_started.elapsed().saturating_sub(capture_cpu);
    match rendered {
        Ok((states, is_empty)) => {
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
                        surface.frame_pending = true;
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
                                "direct scanout"
                            );
                        }
                        // The first frame after a modeset starts a new grid
                        // of vblanks: nothing to measure it against.
                        surface.queued_request =
                            request.filter(|_| !surface.first_frame).map(|r| Request {
                                drawn: Some(now),
                                after: surface.last_vblank_time.filter(|_| r.in_flight),
                                ..r
                            });
                        if scene.locked {
                            surface.lock_frame = LockFrame::Queued;
                        }
                    }
                    Err(err) => {
                        surface.redraw_needed = true;
                        failure = Some(("queue", format!("{err:?}")));
                    }
                }
            } else {
                nothing_new = true;
                if scene.locked {
                    // Nothing changed: the screen already shows the locked scene.
                    surface.lock_frame = LockFrame::Shown;
                }
            }
        }
        Err(err) => {
            surface.redraw_needed = true;
            failure = Some(("render", err));
        }
    }
    if !scene.locked {
        surface.lock_frame = LockFrame::None;
    }
    let queued = surface.frame_pending;
    let wait_over = surface.idle_until.is_none_or(|until| now >= until);
    surface.idle_streak = idle_streak(surface.idle_streak, nothing_new, queued, wait_over);
    surface.idle_until = nothing_new.then(|| {
        let period = crate::frame_stats::refresh_period(&output);
        idle_until(surface.last_vblank_time, now, period)
    });
    let mut retry = None;
    if let Some((stage, err)) = failure {
        retry = surface.recovery.failed();
        if surface.recovery.should_log() {
            warn!(
                output = output.name(),
                stage,
                err,
                failures = surface.recovery.failures(),
                retry_ms = retry.map(|d| d.as_millis() as u64),
                "frame failed; trying again later"
            );
        }
    } else if queued {
        let failures = surface.recovery.succeeded();
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
                .is_some_and(|s| std::mem::take(&mut s.first_frame)),
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
            state.record_frame(&output, cpu);
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
    surface.frame_pending = false;
    // A gamma change's vblank (`gamma.rs`), not a frame's: nothing new was
    // shown, and `last_vblank` stays the last frame's, so a running
    // animation counts this refresh as missed.
    if std::mem::take(&mut surface.gamma_in_flight) {
        render_output(state, id, true);
        return;
    }
    if surface.lock_frame == LockFrame::Queued {
        surface.lock_frame = LockFrame::Shown;
    }

    let frame_duration = crate::frame_stats::refresh_period(&surface.output);
    // The kernel's vblank timestamp, when it gives a monotonic one.
    let hw_time = metadata.as_ref().and_then(|m| match m.time {
        DrmEventTime::Monotonic(time) if !time.is_zero() => Some(time),
        _ => None,
    });
    let seq = metadata.as_ref().map_or(0, |m| m.sequence as u64);
    // While an animation runs every vblank should show a new frame: the
    // vblanks since the last frame shown, beyond one, were missed (if it's
    // still running: the pause after one ends doesn't count).
    let missed = match metadata.is_some().then_some(surface.last_vblank).flatten() {
        Some(last) => missed_refreshes(last, seq, surface.back_to_back && animating),
        None => 0,
    };
    // A frame asked for in time to show at a vblank, shown at a later one
    // (`frame_stats::late_refreshes`). With adaptive sync vblanks have no
    // fixed grid to measure by.
    let shown = hw_time.unwrap_or_else(|| now.into());
    let request = surface.queued_request.take();
    let late = request
        .filter(|_| !surface.presented_variable)
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
    surface.last_vblank = metadata.is_some().then_some(seq);
    surface.last_vblank_time = Some(hw_time.unwrap_or_else(|| now.into()));
    tracing::trace!(target: "pila::frames", output = surface.output.name(), seq, missed, "shown");
    let output = surface.output.clone();
    let (time, flags) = match hw_time {
        Some(time) => (
            time.into(),
            wp_presentation_feedback::Kind::Vsync
                | wp_presentation_feedback::Kind::HwClock
                | wp_presentation_feedback::Kind::HwCompletion,
        ),
        None => (now, wp_presentation_feedback::Kind::Vsync),
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
    if variable != surface.presented_variable {
        surface.presented_variable = variable;
        debug!(
            target: "pila::frames",
            name = surface.output.name(),
            variable,
            "refresh reported to clients"
        );
    }
    match drm_output.frame_submitted() {
        Ok(Some(Some(mut feedback))) => {
            feedback.presented(time, refresh, seq, flags);
        }
        Ok(_) => {}
        Err(err) => warn!(?err, "frame submission failed"),
    }

    // A late frame during an animation is counted once.
    let missed = u32::try_from(missed).unwrap_or(u32::MAX).max(late);
    state.record_missed(&output, missed);
    state.record_late(&output, late);
    if state.lock_pending() {
        maybe_confirm_lock(state);
    }
    render_output(state, id, true);
    let animating = state.animating();
    if let Some(Backend::Udev(data)) = &mut state.backend {
        if let Some(surface) = data.outputs.get_mut(&id) {
            surface.back_to_back = back_to_back(animating, surface.frame_pending);
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
    frame_pending: bool,
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
    (!surely_new && !frame_pending && !lock_pending).then(|| until - now)
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
/// meant: an animation runs, and this output has a frame on its way. One that
/// has nothing new while another output animates just rests, and its idle
/// vblanks aren't missed refreshes.
fn back_to_back(animating: bool, frame_queued: bool) -> bool {
    animating && frame_queued
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
        .map(|surface| (surface.drm_output.is_some(), surface.lock_frame));
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
        assert_eq!(
            idle_wait(until, ms(111), Content::Client, 1, false, false),
            None
        );
        // A redraw that may bring nothing (a commit with no new picture) waits.
        assert_eq!(
            idle_wait(until, ms(111), Content::Maybe, 1, false, false),
            Some(ms(13))
        );
        // Nor does anything wait with a frame in flight (its vblank draws
        // it) or a lock waiting for its frame.
        assert_eq!(
            idle_wait(until, ms(111), Content::Maybe, 1, true, false),
            None
        );
        assert_eq!(
            idle_wait(until, ms(111), Content::Maybe, 1, false, true),
            None
        );
    }

    #[test]
    fn redraws_said_to_bring_something_new_that_dont_wait_only_once() {
        let ms = Duration::from_millis;
        let until = Some(idle_until(Some(ms(100)), ms(110), ms(16)));
        // After one redraw with nothing new, something said new is drawn at
        // once; if that found nothing either, the next waits like the rest:
        // a client sending what shows nowhere (covered, say) in a loop of
        // fifo barriers can't make an output redraw as fast as it loops.
        assert_eq!(
            idle_wait(until, ms(111), Content::Client, 1, false, false),
            None
        );
        assert_eq!(
            idle_wait(until, ms(111), Content::Client, 2, false, false),
            Some(ms(13))
        );
        assert_eq!(
            idle_wait(until, ms(111), Content::Client, 5, false, false),
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
        assert_eq!(
            idle_wait(until, ms(111), Content::Sure, 5, false, false),
            None
        );
        // Unless it keeps finding nothing (something of Pila's asking in a
        // loop for what shows elsewhere): then even it waits, so no output
        // redraws with nothing new more than a few times a refresh.
        assert_eq!(
            idle_wait(until, ms(111), Content::Sure, SURE_STREAK, false, false),
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

    #[test]
    fn a_frame_late_during_an_animation_is_missed_refreshes() {
        assert!(back_to_back(true, true));
        assert_eq!(missed_refreshes(1000, 1001, back_to_back(true, true)), 0);
        assert_eq!(missed_refreshes(1000, 1003, back_to_back(true, true)), 2);
        // With no animation the pause is meant.
        assert_eq!(missed_refreshes(1000, 1003, back_to_back(false, true)), 0);
    }
}
