//! Output scale, and telling clients which scale to draw at.
//!
//! An output's scale comes from its `[[output]]` config (default 1) and may be
//! fractional. pila renders every output at its own scale (`render.rs` takes
//! it from the output), so a window spanning two outputs is drawn at each
//! one's scale. Clients learn the scale to draw at in three ways, all kept to
//! one output per surface: for a window, the output holding most of its
//! geometry (`Pila::window_output`, decided by pila so it doesn't flip
//! between frames: a client told a new scale every frame redraws forever);
//! for other surfaces (layer surfaces, the lock screen) their own output, or
//! the primary output recorded by `frame::update_primary_outputs`:
//!
//! - `wp_fractional_scale_v1.preferred_scale` (exact, for clients that use
//!   viewporter to draw at fractional scales);
//! - `wl_surface.preferred_buffer_scale` (wl_compositor v6; the scale rounded
//!   up);
//! - `wl_output.scale` plus `wl_surface.enter` (older clients; Smithay sends
//!   enter/leave from `Space::refresh`).
//!
//! `update_surface_scales` runs after every frame. A new surface gets the
//! scale of the output it will appear on before its first configure
//! (`send_initial_scale`), so it can draw its first buffer at the right
//! scale. Xwayland is left at scale 1 (X11 apps are upscaled on HiDPI
//! outputs).

use smithay::{
    desktop::{layer_map_for_output, utils::surface_primary_scanout_output},
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Scale, Transform},
    wayland::{
        compositor::{
            get_parent, send_surface_state, with_surface_tree_downward, SurfaceData,
            TraversalAction,
        },
        fractional_scale::with_fractional_scale,
    },
};

use crate::Pila;

/// The scale `output` is rendered at.
pub fn output_scale(output: &Output) -> Scale<f64> {
    Scale::from(output.current_scale().fractional_scale())
}

/// The integer scale pila's own buffers (title bars, cursor images) are
/// drawn at for a render `scale`: rounded up, so they're downscaled rather
/// than blurred.
pub fn buffer_scale(scale: Scale<f64>) -> i32 {
    (scale.x.ceil() as i32).max(1)
}

/// Tell one surface the scale of `output`. Smithay only sends either
/// preference when it changes.
fn apply(surface: &WlSurface, states: &SurfaceData, output: &Output) {
    let scale = output.current_scale();
    with_fractional_scale(states, |fractional| {
        fractional.set_preferred_scale(scale.fractional_scale());
    });
    // Clients are told `Normal` and draw upright: a rotated output's frame
    // is turned by Smithay as it's rendered, and the winit output's
    // `Flipped180` is only how its GL framebuffer is oriented.
    send_surface_state(surface, states, scale.integer_scale(), Transform::Normal);
}

/// Tell `surface` and its subsurfaces the scale of `output`.
pub fn send_scale_to_tree(surface: &WlSurface, output: &Output) {
    with_surface_tree_downward(
        surface,
        (),
        |_, _, _| TraversalAction::DoChildren(()),
        |surface, states, _| apply(surface, states, output),
        |_, _, _| true,
    );
}

impl Pila {
    /// The output the window, popup or layer surface `surface` belongs to is
    /// on (for a window on several, the first), if it's mapped.
    pub fn output_for_surface(&self, surface: &WlSurface) -> Option<Output> {
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        let root = self.popup_root(&root).unwrap_or(root);
        if let Some(window) = self.window_for_surface(&root) {
            return self.window_output(&window);
        }
        self.layer_for_surface(&root).map(|(output, _)| output)
    }

    /// The output a mapped window draws for: the one its geometry overlaps
    /// most (shadows of client-side decorations don't count), the first of
    /// equal ones. `None` if it's not mapped or on no output.
    pub fn window_output(&self, window: &smithay::desktop::Window) -> Option<Output> {
        let geo = self.space.element_geometry(window)?;
        self.space
            .outputs()
            .filter_map(|output| {
                let area = self.space.output_geometry(output)?.intersection(geo)?;
                Some((output, area.size.w as i64 * area.size.h as i64))
            })
            .filter(|(_, area)| *area > 0)
            .fold(None::<(&Output, i64)>, |best, (output, area)| match best {
                Some((_, best_area)) if best_area >= area => best,
                _ => Some((output, area)),
            })
            .map(|(output, _)| output.clone())
    }

    /// The output a new toplevel will be mapped on (`place_new_window`
    /// centers it on the pointer's output).
    pub fn output_for_new_window(&self) -> Option<Output> {
        let pointer = self.seat.get_pointer()?.current_location();
        self.output_at(pointer)
            .or_else(|| self.space.outputs().next().cloned())
    }

    /// Before a new surface's first configure: the scale of the output it
    /// will show up on.
    pub fn send_initial_scale(&self, surface: &WlSurface, output: Option<Output>) {
        if let Some(output) = output.or_else(|| self.output_for_new_window()) {
            send_scale_to_tree(surface, &output);
        }
    }

    /// After a frame on `output`: every surface whose primary output it is
    /// (or that has none yet and was drawn here) gets its scale.
    pub fn update_surface_scales(&self, output: &Output) {
        let update = |surface: &WlSurface, states: &SurfaceData| {
            let primary = surface_primary_scanout_output(surface, states);
            if primary.as_ref().is_none_or(|primary| primary == output) {
                apply(surface, states, output);
            }
        };
        for window in self.space.elements() {
            // Xwayland stays at scale 1 (see the module docs).
            if window.x11_surface().is_some() {
                continue;
            }
            // One output per window, whatever the render states say.
            if self.window_output(window).as_ref() == Some(output) {
                window.with_surfaces(|surface, states| apply(surface, states, output));
            }
        }
        for layer in layer_map_for_output(output).layers() {
            layer.with_surfaces(update);
        }
        if let Some(lock) = self.lock_surface(output) {
            send_scale_to_tree(lock.wl_surface(), output);
        }
    }
}
