//! Client-drawn cursors: surfaces set with `wl_pointer.set_cursor`, as opposed
//! to named shapes from `wp_cursor_shape_v1`.
//!
//! Backend-neutral: a backend renders the surface at `surface_position`
//! (on top of everything else, as `Kind::Cursor`) and sends it frame
//! callbacks. These are free functions over `Pila` fields so a render loop
//! can call them while it holds a mutable borrow of the backend.

use smithay::{
    input::pointer::{CursorImageStatus, CursorImageSurfaceData},
    reexports::wayland_server::{protocol::wl_surface::WlSurface, Resource},
    utils::{Logical, Point},
    wayland::compositor::{with_states, SurfaceAttributes},
};

/// The cursor surface to draw and its top-left corner in global logical
/// coordinates (pointer position minus the hotspot), if the client set one.
pub fn surface_position(
    status: &CursorImageStatus,
    pointer: Point<f64, Logical>,
) -> Option<(WlSurface, Point<i32, Logical>)> {
    let CursorImageStatus::Surface(surface) = status else {
        return None;
    };
    if !surface.is_alive() {
        return None;
    }
    let hotspot = with_states(surface, |states| {
        states
            .data_map
            .get::<CursorImageSurfaceData>()
            .map(|attrs| attrs.lock().unwrap().hotspot)
            .unwrap_or_default()
    });
    Some((surface.clone(), pointer.to_i32_round() - hotspot))
}

/// Keep the hotspot under the pointer when the client moves its buffer
/// (`wl_surface.offset` or an attach offset). Call on every commit.
pub fn handle_commit(status: &CursorImageStatus, surface: &WlSurface) {
    if !matches!(status, CursorImageStatus::Surface(cursor) if cursor == surface) {
        return;
    }
    with_states(surface, |states| {
        let Some(attrs) = states.data_map.get::<CursorImageSurfaceData>() else {
            return;
        };
        let delta = states
            .cached_state
            .get::<SurfaceAttributes>()
            .current()
            .buffer_delta
            .take();
        if let Some(delta) = delta {
            attrs.lock().unwrap().hotspot -= delta;
        }
    });
}
