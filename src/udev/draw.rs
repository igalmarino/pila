//! Drawing outputs, paced by their vblanks, and confirming a session lock.

use super::*;

/// The idle callback queued by `UdevData::queue_redraw`.
pub(super) fn render_all(state: &mut Pila) {
    let Some(Backend::Udev(data)) = &mut state.backend else {
        return;
    };
    data.redraw_scheduled = false;
    let crtcs: Vec<_> = data.outputs.keys().copied().collect();
    for crtc in crtcs {
        render_output(state, crtc, false);
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
pub(super) fn render_output(state: &mut Pila, crtc: crtc::Handle, at_vblank: bool) {
    let Some(Backend::Udev(data)) = &state.backend else {
        return;
    };
    let Some(surface) = data.outputs.get(&crtc) else {
        return;
    };
    if surface.frame_pending
        || !surface.redraw_needed
        || surface.drm_output.is_none()
        || !data.session.is_active()
        || data.recovering
    {
        return;
    }
    let output = surface.output.clone();
    let Some(output_geo) = state.space.output_geometry(&output) else {
        return;
    };

    let started = std::time::Instant::now();
    // A destroyed cursor surface falls back to the default arrow.
    if matches!(&state.cursor_status, CursorImageStatus::Surface(s) if !s.alive()) {
        state.cursor_status = CursorImageStatus::default_named();
    }
    state.signal_commit_timers(&output);
    // Everything taken from `state` is collected before borrowing the backend.
    let input = state.frame_input();
    let scene = &input.scene;
    let pointer_here = output_geo.to_f64().contains(input.pointer);
    let lock_pending = state.lock_pending();
    let scale = crate::scale::output_scale(&output);

    let Pila { backend, blur, .. } = state;
    let Some(Backend::Udev(data)) = backend else {
        return;
    };
    let UdevData {
        renderer,
        outputs,
        cursor_theme,
        ..
    } = &mut **data;
    let surface = outputs.get_mut(&crtc).expect("checked above");
    let drm_output = surface.drm_output.as_mut().expect("checked above");
    surface.redraw_needed = false;

    let mut elements = Vec::new();
    if pointer_here {
        elements =
            render::pointer_elements(renderer, &input, Some(cursor_theme), output_geo.loc, scale);
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

    let rendered = drm_output.render_frame(
        renderer,
        &elements,
        render::clear_color(scene),
        FrameFlags::DEFAULT,
    );
    let cursor_wl_surface = input.cursor_wl_surface();
    let mut recover = false;
    match rendered {
        Ok(result) => {
            frame::update_primary_outputs(scene, cursor_wl_surface, &output, &result.states);
            if !result.is_empty {
                let feedback =
                    frame::presentation_feedback(scene, cursor_wl_surface, &output, &result.states);
                match drm_output.queue_frame(Some(feedback)) {
                    Ok(()) => {
                        surface.frame_pending = true;
                        if scene.locked {
                            surface.lock_frame = LockFrame::Queued;
                        }
                    }
                    Err(err) => {
                        warn!(?err, "failed to queue frame");
                        surface.redraw_needed = true;
                        recover = true;
                    }
                }
            } else if scene.locked {
                // Nothing changed: the screen already shows the locked scene.
                surface.lock_frame = LockFrame::Shown;
            }
        }
        Err(err) => {
            warn!(?err, "failed to render frame");
            surface.redraw_needed = true;
            recover = true;
        }
    }
    if !scene.locked {
        surface.lock_frame = LockFrame::None;
    }
    let queued = surface.frame_pending;
    if recover {
        data.schedule_recovery();
    }

    if lock_pending {
        maybe_confirm_lock(state);
    }
    if queued {
        let cpu = started.elapsed();
        tracing::trace!(
            target: "pila::frames",
            output = output.name(),
            at_vblank,
            cpu_us = cpu.as_micros() as u64,
            animating = state.animating(),
            "queued"
        );
        state.record_frame(&output, cpu);
    }
    frame::send_frames(scene, cursor_wl_surface, &output, state.clock.now().into());
    state.post_render(&output);
}

/// A queued frame reached the screen: send presentation feedback, and draw
/// again if something changed meanwhile.
pub(super) fn frame_finish(
    state: &mut Pila,
    crtc: crtc::Handle,
    metadata: &mut Option<DrmEventMetadata>,
) {
    let now = state.clock.now();
    let animating = state.animating();
    let Some(Backend::Udev(data)) = &mut state.backend else {
        return;
    };
    let Some(surface) = data.outputs.get_mut(&crtc) else {
        return;
    };
    surface.frame_pending = false;
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
    let missed = match (metadata.is_some(), surface.last_vblank) {
        (true, Some(last)) if surface.back_to_back && animating => seq.saturating_sub(last + 1),
        _ => 0,
    };
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
    match drm_output.frame_submitted() {
        Ok(Some(Some(mut feedback))) => {
            feedback.presented(time, Refresh::fixed(frame_duration), seq, flags);
        }
        Ok(_) => {}
        Err(err) => warn!(?err, "frame submission failed"),
    }

    state.record_missed(&output, u32::try_from(missed).unwrap_or(u32::MAX));
    if state.lock_pending() {
        maybe_confirm_lock(state);
    }
    render_output(state, crtc, true);
    let animating = state.animating();
    if let Some(Backend::Udev(data)) = &mut state.backend {
        if let Some(surface) = data.outputs.get_mut(&crtc) {
            surface.back_to_back = animating;
        }
    }
}

/// Confirm a pending session lock once every output shows a locked frame.
pub(super) fn maybe_confirm_lock(state: &mut Pila) {
    let Some(Backend::Udev(data)) = &state.backend else {
        return;
    };
    // A powered-off output shows nothing, locked or not.
    if data
        .outputs
        .values()
        .filter(|surface| surface.drm_output.is_some())
        .all(|surface| surface.lock_frame == LockFrame::Shown)
    {
        state.confirm_lock();
    }
}
