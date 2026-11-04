//! zwp_pointer_constraints_v1: games and 3D apps lock the pointer in place
//! (and read relative motion) or confine it to a surface region.
//!
//! A constraint only activates while the pointer is over its surface (inside
//! the constraint's region) *and* the surface's window has keyboard focus,
//! so an unfocused client can't trap the pointer; moving keyboard focus away
//! (Mod+Tab, clicking elsewhere, locking the session) releases it.
//!
//! Nested, winit only reports absolute positions: relative motion is derived
//! from the host pointer's movement, a locked pointer keeps pila's pointer in
//! place while the host pointer still moves (until it leaves the window), and
//! a confined pointer ignores motion that would leave the region.

use smithay::{
    delegate_pointer_constraints,
    desktop::WindowSurfaceType,
    input::pointer::PointerHandle,
    reexports::wayland_server::{protocol::wl_surface::WlSurface, Resource},
    utils::{Logical, Point, Rectangle},
    wayland::{
        compositor::{get_parent, RegionAttributes},
        pointer_constraints::{
            with_pointer_constraint, PointerConstraint, PointerConstraintsHandler,
        },
        seat::WaylandFocus,
        shell::wlr_layer::Layer,
    },
};

use crate::Pila;

/// What the active constraint (if any) allows for the next pointer motion.
pub enum Constraint {
    None,
    Locked,
    /// Motion must stay on the surface, and inside the region if there's one
    /// (surface-local coordinates).
    Confined(Option<RegionAttributes>),
}

fn root(surface: &WlSurface) -> WlSurface {
    let mut root = surface.clone();
    while let Some(parent) = get_parent(&root) {
        root = parent;
    }
    root
}

/// Whether the surface-local point `local` is in the constraint's region.
fn in_region(region: Option<&RegionAttributes>, local: Point<f64, Logical>) -> bool {
    region.is_none_or(|region| region.contains(local.to_i32_round()))
}

impl Pila {
    /// The surface under `pos` that a constraint can be on: `surface_under`,
    /// except that over one of a window's popups (a menu the constrained
    /// client opened) it's the window's own surface beneath the popup, so a
    /// confinement neither stops at the popup nor ends on it.
    pub fn constraint_surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let (surface, origin) = self.surface_under(pos)?;
        let Some(root) = self.popup_root(&surface) else {
            return Some((surface, origin));
        };
        let window = self
            .space
            .elements()
            .find(|w| w.wl_surface().is_some_and(|s| *s == root))?;
        let render_loc = self.space.element_location(window)? - window.geometry().loc;
        window
            .surface_under(
                pos - render_loc.to_f64(),
                WindowSurfaceType::TOPLEVEL | WindowSurfaceType::SUBSURFACE,
            )
            .map(|(surface, origin)| (surface, (origin + render_loc).to_f64()))
    }

    /// The active constraint on `surface` (origin `origin`), with the
    /// pointer at `pos`.
    pub fn active_constraint(
        &self,
        surface: &WlSurface,
        origin: Point<f64, Logical>,
        pos: Point<f64, Logical>,
    ) -> Constraint {
        let Some(pointer) = self.seat.get_pointer() else {
            return Constraint::None;
        };
        with_pointer_constraint(surface, &pointer, |constraint| match constraint {
            Some(constraint)
                if constraint.is_active() && in_region(constraint.region(), pos - origin) =>
            {
                match &*constraint {
                    PointerConstraint::Locked(_) => Constraint::Locked,
                    PointerConstraint::Confined(confined) => {
                        Constraint::Confined(confined.region().cloned())
                    }
                }
            }
            _ => Constraint::None,
        })
    }

    /// Activate the constraint of the surface under the pointer (`surface`,
    /// origin `origin`, pointer at `pos`), if its window has keyboard focus.
    pub fn maybe_activate_constraint(
        &mut self,
        surface: &WlSurface,
        origin: Point<f64, Logical>,
        pos: Point<f64, Logical>,
    ) {
        if self.is_locked() || !self.has_keyboard_focus(&root(surface)) {
            return;
        }
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let activated = with_pointer_constraint(surface, &pointer, |constraint| match constraint {
            Some(constraint)
                if !constraint.is_active() && in_region(constraint.region(), pos - origin) =>
            {
                constraint.activate();
                true
            }
            _ => false,
        });
        if activated {
            self.constrained = Some(surface.clone());
        }
    }

    /// Release the active constraint when keyboard focus moves to `focused`,
    /// unless that's still the constrained surface's window (or its menu).
    pub fn release_constraint_for_focus(&mut self, focused: Option<&WlSurface>) {
        let Some(surface) = self.constrained.clone() else {
            return;
        };
        let focused_root = focused.map(|f| self.popup_root(f).unwrap_or_else(|| f.clone()));
        if surface.is_alive() && focused_root.as_ref() == Some(&root(&surface)) {
            return;
        }
        self.constrained = None;
        if !surface.is_alive() {
            return;
        }
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        with_pointer_constraint(&surface, &pointer, |constraint| {
            if let Some(constraint) = constraint.filter(|c| c.is_active()) {
                constraint.deactivate();
            }
        });
    }

    /// Global origin of a window's or layer surface's main surface, and
    /// the part of it that's shown (surface-local: a window's geometry, a
    /// layer surface's size).
    fn surface_origin(
        &self,
        surface: &WlSurface,
    ) -> Option<(Point<f64, Logical>, Rectangle<i32, Logical>)> {
        let window = self
            .space
            .elements()
            .find(|w| w.toplevel().is_some_and(|t| t.wl_surface() == surface));
        if let Some(window) = window {
            let geometry = window.geometry();
            let loc = self.space.element_location(window)? - geometry.loc;
            return Some((loc.to_f64(), geometry));
        }
        let (layer, loc) = self
            .layer_surfaces(&[Layer::Overlay, Layer::Top, Layer::Bottom, Layer::Background])
            .into_iter()
            .find(|(layer, _)| layer.wl_surface() == surface)?;
        let size = layer.bbox().size;
        Some((loc.to_f64(), Rectangle::from_size(size)))
    }
}

impl PointerConstraintsHandler for Pila {
    fn new_constraint(&mut self, surface: &WlSurface, pointer: &PointerHandle<Self>) {
        // Activate right away if the pointer is already over the surface.
        let pos = pointer.current_location();
        if let Some((under, origin)) = self.surface_under(pos) {
            if under == *surface {
                self.maybe_activate_constraint(surface, origin, pos);
            }
        }
    }

    fn cursor_position_hint(
        &mut self,
        surface: &WlSurface,
        pointer: &PointerHandle<Self>,
        location: Point<f64, Logical>,
    ) {
        // A locked client draws its own cursor; keep pila's pointer where the
        // client says it is, so unlocking doesn't jump. (Nested, the next host
        // motion moves it back to the host pointer anyway.) The hint is kept
        // on the surface: a client can't send the pointer elsewhere.
        let active =
            with_pointer_constraint(surface, pointer, |c| c.is_some_and(|c| c.is_active()));
        if let (true, Some((origin, shown))) = (active, self.surface_origin(surface)) {
            if shown.is_empty() {
                return;
            }
            let min = shown.loc.to_f64();
            let max = (shown.loc + shown.size.to_point()).to_f64() - Point::from((1.0, 1.0));
            let clamped = Point::from((
                location.x.clamp(min.x, max.x),
                location.y.clamp(min.y, max.y),
            ));
            pointer.set_location(origin + clamped);
        }
    }
}
delegate_pointer_constraints!(Pila);
