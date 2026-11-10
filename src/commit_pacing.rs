//! When a client's commits apply: wp-fifo and wp-commit-timing.
//!
//! wp-fifo gives a client's content updates at most one per refresh. Smithay
//! blocks a commit that waits on a barrier
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
//!
//! wp-commit-timing holds a commit back until the refresh at or after its
//! timestamp. Smithay's unmanaged mode leaves the blocking to pila, which needs
//! to know the deadline: on an idle screen no frame would come to apply it. A
//! timer wakes pila a period before the earliest deadline to draw the frame
//! that may show it (`signal_commit_timers`, before each frame is built), and
//! at the deadline itself applies it anyway, in case no frame was drawn. This
//! isn't frame pacing: the client asked for that moment. A surface is held
//! for as long as its client asks, whatever it's showing.

use std::time::{Duration, Instant};

use smithay::{
    output::Output,
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            RegistrationToken,
        },
        wayland_server::{protocol::wl_surface::WlSurface, Client, DisplayHandle, Resource, Weak},
    },
    utils::{Monotonic, Time},
    wayland::{
        commit_timing::{
            CommitTimerBarrierStateUserData, CommitTimerStateUserData, CommitTimingManagerState,
        },
        compositor::{add_blocker, with_states, CompositorHandler},
        fifo::{FifoBarrierCachedState, FifoManagerState},
    },
};

use crate::{frame_stats::refresh_period, Pila};

/// How long a barrier can go unsignaled by a frame.
const HIDDEN_BARRIER_WAIT: Duration = Duration::from_secs(1);

/// The longest the deadline timer sleeps. A client can ask for any moment,
/// and a far one must not overflow the timer: it just wakes up and checks.
const LONGEST_DEADLINE_WAIT: Duration = Duration::from_secs(3600);

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
    /// Surfaces with commits held back until a timestamp.
    timed: Vec<Weak<WlSurface>>,
    /// The timer for the earliest of their deadlines.
    deadline_timer: Option<RegistrationToken>,
}

pub fn init_globals(dh: &DisplayHandle) {
    FifoManagerState::new::<Pila>(dh);
    CommitTimingManagerState::unmanaged::<Pila>(dh);
}

/// The first vblank after `now`, given the time of an earlier one.
pub fn next_vblank(last: Duration, now: Duration, period: Duration) -> Duration {
    let periods = now.saturating_sub(last).as_nanos() / period.as_nanos().max(1) + 1;
    last + period * u32::try_from(periods).unwrap_or(u32::MAX)
}

/// How long to sleep before acting on a deadline `until` from now: until a
/// period before it, to draw the frame that shows it; once within that, until
/// the deadline itself.
fn deadline_wait(until: Duration, period: Duration) -> Duration {
    match until.saturating_sub(period) {
        Duration::ZERO => until,
        before => before,
    }
    .min(LONGEST_DEADLINE_WAIT)
}

/// `surface`'s earliest deadline, if it has commits held back.
fn next_deadline(surface: &WlSurface) -> Option<Time<Monotonic>> {
    with_states(surface, |states| {
        let barriers = states.data_map.get::<CommitTimerBarrierStateUserData>()?;
        let deadline = barriers.lock().unwrap().next_deadline()?;
        Some(deadline.into())
    })
}

/// Apply `surface`'s commits with timestamps up to `until`; whether there were
/// any.
fn signal_timer(surface: &WlSurface, until: Time<Monotonic>) -> bool {
    with_states(surface, |states| {
        states
            .data_map
            .get::<CommitTimerBarrierStateUserData>()
            .is_some_and(|barriers| barriers.lock().unwrap().signal_until(until))
    })
}

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
        // From `after_frames`: the redraws the commits ask for are deferred.
        self.defer_redraws(|state| {
            state.apply_commits_of_unblocked_clients();
        });
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

    /// Before a commit applies: if it has a timestamp, hold it back until then.
    pub fn hold_commit_until_timestamp(&mut self, surface: &WlSurface) {
        let barrier = with_states(surface, |states| {
            let timestamp = states
                .data_map
                .get::<CommitTimerStateUserData>()
                .and_then(|state| state.borrow_mut().timestamp.take())?;
            let barriers = states
                .data_map
                .get_or_insert_threadsafe(CommitTimerBarrierStateUserData::default);
            let barrier = barriers.lock().unwrap().register(timestamp);
            Some(barrier)
        });
        let Some(barrier) = barrier else {
            return;
        };
        add_blocker(surface, barrier);
        let timed = &mut self.commit_pacing.timed;
        if !timed.iter().any(|known| known.id() == surface.id()) {
            timed.push(surface.downgrade());
        }
        self.schedule_deadline_timer();
    }

    /// About to build a frame of `output`: the commits with a timestamp up to
    /// when it will show, on this output or none, apply and get in the frame.
    pub fn signal_commit_timers(&mut self, output: &Output) {
        if self.commit_pacing.timed.is_empty() {
            return;
        }
        let now = self.clock.now();
        let Some(target) = self
            .backend
            .as_ref()
            .map(|backend| backend.next_frame_time(output, now))
        else {
            return;
        };
        self.signal_timers_until(target, Some(output));
    }

    /// Apply the commits with timestamps up to `until`, of surfaces on
    /// `output` (or on none) if one is given.
    fn signal_timers_until(&mut self, until: Time<Monotonic>, output: Option<&Output>) {
        let mut unblocked: Vec<Client> = Vec::new();
        self.commit_pacing.timed.retain(|surface| {
            let Ok(surface) = surface.upgrade() else {
                return false;
            };
            let here = output.is_none_or(|output| {
                with_states(&surface, |states| {
                    smithay::desktop::utils::surface_primary_scanout_output(&surface, states)
                })
                .is_none_or(|primary| primary == *output)
            });
            if here && signal_timer(&surface, until) {
                if let Some(client) = surface.client() {
                    if !unblocked.contains(&client) {
                        unblocked.push(client);
                    }
                }
            }
            next_deadline(&surface).is_some()
        });
        if unblocked.is_empty() {
            return;
        }
        let display = self.display_handle.clone();
        for client in &unblocked {
            self.client_compositor_state(client)
                .blocker_cleared(self, &display);
        }
        // What applied asks for its redraws (`redraw_committed`), on
        // whichever output it shows.
        self.schedule_deadline_timer();
    }

    /// Set the timer for the earliest deadline of the commits held back.
    fn schedule_deadline_timer(&mut self) {
        if let Some(token) = self.commit_pacing.deadline_timer.take() {
            self.loop_handle.remove(token);
        }
        let earliest = self
            .commit_pacing
            .timed
            .iter()
            .filter_map(|surface| next_deadline(&surface.upgrade().ok()?))
            .min();
        let Some(earliest) = earliest else {
            return;
        };
        let now = self.clock.now();
        let until = if earliest > now {
            Time::elapsed(&now, earliest)
        } else {
            Duration::ZERO
        };
        // The longest period, to wake up in time for any output.
        let period = self
            .space
            .outputs()
            .map(refresh_period)
            .max()
            .unwrap_or(Duration::from_micros(16_667));
        let timer = Timer::from_duration(deadline_wait(until, period));
        let inserted = self.loop_handle.insert_source(timer, |_, _, state| {
            state.commit_pacing.deadline_timer = None;
            // Past deadlines apply even if no frame comes.
            let now = state.clock.now();
            // The commits that apply ask for their redraws.
            state.signal_timers_until(now, None);
            state.housekeeping = true;
            state.schedule_deadline_timer();
            TimeoutAction::Drop
        });
        self.commit_pacing.deadline_timer = inserted.ok();
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
        // The commits that apply ask for their redraws.
        if self.apply_commits_of_unblocked_clients() {
            self.housekeeping = true;
        }
        self.schedule_barrier_timer();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PERIOD: Duration = Duration::from_millis(10);

    #[test]
    fn the_next_vblank_is_always_after_now() {
        let last = Duration::from_millis(100);
        let at = |ms| next_vblank(last, Duration::from_millis(ms), PERIOD);
        assert_eq!(at(100), Duration::from_millis(110));
        assert_eq!(at(105), Duration::from_millis(110));
        assert_eq!(at(110), Duration::from_millis(120));
        assert_eq!(at(137), Duration::from_millis(140));
        // A clock read just before the vblank's own timestamp.
        assert_eq!(at(99), Duration::from_millis(110));
    }

    #[test]
    fn a_deadline_wakes_a_period_before_it_then_at_it() {
        let wait = |ms| deadline_wait(Duration::from_millis(ms), PERIOD);
        assert_eq!(wait(100), Duration::from_millis(90));
        assert_eq!(wait(10), Duration::from_millis(10));
        assert_eq!(wait(4), Duration::from_millis(4));
        assert_eq!(wait(0), Duration::ZERO);
        assert_eq!(
            deadline_wait(Duration::from_secs(u32::MAX as u64), PERIOD),
            LONGEST_DEADLINE_WAIT
        );
    }
}
