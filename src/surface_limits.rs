//! Limits on the sizes and places clients give their surfaces.
//!
//! Every number a client sends about where a surface is and how big it is
//! (buffer sizes, viewports, subsurface positions, window geometry,
//! layer-shell sizes and margins, popup positioners) must stay within
//! `MAX_EXTENT` logical px: far more than any screen, and far from where
//! adding a position to a size overflows. Past it the client gets a protocol
//! error. Checking here, before a commit applies, keeps the numbers out of all
//! of Pila's layout and drawing code and of Smithay's (bounding boxes, popup
//! placement), which no guard at each sum could reach.

use smithay::{
    backend::renderer::buffer_dimensions,
    reexports::wayland_server::{protocol::wl_surface, Resource},
    utils::{Logical, Point, Size},
    wayland::{
        compositor::{
            get_parent, with_surface_tree_downward, BufferAssignment, SubsurfaceCachedState,
            SurfaceAttributes, SurfaceData, TraversalAction, SUBSURFACE_ROLE,
        },
        shell::{
            wlr_layer::{ExclusiveZone, LayerSurfaceCachedState, LAYER_SURFACE_ROLE},
            xdg::{PositionerState, SurfaceCachedState, XDG_POPUP_ROLE, XDG_TOPLEVEL_ROLE},
        },
        viewporter::ViewportCachedState,
    },
};
use tracing::warn;

/// The largest size, and the farthest position from the root of its surface
/// tree, a client may give a surface, in logical px (buffers: in pixels).
pub const MAX_EXTENT: i32 = 32768;

pub fn position_ok(x: i64, y: i64) -> bool {
    let range = -(MAX_EXTENT as i64)..=MAX_EXTENT as i64;
    range.contains(&x) && range.contains(&y)
}

pub fn size_ok(w: i32, h: i32) -> bool {
    (0..=MAX_EXTENT).contains(&w) && (0..=MAX_EXTENT).contains(&h)
}

fn past(what: &str, value: impl std::fmt::Debug) -> String {
    format!("{what} {value:?} is past the limit of {MAX_EXTENT} px")
}

/// Called before a commit of `surface` applies: checks the pending state of
/// its whole surface tree (a parent's commit applies its synchronized
/// subsurfaces' positions without their own commit). What's past the limit
/// is a protocol error, and is clamped or dropped so that what applies
/// before the client is gone is within it.
pub fn check_commit(surface: &wl_surface::WlSurface) {
    let mut root = surface.clone();
    while let Some(parent) = get_parent(&root) {
        root = parent;
    }
    let errors = std::cell::RefCell::new(Vec::new());
    with_surface_tree_downward(
        &root,
        Point::<i64, Logical>::from((0, 0)),
        |surface, states, parent_loc| {
            let mut loc = *parent_loc;
            if states.role == Some(SUBSURFACE_ROLE) && surface != &root {
                let mut cached = states.cached_state.get::<SubsurfaceCachedState>();
                let pending = cached.pending();
                loc += Point::from((pending.location.x as i64, pending.location.y as i64));
                if !position_ok(loc.x, loc.y) {
                    errors
                        .borrow_mut()
                        .push((surface.clone(), past("a subsurface at", (loc.x, loc.y))));
                    pending.location = (0, 0).into();
                    loc = *parent_loc;
                }
            }
            TraversalAction::DoChildren(loc)
        },
        |surface, states, _| {
            if let Some(err) = check_surface(states) {
                errors.borrow_mut().push((surface.clone(), err));
            }
        },
        |_, _, _| true,
    );
    for (surface, err) in errors.into_inner() {
        refuse(&surface, err);
    }
}

/// A protocol error for what's past the limit (it ends the client).
pub fn refuse(surface: &wl_surface::WlSurface, err: String) {
    warn!(surface = surface.id().protocol_id(), "{err}");
    surface.post_error(wl_surface::Error::InvalidSize, err);
}

/// The pending state of one surface, without its place in the tree.
fn check_surface(states: &SurfaceData) -> Option<String> {
    let mut error = None;
    {
        let mut cached = states.cached_state.get::<SurfaceAttributes>();
        let pending = cached.pending();
        if let Some(BufferAssignment::NewBuffer(buffer)) = &pending.buffer {
            if let Some(size) = buffer_dimensions(buffer).filter(|s| !size_ok(s.w, s.h)) {
                error.get_or_insert(past("a buffer of", (size.w, size.h)));
                pending.buffer = Some(BufferAssignment::Removed);
            }
        }
        if let Some(delta) = pending
            .buffer_delta
            .filter(|d| !position_ok(d.x.into(), d.y.into()))
        {
            error.get_or_insert(past("a buffer offset of", (delta.x, delta.y)));
            pending.buffer_delta = None;
        }
    }
    if states.cached_state.has::<ViewportCachedState>() {
        let mut cached = states.cached_state.get::<ViewportCachedState>();
        let pending = cached.pending();
        if let Some(dst) = pending.dst.filter(|d| !size_ok(d.w, d.h)) {
            error.get_or_insert(past("a viewport of", (dst.w, dst.h)));
            pending.dst = None;
        }
    }
    match states.role {
        Some(XDG_TOPLEVEL_ROLE) | Some(XDG_POPUP_ROLE) => {
            let mut cached = states.cached_state.get::<SurfaceCachedState>();
            let pending = cached.pending();
            if let Some(geo) = pending.geometry.filter(|g| {
                !position_ok(g.loc.x.into(), g.loc.y.into()) || !size_ok(g.size.w, g.size.h)
            }) {
                error.get_or_insert(past("a window geometry of", geo));
                pending.geometry = None;
            }
            // Not errors: a large maximum can mean "no limit".
            pending.min_size = clamp_size(pending.min_size);
            pending.max_size = clamp_size(pending.max_size);
        }
        Some(LAYER_SURFACE_ROLE) => {
            let mut cached = states.cached_state.get::<LayerSurfaceCachedState>();
            let pending = cached.pending();
            // Sent as u32: past i32::MAX it reads as negative.
            if !size_ok(pending.size.w, pending.size.h) {
                error.get_or_insert(past("a layer surface of", pending.size));
                pending.size = (0, 0).into();
            }
            let m = pending.margin;
            if ![m.top, m.right, m.bottom, m.left]
                .iter()
                .all(|&v| position_ok(v.into(), 0))
            {
                error.get_or_insert(past("a margin of", (m.top, m.right, m.bottom, m.left)));
                pending.margin = Default::default();
            }
            if let ExclusiveZone::Exclusive(zone) = pending.exclusive_zone {
                if zone > MAX_EXTENT as u32 {
                    error.get_or_insert(past("an exclusive zone of", zone));
                    pending.exclusive_zone = ExclusiveZone::Neutral;
                }
            }
        }
        _ => {}
    }
    error
}

fn clamp_size(size: Size<i32, Logical>) -> Size<i32, Logical> {
    (size.w.min(MAX_EXTENT), size.h.min(MAX_EXTENT)).into()
}

/// A popup's positioner, which Pila applies as the popup is made or moved,
/// not at a commit: its sizes and places within the limit, else the error.
pub fn check_positioner(positioner: &PositionerState) -> Result<(), String> {
    let size = positioner.rect_size;
    let rect = positioner.anchor_rect;
    let offset = positioner.offset;
    let parent = positioner.parent_size.unwrap_or_default();
    if !size_ok(size.w, size.h) || !size_ok(parent.w, parent.h) {
        return Err(past("a popup of", size));
    }
    if !position_ok(rect.loc.x.into(), rect.loc.y.into()) || !size_ok(rect.size.w, rect.size.h) {
        return Err(past("an anchor rectangle of", rect));
    }
    if !position_ok(offset.x.into(), offset.y.into()) {
        return Err(past("a popup offset of", offset));
    }
    Ok(())
}
