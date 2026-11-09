//! ext-session-lock: screen lockers such as swaylock.
//!
//! While locked, only lock surfaces are drawn (on black) and only they get
//! input: `surface_under` returns the lock surface of the output under the
//! pointer, keyboard focus stays on a lock surface, popup and move/resize
//! grabs are refused, and keybindings don't run (except Quit in debug builds,
//! so a nested session can always be escaped). If the locker dies, the session
//! stays locked; another locker may then take over the lock.
//!
//! Locking is confirmed to the client (`SessionLocker::lock`) only after a
//! locked frame has been rendered (`Pila::confirm_lock`, from the backend).
//!
//! Smithay 0.7 called `SessionLockHandler::unlock` for an `unlock_and_destroy`
//! on *any* lock object, even one that never got the lock (it posted a protocol
//! error but still called `unlock`); its main branch checks the lock first, and
//! pila still does. `unlock` only schedules a check: the
//! session unlocks once the lock object of the client holding the lock is
//! gone, which a genuine `unlock_and_destroy` (a destructor) guarantees.

use smithay::{
    desktop::utils::under_from_surface_tree,
    desktop::WindowSurfaceType,
    input::pointer::{CursorImageStatus, MotionEvent},
    output::Output,
    reexports::{
        wayland_protocols::ext::session_lock::v1::server::ext_session_lock_v1::ExtSessionLockV1,
        wayland_server::{
            protocol::{wl_output::WlOutput, wl_surface::WlSurface},
            Resource,
        },
    },
    utils::{Logical, Point, Serial, SERIAL_COUNTER},
    wayland::session_lock::{
        LockSurface, SessionLockHandler, SessionLockManagerState, SessionLocker,
    },
};
use tracing::{info, warn};

use crate::Pila;

#[derive(Default)]
pub enum LockState {
    #[default]
    Unlocked,
    /// Lock requested; confirmed once a locked frame has been rendered.
    Locking(SessionLocker),
    Locked,
}

#[derive(Default)]
pub struct SessionLock {
    pub state: LockState,
    /// The lock object of the client holding the lock.
    lock: Option<ExtSessionLockV1>,
    /// One per output, from the client holding the lock.
    surfaces: Vec<(Output, LockSurface)>,
}

impl Pila {
    pub fn is_locked(&self) -> bool {
        !matches!(self.session_lock.state, LockState::Unlocked)
    }

    /// Locking, but not confirmed yet: the next frame must be drawn in full.
    pub fn lock_pending(&self) -> bool {
        matches!(self.session_lock.state, LockState::Locking(_))
    }

    /// The lock surface for `output`, if the locker made one.
    pub fn lock_surface(&self, output: &Output) -> Option<&LockSurface> {
        self.session_lock
            .surfaces
            .iter()
            .find(|(o, s)| o == output && s.alive())
            .map(|(_, s)| s)
    }

    /// While locked: the (sub)surface of the lock surface under `pos`, with
    /// its origin.
    pub fn lock_surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let output = self.space.output_under(pos).next()?;
        let output_loc = self.space.output_geometry(output)?.loc;
        let surface = self.lock_surface(output)?;
        under_from_surface_tree(
            surface.wl_surface(),
            pos,
            output_loc,
            WindowSurfaceType::ALL,
        )
        .map(|(surface, origin)| (surface, origin.to_f64()))
    }

    /// While locked, keyboard focus goes to the lock surface of the
    /// pointer's output (or any lock surface), never to anything else.
    pub fn focus_lock_surface(&mut self, serial: Serial) {
        let pointer = self.pointer_location();
        let surface = self
            .output_at(pointer)
            .and_then(|output| self.lock_surface(&output).cloned())
            .or_else(|| {
                self.session_lock
                    .surfaces
                    .iter()
                    .find(|(_, s)| s.alive())
                    .map(|(_, s)| s.clone())
            })
            .map(|s| s.wl_surface().clone());
        if self.keyboard_focus() != surface {
            self.set_keyboard_focus(surface.map(Into::into), serial);
        }
    }

    /// Called by the backend after it rendered a locked frame on every output.
    pub fn confirm_lock(&mut self) {
        if let LockState::Locking(_) = self.session_lock.state {
            if let LockState::Locking(locker) =
                std::mem::replace(&mut self.session_lock.state, LockState::Locked)
            {
                locker.lock();
                info!("session locked");
            }
        }
    }

    /// Forget the lock surface of an output that went away. The session stays
    /// locked; the remaining outputs keep theirs.
    pub fn drop_lock_surface(&mut self, output: &Output) {
        self.session_lock
            .surfaces
            .retain(|(o, surface)| o != output && surface.alive());
        if self.is_locked() {
            self.focus_lock_surface(SERIAL_COUNTER.next_serial());
        }
    }

    /// Size each lock surface to its output. Call when outputs change.
    pub fn configure_lock_surfaces(&self) {
        for (output, surface) in &self.session_lock.surfaces {
            let Some(geo) = self.space.output_geometry(output) else {
                continue;
            };
            surface.with_pending_state(|state| {
                state.size = Some((geo.size.w as u32, geo.size.h as u32).into());
            });
            surface.send_configure();
        }
    }

    /// Called on every commit: once a lock surface first shows content,
    /// move the pointer onto it (it had no input region before).
    pub fn handle_lock_commit(&mut self, surface: &WlSurface) {
        let is_lock_surface = self
            .session_lock
            .surfaces
            .iter()
            .any(|(_, s)| s.wl_surface() == surface);
        if !self.is_locked() || !is_lock_surface {
            return;
        }
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let under = self
            .surface_under(pointer.current_location())
            .map(|(s, _)| s);
        if pointer.current_focus() != under {
            self.refresh_pointer_focus();
        }
    }

    /// Re-pick the pointer's focus at its current position, e.g. after the
    /// session (un)locked and what's under it changed.
    pub(crate) fn refresh_pointer_focus(&mut self) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let location = pointer.current_location();
        let under = self.surface_under(location);
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time: smithay::backend::input::InputTime::now(),
            },
        );
        pointer.frame(self);
    }

    /// Second half of `SessionLockHandler::unlock`, run after the request
    /// that triggered it has been dispatched (see the module docs).
    fn finish_unlock(&mut self, lock: Option<ExtSessionLockV1>) {
        if !self.is_locked() || self.session_lock.lock != lock {
            return;
        }
        if lock.as_ref().is_some_and(|l| l.is_alive()) {
            warn!("ignoring unlock request from a client that doesn't hold the lock");
            return;
        }
        info!("session unlocked");
        self.session_lock = SessionLock::default();
        // Runs outside client dispatch, which would otherwise queue it.
        self.queue_redraw();
        self.focus_recent_window();
        self.refresh_pointer_focus();
    }
}

impl SessionLockHandler for Pila {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.protocols.session_lock
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        self.close_window_menu();
        let held = self
            .session_lock
            .lock
            .as_ref()
            .is_some_and(|l| l.is_alive());
        if self.is_locked() && held {
            // Already locked by a live client: dropping `confirmation` tells
            // the new one it failed (`finished`).
            warn!("refusing a second session lock");
            return;
        }

        let already_locked = matches!(self.session_lock.state, LockState::Locked);
        self.session_lock.lock = Some(confirmation.ext_session_lock().clone());
        self.session_lock.surfaces.clear();
        self.session_lock.state = if already_locked {
            // A new locker taking over from one that died: nothing but black
            // is on screen already.
            info!("session lock taken over by a new client");
            confirmation.lock();
            LockState::Locked
        } else {
            info!("locking session");
            LockState::Locking(confirmation)
        };

        // Drop menus, drags and keyboard focus so nothing keeps input.
        self.dismiss_popup_grab();
        if let Some(pointer) = self.seat.get_pointer() {
            let time = smithay::backend::input::InputTime::now();
            pointer.unset_grab(self, SERIAL_COUNTER.next_serial(), time);
        }
        self.focus_lock_surface(SERIAL_COUNTER.next_serial());
        self.set_cursor(CursorImageStatus::default_named());
        self.refresh_pointer_focus();
    }

    fn unlock(&mut self) {
        let lock = self.session_lock.lock.clone();
        self.loop_handle
            .insert_idle(move |state| state.finish_unlock(lock));
    }

    fn new_surface(&mut self, surface: LockSurface, output: WlOutput) {
        // Only the lock in force may cover the screen (Smithay already skips
        // locks that were refused; several may be in flight).
        let ours = self.session_lock.lock.as_ref() == Some(surface.ext_session_lock());
        if !self.is_locked() || !ours {
            warn!("ignoring a lock surface of a lock that isn't in force");
            return;
        }
        let Some(output) = Output::from_resource(&output) else {
            return;
        };
        crate::scale::send_scale_to_tree(surface.wl_surface(), &output);
        if let Some(geo) = self.space.output_geometry(&output) {
            // Smithay sends the initial configure after this returns.
            surface.with_pending_state(|state| {
                state.size = Some((geo.size.w as u32, geo.size.h as u32).into());
            });
        }
        self.session_lock
            .surfaces
            .retain(|(o, s)| o != &output && s.alive());
        self.session_lock.surfaces.push((output, surface));
        self.focus_lock_surface(SERIAL_COUNTER.next_serial());
        self.refresh_pointer_focus();
    }
}
