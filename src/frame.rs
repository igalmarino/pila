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
};

use crate::render::Scene;

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
/// frame described by `states`. Presentation feedback (and later per-output
/// fractional scale) is keyed on it.
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
    for entry in &scene.windows {
        entry.window.with_surfaces(update);
    }
    for entry in scene.layers() {
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
            surface_primary_scanout_output,
            flags,
        );
    }
    for entry in scene.layers() {
        entry.layer.take_presentation_feedback(
            &mut feedback,
            surface_primary_scanout_output,
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

/// Frame callbacks for everything in the scene, so clients draw their next
/// frame. Single output for now: every surface counts as on `output`.
pub fn send_frames(scene: &Scene, cursor: Option<&WlSurface>, output: &Output, time: Duration) {
    let on_output = |_: &WlSurface, _: &_| Some(output.clone());
    for entry in &scene.windows {
        entry
            .window
            .send_frame(output, time, Some(Duration::ZERO), on_output);
    }
    for entry in scene.layers() {
        entry
            .layer
            .send_frame(output, time, Some(Duration::ZERO), on_output);
    }
    for surface in other_trees(scene, cursor) {
        send_frames_surface_tree(surface, output, time, Some(Duration::ZERO), on_output);
    }
}
