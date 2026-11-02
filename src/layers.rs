//! wlr-layer-shell surfaces: panels, launchers, wallpapers, notifications.
//!
//! Layer surfaces live in each output's `LayerMap` (`layer_map_for_output`),
//! not in the `Space`. Stacking, front to back: Overlay, Top, windows, Bottom,
//! Background, except that a fullscreen window at the top of the stack also
//! covers the Top layer (`Pila::layer_order`). Rendering and hit testing both
//! follow that order (`Pila::scene`, `Pila::under`).

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
    state::{fill_state, has_buffer},
    Pila,
};

/// Layers drawn above and below the windows, each front to back.
pub type LayerOrder = (&'static [Layer], &'static [Layer]);

impl Pila {
    /// Which layers go above the windows and which below.
    pub fn layer_order(&self) -> LayerOrder {
        let fullscreen_on_top = self
            .space
            .elements()
            .next_back()
            .is_some_and(|w| fill_state(w).1);
        if fullscreen_on_top {
            (
                &[Layer::Overlay],
                &[Layer::Top, Layer::Bottom, Layer::Background],
            )
        } else {
            (
                &[Layer::Overlay, Layer::Top],
                &[Layer::Bottom, Layer::Background],
            )
        }
    }

    /// The surfaces on `layers` (in that order, each front to back) with their
    /// global locations.
    pub fn layer_surfaces(&self, layers: &[Layer]) -> Vec<(LayerSurface, Point<i32, Logical>)> {
        let mut surfaces = Vec::new();
        for &layer in layers {
            for output in self.space.outputs() {
                let Some(output_geo) = self.space.output_geometry(output) else {
                    continue;
                };
                let map = layer_map_for_output(output);
                for surface in map.layers_on(layer).rev() {
                    if let Some(geo) = map.layer_geometry(surface) {
                        surfaces.push((surface.clone(), output_geo.loc + geo.loc));
                    }
                }
            }
        }
        surfaces
    }

    /// The topmost surface (incl. popups) of a layer surface on `layers` under
    /// `pos`, with its origin in global coordinates.
    pub fn layer_surface_under(
        &self,
        layers: &[Layer],
        pos: Point<f64, Logical>,
    ) -> Option<(LayerSurface, WlSurface, Point<f64, Logical>)> {
        self.layer_surfaces(layers)
            .into_iter()
            .find_map(|(layer, loc)| {
                let (surface, origin) =
                    layer.surface_under(pos - loc.to_f64(), WindowSurfaceType::ALL)?;
                Some((layer, surface, (origin + loc).to_f64()))
            })
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
            .find(|layer| {
                layer.cached_state().keyboard_interactivity == KeyboardInteractivity::Exclusive
                    && has_buffer(layer.wl_surface())
            })
    }

    /// Whether keyboard focus is on `surface` or one of its popups.
    pub fn has_keyboard_focus(&self, surface: &WlSurface) -> bool {
        let Some(focus) = self.seat.get_keyboard().and_then(|k| k.current_focus()) else {
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
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        self.dismiss_popup_grab();
        keyboard.set_focus(self, Some(layer.wl_surface().clone()), serial);
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
        if !initial_configure_sent {
            layer.layer_surface().send_configure();
        }
        if zone_changed {
            self.refit_maximized();
        }

        let serial = SERIAL_COUNTER.next_serial();
        match self.exclusive_layer() {
            Some(exclusive) => {
                if !self.has_keyboard_focus(exclusive.wl_surface()) {
                    self.focus_layer(&exclusive, serial);
                }
            }
            // The focused layer surface was hidden (committed without a buffer).
            None if !has_buffer(surface) && self.has_keyboard_focus(surface) => {
                self.focus_top_window(serial);
            }
            None => {}
        }
    }

    /// Unmap a destroyed layer surface; hand its keyboard focus back to the
    /// top window.
    pub fn remove_layer(&mut self, surface: &WlSurface) {
        let was_focused = self.has_keyboard_focus(surface);
        let mut zone_changed = false;
        for output in self.space.outputs() {
            let mut map = layer_map_for_output(output);
            let layer = map
                .layers()
                .find(|layer| layer.wl_surface() == surface)
                .cloned();
            if let Some(layer) = layer {
                let before = map.non_exclusive_zone();
                map.unmap_layer(&layer);
                zone_changed = map.non_exclusive_zone() != before;
                break;
            }
        }
        if zone_changed {
            self.refit_maximized();
        }
        if was_focused {
            self.focus_top_window(SERIAL_COUNTER.next_serial());
        }
    }
}
