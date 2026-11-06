//! ext-idle-notify and idle-inhibit: tell clients such as swayidle when the
//! user has been idle, unless something visible (a video player) inhibits it.
//!
//! Every input event counts as activity (`Pila::process_input_event`).
//! Smithay runs the per-notification timers on `Pila::loop_handle`. An idle
//! inhibitor only counts while its surface is visible: part of a mapped window
//! or layer surface, and the session not locked. `update_idle_inhibit`
//! re-checks that after every frame.

use std::collections::HashSet;

use smithay::{
    delegate_idle_inhibit,
    reexports::{
        wayland_protocols::ext::idle_notify::v1::server::{
            ext_idle_notification_v1::ExtIdleNotificationV1,
            ext_idle_notifier_v1::ExtIdleNotifierV1,
        },
        wayland_server::{
            delegate_dispatch, protocol::wl_surface::WlSurface, Client, DataInit, DisplayHandle,
            GlobalDispatch, New, Resource,
        },
    },
    wayland::{
        compositor::get_parent,
        idle_inhibit::IdleInhibitHandler,
        idle_notify::{IdleNotificationUserData, IdleNotifierHandler, IdleNotifierState},
    },
};

use crate::Pila;

/// Surfaces with an idle inhibitor, visible or not.
#[derive(Default)]
pub struct IdleInhibitors(HashSet<WlSurface>);

impl Pila {
    /// Count user input as activity: idle notifications resume and restart
    /// their timeouts.
    pub fn notify_activity(&mut self) {
        let seat = self.seat.clone();
        self.globals.idle_notifier.notify_activity(&seat);
    }

    /// Recompute whether a visible surface inhibits idle. Call after each frame.
    pub fn update_idle_inhibit(&mut self) {
        self.idle_inhibitors.0.retain(|surface| surface.is_alive());
        let inhibited = !self.is_locked()
            && self
                .idle_inhibitors
                .0
                .iter()
                .any(|surface| self.is_visible(surface));
        // A no-op unless it changed.
        self.globals.idle_notifier.set_is_inhibited(inhibited);
    }

    /// Whether `surface` belongs to a mapped window or a layer surface.
    fn is_visible(&self, surface: &WlSurface) -> bool {
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        let mapped_window = self
            .space
            .elements()
            .any(|w| w.toplevel().is_some_and(|t| *t.wl_surface() == root));
        mapped_window || self.layer_for_surface(&root).is_some()
    }
}

// ext_idle_notifier_v1
impl IdleNotifierHandler for Pila {
    fn idle_notifier_state(&mut self) -> &mut IdleNotifierState<Self> {
        &mut self.globals.idle_notifier
    }
}
// `delegate_idle_notify!` but for the global, which it would show to everyone:
// when the user is active isn't for sandboxed apps.
impl GlobalDispatch<ExtIdleNotifierV1, ()> for Pila {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ExtIdleNotifierV1>,
        _: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }

    crate::state::privileged_global!();
}
delegate_dispatch!(Pila: [ExtIdleNotifierV1: ()] => IdleNotifierState<Pila>);
delegate_dispatch!(Pila: [ExtIdleNotificationV1: IdleNotificationUserData] => IdleNotifierState<Pila>);

// zwp_idle_inhibit_manager_v1
impl IdleInhibitHandler for Pila {
    fn inhibit(&mut self, surface: WlSurface) {
        self.idle_inhibitors.0.insert(surface);
        self.update_idle_inhibit();
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        self.idle_inhibitors.0.remove(&surface);
        self.update_idle_inhibit();
    }
}
delegate_idle_inhibit!(Pila);
