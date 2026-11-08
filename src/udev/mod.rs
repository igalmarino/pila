//! Bare-metal backend: DRM/KMS outputs, libinput input, libseat session.
//! Run from a TTY (`pila --backend udev`, the default without a host session).
//!
//! Deliberately smaller than anvil's: one GPU (the seat's primary one, which
//! renders and scans out), no direct scan-out tuning, no DRM leasing. Outputs
//! are placed left to right in connector order.
//!
//! Redraws stay on demand (`Backend::queue_redraw`): one idle callback per
//! loop iteration renders every output that needs it. An output with a frame
//! in flight renders again at its next vblank (`frame_finish`) instead, so
//! redraws are paced by the display. A session lock is confirmed only after a
//! locked frame has been shown on every output.

use std::{collections::HashMap, time::Duration};

use smithay::wayland::drm_syncobj::{supports_syncobj_eventfd, DrmSyncobjState};

use crate::edid::Monitor;

use smithay::{
    backend::{
        allocator::{
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
            Fourcc,
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
        renderer::{gles::GlesRenderer, ImportDma, ImportMemWl},
        session::{libseat::LibSeatSession, Event as SessionEvent, Session},
        udev::{all_gpus, primary_gpu, UdevBackend, UdevEvent},
    },
    desktop::{layer_map_for_output, utils::OutputPresentationFeedback},
    input::{keyboard::LedState, pointer::CursorImageStatus},
    output::{Mode as WlMode, Output, PhysicalProperties, Scale as OutputScale},
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            EventLoop, LoopHandle,
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
    wayland::{dmabuf::DmabufFeedbackBuilder, presentation::Refresh},
};
use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};
use tracing::{debug, error, info, warn};

mod draw;
mod gamma;
mod input;
mod outputs;
mod recovery;

use self::{draw::*, gamma::*, input::*, outputs::*, recovery::Recovery};
pub use outputs::apply_output_config;

use crate::{
    backend::Backend,
    config::{AccelProfile, ClickMethod, InputConfig, OutputConfig, PointerConfig},
    cursor::CursorTheme,
    frame,
    output_management::{HeadInfo, ModeInfo},
    render::{self, PilaRenderElement},
    Pila,
};

/// 8-bit formats only: widely supported, and what clients expect.
const COLOR_FORMATS: [Fourcc; 4] = [
    Fourcc::Argb8888,
    Fourcc::Abgr8888,
    Fourcc::Xrgb8888,
    Fourcc::Xbgr8888,
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

/// Fields drop in declaration order, which matters here: the outputs go
/// before the DRM device, and the DRM device (which restores the previous
/// display state when dropped) before the session that grants access to it.
pub struct UdevData {
    outputs: HashMap<crtc::Handle, OutputSurface>,
    drm: PilaDrmOutputManager,
    scanner: DrmScanner,
    pub renderer: GlesRenderer,
    libinput: Libinput,
    /// Keyboards, for the Caps/Num/Scroll Lock LEDs.
    keyboards: Vec<libinput::Device>,
    /// Touchpads and mice, to re-apply `[input]` when the config changes.
    pointers: Vec<libinput::Device>,
    cursor_theme: CursorTheme,
    /// A render pass is already queued for this loop iteration.
    redraw_scheduled: bool,
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
    /// Disabled (`[[output]]` or a display tool): no surface, no global,
    /// not mapped; still listed as a head.
    enabled: bool,
    global: Option<GlobalId>,
    /// A frame was queued and its vblank hasn't arrived yet.
    frame_pending: bool,
    /// The sequence number of the vblank that showed the last frame, and
    /// whether pila wanted a new frame at every vblank after it (an
    /// animation was running; `frame_stats.rs`).
    last_vblank: Option<u64>,
    /// When the last frame was shown (the kernel's timestamp, on the
    /// presentation clock, if it gives one).
    last_vblank_time: Option<Duration>,
    back_to_back: bool,
    /// Something changed since the last frame.
    redraw_needed: bool,
    /// Progress of the frame that confirms a session lock.
    lock_frame: LockFrame,
    /// The connector's modes, for `[[output]]` mode changes on reload.
    modes: Vec<DrmMode>,
    /// The gamma ramps a client set (`gamma_control.rs`), re-applied
    /// whenever the CRTC is set up again.
    gamma: Option<Vec<u16>>,
    /// The CRTC's `GAMMA_LUT`, through which a client's ramps go out without
    /// blocking (`gamma.rs`); `None` on a legacy device or driver.
    gamma_lut: Option<property::Handle>,
    /// Ramps set but not sent yet: they go out once no frame is on its way
    /// (`commit_pending_gamma`).
    gamma_pending: Option<Vec<u16>>,
    /// The commit on its way (`frame_pending`) is a gamma change, not a frame.
    gamma_in_flight: bool,
    /// Frames that failed at the DRM level (`recovery.rs`): drawing waits
    /// for a retry instead of failing again on every event.
    recovery: Recovery,
    /// Adaptive sync as `[[output]]` asks for it, set again whenever the
    /// CRTC is set up again (`outputs::apply_vrr`).
    adaptive_sync: bool,
    /// Whether the last frame shown was reported to clients with a variable
    /// refresh, logged when it changes (`scripts/check --vrr` reads it);
    /// forgotten while the CRTC is off.
    presented_variable: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LockFrame {
    None,
    Queued,
    Shown,
}

impl UdevData {
    /// Ask every output for a new frame (see the module docs).
    pub fn queue_redraw(&mut self) {
        for surface in self.outputs.values_mut() {
            surface.redraw_needed = true;
        }
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
        if let Err(err) = self.drm.device().acquire_master_lock() {
            debug!(?err, "couldn't take DRM master");
        }
    }

    /// A frame on `crtc` failed at the DRM level: draw it again after
    /// `delay` (`Recovery::failed`), taking DRM master first in case that
    /// was it. The other outputs go on meanwhile.
    fn schedule_recovery(&mut self, crtc: crtc::Handle, delay: Duration) {
        let timer = Timer::from_duration(delay);
        let inserted = self.loop_handle.insert_source(timer, move |_, _, state| {
            if let Some(Backend::Udev(data)) = &mut state.backend {
                data.take_master();
                if let Some(surface) = data.outputs.get_mut(&crtc) {
                    surface.recovery.retry();
                    surface.redraw_needed = true;
                }
                if !data.redraw_scheduled {
                    data.redraw_scheduled = true;
                    data.loop_handle.insert_idle(render_all);
                }
            }
            TimeoutAction::Drop
        });
        if let Err(err) = inserted {
            // Without a timer the output would wait forever: retry on the
            // next redraw instead.
            warn!(err = %err.error, "can't schedule a retry for an output");
            if let Some(surface) = self.outputs.get_mut(&crtc) {
                surface.recovery.retry();
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

    fn crtc_of(&self, output: &Output) -> Option<crtc::Handle> {
        self.outputs
            .iter()
            .find(|(_, s)| &s.output == output)
            .map(|(crtc, _)| *crtc)
    }

    /// The length of the output's gamma ramps (0 = no gamma support).
    pub fn gamma_size(&self, output: &Output) -> Option<u32> {
        let crtc = self.crtc_of(output)?;
        self.drm
            .device()
            .get_crtc(crtc)
            .ok()
            .map(|info| info.gamma_length())
            .filter(|n| *n > 0)
    }

    /// Set the output's gamma ramps (`None`: linear) and remember them.
    pub fn set_gamma(&mut self, output: &Output, ramp: Option<&[u16]>) -> bool {
        let Some(crtc) = self.crtc_of(output) else {
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
        let Some(surface) = self.outputs.get_mut(&crtc) else {
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
            if !self.redraw_scheduled {
                self.redraw_scheduled = true;
                self.loop_handle.insert_idle(render_all);
            }
        } else if !apply_gamma(self.drm.device(), crtc, ramp, size) {
            return false;
        }
        let Some(surface) = self.outputs.get_mut(&crtc) else {
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

    /// Whether the output is powered on; `None` for an unknown output.
    pub fn output_power(&self, output: &Output) -> Option<bool> {
        let crtc = self.crtc_of(output)?;
        Some(self.outputs[&crtc].drm_output.is_some())
    }

    /// Turn an output's display off (its CRTC is cleared, so the monitor
    /// goes to sleep) or back on (the CRTC is set up again with the same
    /// mode). The output stays mapped: windows and the pointer keep their
    /// places. `false` if it couldn't be done.
    pub fn set_output_power(&mut self, output: &Output, on: bool) -> bool {
        let Some(crtc) = self.crtc_of(output) else {
            return false;
        };
        let UdevData {
            outputs,
            drm,
            renderer,
            ..
        } = self;
        let surface = outputs.get_mut(&crtc).expect("found above");
        let name = surface.output.name();
        if on == surface.drm_output.is_some() {
            return true;
        }
        let elements = DrmOutputRenderElements::<_, PilaRenderElement>::default();
        if !on {
            surface.drm_output = None;
            surface.frame_pending = false;
            surface.gamma_in_flight = false;
            surface.lock_frame = LockFrame::None;
            // The vblanks of the dark gap aren't missed refreshes.
            surface.last_vblank = None;
            surface.presented_variable = false;
            if let Err(err) = drm.lock().try_to_restore_modifiers(renderer, &elements) {
                debug!(?err, "couldn't restore modifiers after powering off");
            }
            info!(name, "output powered off");
            return true;
        }
        // Another monitor may have taken the bandwidth meanwhile: then
        // the mode changes (`Pila::fit_to_outputs` follows it).
        let Some(drm_output) = initialize_with_fallback(drm, renderer, crtc, surface) else {
            warn!(name, "failed to power the output on");
            return false;
        };
        surface.drm_output = Some(drm_output);
        surface.frame_pending = false;
        surface.redraw_needed = true;
        surface.recovery = Recovery::default();
        info!(name, "output powered on");
        outputs::apply_vrr(surface);
        self.reapply_gamma(crtc);
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
                    enabled: surface.enabled,
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
    fn reapply_gamma(&mut self, crtc: crtc::Handle) {
        let Some(surface) = self.outputs.get_mut(&crtc) else {
            return;
        };
        surface.gamma_pending = None;
        let surface = &self.outputs[&crtc];
        let Some(size) = self.gamma_size(&surface.output) else {
            return;
        };
        let linear;
        let ramp = match &surface.gamma {
            Some(ramp) => ramp,
            None => {
                linear = linear_ramp(size);
                &linear
            }
        };
        if !apply_gamma(self.drm.device(), crtc, ramp, size) {
            warn!(name = surface.output.name(), "failed to re-apply gamma");
        }
    }

    /// Linear gamma on every output: on exit, so the next compositor on
    /// the seat doesn't inherit a client's ramps.
    pub fn reset_gamma(&mut self) {
        let crtcs: Vec<_> = self.outputs.keys().copied().collect();
        for crtc in crtcs {
            if let Some(surface) = self.outputs.get_mut(&crtc) {
                if surface.gamma.take().is_some() {
                    self.reapply_gamma(crtc);
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

    pub fn renderer_and_cursor_theme(&mut self) -> (&mut GlesRenderer, &mut CursorTheme) {
        (&mut self.renderer, &mut self.cursor_theme)
    }

    pub fn cursor_changed(&mut self, _status: &CursorImageStatus) {
        // Drawn by pila in `render_output`; the redraw is queued by the caller.
    }
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
    let node = DrmNode::from_path(&path)?;
    let node = node
        .node_with_type(NodeType::Primary)
        .and_then(Result::ok)
        .unwrap_or(node);
    info!(gpu = %path.display(), "using GPU");

    let fd = session.open(
        &path,
        OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
    )?;
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));
    if supports_syncobj_eventfd(&fd) {
        state.protocols.syncobj = Some(DrmSyncobjState::new::<Pila>(
            &state.display_handle,
            fd.clone(),
        ));
    }
    let (drm, drm_notifier) = DrmDevice::new(fd.clone(), true)?;
    let gbm = GbmDevice::new(fd)?;

    // SAFETY: the GBM device outlives the display (both live in `UdevData`
    // via the renderer and output manager).
    let display = unsafe { EGLDisplay::new(gbm.clone())? };
    let render_node = EGLDevice::device_for_display(&display)?
        .try_get_render_node()
        .ok()
        .flatten()
        .unwrap_or(node);
    let context = EGLContext::new(&display)?;
    // SAFETY: the context was just created for this renderer only.
    let mut renderer = unsafe { GlesRenderer::new(context)? };
    crate::render::warm_up(&mut renderer);
    state.capture.dmabuf = Some(crate::capture::DmabufTarget::new(
        render_node.dev_id(),
        &renderer,
    ));

    let allocator = GbmAllocator::new(
        gbm.clone(),
        GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
    );
    let exporter = GbmFramebufferExporter::new(gbm.clone(), render_node.into());
    let render_formats = renderer.egl_context().dmabuf_render_formats().clone();
    let drm = DrmOutputManager::new(
        drm,
        allocator,
        exporter,
        Some(gbm),
        COLOR_FORMATS,
        render_formats,
    );

    // Client buffers: shm in the formats the renderer reads, dmabuf on this GPU.
    state.protocols.shm.update_formats(renderer.shm_formats());
    let feedback =
        DmabufFeedbackBuilder::new(render_node.dev_id(), renderer.dmabuf_formats()).build()?;
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
            state.process_input_event(event);
            // Input moves the cursor and changes hover state even when no
            // client reacts; coalesced into one render pass per iteration.
            state.queue_redraw();
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
                    data.drm.pause();
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
                    if let Err(err) = data.drm.lock().activate(true) {
                        error!(?err, "failed to resume the DRM device");
                    }
                    let crtcs: Vec<_> = data.outputs.keys().copied().collect();
                    for crtc in crtcs {
                        let surface = data.outputs.get_mut(&crtc).expect("listed");
                        surface.frame_pending = false;
                        surface.gamma_in_flight = false;
                        // Failures while away were for lack of DRM master.
                        surface.recovery = Recovery::default();
                        // Another VT may have changed the LUTs.
                        data.reapply_gamma(crtc);
                    }
                }
                // Monitors plugged or unplugged while away. Those it found
                // then couldn't come on without DRM master: they do here.
                scan_connectors(state);
                state.apply_outputs();
                state.queue_redraw();
            }
        })?;

    // Vblanks.
    event_loop
        .handle()
        .insert_source(drm_notifier, |event, metadata, state| match event {
            DrmEvent::VBlank(crtc) => frame_finish(state, crtc, metadata),
            DrmEvent::Error(err) => error!(?err, "DRM error"),
        })?;

    // Monitor hotplug on this GPU.
    let udev = UdevBackend::new(&seat)?;
    event_loop
        .handle()
        .insert_source(udev, move |event, _, state| match event {
            UdevEvent::Changed { device_id } if device_id == node.dev_id() => {
                scan_connectors(state)
            }
            UdevEvent::Added { device_id, path } => {
                debug!(device_id, path = %path.display(), "ignoring additional GPU");
            }
            _ => {}
        })?;

    state.backend = Some(Backend::Udev(Box::new(UdevData {
        session,
        loop_handle: event_loop.handle(),
        libinput,
        renderer,
        drm,
        scanner: DrmScanner::new(),
        outputs: HashMap::new(),
        redraw_scheduled: false,
        cursor_theme: CursorTheme::load(),
        keyboards: Vec::new(),
        pointers: Vec::new(),
    })));

    scan_connectors(state);
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
