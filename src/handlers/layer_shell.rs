// zwlr_layer_shell_v1: panels, launchers, wallpapers, notifications.
// The window-management side lives in `layers.rs`.

use smithay::{
    delegate_layer_shell,
    desktop::{layer_map_for_output, LayerSurface},
    output::Output,
    reexports::wayland_server::protocol::wl_output,
    wayland::shell::{
        wlr_layer::{
            Layer, LayerSurface as WlrLayerSurface, WlrLayerShellHandler, WlrLayerShellState,
        },
        xdg::PopupSurface,
    },
};
use tracing::{debug, warn};

use crate::Pila;

impl WlrLayerShellHandler for Pila {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        output: Option<wl_output::WlOutput>,
        layer: Layer,
        namespace: String,
    ) {
        // The requested output if it's in use (a client may still hold the
        // wl_output of one that was disabled), else the one under the pointer.
        let pointer = self
            .seat
            .get_pointer()
            .map(|p| p.current_location())
            .unwrap_or_default();
        let output = output
            .as_ref()
            .and_then(Output::from_resource)
            .filter(|output| self.space.outputs().any(|o| o == output))
            .or_else(|| self.output_at(pointer));
        let Some(output) = output else {
            // Nowhere to show it; the client is told to destroy it.
            surface.send_close();
            return;
        };
        debug!(
            namespace,
            ?layer,
            output = output.name(),
            "new layer surface"
        );
        let mut map = layer_map_for_output(&output);
        if let Err(err) = map.map_layer(&LayerSurface::new(surface, namespace)) {
            warn!(?err, "failed to map layer surface");
        }
    }

    fn new_popup(&mut self, _parent: WlrLayerSurface, popup: PopupSurface) {
        // Tracked by `XdgShellHandler::new_popup`; only now does it have a parent.
        self.unconstrain_popup(&popup);
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        self.remove_layer(surface.wl_surface());
    }
}
delegate_layer_shell!(Pila);
