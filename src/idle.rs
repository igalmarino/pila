//! ext-idle-notify and idle-inhibit: tell clients such as swayidle when the
//! user has been idle, unless something visible (a video player) inhibits it,
//! or an app holds an `org.freedesktop.ScreenSaver` inhibit cookie
//! (`screensaver.rs`: browsers, players, call apps, Flatpaks through the
//! portal).
//!
//! Every input event counts as activity (`Pila::process_input_event`), told
//! the notifier once per event-loop iteration (`Pila::after_frames`).
//! Smithay runs the per-notification timers on `Pila::loop_handle`. An idle
//! inhibitor only counts while its surface is visible: part of a mapped window
//! or layer surface, and the session not locked. `update_idle_inhibit`
//! re-checks that after every frame.
//!
//! What to do when idle (lock, turn the monitors off) is the client's:
//! swayidle, in the example's `autostart`.

use std::collections::HashSet;

use smithay::{
    reexports::wayland_server::{protocol::wl_surface::WlSurface, Resource},
    wayland::{
        idle_inhibit::IdleInhibitHandler,
        idle_notify::{IdleNotifierHandler, IdleNotifierState},
        seat::WaylandFocus,
    },
};

use serde_json::{json, Value};

use crate::{screensaver::Inhibitor, Pila};

/// What inhibits idle.
#[derive(Default)]
pub struct IdleInhibitors {
    /// Surfaces with an idle inhibitor, visible or not.
    surfaces: HashSet<WlSurface>,
    /// Apps holding a D-Bus cookie (`screensaver.rs`).
    pub dbus: Vec<Inhibitor>,
}

impl Pila {
    /// Count user input as activity: idle notifications resume and restart
    /// their timeouts.
    pub fn notify_activity(&mut self) {
        let seat = self.seat.clone();
        self.globals.idle_notifier.notify_activity(&seat);
    }

    /// Recompute whether a visible surface inhibits idle. Call after each frame.
    pub fn update_idle_inhibit(&mut self) {
        self.idle_inhibitors
            .surfaces
            .retain(|surface| surface.is_alive());
        let inhibited = self.idle_inhibited();
        // A no-op unless it changed.
        self.globals.idle_notifier.set_is_inhibited(inhibited);
    }

    /// The apps holding a D-Bus inhibit cookie now.
    pub fn set_dbus_inhibitors(&mut self, inhibitors: Vec<Inhibitor>) {
        self.idle_inhibitors.dbus = inhibitors;
        self.update_idle_inhibit();
    }

    /// Whether something inhibits idle now: a D-Bus cookie or a visible
    /// surface's inhibitor, unless locked.
    pub fn idle_inhibited(&self) -> bool {
        !self.is_locked()
            && (!self.idle_inhibitors.dbus.is_empty()
                || self
                    .idle_inhibitors
                    .surfaces
                    .iter()
                    .any(|surface| self.surface_alive_and_visible(surface)))
    }

    /// What keeps the screen on now, for `pila msg idle-inhibitors`: the
    /// apps holding a D-Bus cookie, then the windows and layer surfaces whose
    /// Wayland inhibitor counts (visible ones), by their app id or
    /// namespace. Listed while locked too, when none of them counts.
    pub(crate) fn ipc_idle_inhibitors(&self) -> Vec<Value> {
        let dbus = self.idle_inhibitors.dbus.iter().map(|inhibitor| {
            json!({ "source": "dbus", "app": inhibitor.app, "reason": inhibitor.reason })
        });
        let wayland = self
            .idle_inhibitors
            .surfaces
            .iter()
            .filter(|surface| self.surface_alive_and_visible(surface))
            .filter_map(|surface| {
                let root = crate::redraw::root_surface(surface);
                let app = match self.window_for_surface(&root) {
                    Some(window) => crate::foreign_toplevel::app_id(&window),
                    None => self.layer_for_surface(&root)?.1.namespace().to_string(),
                };
                Some(json!({ "source": "wayland", "app": app, "reason": Value::Null }))
            });
        let mut wayland: Vec<Value> = wayland.collect();
        // A stable order (the surfaces are a set).
        wayland.sort_by(|a, b| a["app"].as_str().cmp(&b["app"].as_str()));
        dbus.chain(wayland).collect()
    }

    /// Whether `surface` is alive and belongs to a mapped window or a layer
    /// surface.
    fn surface_alive_and_visible(&self, surface: &WlSurface) -> bool {
        if !surface.is_alive() {
            return false;
        }
        let root = crate::redraw::root_surface(surface);
        let mapped_window = self
            .space
            .elements()
            .any(|w| w.wl_surface().is_some_and(|s| *s == root));
        mapped_window || self.layer_for_surface(&root).is_some()
    }
}

// ext_idle_notifier_v1
impl IdleNotifierHandler for Pila {
    fn idle_notifier_state(&mut self) -> &mut IdleNotifierState<Self> {
        &mut self.globals.idle_notifier
    }
}

// zwp_idle_inhibit_manager_v1
impl IdleInhibitHandler for Pila {
    fn inhibit(&mut self, surface: WlSurface) {
        self.idle_inhibitors.surfaces.insert(surface);
        self.update_idle_inhibit();
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        self.idle_inhibitors.surfaces.remove(&surface);
        self.update_idle_inhibit();
    }
}
