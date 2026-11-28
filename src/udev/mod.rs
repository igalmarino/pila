//! Bare-metal backend: DRM/KMS outputs, libinput input, libseat session.
//! Run from a TTY (`pila --backend udev`, the default without a host session).
//!
//! Deliberately smaller than anvil's: one GPU renders (the seat's primary;
//! the others only scan out their monitors, `secondary.rs`), no DRM
//! leasing. An output without a position in `[[output]]` goes right of those
//! already there, in the order they're connected.
//!
//! Redraws stay on demand and per output (`redraw.rs`): one idle callback per
//! loop iteration renders every output asked for. An output with a frame in
//! flight renders again at its next vblank (`frame_finish`) instead, so
//! redraws are paced by the display. A session lock is confirmed only after a
//! locked frame has been shown on every output.

use std::{collections::HashMap, path::Path, time::Duration};

use smithay::wayland::drm_syncobj::{supports_syncobj_eventfd, DrmSyncobjState};

use crate::edid::Monitor;

use smithay::{
    backend::{
        allocator::{
            format::FormatSet,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
            Format, Fourcc,
        },
        drm::VrrSupport,
        drm::{
            compositor::FrameFlags,
            exporter::gbm::GbmFramebufferExporter,
            output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements},
            DrmDevice, DrmDeviceFd, DrmEvent, DrmEventMetadata, DrmEventTime, DrmNode, NodeType,
        },
        egl::{EGLContext, EGLDevice, EGLDisplay},
        input::InputEvent,
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            gles::GlesRenderer,
            multigpu::{GpuManager, MultiRenderer},
            ImportDma, ImportMemWl,
        },
        session::{libseat::LibSeatSession, Event as SessionEvent, Session},
        udev::{all_gpus, primary_gpu, UdevBackend, UdevEvent},
    },
    desktop::{layer_map_for_output, utils::OutputPresentationFeedback},
    input::{keyboard::LedState, pointer::CursorImageStatus},
    output::{Mode as WlMode, Output, PhysicalProperties, Scale as OutputScale},
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            EventLoop, LoopHandle, RegistrationToken,
        },
        drm::{
            control::{
                atomic::AtomicModeReq, connector, crtc, property, AtomicCommitFlags, Device as _,
                Mode as DrmMode, ModeFlags, ModeTypeFlags,
            },
            Device as _,
        },
        input::{self as libinput, Libinput},
        rustix::fs::OFlags,
        wayland_protocols::wp::presentation_time::server::wp_presentation_feedback,
        wayland_server::backend::GlobalId,
    },
    utils::{DeviceFd, IsAlive, Logical, Point, Transform},
    wayland::{
        dmabuf::{DmabufFeedback, DmabufFeedbackBuilder},
        presentation::Refresh,
    },
};
use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};
use tracing::{debug, error, info, warn};

mod draw;
mod gamma;
mod gpus;
mod input;
mod outputs;
mod recovery;
mod render_late;
mod secondary;

use self::{draw::*, gamma::*, input::*, outputs::*, recovery::Recovery};
pub use outputs::apply_output_config;
pub use secondary::apply_other_gpus;

use crate::{
    backend::Backend,
    config::{AccelProfile, ClickMethod, InputConfig, OutputConfig, PointerConfig},
    frame,
    output_management::{HeadInfo, ModeInfo},
    render::{self, PilaRenderElement},
    Pila,
};

/// 8-bit formats only: widely supported, and what clients expect. Opaque
/// ones first: the primary plane is the bottom one, so the frame's alpha is
/// never used, and Smithay adds a client's buffer shown there as it is
/// (direct scanout) as the opaque variant of its format. A swapchain with
/// alpha would change the plane's format at every switch between the two,
/// which amdgpu answers with a plane reset that waits for every monitor's
/// flip (11 to 14 ms with the event loop held, measured on Renoir).
const COLOR_FORMATS: [Fourcc; 4] = [
    Fourcc::Xrgb8888,
    Fourcc::Xbgr8888,
    Fourcc::Argb8888,
    Fourcc::Abgr8888,
];

type PilaDrmOutput = DrmOutput<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    Option<OutputPresentationFeedback>,
    DrmDeviceFd,
>;
type PilaDrmOutputManager = DrmOutputManager<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    Option<OutputPresentationFeedback>,
    DrmDeviceFd,
>;

pub use gpus::GpuApi;

/// The primary GPU's renderer, lent by the `GpuManager` that owns it: a
/// `GlesRenderer` through `as_ref`/`as_mut`.
pub type PrimaryRenderer<'a> = MultiRenderer<'a, 'a, GpuApi, GpuApi>;

/// Lend the primary GPU's renderer; `None` if the GPU manager can't (`lend`).
fn primary_renderer<'a>(
    gpus: &'a mut GpuManager<GpuApi>,
    primary: &DrmNode,
) -> Option<PrimaryRenderer<'a>> {
    lend(gpus.single_renderer(primary))
}

/// A renderer the GPU manager lent, or `None`, its error logged once until
/// one is lent again (they're asked for every frame).
fn lend<'a>(
    lent: Result<PrimaryRenderer<'a>, smithay::backend::renderer::multigpu::Error<GpuApi, GpuApi>>,
) -> Option<PrimaryRenderer<'a>> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static FAILING: AtomicBool = AtomicBool::new(false);
    match lent {
        Ok(renderer) => {
            FAILING.store(false, Ordering::Relaxed);
            Some(renderer)
        }
        Err(err) => {
            if !FAILING.swap(true, Ordering::Relaxed) {
                warn!(?err, "no renderer for a GPU");
            }
            None
        }
    }
}

/// An output: a CRTC of a GPU's DRM device (two GPUs may number their
/// CRTCs alike).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct OutputId {
    /// The device's primary node: its key in `UdevData::devices`.
    pub node: DrmNode,
    pub crtc: crtc::Handle,
}

/// A GPU's display side: its DRM device and connectors.
pub(super) struct GpuDevice {
    drm: PilaDrmOutputManager,
    scanner: DrmScanner,
    /// Its render node: its key in `UdevData::gpus`.
    render_node: DrmNode,
    /// Its DRM events (vblanks); a secondary GPU's is removed with it.
    events: Option<RegistrationToken>,
}

/// The renderer an output is set up or drawn with: on the primary GPU its
/// own `GlesRenderer` (`as_mut`), with Pila's elements as they are; on
/// another, a `MultiRenderer` with them wrapped (`secondary::PrimaryGpuElement`).
pub(super) enum OutputRenderer<'a> {
    Primary(PrimaryRenderer<'a>),
    Secondary(PrimaryRenderer<'a>),
}

/// The renderer to set outputs of the GPU `render_node` up with (its own;
/// they're drawn with `draw_renderer`). `None` (logged once) if it can't be
/// lent.
fn setup_renderer<'a>(
    gpus: &'a mut GpuManager<GpuApi>,
    primary: &DrmNode,
    render_node: &DrmNode,
) -> Option<OutputRenderer<'a>> {
    if render_node == primary {
        return primary_renderer(gpus, primary).map(OutputRenderer::Primary);
    }
    lend(gpus.single_renderer(render_node)).map(OutputRenderer::Secondary)
}

/// The renderer to draw an output of the GPU `render_node` with, whose
/// frames are in `format`: rendering on the primary, copied to that GPU.
fn draw_renderer<'a>(
    gpus: &'a mut GpuManager<GpuApi>,
    primary: &DrmNode,
    render_node: &DrmNode,
    format: Fourcc,
) -> Option<OutputRenderer<'a>> {
    if render_node == primary {
        return primary_renderer(gpus, primary).map(OutputRenderer::Primary);
    }
    lend(gpus.renderer(primary, render_node, format)).map(OutputRenderer::Secondary)
}

/// Fields drop in declaration order, which matters here: the outputs go
/// before the DRM devices, and the DRM devices (which restore the previous
/// display state when dropped) before the session that grants access to
/// them.
pub struct UdevData {
    outputs: HashMap<OutputId, OutputSurface>,
    /// By primary node.
    devices: HashMap<DrmNode, GpuDevice>,
    /// Every GPU's renderer; only the primary's for now, which renders
    /// everything (`primary_renderer`).
    gpus: GpuManager<GpuApi>,
    /// The primary GPU's render node: its key in `gpus`.
    primary: DrmNode,
    /// What clients are told about dmabufs: the primary's formats. A surface
    /// an output shows as it is gets the output's instead
    /// (`OutputSurface::scanout_feedback`).
    dmabuf_feedback: DmabufFeedback,
    /// The dmabuf formats the primary's renderer reads.
    dmabuf_formats: FormatSet,
    libinput: Libinput,
    /// Keyboards, for the Caps/Num/Scroll Lock LEDs.
    keyboards: Vec<libinput::Device>,
    /// Touchpads and mice, to re-apply `[input]` when the config changes.
    pointers: Vec<libinput::Device>,
    /// A render pass is already queued for this loop iteration.
    redraw_scheduled: bool,
    /// When redraws are asked for (`CrtcState::request`).
    clock: smithay::utils::Clock<smithay::utils::Monotonic>,
    loop_handle: LoopHandle<'static, Pila>,
    /// Last: see the struct docs.
    pub session: LibSeatSession,
}

struct OutputSurface {
    output: Output,
    /// The CRTC's surface: `None` while the output is powered off
    /// (`set_output_power`), which clears the CRTC.
    drm_output: Option<PilaDrmOutput>,
    connector: connector::Handle,
    /// The mode in use, to set the CRTC up again.
    mode: DrmMode,
    /// Its `wl_output` global: `None` while disabled (`enabled`).
    global: Option<GlobalId>,
    /// When the last frame was shown (the kernel's timestamp, on the
    /// presentation clock, if it gives one).
    last_vblank_time: Option<Duration>,
    /// Something changed since the last frame.
    redraw_needed: bool,
    /// The connector's modes, for `[[output]]` mode changes on reload.
    modes: Vec<DrmMode>,
    /// The gamma ramps a client set (`gamma_control.rs`), re-applied
    /// whenever the CRTC is set up again.
    gamma: Option<Vec<u16>>,
    /// The CRTC's `GAMMA_LUT`, through which a client's ramps go out without
    /// blocking (`gamma.rs`); `None` on a legacy device or driver.
    gamma_lut: Option<property::Handle>,
    /// Powered on while the session was on another VT: it comes on when the
    /// session is back (`set_output_power`).
    power_on_when_active: bool,
    /// Ramps set but not sent yet: they go out once no frame is on its way
    /// (`commit_pending_gamma`).
    gamma_pending: Option<Vec<u16>>,
    /// Adaptive sync as `[[output]]` asks for it, set again whenever the
    /// CRTC is set up again (`outputs::apply_vrr`).
    adaptive_sync: bool,
    /// The frame last queued was a client's buffer as it is, on the primary
    /// plane (`draw::frame_flags`); logged when it changes.
    direct: bool,
    /// The format and modifier of the swapchain's last buffer and of the
    /// client's buffer last shown as it is, logged with `direct`: a change of
    /// either on the primary plane makes some drivers (amdgpu) wait for the
    /// device's other outputs' flips.
    swapchain_format: Option<Format>,
    client_format: Option<Format>,
    /// A client's buffer last refused the primary plane, and why
    /// (`draw::refused_scanout`); logged when it changes.
    refused_scanout: Option<(
        Option<Format>,
        smithay::backend::renderer::element::RenderingReason,
    )>,
    /// What a surface this output shows as it is, or could, is told about
    /// dmabufs: the formats its primary plane takes first (`scanout_feedback`).
    /// Only for an output on the primary GPU, with a CRTC.
    scanout_feedback: Option<DmabufFeedback>,
    /// That timer is set.
    idle_timer: bool,
    /// The most the redraws asked for since the last one bring
    /// (`redraw::Content`): a change of Pila's own waits only past
    /// `SURE_STREAK` redraws in a row that found nothing new, a client's
    /// new picture doesn't unless the last two redraws found nothing new
    /// (`idle_streak`), what may bring nothing does (`draw::idle_wait`).
    redraw_content: crate::redraw::Content,
    /// What its recent frames took to be ready, for rendering late
    /// (`render_late.rs`).
    render_timing: render_late::RenderTiming,
    /// What lasts as long as the CRTC's current setup (`CrtcState`).
    crtc_state: CrtcState,
}

/// An output's state that lasts as long as its CRTC's current setup: the
/// frame in flight, the vblanks seen, the waits and holds on its frames, its
/// DRM failures. All of it starts over together whenever the CRTC is cleared
/// or set up again (powered off or on, disabled, enabled, the session back
/// from another VT): `CrtcState::default`, never field by field.
struct CrtcState {
    /// A frame was queued and its vblank hasn't arrived yet.
    frame_pending: bool,
    /// The sequence number of the vblank that showed the last frame.
    last_vblank: Option<u64>,
    /// Pila wanted a new frame at every vblank after `last_vblank` (an
    /// animation was running; `frame_stats.rs`).
    back_to_back: bool,
    /// Progress of the frame that confirms a session lock.
    lock_frame: LockFrame,
    /// The commit on its way (`frame_pending`) is a gamma change, not a frame.
    gamma_in_flight: bool,
    /// Frames that failed at the DRM level (`recovery.rs`): drawing waits
    /// for a retry instead of failing again on every event.
    recovery: Recovery,
    /// Whether the last frame shown was reported to clients with a variable
    /// refresh, logged when it changes (`scripts/check --vrr` reads it);
    /// forgotten while the CRTC is off.
    presented_variable: bool,
    /// The next frame is the first since the CRTC was set up (connected,
    /// enabled, powered on, a new mode, the session back): it allocates the
    /// swapchain and commits the modeset, a cost of setting the output up
    /// that's logged on its own, not counted in the frame budget.
    first_frame: bool,
    /// After a redraw that had nothing new to show, the next that may bring
    /// nothing either waits until then (`draw::idle_until`, `draw::idle_wait`):
    /// a client committing no new picture (no buffer, or one without damage)
    /// in a loop of frame callbacks or fifo barriers, or the
    /// pointer moving with no cursor drawn, redraw at most once per refresh.
    /// A timer then draws what's due.
    idle_until: Option<Duration>,
    /// Redraws in a row that found nothing new, since the last frame queued:
    /// past one a client's new picture waits too, past `SURE_STREAK` even a
    /// change of Pila's own (`draw::idle_wait`).
    idle_streak: u32,
    /// The first such request since the last redraw, then the one of the
    /// frame in flight: how late it shows is measured (`frame_stats.rs`).
    request: Option<Request>,
    queued_request: Option<Request>,
    /// Held to be drawn then, its margin before the next vblank
    /// (`render_late.rs`); a timer draws it. Cleared when drawn.
    render_at: Option<render_late::Hold>,
    /// The frame in flight was held so (`render_at`): if it shows late,
    /// rendering late backs off.
    queued_held: bool,
    /// The last frame shown was torn (`tearing.rs`): not held, a game that
    /// tears wants its frames at once.
    torn: bool,
}

impl Default for CrtcState {
    fn default() -> Self {
        Self {
            frame_pending: false,
            last_vblank: None,
            back_to_back: false,
            lock_frame: LockFrame::None,
            gamma_in_flight: false,
            recovery: Recovery::default(),
            presented_variable: false,
            first_frame: true,
            idle_until: None,
            idle_streak: 0,
            request: None,
            queued_request: None,
            render_at: None,
            queued_held: false,
            torn: false,
        }
    }
}

/// A redraw asked for with something new to show (`CrtcState::request`).
#[derive(Clone, Copy, Debug)]
struct Request {
    /// When (monotonic).
    at: Duration,
    /// A frame was on its way then: the vblank after its own is due.
    in_flight: bool,
    /// The last vblank that showed a frame then, the grid of the next ones.
    vblank: Duration,
    /// Held back by the wait after a redraw with nothing new
    /// (`draw::idle_wait`), and when it was drawn (for the log of a frame
    /// shown late).
    held: bool,
    drawn: Option<Duration>,
    /// With a frame in flight when asked for: the vblank that showed that
    /// frame, set when this one is drawn (it's due one period later).
    after: Option<Duration>,
}

impl OutputSurface {
    /// The CRTC was set up (connected, enabled, powered on): everything
    /// about its frames starts over, and it draws its first.
    fn crtc_set_up(&mut self, drm_output: PilaDrmOutput) {
        self.drm_output = Some(drm_output);
        self.crtc_state = CrtcState::default();
        self.redraw_needed = true;
    }

    /// The CRTC was cleared (powered off, disabled): no frame is on its way,
    /// and the vblanks of the dark gap aren't missed refreshes.
    fn crtc_cleared(&mut self) {
        self.drm_output = None;
        self.crtc_state = CrtcState::default();
        self.power_on_when_active = false;
    }

    /// Not disabled (`[[output]]` or a display tool): a disabled one has no
    /// surface, no global and isn't mapped, but is still listed as a head.
    fn enabled(&self) -> bool {
        self.global.is_some()
    }

    /// Something changed on this output at `now`; `content`: surely shows.
    fn ask_redraw(&mut self, content: crate::redraw::Content, now: Duration) {
        self.redraw_needed = true;
        if content == crate::redraw::Content::Maybe {
            return;
        }
        self.redraw_content = self.redraw_content.max(content);
        if self.crtc_state.request.is_none() {
            self.crtc_state.request = self.last_vblank_time.map(|vblank| Request {
                at: now,
                in_flight: self.crtc_state.frame_pending,
                vblank,
                held: false,
                drawn: None,
                after: None,
            });
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LockFrame {
    None,
    Queued,
    Shown,
}

impl UdevData {
    /// The primary GPU's render node (`Backend::dmabuf_node`).
    pub fn primary_node(&self) -> DrmNode {
        self.primary
    }

    /// Ask every output for a new frame (see the module docs).
    pub fn queue_redraw(&mut self) {
        let now = self.clock.now().into();
        for surface in self.outputs.values_mut() {
            surface.ask_redraw(crate::redraw::Content::Sure, now);
        }
        self.schedule_render();
    }

    /// Ask `output` for a new frame; `content`: what changed surely shows.
    pub fn queue_redraw_output(&mut self, output: &Output, content: crate::redraw::Content) {
        let now = self.clock.now().into();
        let Some(surface) = self.outputs.values_mut().find(|s| s.output == *output) else {
            return;
        };
        surface.ask_redraw(content, now);
        self.schedule_render();
    }

    /// Render the outputs asked for in this loop iteration's idle callback.
    fn schedule_render(&mut self) {
        if !self.redraw_scheduled {
            self.redraw_scheduled = true;
            self.loop_handle.insert_idle(render_all);
        }
    }

    /// Take DRM master (again). logind grants it to the active session's
    /// fd, but after a switch between two compositors on one seat it has
    /// been seen missing; the kernel lets a process that was master once
    /// take it back without privileges.
    fn take_master(&self) {
        for device in self.devices.values() {
            if let Err(err) = device.drm.device().acquire_master_lock() {
                debug!(?err, "couldn't take DRM master");
            }
        }
    }

    /// A frame on `id` failed at the DRM level: draw it again after
    /// `delay` (`Recovery::failed`), taking DRM master first in case that
    /// was it. The other outputs go on meanwhile.
    fn schedule_recovery(&mut self, id: OutputId, delay: Duration) {
        let timer = Timer::from_duration(delay);
        let inserted = self.loop_handle.insert_source(timer, move |_, _, state| {
            if let Some(Backend::Udev(data)) = &mut state.backend {
                data.take_master();
                if let Some(surface) = data.outputs.get_mut(&id) {
                    surface.crtc_state.recovery.retry();
                    surface.redraw_needed = true;
                }
                data.schedule_render();
            }
            TimeoutAction::Drop
        });
        if let Err(err) = inserted {
            // Without a timer the output would wait forever: retry on the
            // next redraw instead.
            warn!(err = %err.error, "can't schedule a retry for an output");
            if let Some(surface) = self.outputs.get_mut(&id) {
                surface.crtc_state.recovery.retry();
            }
        }
    }

    /// Draw `id` at `at` (monotonic), its margin before the next vblank
    /// (`render_late.rs`, `CrtcState::render_at`, which `draw::prepare`
    /// lets through from then on).
    fn schedule_late_render(&mut self, id: OutputId, at: Duration) {
        let delay = at.saturating_sub(self.clock.now().into());
        let inserted =
            self.loop_handle
                .insert_source(Timer::from_duration(delay), move |_, _, state| {
                    if let Some(Backend::Udev(data)) = &mut state.backend {
                        data.schedule_render();
                    }
                    TimeoutAction::Drop
                });
        if let Err(err) = inserted {
            // Without a timer the output would wait for the next redraw
            // asked for: draw it now instead.
            warn!(err = %err.error, "can't schedule an output's late render");
            if let Some(surface) = self.outputs.get_mut(&id) {
                surface.crtc_state.render_at = None;
            }
        }
    }

    /// The GPU finished, at `signal`, the frame of `output` whose drawing
    /// started at `started` (`frame_stats::GpuFrame`), for rendering late:
    /// how long before its vblank it was ready, if it was held.
    pub fn gpu_done(
        &mut self,
        output: &Output,
        started: Duration,
        signal: Duration,
    ) -> Option<Duration> {
        let surface = self.outputs.values_mut().find(|s| s.output == *output)?;
        surface.render_timing.gpu_done(started, signal)
    }

    /// Draw `id` again at `at` (monotonic), when its wait after a redraw
    /// with nothing new ends (`draw::idle_until`). One timer per output.
    fn schedule_idle_redraw(&mut self, id: OutputId, delay: Duration) {
        let Some(surface) = self.outputs.get_mut(&id) else {
            return;
        };
        if surface.idle_timer {
            return;
        }
        surface.idle_timer = true;
        let timer = Timer::from_duration(delay);
        let inserted = self.loop_handle.insert_source(timer, move |_, _, state| {
            if let Some(Backend::Udev(data)) = &mut state.backend {
                if let Some(surface) = data.outputs.get_mut(&id) {
                    surface.idle_timer = false;
                    surface.crtc_state.idle_until = None;
                }
                data.schedule_render();
            }
            TimeoutAction::Drop
        });
        if let Err(err) = inserted {
            // Without a timer the output would wait for the next redraw
            // asked for: don't wait at all then.
            warn!(err = %err.error, "can't schedule an output's redraw");
            if let Some(surface) = self.outputs.get_mut(&id) {
                surface.idle_timer = false;
                surface.crtc_state.idle_until = None;
            }
        }
    }

    pub fn update_leds(&mut self, leds: LedState) {
        for keyboard in &mut self.keyboards {
            keyboard.led_update(leds.into());
        }
    }

    pub fn change_vt(&mut self, vt: i32) {
        if let Err(err) = self.session.change_vt(vt) {
            warn!(vt, ?err, "failed to switch VT");
        }
    }

    /// Re-apply `[input]` to the connected touchpads and mice.
    pub fn apply_input_config(&mut self, input: &InputConfig) {
        for device in &mut self.pointers {
            if let Some(config) = pointer_config(device, input) {
                configure_pointer(device, config);
            }
        }
    }

    fn output_id(&self, output: &Output) -> Option<OutputId> {
        self.outputs
            .iter()
            .find(|(_, s)| &s.output == output)
            .map(|(id, _)| *id)
    }

    /// The DRM device of `id`'s GPU.
    fn drm_device(&self, id: OutputId) -> Option<&DrmDevice> {
        Some(self.devices.get(&id.node)?.drm.device())
    }

    /// The length of the output's gamma ramps (0 = no gamma support).
    pub fn gamma_size(&self, output: &Output) -> Option<u32> {
        let id = self.output_id(output)?;
        self.drm_device(id)?
            .get_crtc(id.crtc)
            .ok()
            .map(|info| info.gamma_length())
            .filter(|n| *n > 0)
    }

    /// Set the output's gamma ramps (`None`: linear) and remember them.
    pub fn set_gamma(&mut self, output: &Output, ramp: Option<&[u16]>) -> bool {
        let Some(id) = self.output_id(output) else {
            return false;
        };
        let Some(size) = self.gamma_size(output) else {
            return false;
        };
        let linear;
        let ramp = match ramp {
            Some(ramp) if ramp.len() == size as usize * 3 => ramp,
            Some(_) => return false,
            None => {
                linear = linear_ramp(size);
                &linear
            }
        };
        let active = self.session.is_active();
        let Some(surface) = self.outputs.get_mut(&id) else {
            return false;
        };
        if !active || surface.drm_output.is_none() {
            // On another VT (no DRM master: setting it would fail, and the
            // client's control with it) or powered off: remembered, and set
            // when the CRTC is back (`reapply_gamma`).
            surface.gamma_pending = None;
        } else if surface.gamma_lut.is_some() {
            // Sent when the CRTC is free, without blocking (`gamma.rs`); a
            // newer change before then replaces it.
            surface.gamma_pending = Some(ramp.to_vec());
            self.schedule_render();
        } else if !self
            .drm_device(id)
            .is_some_and(|drm| apply_gamma(drm, id.crtc, ramp, size, GammaCause::Client))
        {
            return false;
        }
        let Some(surface) = self.outputs.get_mut(&id) else {
            return false;
        };
        surface.gamma = ramp_if_set(ramp, size);
        info!(
            name = surface.output.name(),
            custom = surface.gamma.is_some(),
            "gamma set"
        );
        true
    }

    /// Whether the output is powered on, or asked to come on when the
    /// session is back (`power_on_when_active`: what power clients were
    /// told, and what `power-off-monitors` must turn off); `None` for an
    /// unknown output.
    pub fn output_power(&self, output: &Output) -> Option<bool> {
        let id = self.output_id(output)?;
        let surface = &self.outputs[&id];
        Some(surface.drm_output.is_some() || surface.power_on_when_active)
    }

    /// Turn an output's display off (its CRTC is cleared, so the monitor
    /// goes to sleep) or back on (the CRTC is set up again with the same
    /// mode). The output stays mapped: windows and the pointer keep their
    /// places. `false` if it couldn't be done.
    pub fn set_output_power(&mut self, output: &Output, on: bool) -> bool {
        let Some(id) = self.output_id(output) else {
            return false;
        };
        let UdevData {
            outputs,
            devices,
            gpus,
            primary,
            session,
            ..
        } = self;
        let Some(GpuDevice {
            drm, render_node, ..
        }) = devices.get_mut(&id.node)
        else {
            return false;
        };
        let surface = outputs.get_mut(&id).expect("found above");
        let name = surface.output.name();
        if !on {
            surface.power_on_when_active = false;
        }
        if on == surface.drm_output.is_some() {
            return true;
        }
        if on && !session.is_active() {
            // No DRM master on another VT: setting the CRTC up would fail.
            // It comes on at `ActivateSession`.
            surface.power_on_when_active = true;
            info!(name, "output comes on when the session is back");
            return true;
        }
        if !on {
            surface.crtc_cleared();
            // Off goes ahead without a renderer; only this needs one.
            let restored = match setup_renderer(gpus, primary, render_node) {
                Some(OutputRenderer::Primary(mut renderer)) => drm
                    .lock()
                    .try_to_restore_modifiers(
                        renderer.as_mut(),
                        &DrmOutputRenderElements::<_, PilaRenderElement>::default(),
                    )
                    .map_err(|err| format!("{err:?}")),
                Some(OutputRenderer::Secondary(mut renderer)) => drm
                    .lock()
                    .try_to_restore_modifiers(
                        &mut renderer,
                        &DrmOutputRenderElements::<_, secondary::PrimaryGpuElement>::default(),
                    )
                    .map_err(|err| format!("{err:?}")),
                None => Ok(()),
            };
            if let Err(err) = restored {
                debug!(?err, "couldn't restore modifiers after powering off");
            }
            info!(name, "output powered off");
            return true;
        }
        // Another monitor may have taken the bandwidth meanwhile: then
        // the mode changes (`Pila::fit_to_outputs` follows it).
        let Some(drm_output) =
            outputs::initialize_on(drm, gpus, primary, render_node, id.crtc, surface)
        else {
            warn!(name, "failed to power the output on");
            return false;
        };
        surface.crtc_set_up(drm_output);
        info!(name, "output powered on");
        outputs::apply_vrr(surface);
        self.reapply_gamma(id);
        self.queue_redraw();
        true
    }

    /// The monitors as wlr-output-management describes them, disabled ones
    /// included, left to right.
    pub fn output_heads(&self) -> Vec<HeadInfo> {
        let mut heads: Vec<HeadInfo> = self
            .outputs
            .values()
            .map(|surface| {
                let props = surface.output.physical_properties();
                let loc = surface.output.current_location();
                let modes: Vec<ModeInfo> = surface
                    .modes
                    .iter()
                    .map(|&mode| {
                        let wl = WlMode::from(mode);
                        ModeInfo {
                            size: (wl.size.w, wl.size.h),
                            refresh_mhz: wl.refresh,
                            preferred: mode.mode_type().contains(ModeTypeFlags::PREFERRED),
                        }
                    })
                    .collect();
                let adaptive_sync = surface.drm_output.as_ref().and_then(|o| {
                    o.with_compositor(|c| match c.vrr_supported(surface.connector) {
                        Ok(VrrSupport::NotSupported) | Err(_) => None,
                        Ok(_) => Some(c.vrr_enabled()),
                    })
                });
                HeadInfo {
                    name: surface.output.name(),
                    make: props.make,
                    model: props.model,
                    serial: None,
                    physical: (props.size.w, props.size.h),
                    current_mode: surface.modes.iter().position(|m| *m == surface.mode),
                    modes,
                    enabled: surface.enabled(),
                    position: (loc.x, loc.y),
                    transform: surface.output.current_transform(),
                    scale: surface.output.current_scale().fractional_scale(),
                    adaptive_sync,
                }
            })
            .collect();
        heads.sort_by(|a, b| a.position.cmp(&b.position).then(a.name.cmp(&b.name)));
        heads
    }

    /// Put the output's gamma back after the CRTC was set up again or
    /// another compositor had the device (VT switch back, mode change,
    /// power on): the remembered ramps, or linear when no client set any
    /// (whoever had the CRTC may have left theirs).
    /// Blocking, which a modeset is too. Ramps still pending go with it.
    fn reapply_gamma(&mut self, id: OutputId) {
        let Some(surface) = self.outputs.get_mut(&id) else {
            return;
        };
        surface.gamma_pending = None;
        let output = surface.output.clone();
        let Some(size) = self.gamma_size(&output) else {
            return;
        };
        let Some(surface) = self.outputs.get_mut(&id) else {
            return;
        };
        // Set for the CRTC's size then; should it have changed since, the
        // ramps are dropped rather than read past their end.
        if surface
            .gamma
            .as_ref()
            .is_some_and(|ramp| !ramp_fits(ramp, size))
        {
            warn!(
                name = output.name(),
                size, "the CRTC's gamma size changed: its ramps go back to linear"
            );
            surface.gamma = None;
            // Its client is told, to make a control of the new size and set
            // its ramps again (wlsunset does), not to think them still set.
            let lost = output.clone();
            self.loop_handle
                .insert_idle(move |state| state.gamma_control_lost(&lost));
        }
        let surface = &self.outputs[&id];
        let linear;
        let ramp = match &surface.gamma {
            Some(ramp) => ramp,
            None => {
                linear = linear_ramp(size);
                &linear
            }
        };
        let applied = self
            .drm_device(id)
            .is_some_and(|drm| apply_gamma(drm, id.crtc, ramp, size, GammaCause::Modeset));
        if !applied {
            warn!(name = surface.output.name(), "failed to re-apply gamma");
        }
    }

    /// Linear gamma on every output: on exit, so the next compositor on
    /// the seat doesn't inherit a client's ramps.
    pub fn reset_gamma(&mut self) {
        let ids: Vec<_> = self.outputs.keys().copied().collect();
        for id in ids {
            if let Some(surface) = self.outputs.get_mut(&id) {
                if surface.gamma.take().is_some() {
                    self.reapply_gamma(id);
                }
            }
        }
    }

    /// When `output`'s last frame was shown.
    pub fn last_vblank_time(&self, output: &Output) -> Option<Duration> {
        self.outputs
            .values()
            .find(|surface| surface.output == *output)?
            .last_vblank_time
    }

    /// The primary GPU's renderer; `None` (logged) if it can't be lent.
    pub fn renderer(&mut self) -> Option<PrimaryRenderer<'_>> {
        primary_renderer(&mut self.gpus, &self.primary)
    }
}

/// The primary node of the DRM device at `path` (a seat may name another):
/// its key in `UdevData::devices`.
fn primary_node(path: &Path) -> Result<DrmNode, Box<dyn std::error::Error>> {
    let node = DrmNode::from_path(path)?;
    Ok(node
        .node_with_type(NodeType::Primary)
        .and_then(Result::ok)
        .unwrap_or(node))
}

/// Open the DRM device at `path` through the session.
fn open_device(
    session: &mut LibSeatSession,
    path: &Path,
) -> Result<DrmDeviceFd, Box<dyn std::error::Error>> {
    let fd = session.open(
        path,
        OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
    )?;
    Ok(DrmDeviceFd::new(DeviceFd::from(fd)))
}

/// A GPU's render node (`node` if it has none), which its renderer is keyed
/// by, and the EGL display it was found with. Smithay shares one display per
/// GBM device and terminates it with the last handle: keep this one until the
/// GPU manager holds its own (`add_node`), or the display is initialized
/// twice.
fn render_node(
    gbm: &GbmDevice<DrmDeviceFd>,
    node: DrmNode,
) -> Result<(DrmNode, EGLDisplay), Box<dyn std::error::Error>> {
    // SAFETY: the GBM device outlives the display (the output manager and
    // the GPU manager hold it, in `UdevData`).
    let display = unsafe { EGLDisplay::new(gbm.clone())? };
    let render_node = EGLDevice::device_for_display(&display)?
        .try_get_render_node()
        .ok()
        .flatten()
        .unwrap_or(node);
    Ok((render_node, display))
}

/// A GPU's output manager: buffers allocated on it for rendering and scanout,
/// in the formats its renderer draws (`render_formats`).
fn output_manager(
    drm: DrmDevice,
    gbm: GbmDevice<DrmDeviceFd>,
    render_node: DrmNode,
    render_formats: FormatSet,
) -> PilaDrmOutputManager {
    let allocator = GbmAllocator::new(
        gbm.clone(),
        GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
    );
    let exporter = GbmFramebufferExporter::new(gbm.clone(), render_node.into());
    DrmOutputManager::new(
        drm,
        allocator,
        exporter,
        Some(gbm),
        COLOR_FORMATS,
        render_formats,
    )
}

/// Set up the session, the GPU, outputs and input, and make pila run on them.
pub fn init_udev(
    event_loop: &mut EventLoop<'static, Pila>,
    state: &mut Pila,
) -> Result<(), Box<dyn std::error::Error>> {
    let (mut session, notifier) = LibSeatSession::new()
        .map_err(|err| format!("could not start a session (run pila from a TTY): {err}"))?;
    let seat = session.seat();
    info!(seat, "session started");

    // The seat's primary GPU, or any GPU.
    let path = primary_gpu(&seat)?
        .or_else(|| all_gpus(&seat).ok()?.into_iter().next())
        .ok_or("no GPU found")?;
    let node = primary_node(&path)?;
    info!(gpu = %path.display(), "using GPU");

    let fd = open_device(&mut session, &path)?;
    if supports_syncobj_eventfd(&fd) {
        state.protocols.syncobj = Some(DrmSyncobjState::new::<Pila>(
            &state.display_handle,
            fd.clone(),
        ));
    }
    let (drm, drm_notifier) = DrmDevice::new(fd.clone(), true)?;
    let gbm = GbmDevice::new(fd)?;

    let (render_node, _display) = render_node(&gbm, node)?;
    // The GPU manager owns the renderer, keyed by the render node. It only
    // logs a renderer it can't create and leaves the GPU out: keep the error
    // to say why the GPU is missing.
    let factory_error = std::rc::Rc::new(std::cell::RefCell::new(None));
    let error_slot = factory_error.clone();
    let mut api = GpuApi::with_factory(move |display| {
        let created = EGLContext::new(display)
            .map_err(smithay::backend::renderer::multigpu::gbm::Error::from)
            // SAFETY: the context was just created for this renderer only.
            .and_then(|context| Ok(unsafe { GlesRenderer::new(context)? }));
        if let Err(err) = &created {
            *error_slot.borrow_mut() = Some(err.to_string());
        }
        created
    });
    api.add_node(render_node, gbm.clone())?;
    let mut gpus = GpuManager::new(api)?;
    let mut lent = gpus.single_renderer(&render_node).map_err(|err| {
        match factory_error.borrow_mut().take() {
            Some(cause) => format!("could not set up the GPU's renderer: {cause}"),
            None => format!("could not set up the GPU's renderer: {err}"),
        }
    })?;
    let renderer = lent.as_mut();
    crate::render::warm_up(renderer);
    state.capture.dmabuf = crate::capture::DmabufTarget::new(render_node.dev_id(), renderer);

    let render_formats = renderer.egl_context().dmabuf_render_formats().clone();
    let drm = output_manager(drm, gbm, render_node, render_formats);

    // Client buffers: shm in the formats the renderer reads, dmabuf on this GPU.
    state.protocols.shm.update_formats(renderer.shm_formats());
    let dmabuf_formats = renderer.dmabuf_formats();
    let feedback =
        DmabufFeedbackBuilder::new(render_node.dev_id(), dmabuf_formats.clone()).build()?;
    drop(lent);
    state
        .protocols
        .dmabuf
        .create_global_with_default_feedback::<Pila>(&state.display_handle, &feedback);

    // Input.
    let mut libinput =
        Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
    libinput
        .udev_assign_seat(&seat)
        .map_err(|()| "failed to assign the libinput seat")?;
    event_loop.handle().insert_source(
        LibinputInputBackend::new(libinput.clone()),
        |mut event, _, state| {
            on_input_device(state, &mut event);
            // It asks for the redraws it needs (`process_input_event`).
            state.process_input_event(event);
        },
    )?;

    // VT switches.
    event_loop
        .handle()
        .insert_source(notifier, |event, _, state| match event {
            SessionEvent::PauseSession => {
                info!("session paused");
                // Keys held for the VT switch never get a release otherwise.
                state.release_all_keys();
                if let Some(Backend::Udev(data)) = &mut state.backend {
                    data.libinput.suspend();
                    for device in data.devices.values_mut() {
                        device.drm.pause();
                    }
                }
            }
            SessionEvent::ActivateSession => {
                info!("session resumed");
                if let Some(Backend::Udev(data)) = &mut state.backend {
                    if let Err(err) = data.libinput.resume() {
                        error!(?err, "failed to resume libinput");
                    }
                    // Take master first: resetting the device needs it.
                    data.take_master();
                    // Start from a clean device: another compositor on the
                    // seat may have left the connectors on other CRTCs than
                    // pila's, and moving them over piecemeal fails
                    // (EINVAL). Disabling everything costs one modeset.
                    for device in data.devices.values_mut() {
                        if let Err(err) = device.drm.lock().activate(true) {
                            error!(?err, "failed to resume the DRM device");
                        }
                    }
                    let ids: Vec<_> = data.outputs.keys().copied().collect();
                    for id in ids {
                        let surface = data.outputs.get_mut(&id).expect("listed");
                        // The device was reset: no frame is on its way, the
                        // vblanks start over, failures while away were for
                        // lack of DRM master.
                        surface.crtc_state = CrtcState::default();
                        // Another VT may have changed the LUTs.
                        data.reapply_gamma(id);
                    }
                    // Powered on while away.
                    let waiting: Vec<Output> = data
                        .outputs
                        .values_mut()
                        .filter_map(|surface| {
                            (std::mem::take(&mut surface.power_on_when_active) && surface.enabled())
                                .then(|| surface.output.clone())
                        })
                        .collect();
                    for output in waiting {
                        if !data.set_output_power(&output, true) {
                            warn!(name = output.name(), "couldn't power the output on");
                        }
                    }
                }
                // Monitors plugged or unplugged while away. Those it found
                // then couldn't come on without DRM master: they do here.
                scan_connectors(state, None);
                // And GPUs added meanwhile, which couldn't be opened then.
                secondary::apply_other_gpus(state);
                state.apply_outputs();
                state.queue_redraw();
            }
        })?;

    // Vblanks.
    event_loop
        .handle()
        .insert_source(drm_notifier, move |event, metadata, state| match event {
            DrmEvent::VBlank(crtc) => frame_finish(state, OutputId { node, crtc }, metadata),
            DrmEvent::Error(err) => error!(?err, "DRM error"),
        })?;

    // Monitors plugged in and out, GPUs added and removed.
    let udev = UdevBackend::new(&seat)?;
    event_loop
        .handle()
        .insert_source(udev, move |event, _, state| match event {
            UdevEvent::Changed { device_id } => {
                let node = match &state.backend {
                    Some(Backend::Udev(data)) => data
                        .devices
                        .keys()
                        .copied()
                        .find(|node| node.dev_id() == device_id),
                    _ => None,
                };
                if let Some(node) = node {
                    scan_connectors(state, Some(node));
                }
            }
            UdevEvent::Added { path, .. } => {
                if state.config.other_gpus {
                    secondary::add_gpu(state, &path);
                } else {
                    secondary::say_unused(&path);
                }
            }
            UdevEvent::Removed { device_id } => secondary::remove_gpu(state, device_id),
        })?;

    state.backend = Some(Backend::Udev(Box::new(UdevData {
        session,
        loop_handle: event_loop.handle(),
        libinput,
        gpus,
        primary: render_node,
        dmabuf_feedback: feedback,
        dmabuf_formats,
        devices: HashMap::from([(
            node,
            GpuDevice {
                drm,
                scanner: DrmScanner::new(),
                render_node,
                events: None,
            },
        )]),
        outputs: HashMap::new(),
        redraw_scheduled: false,
        clock: smithay::utils::Clock::new(),
        keyboards: Vec::new(),
        pointers: Vec::new(),
    })));

    scan_connectors(state, None);
    // The seat's other GPUs: their monitors drawn on the primary, if the
    // config asks for them.
    secondary::apply_other_gpus(state);
    // Start with the pointer in the middle of the first output.
    if let (Some(geo), Some(pointer)) = (
        state.output_geometry_at((0.0, 0.0).into()),
        state.seat.get_pointer(),
    ) {
        pointer.set_location((geo.loc + geo.size.downscale(2).to_point()).to_f64());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_swapchain_is_opaque_like_a_clients_buffer_on_the_primary_plane() {
        // Smithay adds a client's buffer shown on the primary plane as the
        // opaque variant of its format (ARGB8888 as XRGB8888): a swapchain
        // with alpha would change the plane's format at every switch, which
        // amdgpu answers with a plane reset that waits for every monitor.
        use smithay::backend::allocator::format::has_alpha;
        assert!(!has_alpha(COLOR_FORMATS[0]), "{:?}", COLOR_FORMATS[0]);
        assert!(!has_alpha(COLOR_FORMATS[1]), "{:?}", COLOR_FORMATS[1]);
    }

    #[test]
    fn a_crtc_set_up_again_starts_with_its_first_frame_and_nothing_on_its_way() {
        let state = CrtcState::default();
        assert!(state.first_frame);
        assert!(!state.frame_pending && !state.gamma_in_flight && !state.queued_held);
        assert!(state.lock_frame == LockFrame::None);
        // The vblanks before aren't counted against the ones after.
        assert!(state.last_vblank.is_none() && !state.back_to_back);
        assert!(state.render_at.is_none() && state.idle_until.is_none());
        assert!(state.request.is_none() && state.queued_request.is_none());
        assert!(!state.recovery.blocks_render());
    }

    #[test]
    fn a_connector_name_another_gpu_has_gets_a_number() {
        use super::outputs::free_name;
        assert_eq!(free_name("DP-1".into(), |_| false), "DP-1");
        let taken = ["DP-1", "DP-1-2"];
        assert_eq!(free_name("DP-1".into(), |n| taken.contains(&n)), "DP-1-3");
        assert_eq!(
            free_name("HDMI-A-1".into(), |n| taken.contains(&n)),
            "HDMI-A-1"
        );
    }

    #[test]
    fn built_in_screens_are_recognized_by_connector() {
        use super::outputs::is_internal;
        for name in ["eDP-1", "LVDS-1", "DSI-2"] {
            assert!(is_internal(name), "{name}");
        }
        for name in ["HDMI-A-1", "DP-2", "DVI-D-1", "eDP"] {
            assert!(!is_internal(name), "{name}");
        }
    }

    #[test]
    fn linear_ramps_are_recognized() {
        let linear = linear_ramp(256);
        assert_eq!(linear.len(), 3 * 256);
        assert_eq!((linear[0], linear[255], linear[256]), (0, 0xffff, 0));
        assert_eq!(ramp_if_set(&linear, 256), None);
        let mut dimmed = linear.clone();
        dimmed[100] /= 2;
        assert_eq!(ramp_if_set(&dimmed, 256), Some(dimmed.clone()));
        // A size of one doesn't divide by zero.
        assert_eq!(linear_ramp(1), vec![0, 0, 0]);
    }
}
