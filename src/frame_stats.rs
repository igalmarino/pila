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
//! On bare metal a frame can be a client's buffer as it is (direct scanout:
//! a fullscreen video or game that is all the output shows, on the primary
//! plane; `udev/draw.rs::frame_flags`): nothing is drawn for it. Those are
//! counted in `frames` and apart, `direct`. A frame shown at once, torn, by an
//! asynchronous page flip rather than at a vblank (`tearing.rs`) is counted
//! apart too, `torn`, and misses no refresh.
//!
//! Screen captures (`capture.rs`, `screencopy.rs`) build their frames on
//! the event loop too, after the outputs' (`Pila::after_frames`): a frame's
//! scene, elements and damage while one is waiting, and its copy when the
//! source changed. That's CPU time on top of the outputs' frames, logged on
//! its own line, `captures`, with the same fields (`passes` for frames), so
//! the outputs' own budget stays readable.
//!
//! Clients' shm buffers are copied to the GPU at their commits rather than
//! in the frame that shows them (`upload.rs`): CPU time outside the frames,
//! logged on its own line, `uploads`, with how many commits and their CPU
//! time, average and worst.
//!
//! The event loop's own bookkeeping after each iteration (`Pila::after_frames`:
//! taskbars, IPC events, the `Space`'s refresh, animations; captures left out)
//! runs as often as events come (input, clients), not once per frame: a
//! `loop` line counts the iterations, those that did all of it
//! (`housekeeping`) and their CPU time, summed and worst (`LoopStats`).
//!
//! The GPU's part of a frame is measured apart, where the renderer gives its
//! frames a native fence (`SyncPoint::export`): from when the frame was handed
//! to the GPU (its rendering submitted) to when the GPU finished it, the time
//! the kernel stamped on the fence when it signaled (`fence_signaled_at`),
//! read once the fence is ready (`Pila::watch_gpu`, an event source on it, so
//! nothing waits for the GPU). That's the frame's own GPU work and whatever
//! the GPU had queued before it (clients' rendering), the time the frame
//! needs on top of its CPU time to be ready for its vblank. The report gives
//! how many frames were measured (`gpu_frames`: none without native fences,
//! and direct scanout draws nothing) and their GPU time, average and worst; a
//! frame whose GPU time alone takes more than half the period is logged on its
//! own, and at trace level every frame's (`GPU done`). Not judged: there's no
//! budget for it (yet).
//!
//! On bare metal more: how many frames were held to be drawn close to their
//! vblank (`held`, `udev/render_late.rs`) and how far before it (`margin_avg_ms`,
//! `margin_max_ms`), how long before it they were ready (`spare_frames`,
//! `spare_avg_ms`, `spare_min_ms`: the room the margin left), and the latency of the frames
//! with something new: from the first redraw asked for with it (a client's
//! commit, input, an animation) to the vblank that showed it, average and
//! worst (`latency_avg_ms`, `latency_max_ms`).

use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::time::Duration;

use smithay::backend::renderer::sync::SyncPoint;
use smithay::output::Output;
use smithay::reexports::calloop::{generic::Generic, Interest, Mode, PostAction};
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
    direct: u32,
    torn: u32,
    idle: u32,
    idle_cpu: Duration,
    gpu_frames: u32,
    gpu_total: Duration,
    gpu_max: Duration,
    held: u32,
    margin_total: Duration,
    margin_max: Duration,
    spare_frames: u32,
    spare_total: Duration,
    spare_min: Option<Duration>,
    latency_frames: u32,
    latency_total: Duration,
    latency_max: Duration,
    /// When the GPU last finished one of its frames (`gpu_time`), kept
    /// across reports.
    last_gpu_signal: Option<Duration>,
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
    /// Frames that were a client's buffer as it is (direct scanout; bare
    /// metal), counted in `frames` too.
    pub direct: u32,
    /// Frames shown at once, torn, rather than at a vblank (`tearing.rs`;
    /// bare metal), counted in `frames` too.
    pub torn: u32,
    /// Redraws that had nothing to show, and their CPU time.
    pub idle: u32,
    pub idle_cpu: Duration,
    /// Frames whose GPU time was measured (native fences), and that time.
    pub gpu_frames: u32,
    pub gpu_avg: Duration,
    pub gpu_max: Duration,
    /// Frames held to be drawn close to their vblank (bare metal), counted
    /// in `frames` too.
    pub held: u32,
    /// How far before their vblank the held frames were drawn: average and
    /// worst.
    pub margin_avg: Duration,
    pub margin_max: Duration,
    /// How long before their vblank the held frames were ready (bare
    /// metal): how many, average and least.
    pub spare_frames: u32,
    pub spare_avg: Duration,
    pub spare_min: Duration,
    /// From a redraw asked for to the vblank that showed it (bare metal):
    /// how many, average and worst.
    pub latency_frames: u32,
    pub latency_avg: Duration,
    pub latency_max: Duration,
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
            direct: self.direct,
            torn: self.torn,
            idle: self.idle,
            idle_cpu: self.idle_cpu,
            gpu_frames: self.gpu_frames,
            gpu_avg: self
                .gpu_total
                .checked_div(self.gpu_frames)
                .unwrap_or_default(),
            gpu_max: self.gpu_max,
            held: self.held,
            margin_avg: self.margin_total.checked_div(self.held).unwrap_or_default(),
            margin_max: self.margin_max,
            spare_frames: self.spare_frames,
            spare_avg: self
                .spare_total
                .checked_div(self.spare_frames)
                .unwrap_or_default(),
            spare_min: self.spare_min.unwrap_or_default(),
            latency_frames: self.latency_frames,
            latency_avg: self
                .latency_total
                .checked_div(self.latency_frames)
                .unwrap_or_default(),
            latency_max: self.latency_max,
        };
        *self = Self {
            start: Some(now),
            last_gpu_signal: self.last_gpu_signal,
            ..Self::default()
        };
        Some(report)
    }

    /// A frame handed to the GPU at `submitted`, whose rendering began at
    /// `started`, was finished by the GPU at `signal`: its GPU time, recorded
    /// for the next report, if the fence was this frame's (`gpu_time`).
    pub fn record_gpu(
        &mut self,
        started: Duration,
        submitted: Duration,
        signal: Duration,
    ) -> Option<Duration> {
        let gpu = gpu_time(started, submitted, signal, self.last_gpu_signal)?;
        self.last_gpu_signal = Some(signal);
        self.gpu_frames += 1;
        self.gpu_total += gpu;
        self.gpu_max = self.gpu_max.max(gpu);
        Some(gpu)
    }

    /// `count` refreshes went by without the frame that was due.
    pub fn missed(&mut self, count: u32) {
        self.missed += count;
    }

    /// A frame was shown late (its refreshes go to `missed`).
    pub fn late(&mut self) {
        self.late += 1;
    }

    /// The frame about to be recorded is a client's buffer as it is.
    pub fn direct(&mut self) {
        self.direct += 1;
    }

    /// A frame was shown at once, torn.
    pub fn torn(&mut self) {
        self.torn += 1;
    }

    /// The frame about to be recorded was held to be drawn `margin` before
    /// its vblank.
    pub fn held(&mut self, margin: Duration) {
        self.held += 1;
        self.margin_total += margin;
        self.margin_max = self.margin_max.max(margin);
    }

    /// A held frame was ready `spare` before its vblank.
    pub fn spare(&mut self, spare: Duration) {
        self.spare_frames += 1;
        self.spare_total += spare;
        self.spare_min = Some(self.spare_min.map_or(spare, |min| min.min(spare)));
    }

    /// A frame was shown `latency` after the redraw that asked for it.
    pub fn latency(&mut self, latency: Duration) {
        self.latency_frames += 1;
        self.latency_total += latency;
        self.latency_max = self.latency_max.max(latency);
    }
}

/// The event loop's iterations since the last report (see the module docs).
#[derive(Debug, Default)]
pub struct LoopStats {
    start: Option<Duration>,
    iterations: u32,
    housekeeping: u32,
    cpu_total: Duration,
    cpu_max: Duration,
    /// Iterations and full housekeeping runs since startup (`state_json`).
    pub total: (u64, u64),
}

/// The GPU time of a frame (see the module docs) whose rendering began at
/// `started` and was handed to the GPU at `submitted`, from the time its
/// fence signaled, `signal`, if that fence was the frame's own: one that
/// signaled before the frame began, or at the very moment the last one
/// measured did (`last`: the same fence, which Smithay hands back when the
/// primary plane had nothing to draw and kept its last buffer), is an earlier
/// frame's. A GPU done before the submission ended took no time past it.
pub fn gpu_time(
    started: Duration,
    submitted: Duration,
    signal: Duration,
    last: Option<Duration>,
) -> Option<Duration> {
    if signal < started || last == Some(signal) {
        return None;
    }
    Some(signal.saturating_sub(submitted))
}

/// A frame's rendering on its way to the GPU, to measure (`Pila::watch_gpu`).
#[derive(Debug)]
pub struct GpuFrame {
    /// Its native fence, signaled when the GPU is done with it.
    fence: OwnedFd,
    /// When its rendering began and when it was handed to the GPU, on the
    /// monotonic clock (`Pila::clock`).
    started: Duration,
    submitted: Duration,
}

impl GpuFrame {
    /// The frame whose rendering ended in `sync`, begun at `started` and
    /// submitted at `submitted`; `None` if `sync` has no native fence (the
    /// renderer has none, or it waited for the GPU itself), which is logged
    /// once.
    pub fn new(sync: &SyncPoint, started: Duration, submitted: Duration) -> Option<Self> {
        let Some(fence) = sync.export() else {
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| {
                debug!(
                    target: "pila::frames",
                    "GPU time not measured: a frame came with no native fence \
                     (software rendering gives none)"
                );
            });
            return None;
        };
        Some(Self {
            fence,
            started,
            submitted,
        })
    }
}

/// `struct sync_file_info` (`linux/sync_file.h`).
#[repr(C)]
#[derive(Default)]
struct SyncFileInfo {
    name: [u8; 32],
    status: i32,
    flags: u32,
    num_fences: u32,
    pad: u32,
    sync_fence_info: u64,
}

/// `struct sync_fence_info` (`linux/sync_file.h`).
#[repr(C)]
#[derive(Clone, Copy)]
struct SyncFenceInfo {
    obj_name: [u8; 32],
    driver_name: [u8; 32],
    status: i32,
    flags: u32,
    timestamp_ns: u64,
}

const _: () = assert!(std::mem::size_of::<SyncFileInfo>() == 56);
const _: () = assert!(std::mem::size_of::<SyncFenceInfo>() == 80);

/// `SYNC_IOC_FILE_INFO`: `_IOWR('>', 4, struct sync_file_info)`.
const SYNC_IOC_FILE_INFO: u32 = (3 << 30) | (56 << 16) | ((b'>' as u32) << 8) | 4;

/// When the GPU finished the work behind `fence` (a sync_file), on the
/// monotonic clock, if it has: the time the kernel stamped on it when it
/// signaled (`SYNC_IOC_FILE_INFO`), exact however late it's read. `None` if it
/// hasn't signaled, signaled an error, or isn't a sync_file.
pub fn fence_signaled_at(fence: BorrowedFd<'_>) -> Option<Duration> {
    let ioctl = |info: &mut SyncFileInfo| {
        // SAFETY: `info` is a valid `sync_file_info` for the call, and its
        // `sync_fence_info`, when set, points to room for `num_fences`
        // entries, which outlives the call.
        unsafe {
            libc::ioctl(
                fence.as_raw_fd(),
                SYNC_IOC_FILE_INFO as libc::Ioctl,
                info as *mut SyncFileInfo,
            )
        }
    };
    // First how many fences it holds (and whether all signaled), then theirs.
    let mut info = SyncFileInfo::default();
    if ioctl(&mut info) != 0 || info.status != 1 || info.num_fences == 0 {
        return None;
    }
    let empty = SyncFenceInfo {
        obj_name: [0; 32],
        driver_name: [0; 32],
        status: 0,
        flags: 0,
        timestamp_ns: 0,
    };
    let mut fences = vec![empty; info.num_fences as usize];
    info.sync_fence_info = fences.as_mut_ptr() as u64;
    if ioctl(&mut info) != 0 || info.status != 1 {
        return None;
    }
    let count = (info.num_fences as usize).min(fences.len());
    fences[..count]
        .iter()
        .map(|fence| Duration::from_nanos(fence.timestamp_ns))
        .max()
}

/// What `LoopStats` logs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoopReport {
    pub secs: f64,
    pub iterations: u32,
    /// Iterations that did all of `after_frames`' work.
    pub housekeeping: u32,
    pub cpu_total: Duration,
    pub cpu_max: Duration,
}

impl LoopStats {
    /// An iteration at `now` took `cpu` after its events, `housekeeping`
    /// whether it did all of it. Returns a report when one is due.
    pub fn record(
        &mut self,
        now: Duration,
        housekeeping: bool,
        cpu: Duration,
    ) -> Option<LoopReport> {
        self.iterations += 1;
        self.housekeeping += u32::from(housekeeping);
        self.total.0 += 1;
        self.total.1 += u64::from(housekeeping);
        self.cpu_total += cpu;
        self.cpu_max = self.cpu_max.max(cpu);
        let start = *self.start.get_or_insert(now);
        if now.saturating_sub(start) < REPORT_EVERY {
            return None;
        }
        self.take(now)
    }

    /// What was recorded since the last report, starting over at `now`.
    pub fn take(&mut self, now: Duration) -> Option<LoopReport> {
        let start = self.start?;
        if self.iterations == 0 {
            return None;
        }
        let report = LoopReport {
            secs: now.saturating_sub(start).as_secs_f64(),
            iterations: self.iterations,
            housekeeping: self.housekeeping,
            cpu_total: self.cpu_total,
            cpu_max: self.cpu_max,
        };
        *self = Self {
            start: Some(now),
            total: self.total,
            ..Self::default()
        };
        Some(report)
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

/// Nested, the refreshes missed between frames (see the module docs): the
/// host's vblanks aren't known, only when its frame callbacks let Pila draw.
/// A frame within half a period of the last can't have had a vblank of its
/// own: it takes the next one, and the gap after it is counted from the frame
/// before it, less the refresh it took (a host that draws the first frame
/// after a pause at once and the next late, `udev/render_late.rs`, makes such
/// pairs).
#[derive(Debug, Default)]
pub struct NestedPacing {
    /// The last frame with a refresh of its own, and whether an animation
    /// ran after the last frame (the next was due a period later).
    last: Option<(Duration, bool)>,
    /// Frames since `last` that came within half a period of the one before.
    early: u32,
}

impl NestedPacing {
    /// A frame was drawn at `now`, `animating` after it, on an output whose
    /// refresh lasts `period`: the refreshes missed since the last one.
    pub fn frame(&mut self, now: Duration, animating: bool, period: Duration) -> u32 {
        let missed = match self.last {
            Some((last, _)) if now.saturating_sub(last) < period / 2 => {
                self.early += 1;
                self.last = Some((last, animating));
                return 0;
            }
            Some((last, true)) => {
                missed_in_gap(now.saturating_sub(last), period).saturating_sub(self.early)
            }
            _ => 0,
        };
        self.early = 0;
        self.last = Some((now, animating));
        missed
    }
}

/// A frame asked for at least this long before a vblank (or half a period,
/// if that's shorter) is in time for it: the pillar's budget per frame.
pub const IN_TIME: Duration = Duration::from_millis(4);

/// Refreshes a frame was shown late by: asked for at `requested`, shown at
/// `shown`, on an output whose vblanks are `period` apart from one at
/// `vblank`. Due is the first vblank at least `IN_TIME` after the request,
/// or, with another frame in flight then, the one after the vblank that
/// showed that frame (`in_flight`): it's drawn then, at the earliest.
pub fn late_refreshes(
    requested: Duration,
    in_flight: Option<Duration>,
    vblank: Duration,
    shown: Duration,
    period: Duration,
) -> u32 {
    if period.is_zero() {
        return 0;
    }
    let next = crate::commit_pacing::next_vblank;
    let due = if let Some(in_flight_shown) = in_flight {
        in_flight_shown + period
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
        direct = report.direct,
        torn = report.torn,
        idle = report.idle,
        idle_cpu_ms = ms(report.idle_cpu),
        gpu_frames = report.gpu_frames,
        gpu_avg_ms = ms(report.gpu_avg),
        gpu_max_ms = ms(report.gpu_max),
        held = report.held,
        margin_avg_ms = ms(report.margin_avg),
        margin_max_ms = ms(report.margin_max),
        spare_frames = report.spare_frames,
        spare_avg_ms = ms(report.spare_avg),
        spare_min_ms = ms(report.spare_min),
        latency_frames = report.latency_frames,
        latency_avg_ms = ms(report.latency_avg),
        latency_max_ms = ms(report.latency_max),
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

fn log_copy_report(report: &Report) {
    debug!(
        target: "pila::frames",
        secs = (report.secs * 10.0).round() / 10.0,
        copies = report.frames,
        cpu_avg_ms = ms(report.cpu_avg),
        cpu_max_ms = ms(report.cpu_max),
        "copies"
    );
}

fn log_upload_report(report: &Report) {
    debug!(
        target: "pila::frames",
        secs = (report.secs * 10.0).round() / 10.0,
        uploads = report.frames,
        cpu_avg_ms = ms(report.cpu_avg),
        cpu_max_ms = ms(report.cpu_max),
        "uploads"
    );
}

fn log_loop_report(report: &LoopReport) {
    debug!(
        target: "pila::frames",
        secs = (report.secs * 10.0).round() / 10.0,
        iterations = report.iterations,
        housekeeping = report.housekeeping,
        cpu_ms = ms(report.cpu_total),
        cpu_max_ms = ms(report.cpu_max),
        "loop"
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

    /// A capture's copy, finished once the GPU was done (written into the
    /// client's shm buffer, or a portal share's; `Pila::when_copied`), took
    /// `cpu` on the event loop.
    pub fn record_copy(&mut self, cpu: Duration) {
        let now = self.animation_time();
        if let Some(report) = self.copy_stats.record(now, cpu) {
            log_copy_report(&report);
        }
    }

    /// A client's buffers, uploaded at its commit (`upload.rs`), took `cpu`.
    pub fn record_upload(&mut self, cpu: Duration) {
        let now = self.animation_time();
        if let Some(report) = self.upload_stats.record(now, cpu) {
            log_upload_report(&report);
        }
    }

    /// An event-loop iteration's `after_frames` took `cpu` (captures left
    /// out), `housekeeping` whether it did all of it (see the module docs).
    pub fn record_loop(&mut self, housekeeping: bool, cpu: Duration) {
        let now = self.animation_time();
        if let Some(report) = self.loop_stats.record(now, housekeeping, cpu) {
            log_loop_report(&report);
        }
    }

    /// Log what each output drew since its last report, the captures and
    /// the event loop's own work (pila is quitting).
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
        if let Some(report) = self.copy_stats.take(now) {
            log_copy_report(&report);
        }
        if let Some(report) = self.upload_stats.take(now) {
            log_upload_report(&report);
        }
        if let Some(report) = self.loop_stats.take(now) {
            log_loop_report(&report);
        }
    }

    /// Measure the GPU time of `frame`, one of `output`'s (see the module
    /// docs): once its fence is ready, from an event source, so nothing waits
    /// for the GPU.
    pub fn watch_gpu(&mut self, output: &Output, frame: GpuFrame) {
        let GpuFrame {
            fence,
            started,
            submitted,
        } = frame;
        let name = output.name();
        let output = output.clone();
        let period = refresh_period(&output);
        let inserted = self.loop_handle.insert_source(
            Generic::new(fence, Interest::READ, Mode::OneShot),
            move |_, fence, state| {
                let Some(signal) = fence_signaled_at(fence.as_fd()) else {
                    // An error on the GPU, or not a sync_file after all.
                    debug!(target: "pila::frames", output = name, "a frame's fence gave no time");
                    return Ok(PostAction::Remove);
                };
                let stats = state.frame_stats.entry(name.clone()).or_default();
                let gpu = stats.record_gpu(started, submitted, signal);
                if let Some(gpu) = gpu {
                    tracing::trace!(target: "pila::frames", output = name, gpu_us = gpu.as_micros() as u64, "GPU done");
                    let spare = state
                        .backend
                        .as_mut()
                        .and_then(|backend| backend.gpu_done(&output, started, signal));
                    if let Some(spare) = spare {
                        state.record_spare(&output, spare);
                    }
                }
                if let Some(gpu) = gpu.filter(|gpu| *gpu > period / 2) {
                    debug!(
                        target: "pila::frames",
                        output = name,
                        gpu_ms = ms(gpu),
                        period_ms = ms(period),
                        "slow frame on the GPU"
                    );
                }
                Ok(PostAction::Remove)
            },
        );
        if let Err(err) = inserted {
            debug!(target: "pila::frames", err = %err.error, "can't watch a frame's fence");
        }
    }

    /// The frame of `output` recorded next is a client's buffer as it is, on
    /// the primary plane (see the module docs).
    pub fn record_direct_frame(&mut self, output: &Output) {
        self.frame_stats.entry(output.name()).or_default().direct();
    }

    /// The frame of `output` recorded next was held to be drawn `margin`
    /// before its vblank (`udev/render_late.rs`).
    pub fn record_held_frame(&mut self, output: &Output, margin: Duration) {
        self.frame_stats
            .entry(output.name())
            .or_default()
            .held(margin);
    }

    /// A held frame of `output` was ready `spare` before its vblank.
    pub fn record_spare(&mut self, output: &Output, spare: Duration) {
        self.frame_stats
            .entry(output.name())
            .or_default()
            .spare(spare);
    }

    /// A frame of `output` was shown `latency` after the redraw that asked
    /// for it (see the module docs).
    pub fn record_latency(&mut self, output: &Output, latency: Duration) {
        self.frame_stats
            .entry(output.name())
            .or_default()
            .latency(latency);
    }

    /// A frame of `output` was shown at once, torn (`tearing.rs`).
    pub fn record_torn_frame(&mut self, output: &Output) {
        self.frame_stats.entry(output.name()).or_default().torn();
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
    fn frames_that_are_a_clients_buffer_are_counted_apart_and_as_frames() {
        let mut stats = FrameStats::default();
        stats.record(1000 * MS, MS);
        stats.direct();
        stats.record(2000 * MS, MS);
        stats.direct();
        let report = stats.record(6000 * MS, MS).expect("due after 5 s");
        assert_eq!((report.frames, report.direct), (3, 2));
        // And start over with the report.
        let report = stats.record(12000 * MS, MS).expect("due after 5 s");
        assert_eq!((report.frames, report.direct), (1, 0));
    }

    #[test]
    fn a_frames_gpu_time_runs_from_its_submission_to_its_fence() {
        // Begun at 100, submitted at 102, the GPU done at 105.
        assert_eq!(gpu_time(100 * MS, 102 * MS, 105 * MS, None), Some(3 * MS));
        // Done before the submission ended: no time past it.
        assert_eq!(
            gpu_time(100 * MS, 102 * MS, 101 * MS, None),
            Some(Duration::ZERO)
        );
        // A fence that signaled before the frame began is an earlier frame's.
        assert_eq!(gpu_time(100 * MS, 102 * MS, 99 * MS, None), None);
        // So is the very fence measured last (the primary plane kept its
        // buffer), even if it signaled after this frame began.
        assert_eq!(gpu_time(100 * MS, 102 * MS, 101 * MS, Some(101 * MS)), None);
        assert_eq!(
            gpu_time(100 * MS, 102 * MS, 104 * MS, Some(101 * MS)),
            Some(2 * MS)
        );
    }

    #[test]
    fn gpu_times_are_reported_with_the_frames_and_the_last_fence_is_kept() {
        let mut stats = FrameStats::default();
        stats.record(Duration::ZERO, MS);
        assert_eq!(stats.record_gpu(Duration::ZERO, MS, 3 * MS), Some(2 * MS));
        stats.record(1000 * MS, MS);
        assert_eq!(
            stats.record_gpu(1000 * MS, 1001 * MS, 1005 * MS),
            Some(4 * MS)
        );
        // The same fence again measures nothing.
        assert_eq!(stats.record_gpu(2000 * MS, 2001 * MS, 1005 * MS), None);
        let report = stats.record(5000 * MS, MS).expect("due after 5 s");
        assert_eq!(report.gpu_frames, 2);
        assert_eq!((report.gpu_avg, report.gpu_max), (3 * MS, 4 * MS));
        // A new report starts over, but still knows the last fence.
        assert_eq!(stats.record_gpu(5000 * MS, 5001 * MS, 1005 * MS), None);
        let report = stats.record(10_000 * MS, MS).expect("due again");
        assert_eq!((report.gpu_frames, report.gpu_avg), (0, Duration::ZERO));
    }

    #[test]
    fn held_frames_and_latencies_are_reported_with_the_frames() {
        let mut stats = FrameStats::default();
        stats.held(6 * MS);
        stats.spare(3 * MS);
        stats.spare(MS);
        stats.record(Duration::ZERO, MS);
        stats.latency(10 * MS);
        stats.record(1000 * MS, MS);
        stats.latency(20 * MS);
        let report = stats.record(5000 * MS, MS).expect("due after 5 s");
        assert_eq!((report.frames, report.held), (3, 1));
        assert_eq!((report.margin_avg, report.margin_max), (6 * MS, 6 * MS));
        assert_eq!(report.spare_frames, 2);
        assert_eq!((report.spare_avg, report.spare_min), (2 * MS, MS));
        assert_eq!(report.latency_frames, 2);
        assert_eq!((report.latency_avg, report.latency_max), (15 * MS, 20 * MS));
        let report = stats.record(10_000 * MS, MS).expect("due again");
        assert_eq!((report.held, report.latency_frames), (0, 0));
    }

    #[test]
    fn a_file_that_isnt_a_fence_gives_no_time() {
        let file = std::fs::File::open("/dev/null").expect("/dev/null");
        assert_eq!(fence_signaled_at(file.as_fd()), None);
    }

    #[test]
    fn the_loops_iterations_are_reported_apart_with_their_housekeeping() {
        let mut stats = LoopStats::default();
        assert_eq!(stats.record(Duration::ZERO, true, 2 * MS), None);
        assert_eq!(stats.record(1000 * MS, false, MS / 2), None);
        let report = stats
            .record(5000 * MS, false, MS / 2)
            .expect("due after 5 s");
        assert_eq!((report.iterations, report.housekeeping), (3, 1));
        assert_eq!((report.cpu_total, report.cpu_max), (3 * MS, 2 * MS));
        assert_eq!(report.secs, 5.0);
        // Starting over, with the totals kept.
        assert_eq!(stats.take(6000 * MS), None);
        stats.record(6000 * MS, true, MS);
        let report = stats.take(7000 * MS).expect("one iteration left");
        assert_eq!((report.iterations, report.housekeeping), (1, 1));
        assert_eq!(stats.total, (4, 2));
    }

    #[test]
    fn a_frame_asked_for_in_time_but_shown_a_vblank_late_is_counted() {
        let period = 16 * MS;
        // Vblanks at 100, 116, 132, ...
        // Asked for at 111, 5 ms before 116 (more than the 4 ms budget):
        // due at 116, shown at 132, one late.
        assert_eq!(
            late_refreshes(111 * MS, None, 100 * MS, 132 * MS, period),
            1
        );
        assert_eq!(
            late_refreshes(111 * MS, None, 100 * MS, 116 * MS, period),
            0
        );
        // Asked for 3 ms before a vblank: in time for the next only.
        assert_eq!(
            late_refreshes(113 * MS, None, 100 * MS, 132 * MS, period),
            0
        );
        assert_eq!(
            late_refreshes(113 * MS, None, 100 * MS, 148 * MS, period),
            1
        );
        // With a frame in flight the vblank after its own is due.
        assert_eq!(
            late_refreshes(102 * MS, Some(116 * MS), 100 * MS, 132 * MS, period),
            0
        );
        assert_eq!(
            late_refreshes(102 * MS, Some(116 * MS), 100 * MS, 164 * MS, period),
            2
        );
        // That frame was itself shown late (at 132, queued right before 116,
        // say): this one, drawn at once then, isn't.
        assert_eq!(
            late_refreshes(102 * MS, Some(132 * MS), 100 * MS, 148 * MS, period),
            0
        );
        // At 144 Hz the budget is half a period at most.
        let fast = Duration::from_micros(6944);
        let vblank = 100 * MS;
        let next = vblank + fast;
        assert_eq!(
            late_refreshes(next - 3 * MS, None, vblank, next + fast, fast),
            0
        );
        assert_eq!(
            late_refreshes(next - 4 * MS, None, vblank, next + fast, fast),
            1
        );
        // A vblank known from long before still sets the grid.
        assert_eq!(
            late_refreshes(1001 * MS, None, 100 * MS, 1012 * MS, period),
            0
        );
        assert_eq!(
            late_refreshes(1001 * MS, None, 100 * MS, 1028 * MS, period),
            1
        );
    }

    /// Seen nested on a host that renders late: after a pause, the host drew
    /// the first frame of an animation at once and the second came 1 ms
    /// later; the host showed it a vblank after the first, and its frame
    /// callback for the third came 27 ms after the second: three frames on
    /// three vblanks in a row, counted as one missed.
    #[test]
    fn a_frame_right_after_another_takes_the_next_refresh_nested() {
        let period = Duration::from_micros(16_667);
        let mut pacing = NestedPacing::default();
        let at = |ms: f64| Duration::from_secs_f64(46.0 + ms / 1000.0);
        let missed: Vec<u32> = [162.7, 163.7, 190.9, 208.0, 224.9]
            .into_iter()
            .map(|ms| pacing.frame(at(ms), true, period))
            .collect();
        assert_eq!(missed, [0, 0, 0, 0, 0]);
        // A real miss after such a pair still counts: 45 ms after the first
        // is two vblanks later than the pair's two.
        let mut pacing = NestedPacing::default();
        pacing.frame(at(0.0), true, period);
        pacing.frame(at(1.0), true, period);
        assert_eq!(pacing.frame(at(50.0), true, period), 1);
        // Evenly paced, a gap of two periods is one missed.
        let mut pacing = NestedPacing::default();
        pacing.frame(at(0.0), true, period);
        assert_eq!(pacing.frame(at(33.3), true, period), 1);
        // Not animating before: the pause was meant.
        let mut pacing = NestedPacing::default();
        pacing.frame(at(0.0), false, period);
        assert_eq!(pacing.frame(at(100.0), true, period), 0);
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
