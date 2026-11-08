//! Frame statistics, for pila's performance pillar (see CLAUDE.md): never
//! make an output miss a refresh. Per output, every `REPORT_EVERY`, one line
//! on the `pila::frames` log target (`RUST_LOG=pila::frames=debug`): frames
//! drawn, the CPU time pila spent on them (average and worst) and refreshes
//! missed. A frame whose CPU time alone takes more than half the output's
//! period is logged on its own.
//!
//! CPU time runs from collecting the scene to queuing the frame; the GPU's
//! work isn't in it, but shows up as missed refreshes: vblanks without a new
//! frame while an animation runs. Those are exact on bare metal (by the DRM
//! sequence numbers of the vblanks that showed frames; `udev/`) and an
//! estimate nested (a gap of more than 1.5 periods between frames;
//! `winit.rs`), where the host decides when frames are shown. At trace level
//! every frame is logged: when it was queued (at a vblank or not) and the
//! vblank that showed it.
//!
//! Screen captures (`capture.rs`, `screencopy.rs`) build their frames on
//! the event loop too, after the outputs' (`Pila::after_frames`): a frame's
//! scene, elements and damage while one is waiting, and its copy when the
//! source changed. That's CPU time on top of the outputs' frames, logged on
//! its own line, `captures`, with the same fields (`passes` for frames), so
//! the outputs' own budget stays readable.

use std::time::Duration;

use smithay::output::Output;
use tracing::debug;

use crate::Pila;

/// How often each output's statistics are logged (when it drew anything).
const REPORT_EVERY: Duration = Duration::from_secs(5);

/// One output's frames since its last report.
#[derive(Debug, Default)]
pub struct FrameStats {
    start: Option<Duration>,
    frames: u32,
    cpu_total: Duration,
    cpu_max: Duration,
    missed: u32,
}

/// What `FrameStats` logs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Report {
    /// Seconds the report covers.
    pub secs: f64,
    pub frames: u32,
    pub cpu_avg: Duration,
    pub cpu_max: Duration,
    pub missed: u32,
}

impl FrameStats {
    /// A frame was drawn at `now`, taking `cpu`. Returns a report when one
    /// is due, and starts over.
    pub fn record(&mut self, now: Duration, cpu: Duration) -> Option<Report> {
        let start = *self.start.get_or_insert(now);
        self.frames += 1;
        self.cpu_total += cpu;
        self.cpu_max = self.cpu_max.max(cpu);
        if now.saturating_sub(start) < REPORT_EVERY {
            return None;
        }
        self.take(now)
    }

    /// What was recorded since the last report, if any frame was drawn,
    /// starting over at `now` (also for the last report, on exit).
    pub fn take(&mut self, now: Duration) -> Option<Report> {
        let start = self.start?;
        if self.frames == 0 {
            return None;
        }
        let report = Report {
            secs: now.saturating_sub(start).as_secs_f64(),
            frames: self.frames,
            cpu_avg: self.cpu_total / self.frames,
            cpu_max: self.cpu_max,
            missed: self.missed,
        };
        *self = Self {
            start: Some(now),
            ..Self::default()
        };
        Some(report)
    }

    /// `count` refreshes went by without the frame that was due.
    pub fn missed(&mut self, count: u32) {
        self.missed += count;
    }
}

/// How long one refresh of `output` lasts (60 Hz if it doesn't say).
pub fn refresh_period(output: &Output) -> Duration {
    output
        .current_mode()
        .filter(|mode| mode.refresh > 0)
        .map(|mode| Duration::from_secs_f64(1000.0 / mode.refresh as f64))
        .unwrap_or(Duration::from_micros(16_667))
}

/// Refreshes missed between two frames `gap` apart that should have been
/// one `period` apart, counting a frame as late past 1.5 periods.
pub fn missed_in_gap(gap: Duration, period: Duration) -> u32 {
    if period.is_zero() || gap.as_secs_f64() <= period.as_secs_f64() * 1.5 {
        return 0;
    }
    ((gap.as_secs_f64() / period.as_secs_f64()).round() as u32).saturating_sub(1)
}

fn log_report(output: &str, report: &Report) {
    debug!(
        target: "pila::frames",
        output,
        secs = (report.secs * 10.0).round() / 10.0,
        frames = report.frames,
        cpu_avg_ms = ms(report.cpu_avg),
        cpu_max_ms = ms(report.cpu_max),
        missed = report.missed,
        "frames"
    );
}

fn log_capture_report(report: &Report) {
    debug!(
        target: "pila::frames",
        secs = (report.secs * 10.0).round() / 10.0,
        passes = report.frames,
        cpu_avg_ms = ms(report.cpu_avg),
        cpu_max_ms = ms(report.cpu_max),
        "captures"
    );
}

fn ms(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 1e5).round() / 100.0
}

impl Pila {
    /// `output` drew a frame that took `cpu` (see the module docs).
    pub fn record_frame(&mut self, output: &Output, cpu: Duration) {
        let name = output.name();
        let period = refresh_period(output);
        if cpu > period / 2 {
            debug!(
                target: "pila::frames",
                output = name,
                cpu_ms = ms(cpu),
                period_ms = ms(period),
                "slow frame"
            );
        }
        let now = self.animation_time();
        let report = self
            .frame_stats
            .entry(name.clone())
            .or_default()
            .record(now, cpu);
        if let Some(report) = report {
            log_report(&name, &report);
        }
    }

    /// A capture built its frame (and copied it if it changed), taking
    /// `cpu` (see the module docs).
    pub fn record_capture(&mut self, cpu: Duration) {
        let now = self.animation_time();
        if let Some(report) = self.capture_stats.record(now, cpu) {
            log_capture_report(&report);
        }
    }

    /// Log what each output drew since its last report, and the captures
    /// (pila is quitting).
    pub fn report_frames(&mut self) {
        let now = self.animation_time();
        for (name, stats) in &mut self.frame_stats {
            if let Some(report) = stats.take(now) {
                log_report(name, &report);
            }
        }
        if let Some(report) = self.capture_stats.take(now) {
            log_capture_report(&report);
        }
    }

    /// `output` missed `count` refreshes.
    pub fn record_missed(&mut self, output: &Output, count: u32) {
        if count == 0 {
            return;
        }
        debug!(target: "pila::frames", output = output.name(), count, "missed refresh");
        self.frame_stats
            .entry(output.name())
            .or_default()
            .missed(count);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn reports_every_few_seconds_and_starts_over() {
        let mut stats = FrameStats::default();
        assert_eq!(stats.record(Duration::ZERO, 2 * MS), None);
        stats.missed(1);
        assert_eq!(stats.record(1000 * MS, 4 * MS), None);
        let report = stats.record(5000 * MS, 3 * MS).expect("due after 5 s");
        assert_eq!(report.frames, 3);
        assert_eq!(report.cpu_avg, 3 * MS);
        assert_eq!(report.cpu_max, 4 * MS);
        assert_eq!(report.missed, 1);
        assert_eq!(report.secs, 5.0);
        // A new window, starting at the report.
        assert_eq!(stats.record(6000 * MS, 1 * MS), None);
        let report = stats.record(10_000 * MS, 1 * MS).expect("due again");
        assert_eq!(report.frames, 2);
        assert_eq!(report.missed, 0);
        // On exit, whatever is left; nothing when nothing was drawn.
        assert_eq!(stats.take(11_000 * MS), None);
        stats.record(11_000 * MS, 2 * MS);
        let report = stats.take(12_000 * MS).expect("one frame left");
        assert_eq!((report.frames, report.secs), (1, 2.0));
    }

    #[test]
    fn gaps_count_whole_refreshes() {
        let period = Duration::from_micros(16_667);
        assert_eq!(missed_in_gap(17 * MS, period), 0);
        assert_eq!(missed_in_gap(24 * MS, period), 0);
        assert_eq!(missed_in_gap(33 * MS, period), 1);
        assert_eq!(missed_in_gap(51 * MS, period), 2);
        assert_eq!(missed_in_gap(51 * MS, Duration::ZERO), 0);
    }
}
