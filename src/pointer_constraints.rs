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
    input::pointer::PointerHandle,
    reexports::wayland_server::{protocol::wl_surface::WlSurface, Resource},
    utils::{Logical, Point},
    wayland::{
        compositor::{get_parent, RegionAttributes},
        pointer_constraints::{
            with_pointer_constraint, PointerConstraint, PointerConstraintsHandler,
        },
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

    /// Global origin of a window's or layer surface's main surface.
    fn surface_origin(&self, surface: &WlSurface) -> Option<Point<f64, Logical>> {
        let window = self
            .space
            .elements()
            .find(|w| w.toplevel().is_some_and(|t| t.wl_surface() == surface));
        if let Some(window) = window {
            let loc = self.space.element_location(window)? - window.geometry().loc;
            return Some(loc.to_f64());
        }
        self.layer_surfaces(&[
            smithay::wayland::shell::wlr_layer::Layer::Overlay,
            smithay::wayland::shell::wlr_layer::Layer::Top,
            smithay::wayland::shell::wlr_layer::Layer::Bottom,
            smithay::wayland::shell::wlr_layer::Layer::Background,
        ])
        .into_iter()
        .find(|(layer, _)| layer.wl_surface() == surface)
        .map(|(_, loc)| loc.to_f64())
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
        // motion moves it back to the host pointer anyway.)
        let active =
            with_pointer_constraint(surface, pointer, |c| c.is_some_and(|c| c.is_active()));
        if let (true, Some(origin)) = (active, self.surface_origin(surface)) {
            pointer.set_location(origin + location);
        }
    }
}
delegate_pointer_constraints!(Pila);
