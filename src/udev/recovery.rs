//! Recovering an output from frames that fail at the DRM level (not DRM
//! master yet after a VT switch, a monitor beyond the link's bandwidth, an
//! MST hub going away): each output on its own, so one that keeps failing
//! doesn't stop the others, trying again later and later and logging less
//! and less.

use std::time::Duration;

/// The wait after an output's first failure; it doubles with every one
/// after it, up to `MAX_DELAY`.
const FIRST_DELAY: Duration = Duration::from_millis(100);
const MAX_DELAY: Duration = Duration::from_secs(10);

/// An output's failures since its last frame queued (`OutputSurface::recovery`).
#[derive(Debug, Default)]
pub(super) struct Recovery {
    failures: u32,
    /// A retry is scheduled: the output isn't drawn until it's due.
    waiting: bool,
}

impl Recovery {
    /// A frame failed: how long to wait before trying again, or `None` if a
    /// retry is scheduled already.
    pub fn failed(&mut self) -> Option<Duration> {
        self.failures = self.failures.saturating_add(1);
        if self.waiting {
            return None;
        }
        self.waiting = true;
        Some(delay(self.failures))
    }

    /// The retry is due: the output may be drawn again.
    pub fn retry(&mut self) {
        self.waiting = false;
    }

    /// A frame was queued: start from scratch. The failures before it.
    pub fn succeeded(&mut self) -> u32 {
        std::mem::take(self).failures
    }

    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// Whether this failure is logged: the 1st, 2nd, 4th, 8th…, so an output
    /// that never recovers doesn't flood the log.
    pub fn should_log(&self) -> bool {
        self.failures.is_power_of_two()
    }

    /// Whether the output waits for its retry instead of being drawn.
    pub fn blocks_render(&self) -> bool {
        self.waiting
    }
}

/// The wait after the `failures`th failure in a row.
fn delay(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(16);
    FIRST_DELAY.saturating_mul(1 << doublings).min(MAX_DELAY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wait_doubles_up_to_a_limit() {
        let waits: Vec<u64> = (1..=10).map(|n| delay(n).as_millis() as u64).collect();
        assert_eq!(
            waits,
            [100, 200, 400, 800, 1600, 3200, 6400, 10_000, 10_000, 10_000]
        );
        assert_eq!(delay(u32::MAX), MAX_DELAY);
    }

    #[test]
    fn a_failing_output_waits_for_its_retry() {
        let mut recovery = Recovery::default();
        assert!(!recovery.blocks_render());
        assert_eq!(recovery.failed(), Some(Duration::from_millis(100)));
        assert!(recovery.blocks_render());
        // Already scheduled: no second retry.
        assert_eq!(recovery.failed(), None);
        recovery.retry();
        assert!(!recovery.blocks_render());
        assert_eq!(recovery.failed(), Some(Duration::from_millis(400)));
    }

    #[test]
    fn failures_are_logged_ever_more_rarely() {
        let mut recovery = Recovery::default();
        let mut logged = Vec::new();
        for _ in 0..20 {
            recovery.failed();
            recovery.retry();
            if recovery.should_log() {
                logged.push(recovery.failures());
            }
        }
        assert_eq!(logged, [1, 2, 4, 8, 16]);
    }

    #[test]
    fn a_frame_queued_starts_from_scratch() {
        let mut recovery = Recovery::default();
        recovery.failed();
        recovery.retry();
        recovery.failed();
        recovery.retry();
        assert_eq!(recovery.succeeded(), 2);
        assert_eq!(recovery.failures(), 0);
        assert_eq!(recovery.failed(), Some(FIRST_DELAY));
        assert_eq!(Recovery::default().succeeded(), 0);
    }

    #[test]
    fn outputs_recover_independently() {
        let (mut broken, mut fine) = (Recovery::default(), Recovery::default());
        broken.failed();
        assert!(broken.blocks_render());
        assert!(!fine.blocks_render());
        assert_eq!(fine.succeeded(), 0);
    }
}
