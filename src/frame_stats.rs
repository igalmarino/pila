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
//! `winit.rs`), where the host decides when frames are shown. On bare metal
//! a frame with something new (a client's buffer, the cursor moving) that was
//! asked for in time for a vblank (`IN_TIME` before it) but shown at a later
//! one counts its refreshes as missed too, animation or not, and is counted
//! `late` (`late_refreshes`). At trace level every frame is logged: when it
//! was queued (at a vblank or not) and the vblank that showed it.
//!
//! A redraw that found nothing to show on an output (it built the scene and
//! its elements, and the damage tracker found nothing new: a commit with no
//! new picture, the pointer moving with no cursor drawn) queues no frame. Those
//! are counted apart, `idle` with their CPU time summed, `idle_cpu_ms`. Only
//! the outputs a change shows on are redrawn (`redraw.rs`); on bare metal one
//! that just had nothing new waits before the next redraw that may bring
//! nothing (`udev/draw.rs::idle_wait`), at most one such redraw per refresh.
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
    late: u32,
    idle: u32,
    idle_cpu: Duration,
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
    /// Frames shown later than due (`late_refreshes`; bare metal), counted
    /// in `missed` too.
    pub late: u32,
    /// Redraws that had nothing to show, and their CPU time.
    pub idle: u32,
    pub idle_cpu: Duration,
}

impl FrameStats {
    /// A frame was drawn at `now`, taking `cpu`. Returns a report when one
    /// is due, and starts over.
    pub fn record(&mut self, now: Duration, cpu: Duration) -> Option<Report> {
        self.frames += 1;
        self.cpu_total += cpu;
        self.cpu_max = self.cpu_max.max(cpu);
        self.report_if_due(now)
    }

    /// A redraw at `now` took `cpu` and had nothing to show (no frame
    /// queued). Returns a report when one is due, as `record`.
    pub fn record_idle(&mut self, now: Duration, cpu: Duration) -> Option<Report> {
        self.idle += 1;
        self.idle_cpu += cpu;
        self.report_if_due(now)
    }

    fn report_if_due(&mut self, now: Duration) -> Option<Report> {
        let start = *self.start.get_or_insert(now);
        if now.saturating_sub(start) < REPORT_EVERY {
            return None;
        }
        self.take(now)
    }

    /// What was recorded since the last report, if any frame was drawn or
    /// redraw done, starting over at `now` (also for the last report, on exit).
    pub fn take(&mut self, now: Duration) -> Option<Report> {
        let start = self.start?;
        if self.frames == 0 && self.idle == 0 {
            return None;
        }
        let report = Report {
            secs: now.saturating_sub(start).as_secs_f64(),
            frames: self.frames,
            cpu_avg: self.cpu_total.checked_div(self.frames).unwrap_or_default(),
            cpu_max: self.cpu_max,
            missed: self.missed,
            late: self.late,
            idle: self.idle,
            idle_cpu: self.idle_cpu,
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

    /// A frame was shown late (its refreshes go to `missed`).
    pub fn late(&mut self) {
        self.late += 1;
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

/// A frame asked for at least this long before a vblank (or half a period,
/// if that's shorter) is in time for it: the pillar's budget per frame.
pub const IN_TIME: Duration = Duration::from_millis(4);

/// Refreshes a frame was shown late by: asked for at `requested`, with
/// another frame `in_flight` then or not, shown at `shown`, on an output whose
/// vblanks are `period` apart from one at `vblank`. Due is the first vblank
/// at least `IN_TIME` after the request, or with a frame in flight the one
/// after that frame's.
pub fn late_refreshes(
    requested: Duration,
    in_flight: bool,
    vblank: Duration,
    shown: Duration,
    period: Duration,
) -> u32 {
    if period.is_zero() {
        return 0;
    }
    let next = crate::commit_pacing::next_vblank;
    let due = if in_flight {
        next(vblank, requested, period) + period
    } else {
        // The first vblank at or after the request plus the budget.
        let earliest = requested + IN_TIME.min(period / 2);
        next(
            vblank,
            earliest.saturating_sub(Duration::from_nanos(1)),
            period,
        )
    };
    let late = shown.saturating_sub(due).as_secs_f64() / period.as_secs_f64();
    late.round() as u32
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
        late = report.late,
        idle = report.idle,
        idle_cpu_ms = ms(report.idle_cpu),
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

    /// A redraw of `output` took `cpu` and had nothing to show (see the
    /// module docs).
    pub fn record_idle_frame(&mut self, output: &Output, cpu: Duration) {
        let name = output.name();
        let now = self.animation_time();
        let report = self
            .frame_stats
            .entry(name.clone())
            .or_default()
            .record_idle(now, cpu);
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

    /// A frame of `output` was shown `count` refreshes late (counted in
    /// `missed` by the caller).
    pub fn record_late(&mut self, output: &Output, count: u32) {
        if count == 0 {
            return;
        }
        debug!(target: "pila::frames", output = output.name(), count, "late frame");
        self.frame_stats.entry(output.name()).or_default().late();
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
    fn redraws_with_nothing_to_show_are_counted_apart_and_reported_alone() {
        let mut stats = FrameStats::default();
        stats.record(Duration::ZERO, 2 * MS);
        stats.record_idle(1000 * MS, MS / 2);
        stats.record_idle(2000 * MS, MS / 2);
        let report = stats.record_idle(5000 * MS, MS).expect("due after 5 s");
        assert_eq!((report.frames, report.cpu_avg), (1, 2 * MS));
        assert_eq!((report.idle, report.idle_cpu), (3, 2 * MS));
        // An output with nothing new at all still reports what it spent.
        stats.record_idle(6000 * MS, MS);
        let report = stats.take(7000 * MS).expect("an idle redraw");
        assert_eq!((report.frames, report.idle), (0, 1));
        assert_eq!(report.cpu_avg, Duration::ZERO);
    }

    #[test]
    fn a_frame_asked_for_in_time_but_shown_a_vblank_late_is_counted() {
        let period = 16 * MS;
        // Vblanks at 100, 116, 132, ...
        // Asked for at 111, 5 ms before 116 (more than the 4 ms budget):
        // due at 116, shown at 132, one late.
        assert_eq!(
            late_refreshes(111 * MS, false, 100 * MS, 132 * MS, period),
            1
        );
        assert_eq!(
            late_refreshes(111 * MS, false, 100 * MS, 116 * MS, period),
            0
        );
        // Asked for 3 ms before a vblank: in time for the next only.
        assert_eq!(
            late_refreshes(113 * MS, false, 100 * MS, 132 * MS, period),
            0
        );
        assert_eq!(
            late_refreshes(113 * MS, false, 100 * MS, 148 * MS, period),
            1
        );
        // With a frame in flight the vblank after its own is due.
        assert_eq!(
            late_refreshes(102 * MS, true, 100 * MS, 132 * MS, period),
            0
        );
        assert_eq!(
            late_refreshes(102 * MS, true, 100 * MS, 164 * MS, period),
            2
        );
        // At 144 Hz the budget is half a period at most.
        let fast = Duration::from_micros(6944);
        let vblank = 100 * MS;
        let next = vblank + fast;
        assert_eq!(
            late_refreshes(next - 3 * MS, false, vblank, next + fast, fast),
            0
        );
        assert_eq!(
            late_refreshes(next - 4 * MS, false, vblank, next + fast, fast),
            1
        );
        // A vblank known from long before still sets the grid.
        assert_eq!(
            late_refreshes(1001 * MS, false, 100 * MS, 1012 * MS, period),
            0
        );
        assert_eq!(
            late_refreshes(1001 * MS, false, 100 * MS, 1028 * MS, period),
            1
        );
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
