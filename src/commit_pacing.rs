//! wp-fifo: a client's content updates at most one per refresh.
//!
//! Smithay blocks a commit that waits on a barrier
//! (`FifoManagerState`, managed); pila signals the barrier. A commit that
//! set one registers its surface here (`barriers`, so a frame looks at those
//! few surfaces and not at every one), and the frame that shows it signals
//! it: the next commit applies, and reaches the screen on the next refresh
//! at the earliest, since a frame is submitted once per refresh.
//!
//! A surface that isn't being drawn must not hold its client back for good:
//! a barrier older than `HIDDEN_BARRIER_WAIT` is signaled anyway, wherever
//! the surface is (off the `Space`, covered, locked away, an output turned
//! off) and whether or not anything is drawn, as the protocol allows. So a
//! hidden client goes on at the pace of its hidden frame callbacks.

use std::time::{Duration, Instant};

use smithay::{
    delegate_fifo,
    output::Output,
    reexports::{
        calloop::timer::{TimeoutAction, Timer},
        wayland_server::{protocol::wl_surface::WlSurface, Client, DisplayHandle, Resource, Weak},
    },
    wayland::{
        compositor::{with_states, CompositorHandler},
        fifo::{FifoBarrierCachedState, FifoManagerState},
    },
};

use crate::Pila;

/// How long a barrier can go unsignaled by a frame.
const HIDDEN_BARRIER_WAIT: Duration = Duration::from_secs(1);

/// A surface whose last commit set a barrier.
struct PendingBarrier {
    surface: Weak<WlSurface>,
    since: Instant,
}

/// What's waiting to be signaled, in `Pila`.
#[derive(Default)]
pub struct CommitPacing {
    barriers: Vec<PendingBarrier>,
    /// Clients with a barrier signaled, whose commits are still to apply.
    unblocked: Vec<Client>,
    /// The timer for `HIDDEN_BARRIER_WAIT` is running.
    barrier_timer: bool,
}

pub fn init_globals(dh: &DisplayHandle) {
    FifoManagerState::new::<Pila>(dh);
}
delegate_fifo!(Pila);

/// Signal the barrier `surface`'s last applied commit set, if it's still there.
fn signal_barrier(surface: &WlSurface, unblocked: &mut Vec<Client>) {
    let barrier = with_states(surface, |states| {
        states
            .cached_state
            .get::<FifoBarrierCachedState>()
            .current()
            .barrier
            .take()
    });
    if let Some(barrier) = barrier {
        barrier.signal();
        if let Some(client) = surface.client() {
            if !unblocked.contains(&client) {
                unblocked.push(client);
            }
        }
    }
}

impl Pila {
    /// A commit of `surface` was applied: remember a barrier it set.
    pub fn commit_paced(&mut self, surface: &WlSurface) {
        let has_barrier = with_states(surface, |states| {
            states
                .cached_state
                .get::<FifoBarrierCachedState>()
                .current()
                .barrier
                .is_some()
        });
        if !has_barrier {
            return;
        }
        let pacing = &mut self.commit_pacing;
        let known = pacing
            .barriers
            .iter()
            .any(|b| b.surface.id() == surface.id());
        if !known {
            pacing.barriers.push(PendingBarrier {
                surface: surface.downgrade(),
                since: Instant::now(),
            });
        }
        self.schedule_barrier_timer();
    }

    /// `output` was drawn: signal the barriers of the surfaces it shows.
    pub fn signal_barriers_on(&mut self, output: &Output) {
        let pacing = &mut self.commit_pacing;
        let unblocked = &mut pacing.unblocked;
        pacing.barriers.retain(|barrier| {
            let Ok(surface) = barrier.surface.upgrade() else {
                return false;
            };
            let shown_here = with_states(&surface, |states| {
                smithay::desktop::utils::surface_primary_scanout_output(&surface, states)
            })
            .is_some_and(|primary| primary == *output);
            if shown_here {
                signal_barrier(&surface, unblocked);
            }
            !shown_here
        });
    }

    /// Apply the commits that were waiting on barriers just signaled, and
    /// draw them. Called from `after_frames`.
    pub fn apply_unblocked_commits(&mut self) {
        if self.apply_commits_of_unblocked_clients() {
            self.loop_handle.insert_idle(|state| state.queue_redraw());
            self.wake.ping();
        }
    }

    /// Whether any client had a barrier signaled.
    fn apply_commits_of_unblocked_clients(&mut self) -> bool {
        let clients = std::mem::take(&mut self.commit_pacing.unblocked);
        let display = self.display_handle.clone();
        for client in &clients {
            self.client_compositor_state(client)
                .blocker_cleared(self, &display);
        }
        !clients.is_empty()
    }

    /// Run the timer that frees barriers no frame signals, while there are any.
    fn schedule_barrier_timer(&mut self) {
        if self.commit_pacing.barrier_timer {
            return;
        }
        let Some(oldest) = self.commit_pacing.barriers.iter().map(|b| b.since).min() else {
            return;
        };
        let timer = Timer::from_deadline(oldest + HIDDEN_BARRIER_WAIT);
        let inserted = self.loop_handle.insert_source(timer, |_, _, state| {
            state.commit_pacing.barrier_timer = false;
            state.signal_old_barriers();
            TimeoutAction::Drop
        });
        self.commit_pacing.barrier_timer = inserted.is_ok();
    }

    /// Signal the barriers that waited too long, wherever their surfaces are.
    fn signal_old_barriers(&mut self) {
        let now = Instant::now();
        let pacing = &mut self.commit_pacing;
        let unblocked = &mut pacing.unblocked;
        pacing.barriers.retain(|barrier| {
            let Ok(surface) = barrier.surface.upgrade() else {
                return false;
            };
            let old = now.duration_since(barrier.since) >= HIDDEN_BARRIER_WAIT;
            if old {
                signal_barrier(&surface, unblocked);
            }
            !old
        });
        // Out of a timer, so no ping: the redraw comes right after it.
        if self.apply_commits_of_unblocked_clients() {
            self.queue_redraw();
        }
        self.schedule_barrier_timer();
    }
}
