//! Bookkeeping after a frame is rendered, shared by backends: which output
//! each drawn surface is mainly shown on, presentation-time feedback, and
//! frame callbacks. Everything works from the `Scene` (and the cursor surface)
//! that was rendered, so while locked only lock surfaces are touched.

use std::time::Duration;

use smithay::{
    backend::renderer::element::{
        default_primary_scanout_output_compare, utils::select_dmabuf_feedback, RenderElementStates,
    },
    desktop::layer_map_for_output,
    desktop::utils::{
        send_dmabuf_feedback_surface_tree, send_frames_surface_tree,
        surface_presentation_feedback_flags_from_states, surface_primary_scanout_output,
        take_presentation_feedback_surface_tree, update_surface_primary_scanout_output,
        with_surfaces_surface_tree, OutputPresentationFeedback,
    },
    output::Output,
    reexports::{
        calloop::timer::{TimeoutAction, Timer},
        wayland_server::protocol::wl_surface::WlSurface,
    },
    wayland::{
        compositor::{SurfaceAttributes, SurfaceData},
        dmabuf::DmabufFeedback,
    },
};

use crate::{animation::WindowEffect, render::Scene, Pila};

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
        .chain(scene.dnd_icon.as_ref().map(|(surface, _)| surface))
        .chain(cursor)
}

/// Record `output` as the primary output of every surface it showed in the
/// frame described by `states`. Presentation feedback and the scale clients
/// are told (`scale.rs`) are keyed on it. Returns whether any surface lost its
/// primary output (became hidden), so `schedule_hidden_frames` re-checks it.
pub fn update_primary_outputs(
    scene: &Scene,
    cursor: Option<&WlSurface>,
    output: &Output,
    states: &RenderElementStates,
) -> bool {
    let mut became_hidden = false;
    let mut update = |surface: &WlSurface, data: &_| {
        let had = surface_primary_scanout_output(surface, data).is_some();
        update_surface_primary_scanout_output(
            surface,
            output,
            data,
            None,
            states,
            default_primary_scanout_output_compare,
        );
        if had && surface_primary_scanout_output(surface, data).is_none() {
            became_hidden = true;
        }
    };
    // An animating window is drawn through a texture, so it isn't in
    // `states`: it keeps the output it had (`send_frames` paces it).
    for entry in scene
        .windows
        .iter()
        .filter(|e| e.offscreen_output().is_none())
    {
        entry.window.with_surfaces(&mut update);
    }
    for entry in scene.layers().filter(|e| e.effect.is_none()) {
        entry.layer.with_surfaces(&mut update);
    }
    for surface in other_trees(scene, cursor) {
        with_surfaces_surface_tree(surface, &mut update);
    }
    became_hidden
}

/// Whether a surface was last told an output's scanout formats
/// (`send_dmabuf_feedback`).
#[derive(Default)]
struct ScanoutFeedbackSent(std::cell::Cell<bool>);

/// Tell the surfaces `output` showed in the frame described by `states`, those
/// that asked, which dmabuf formats to use: `scanout`'s (what `output` can
/// show as it is, first) for a surface that went to the screen as it is or was
/// tried and refused, `render`'s for the others. Sent only when it changes
/// (bare metal's direct scanout, `udev/draw.rs`).
pub fn send_dmabuf_feedback(
    scene: &Scene,
    cursor: Option<&WlSurface>,
    output: &Output,
    states: &RenderElementStates,
    render: &DmabufFeedback,
    scanout: &DmabufFeedback,
) {
    let select = |surface: &WlSurface, data: &SurfaceData| {
        let chosen = select_dmabuf_feedback(surface, states, render, scanout);
        // Which one it was last told, logged when it changes: whether a
        // client refused the primary plane gets the plane's formats.
        let to_scanout = std::ptr::eq(chosen, scanout) && !std::ptr::eq(render, scanout);
        let told = data.data_map.get_or_insert(ScanoutFeedbackSent::default);
        if told.0.replace(to_scanout) != to_scanout {
            tracing::debug!(
                target: "pila::frames",
                surface = ?smithay::reexports::wayland_server::Resource::id(surface),
                output = output.name(),
                scanout = to_scanout,
                "dmabuf feedback"
            );
        }
        chosen
    };
    for entry in &scene.windows {
        entry
            .window
            .send_dmabuf_feedback(output, surface_primary_scanout_output, select);
    }
    for entry in scene.layers() {
        entry
            .layer
            .send_dmabuf_feedback(output, surface_primary_scanout_output, select);
    }
    for surface in other_trees(scene, cursor) {
        send_dmabuf_feedback_surface_tree(surface, output, surface_primary_scanout_output, select);
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
        surface_presentation_feedback_flags_from_states(surface, None, states)
    };
    for entry in &scene.windows {
        entry.window.take_presentation_feedback(
            &mut feedback,
            |surface, data| paced_by(entry.offscreen_output().flatten(), surface, data),
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
/// (`offscreen`). Drawn through a texture but on no output, or drawn as
/// usual: whatever it was last shown on.
fn paced_by(offscreen: Option<Output>, surface: &WlSurface, data: &SurfaceData) -> Option<Output> {
    offscreen.or_else(|| surface_primary_scanout_output(surface, data))
}

/// A layer surface's `offscreen` output for `paced_by`.
fn layer_offscreen(effect: Option<&WindowEffect>) -> Option<Output> {
    effect.and_then(|effect| effect.output.clone())
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
                paced_by(entry.offscreen_output().flatten(), surface, data)
            });
    }
    for entry in scene.layers() {
        entry
            .layer
            .send_frame(output, time, throttle, |surface, data| {
                paced_by(layer_offscreen(entry.effect.as_ref()), surface, data)
            });
    }
    // The switcher's previews are live: their windows draw for its output.
    for window in scene
        .switcher
        .iter()
        .filter(|view| view.output == output.name())
        .flat_map(|view| view.windows())
    {
        window.send_frame(output, time, None, |_, _| Some(output.clone()));
    }
    // So are the share picker's windows: hidden ones too (minimized, on
    // other workspaces).
    let picker = match &scene.modal {
        Some(crate::render::SceneModal::SharePicker(view)) => Some(view),
        _ => None,
    };
    for window in picker
        .into_iter()
        .filter(|view| view.output == output.name())
        .flat_map(|view| view.windows())
    {
        window.send_frame(output, time, None, |_, _| Some(output.clone()));
    }
    // So are the overview's workspace thumbnails: the windows of the other
    // workspaces draw for their output too while it's shown.
    let name = output.name();
    for window in scene
        .overview
        .iter()
        .flat_map(|view| view.thumb_windows(&name))
    {
        window.send_frame(output, time, None, |_, _| Some(output.clone()));
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

/// Whether a surface shown on no output has frame callbacks waiting.
pub(crate) fn hidden_and_waiting(surface: &WlSurface, data: &SurfaceData) -> bool {
    surface_primary_scanout_output(surface, data).is_none()
        && !data
            .cached_state
            .get::<SurfaceAttributes>()
            .current()
            .frame_callbacks
            .is_empty()
}

impl Pila {
    /// Frames are drawn on demand, and a surface shown nowhere gets its
    /// throttled frame callback (`HIDDEN_FRAME_THROTTLE`) only when a frame
    /// is drawn: on an idle screen a covered client asking for one would
    /// wait forever. While such a surface waits, a redraw of one output is
    /// asked for once the throttle has passed. Not frame pacing: at most one wake-up per
    /// period, and none while no hidden client waits. Called from
    /// `after_frames`.
    pub fn schedule_hidden_frames(&mut self) {
        if self.hidden_frame_timer || !self.hidden_frames_dirty {
            return;
        }
        self.hidden_frames_dirty = false;
        let mut waiting = false;
        let mut check = |surface: &WlSurface, data: &SurfaceData| {
            waiting |= hidden_and_waiting(surface, data);
        };
        for window in self.space.elements() {
            window.with_surfaces(&mut check);
        }
        for output in self.space.outputs() {
            for layer in layer_map_for_output(output).layers() {
                layer.with_surfaces(&mut check);
            }
        }
        if !waiting {
            return;
        }
        let timer = Timer::from_duration(HIDDEN_FRAME_THROTTLE);
        let inserted = self.loop_handle.insert_source(timer, |_, _, state| {
            state.hidden_frame_timer = false;
            state.hidden_frames_dirty = true;
            // Any output's redraw sends them (`send_frames`); one is enough,
            // and it may well have nothing new to show.
            let output = state.space.outputs().next().cloned();
            if let Some(output) = output {
                state.request_redraw_as(&output, crate::redraw::Content::Maybe);
            }
            TimeoutAction::Drop
        });
        self.hidden_frame_timer = inserted.is_ok();
    }
}
