//! wlr-layer-shell surfaces: panels, launchers, wallpapers, notifications.
//!
//! Layer surfaces live in each output's `LayerMap` (`layer_map_for_output`),
//! not in the `Space`. Stacking, front to back: Overlay, Top, windows, Bottom,
//! Background, except that a fullscreen window at the top of the stack also
//! covers the Top layer of its output (`Pila::top_layer_covered`), unless a Top
//! surface there takes the keyboard (a launcher: it's shown, and the rest of
//! Top with it, while it's up). Rendering and hit testing both follow that
//! order (`Pila::upper_layers`, `Pila::lower_layers`; `Pila::scene`,
//! `Pila::under`).

use smithay::{
    desktop::{layer_map_for_output, LayerSurface, WindowSurfaceType},
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Serial, SERIAL_COUNTER},
    wayland::{
        compositor::with_states,
        shell::wlr_layer::{KeyboardInteractivity, Layer, LayerSurfaceData},
    },
};

use crate::{
    state::{fill_state, has_buffer, MapChange},
    workspaces::is_override_redirect,
    Pila,
};

/// Whether a layer surface is mapped and takes all keyboard input.
fn takes_keyboard(layer: &LayerSurface) -> bool {
    layer.cached_state().keyboard_interactivity == KeyboardInteractivity::Exclusive
        && has_buffer(layer.wl_surface())
}

/// A surface above the windows that takes the keyboard on demand: it gets it
/// when it maps.
fn takes_keyboard_on_map(layer: &LayerSurface) -> bool {
    matches!(layer.layer(), Layer::Top | Layer::Overlay)
        && layer.cached_state().keyboard_interactivity == KeyboardInteractivity::OnDemand
}

/// Layer surfaces with their global locations, front to back.
pub type LayerSurfaces = Vec<(LayerSurface, Point<i32, Logical>)>;

/// The topmost surface (incl. popups) of `layers` under `pos`, with its origin
/// in global coordinates.
pub fn layer_surface_under(
    layers: LayerSurfaces,
    pos: Point<f64, Logical>,
) -> Option<(LayerSurface, WlSurface, Point<f64, Logical>)> {
    layers.into_iter().find_map(|(layer, loc)| {
        let (surface, origin) = layer.surface_under(pos - loc.to_f64(), WindowSurfaceType::ALL)?;
        Some((layer, surface, (origin + loc).to_f64()))
    })
}

impl Pila {
    /// Whether a fullscreen window on top of `output`'s windows covers its Top
    /// layer. A Top surface that takes the keyboard (a launcher) stays above
    /// it, so what gets the keys is shown.
    fn top_layer_covered(&self, output: &Output) -> bool {
        let top = self
            .space
            .elements()
            .rev()
            .filter(|w| !is_override_redirect(w))
            // One kept above a fullscreen window doesn't bring panels back.
            .filter(|w| !crate::stacking::is_always_on_top(w) || fill_state(w).1)
            .find(|w| self.window_output(w).as_ref() == Some(output));
        top.is_some_and(|w| fill_state(w).1)
            && !layer_map_for_output(output)
                .layers_on(Layer::Top)
                .any(takes_keyboard)
    }

    /// The surfaces on `output`'s `layer`, front to back.
    fn push_layer(&self, surfaces: &mut LayerSurfaces, output: &Output, layer: Layer) {
        let Some(output_geo) = self.space.output_geometry(output) else {
            return;
        };
        let map = layer_map_for_output(output);
        for surface in map.layers_on(layer).rev() {
            if let Some(geo) = map.layer_geometry(surface) {
                surfaces.push((surface.clone(), output_geo.loc + geo.loc));
            }
        }
    }

    /// What's drawn above the windows, front to back: Overlay, and Top where
    /// no fullscreen window covers it.
    pub fn upper_layers(&self) -> LayerSurfaces {
        let mut surfaces = self.layer_surfaces(&[Layer::Overlay]);
        for output in self.space.outputs() {
            if !self.top_layer_covered(output) {
                self.push_layer(&mut surfaces, output, Layer::Top);
            }
        }
        surfaces
    }

    /// What's drawn below the windows, front to back: Top where a fullscreen
    /// window covers it, Bottom and Background.
    pub fn lower_layers(&self) -> LayerSurfaces {
        let mut surfaces = Vec::new();
        for output in self.space.outputs() {
            if self.top_layer_covered(output) {
                self.push_layer(&mut surfaces, output, Layer::Top);
            }
        }
        surfaces.extend(self.layer_surfaces(&[Layer::Bottom, Layer::Background]));
        surfaces
    }

    /// The surfaces on `layers` (in that order, each front to back) with their
    /// global locations.
    pub fn layer_surfaces(&self, layers: &[Layer]) -> LayerSurfaces {
        let mut surfaces = Vec::new();
        for &layer in layers {
            for output in self.space.outputs() {
                self.push_layer(&mut surfaces, output, layer);
            }
        }
        surfaces
    }

    /// The layer surface whose main surface is `surface`, and its output.
    pub fn layer_for_surface(&self, surface: &WlSurface) -> Option<(Output, LayerSurface)> {
        self.space.outputs().find_map(|output| {
            let map = layer_map_for_output(output);
            let layer = map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)?;
            Some((output.clone(), layer.clone()))
        })
    }

    /// A mapped Top/Overlay surface that takes all keyboard input while it's
    /// shown (e.g. a launcher). Applies even over a fullscreen window.
    pub fn exclusive_layer(&self) -> Option<LayerSurface> {
        self.layer_surfaces(&[Layer::Overlay, Layer::Top])
            .into_iter()
            .map(|(layer, _)| layer)
            .find(takes_keyboard)
    }

    /// Whether keyboard focus is on `surface` or one of its popups.
    pub fn has_keyboard_focus(&self, surface: &WlSurface) -> bool {
        let Some(focus) = self.keyboard_focus() else {
            return false;
        };
        self.popup_root(&focus).unwrap_or(focus) == *surface
    }

    /// Give keyboard focus to a layer surface, unless another one holds
    /// exclusive focus. Windows keep their stacking order.
    pub fn focus_layer(&mut self, layer: &LayerSurface, serial: Serial) {
        if self.is_locked() || self.exclusive_layer().is_some_and(|l| l != *layer) {
            return;
        }
        self.dismiss_popup_grab();
        self.set_keyboard_focus(Some(layer.wl_surface().clone().into()), serial);
    }

    /// Called on every `wl_surface.commit`: lays out the surface's output,
    /// sends the initial configure, and moves keyboard focus when a surface
    /// claims or gives up exclusive focus.
    pub fn handle_layer_commit(&mut self, surface: &WlSurface) {
        let Some((output, layer)) = self.layer_for_surface(surface) else {
            return;
        };

        // Arrange before the initial configure so it carries the right size.
        let zone_changed = {
            let mut map = layer_map_for_output(&output);
            let before = map.non_exclusive_zone();
            map.arrange();
            map.non_exclusive_zone() != before
        };
        let initial_configure_sent = with_states(surface, |states| {
            states
                .data_map
                .get::<LayerSurfaceData>()
                .is_some_and(|data| data.lock().unwrap().initial_configure_sent)
        });
        // Hidden by this commit: shown again by the next one.
        let change = crate::state::commit_map_change(surface);
        if !initial_configure_sent && change != MapChange::Unmapped {
            self.send_initial_scale(surface, Some(output.clone()));
            layer.layer_surface().send_configure();
        }
        if zone_changed {
            self.refit_maximized();
        }
        self.layer_committed(&layer);

        let serial = SERIAL_COUNTER.next_serial();
        match self.exclusive_layer() {
            Some(exclusive) => {
                if !self.has_keyboard_focus(exclusive.wl_surface()) {
                    self.focus_layer(&exclusive, serial);
                }
            }
            // The focused layer surface was hidden (committed without a buffer).
            None if !has_buffer(surface) && self.has_keyboard_focus(surface) => {
                self.focus_recent_window();
            }
            // A launcher or dialog above the windows that takes the keyboard
            // on demand gets it when it shows up (as on niri and COSMIC).
            None if change == MapChange::Mapped && takes_keyboard_on_map(&layer) => {
                self.focus_layer(&layer, serial);
            }
            None => {}
        }
    }

    /// Unmap a destroyed layer surface; hand its keyboard focus back to the
    /// window used most recently.
    pub fn remove_layer(&mut self, surface: &WlSurface) {
        let was_focused = self.has_keyboard_focus(surface);
        let mut zone_changed = false;
        let mut outputs = Vec::new();
        for output in self.space.outputs() {
            let mut map = layer_map_for_output(output);
            let layer = map
                .layers()
                .find(|layer| layer.wl_surface() == surface)
                .cloned();
            if let Some(layer) = layer {
                outputs.push(output.clone());
                let before = map.non_exclusive_zone();
                map.unmap_layer(&layer);
                zone_changed = map.non_exclusive_zone() != before;
                break;
            }
        }
        for output in outputs {
            self.queue_redraw_output(&output);
        }
        if zone_changed {
            self.refit_maximized();
        }
        if was_focused {
            self.focus_recent_window();
        }
    }
}
