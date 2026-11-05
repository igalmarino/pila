//! Bookkeeping after a frame is rendered, shared by backends: which output
//! each drawn surface is mainly shown on, presentation-time feedback, and
//! frame callbacks. Everything works from the `Scene` (and the cursor surface)
//! that was rendered, so while locked only lock surfaces are touched.

use std::time::Duration;

use smithay::{
    backend::renderer::element::{default_primary_scanout_output_compare, RenderElementStates},
    desktop::utils::{
        send_frames_surface_tree, surface_presentation_feedback_flags_from_states,
        surface_primary_scanout_output, take_presentation_feedback_surface_tree,
        update_surface_primary_scanout_output, with_surfaces_surface_tree,
        OutputPresentationFeedback,
    },
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    wayland::compositor::SurfaceData,
};

use crate::{animation::WindowEffect, render::Scene};

/// Surface trees that aren't windows or layer surfaces: lock surfaces and the
/// client cursor.
fn other_trees<'a>(
    scene: &'a Scene,
    cursor: Option<&'a WlSurface>,
) -> impl Iterator<Item = &'a WlSurface> {
    scene
        .lock_surfaces
        .iter()
        .map(|(surface, _)| surface)
        .chain(cursor)
}

/// Record `output` as the primary output of every surface it showed in the
/// frame described by `states`. Presentation feedback and the scale clients
/// are told (`scale.rs`) are keyed on it.
pub fn update_primary_outputs(
    scene: &Scene,
    cursor: Option<&WlSurface>,
    output: &Output,
    states: &RenderElementStates,
) {
    let update = |surface: &WlSurface, data: &_| {
        update_surface_primary_scanout_output(
            surface,
            output,
            data,
            states,
            default_primary_scanout_output_compare,
        );
    };
    // An animating window is drawn through a texture, so it isn't in
    // `states`: it keeps the output it had (`send_frames` paces it).
    for entry in scene
        .windows
        .iter()
        .filter(|e| e.offscreen_output().is_none())
    {
        entry.window.with_surfaces(update);
    }
    for entry in scene.layers().filter(|e| e.effect.is_none()) {
        entry.layer.with_surfaces(update);
    }
    for surface in other_trees(scene, cursor) {
        with_surfaces_surface_tree(surface, update);
    }
}

/// The presentation feedback of every surface shown on `output` in this
/// frame. Call after the frame was submitted, then mark it `presented`.
pub fn presentation_feedback(
    scene: &Scene,
    cursor: Option<&WlSurface>,
    output: &Output,
    states: &RenderElementStates,
) -> OutputPresentationFeedback {
    let mut feedback = OutputPresentationFeedback::new(output);
    let flags = |surface: &WlSurface, _: &_| {
        surface_presentation_feedback_flags_from_states(surface, states)
    };
    for entry in &scene.windows {
        entry.window.take_presentation_feedback(
            &mut feedback,
            |surface, data| paced_by(entry.offscreen_output(), surface, data),
            flags,
        );
    }
    for entry in scene.layers() {
        entry.layer.take_presentation_feedback(
            &mut feedback,
            |surface, data| paced_by(layer_offscreen(entry.effect.as_ref()), surface, data),
            flags,
        );
    }
    for surface in other_trees(scene, cursor) {
        take_presentation_feedback_surface_tree(
            surface,
            &mut feedback,
            surface_primary_scanout_output,
            flags,
        );
    }
    feedback
}

/// The output a window's or layer surface's surface is paced by: its
/// primary output, or while it's drawn through a texture (an animation, a
/// workspace slide; absent from the render states then) the one it's on
/// (`offscreen`).
fn paced_by(
    offscreen: Option<Option<Output>>,
    surface: &WlSurface,
    data: &SurfaceData,
) -> Option<Output> {
    match offscreen {
        Some(output) => output,
        None => surface_primary_scanout_output(surface, data),
    }
}

/// A layer surface's `offscreen` output for `paced_by`.
fn layer_offscreen(effect: Option<&WindowEffect>) -> Option<Option<Output>> {
    effect.map(|effect| effect.output.clone())
}

/// How often a surface that isn't shown anywhere (covered, or off every
/// output) still gets a frame callback, so its client isn't stalled forever.
const HIDDEN_FRAME_THROTTLE: Duration = Duration::from_secs(1);

/// Frame callbacks for the surfaces whose primary output is `output` (call
/// after `update_primary_outputs`), so clients draw their next frame paced by
/// that output. Surfaces shown nowhere get one every `HIDDEN_FRAME_THROTTLE`:
/// sending them one on every render would let an animating client that adds
/// no damage (covered, or on another output) draw as fast as pila loops.
pub fn send_frames(scene: &Scene, cursor: Option<&WlSurface>, output: &Output, time: Duration) {
    let throttle = Some(HIDDEN_FRAME_THROTTLE);
    for entry in &scene.windows {
        entry
            .window
            .send_frame(output, time, throttle, |surface, data| {
                paced_by(entry.offscreen_output(), surface, data)
            });
    }
    for entry in scene.layers() {
        entry
            .layer
            .send_frame(output, time, throttle, |surface, data| {
                paced_by(layer_offscreen(entry.effect.as_ref()), surface, data)
            });
    }
    for surface in other_trees(scene, cursor) {
        send_frames_surface_tree(
            surface,
            output,
            time,
            throttle,
            surface_primary_scanout_output,
        );
    }
}
