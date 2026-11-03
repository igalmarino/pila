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
        calloop::{EventLoop, LoopHandle},
        drm::control::{connector, crtc, Device as _, Mode as DrmMode, ModeTypeFlags},
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

use crate::{
    backend::Backend,
    config::{AccelProfile, ClickMethod, InputConfig, OutputConfig, PointerConfig},
    cursor::{self, CursorTheme},
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
    /// Something changed since the last frame.
    redraw_needed: bool,
    /// Progress of the frame that confirms a session lock.
    lock_frame: LockFrame,
    /// The connector's modes, for `[[output]]` mode changes on reload.
    modes: Vec<DrmMode>,
    /// The gamma ramps a client set (`gamma_control.rs`), re-applied
    /// whenever the CRTC is set up again.
    gamma: Option<Vec<u16>>,
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
                let one: Vec<u16> = (0..size)
                    .map(|i| (u64::from(i) * 0xffff / u64::from(size - 1).max(1)) as u16)
                    .collect();
                linear = [one.clone(), one.clone(), one].concat();
                &linear
            }
        };
        if !apply_gamma(self.drm.device(), crtc, ramp, size) {
            return false;
        }
        if let Some(surface) = self.outputs.get_mut(&crtc) {
            surface.gamma = ramp_if_set(ramp, size);
        }
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
        let elements = DrmOutputRenderElements::<_, PilaRenderElement<GlesRenderer>>::default();
        if !on {
            surface.drm_output = None;
            surface.frame_pending = false;
            surface.lock_frame = LockFrame::None;
            if let Err(err) = drm.try_to_restore_modifiers(renderer, &elements) {
                debug!(?err, "couldn't restore modifiers after powering off");
            }
            info!(name, "output powered off");
            return true;
        }
        let planes = drm.device().planes(&crtc).ok();
        match drm.initialize_output(
            crtc,
            surface.mode,
            &[surface.connector],
            &surface.output,
            planes,
            renderer,
            &elements,
        ) {
            Ok(drm_output) => {
                surface.drm_output = Some(drm_output);
                surface.frame_pending = false;
                surface.redraw_needed = true;
                info!(name, "output powered on");
            }
            Err(err) => {
                warn!(name, ?err, "failed to power the output on");
                return false;
            }
        }
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

    /// Put the remembered gamma ramps back after the CRTC was set up again
    /// (VT switch back, mode change, power on).
    fn reapply_gamma(&self, crtc: crtc::Handle) {
        let Some(surface) = self.outputs.get(&crtc) else {
            return;
        };
        if let (Some(ramp), Some(size)) = (&surface.gamma, self.gamma_size(&surface.output)) {
            if !apply_gamma(self.drm.device(), crtc, ramp, size) {
                warn!(name = surface.output.name(), "failed to re-apply gamma");
            }
        }
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
    let renderer = unsafe { GlesRenderer::new(context)? };
    state.capture_dmabuf = Some(crate::capture::DmabufTarget::new(
        render_node.dev_id(),
        &renderer,
    ));

    let allocator = GbmAllocator::new(
        gbm.clone(),
        GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
    );
    let exporter = GbmFramebufferExporter::new(gbm.clone(), Some(render_node));
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
    state.shm_state.update_formats(renderer.shm_formats());
    let feedback =
        DmabufFeedbackBuilder::new(render_node.dev_id(), renderer.dmabuf_formats()).build()?;
    state
        .dmabuf_state
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
                    if let Err(err) = data.drm.activate(false) {
                        error!(?err, "failed to resume the DRM device");
                    }
                    let crtcs: Vec<_> = data.outputs.keys().copied().collect();
                    for crtc in crtcs {
                        data.outputs.get_mut(&crtc).expect("listed").frame_pending = false;
                        // Another VT may have changed the LUTs.
                        data.reapply_gamma(crtc);
                    }
                }
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

/// Device setup before an input event is processed: keyboards for LEDs,
/// `[input]` settings on touchpads and mice.
fn on_input_device(state: &mut Pila, event: &mut InputEvent<LibinputInputBackend>) {
    let Some(Backend::Udev(data)) = &mut state.backend else {
        return;
    };
    let input = &state.config.input;
    match event {
        InputEvent::DeviceAdded { device } => {
            if device.has_capability(libinput::DeviceCapability::Keyboard) {
                if let Some(keyboard) = state.seat.get_keyboard() {
                    device.led_update(keyboard.led_state().into());
                }
                data.keyboards.push(device.clone());
            }
            if let Some(config) = pointer_config(device, input) {
                configure_pointer(device, config);
                data.pointers.push(device.clone());
            }
        }
        InputEvent::DeviceRemoved { device } => {
            data.keyboards.retain(|keyboard| keyboard != device);
            data.pointers.retain(|pointer| pointer != device);
        }
        _ => {}
    }
}

/// The `[input]` section for a device: touchpads are the devices that can
/// tap; other pointing devices are mice. `None` for anything else.
fn pointer_config<'a>(
    device: &libinput::Device,
    input: &'a InputConfig,
) -> Option<&'a PointerConfig> {
    if device.config_tap_finger_count() > 0 {
        Some(&input.touchpad)
    } else if device.has_capability(libinput::DeviceCapability::Pointer) {
        Some(&input.mouse)
    } else {
        None
    }
}

/// Apply `[input.touchpad]` or `[input.mouse]` to a device. Unset options
/// get libinput's default for the device (so removing one from the config
/// undoes it on reload); options the device doesn't support are skipped.
fn configure_pointer(device: &mut libinput::Device, config: &PointerConfig) {
    let name = device.name().to_string();
    let mut failed = Vec::new();
    let mut check = |what: &'static str, result: libinput::DeviceConfigResult| {
        if let Err(err) = result {
            failed.push((what, err));
        }
    };
    if device.config_tap_finger_count() > 0 {
        let tap = config
            .tap
            .unwrap_or_else(|| device.config_tap_default_enabled());
        check("tap", device.config_tap_set_enabled(tap));
    }
    if device.config_scroll_has_natural_scroll() {
        let natural = config
            .natural_scroll
            .unwrap_or_else(|| device.config_scroll_default_natural_scroll_enabled());
        check(
            "natural-scroll",
            device.config_scroll_set_natural_scroll_enabled(natural),
        );
    }
    if device.config_accel_is_available() {
        let speed = config
            .accel_speed
            .unwrap_or_else(|| device.config_accel_default_speed());
        check("accel-speed", device.config_accel_set_speed(speed));
        let profile = match config.accel_profile {
            Some(AccelProfile::Adaptive) => Some(libinput::AccelProfile::Adaptive),
            Some(AccelProfile::Flat) => Some(libinput::AccelProfile::Flat),
            None => device.config_accel_default_profile(),
        };
        if let Some(profile) = profile.filter(|p| device.config_accel_profiles().contains(p)) {
            check("accel-profile", device.config_accel_set_profile(profile));
        }
    }
    if device.config_dwt_is_available() {
        let dwt = config
            .disable_while_typing
            .unwrap_or_else(|| device.config_dwt_default_enabled());
        check("disable-while-typing", device.config_dwt_set_enabled(dwt));
    }
    let method = match config.click_method {
        Some(ClickMethod::ButtonAreas) => Some(libinput::ClickMethod::ButtonAreas),
        Some(ClickMethod::Clickfinger) => Some(libinput::ClickMethod::Clickfinger),
        None => device.config_click_default_method(),
    };
    if let Some(method) = method.filter(|m| device.config_click_methods().contains(m)) {
        check("click-method", device.config_click_set_method(method));
    }
    if device.config_middle_emulation_is_available() {
        let middle = config
            .middle_emulation
            .unwrap_or_else(|| device.config_middle_emulation_default_enabled());
        check(
            "middle-emulation",
            device.config_middle_emulation_set_enabled(middle),
        );
    }
    for (what, err) in &failed {
        debug!(device = name, what, ?err, "input setting not applied");
    }
    info!(device = name, ?config, "configured pointer device");
}

/// Set a CRTC's gamma ramps through the legacy ioctl, which atomic drivers
/// serve too.
fn apply_gamma(device: &DrmDevice, crtc: crtc::Handle, ramp: &[u16], size: u32) -> bool {
    let n = size as usize;
    match device.set_gamma(crtc, &ramp[..n], &ramp[n..2 * n], &ramp[2 * n..3 * n]) {
        Ok(()) => true,
        Err(err) => {
            warn!(?err, "set_gamma failed");
            false
        }
    }
}

/// The ramp to remember: none if it's the linear one.
fn ramp_if_set(ramp: &[u16], size: u32) -> Option<Vec<u16>> {
    let n = size as usize;
    let linear = (0..3 * n).all(|i| {
        let j = (i % n) as u64;
        ramp[i] as u64 == j * 0xffff / u64::from(size - 1).max(1)
    });
    (!linear).then(|| ramp.to_vec())
}

/// The mode `config` asks for among a connector's `modes`, else its
/// preferred mode, else its first.
fn pick_mode(name: &str, modes: &[DrmMode], config: Option<&OutputConfig>) -> Option<DrmMode> {
    let preferred = modes
        .iter()
        .position(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED));
    let configured = config.and_then(|c| c.mode).and_then(|spec| {
        let sizes: Vec<_> = modes
            .iter()
            .map(|&mode| {
                let wl = WlMode::from(mode);
                ((wl.size.w, wl.size.h), wl.refresh)
            })
            .collect();
        let picked = spec.pick(&sizes, preferred);
        if picked.is_none() {
            warn!(
                name,
                ?spec,
                "the monitor has no such mode; using its preferred one"
            );
        }
        picked
    });
    configured
        .or(preferred)
        .and_then(|i| modes.get(i))
        .or_else(|| modes.first())
        .copied()
}

/// Re-apply `[[output]]` (mode, scale, position) to the connected outputs
/// after the config changed. Outputs keep their left-to-right order; those
/// without a position go right of the ones placed before them.
pub fn apply_output_config(state: &mut Pila) {
    let Some(Backend::Udev(data)) = &state.backend else {
        return;
    };
    let mut order: Vec<(crtc::Handle, i32)> = data
        .outputs
        .iter()
        .map(|(crtc, s)| (*crtc, s.output.current_location().x))
        .collect();
    order.sort_by_key(|(_, x)| *x);
    let configs: Vec<(crtc::Handle, OutputConfig)> = order
        .iter()
        .map(|(crtc, _)| {
            let name = data.outputs[crtc].output.name();
            (*crtc, effective_output_config(state, &name))
        })
        .collect();

    // Enabling and disabling first: they change what the space holds.
    for (crtc, config) in &configs {
        let enabled = config.enabled.unwrap_or(true);
        if enabled {
            enable_output(state, *crtc);
        } else {
            disable_output(state, *crtc);
        }
    }

    let Some(Backend::Udev(data)) = &mut state.backend else {
        return;
    };
    let UdevData {
        outputs,
        drm,
        renderer,
        ..
    } = &mut **data;
    let mut placed: Vec<(Output, Point<i32, Logical>)> = Vec::new();
    let mut remodeled: Vec<crtc::Handle> = Vec::new();
    let mut vrr: Vec<(crtc::Handle, bool)> = Vec::new();
    let mut right_edge = 0;
    for (crtc, config) in &configs {
        let crtc = *crtc;
        let surface = outputs.get_mut(&crtc).expect("listed above");
        let name = surface.output.name();
        let config = Some(config);
        if let Some(mode) = pick_mode(&name, &surface.modes, config) {
            let wl_mode = WlMode::from(mode);
            if surface.output.current_mode() != Some(wl_mode) {
                // Powered off: the mode is set when the CRTC comes back.
                let result = match surface.drm_output {
                    Some(_) => drm.use_mode(
                        &crtc,
                        mode,
                        renderer,
                        &DrmOutputRenderElements::<_, PilaRenderElement<GlesRenderer>>::default(),
                    ),
                    None => Ok(()),
                };
                match result {
                    Ok(()) => {
                        surface.mode = mode;
                        surface
                            .output
                            .change_current_state(Some(wl_mode), None, None, None);
                        info!(name, mode = ?wl_mode.size, refresh_mhz = wl_mode.refresh, "output mode changed");
                        remodeled.push(crtc);
                    }
                    Err(err) => warn!(name, ?err, "failed to change the output mode"),
                }
            }
        }
        let scale = config.and_then(|c| c.scale).unwrap_or(1.0);
        let transform = config
            .and_then(|c| c.transform)
            .unwrap_or(Transform::Normal);
        surface.output.change_current_state(
            None,
            Some(transform),
            Some(OutputScale::Fractional(scale)),
            None,
        );
        let loc: Point<i32, Logical> = config
            .and_then(|c| c.position)
            .unwrap_or((right_edge, 0))
            .into();
        surface
            .output
            .change_current_state(None, None, None, Some(loc));
        // Rotated a quarter turn, width and height swap.
        let width = surface
            .output
            .current_mode()
            .map(|m| {
                transform
                    .transform_size(m.size)
                    .to_f64()
                    .to_logical(scale)
                    .to_i32_round::<i32>()
                    .w
            })
            .unwrap_or(0);
        if let Some(on) = config.and_then(|c| c.adaptive_sync) {
            vrr.push((crtc, on));
        }
        if !surface.enabled {
            continue;
        }
        right_edge = right_edge.max(loc.x + width);
        surface.redraw_needed = true;
        placed.push((surface.output.clone(), loc));
    }
    for crtc in remodeled {
        data.reapply_gamma(crtc);
    }
    for (crtc, on) in vrr {
        set_vrr(data, crtc, on);
    }
    for (output, loc) in placed {
        state.space.map_output(&output, loc);
    }
}

/// Set up newly connected monitors and drop disconnected ones.
fn scan_connectors(state: &mut Pila) {
    let Some(Backend::Udev(data)) = &mut state.backend else {
        return;
    };
    let events = match data.scanner.scan_connectors(data.drm.device()) {
        Ok(result) => result.into_iter().collect::<Vec<_>>(),
        Err(err) => {
            warn!(?err, "failed to scan connectors");
            return;
        }
    };
    for event in events {
        match event {
            DrmScanEvent::Connected {
                connector,
                crtc: Some(crtc),
            } => connector_connected(state, connector, crtc),
            DrmScanEvent::Disconnected {
                crtc: Some(crtc), ..
            } => connector_disconnected(state, crtc),
            _ => {}
        }
    }
}

fn connector_connected(state: &mut Pila, connector: connector::Info, crtc: crtc::Handle) {
    let name = format!(
        "{}-{}",
        connector.interface().as_str(),
        connector.interface_id()
    );
    let output_config = effective_output_config(state, &name);
    let modes = connector.modes().to_vec();
    let Some(drm_mode) = pick_mode(&name, &modes, Some(&output_config)) else {
        warn!(name, "connector has no modes");
        return;
    };
    let scale = output_config.scale.unwrap_or(1.0);
    let transform = output_config.transform.unwrap_or(Transform::Normal);
    let enabled = output_config.enabled.unwrap_or(true);

    // Where the config puts it, else right of the outputs already there.
    let (x, y) = output_config.position.unwrap_or_else(|| {
        let x = state
            .space
            .outputs()
            .filter_map(|o| state.space.output_geometry(o))
            .map(|geo| geo.loc.x + geo.size.w)
            .max()
            .unwrap_or(0);
        (x, 0)
    });

    let Some(Backend::Udev(data)) = &mut state.backend else {
        return;
    };
    if is_non_desktop(&data.drm, &connector) {
        info!(name, "skipping non-desktop connector (e.g. a VR headset)");
        return;
    }

    let (phys_w, phys_h) = connector.size().unwrap_or((0, 0));
    let output = Output::new(
        name.clone(),
        PhysicalProperties {
            size: (phys_w as i32, phys_h as i32).into(),
            subpixel: connector.subpixel().into(),
            // EDID parsing is off (see Cargo.toml).
            make: "Unknown".into(),
            model: "Unknown".into(),
        },
    );
    let wl_mode = WlMode::from(drm_mode);
    output.set_preferred(wl_mode);
    output.change_current_state(
        Some(wl_mode),
        Some(transform),
        Some(OutputScale::Fractional(scale)),
        Some((x, y).into()),
    );

    data.outputs.insert(
        crtc,
        OutputSurface {
            output: output.clone(),
            drm_output: None,
            connector: connector.handle(),
            mode: drm_mode,
            enabled: false,
            global: None,
            frame_pending: false,
            redraw_needed: true,
            lock_frame: LockFrame::None,
            modes,
            gamma: None,
        },
    );
    if !enabled {
        info!(name, "output connected, disabled by its config");
        state.refresh_output_heads();
        return;
    }
    if !enable_output(state, crtc) {
        return;
    }
    info!(
        name,
        mode = ?wl_mode.size,
        refresh_mhz = wl_mode.refresh,
        scale,
        x,
        y,
        "output connected"
    );
    if let (Some(on), Some(Backend::Udev(data))) = (output_config.adaptive_sync, &state.backend) {
        set_vrr(data, crtc, on);
    }
    state.refit_maximized();
    state.configure_lock_surfaces();
    state.refresh_output_heads();
    state.queue_redraw();
}

/// Set a connected output up: its CRTC, its `wl_output` global and its
/// place in the space. `false` if the CRTC couldn't be set up.
fn enable_output(state: &mut Pila, crtc: crtc::Handle) -> bool {
    let Some(Backend::Udev(data)) = &mut state.backend else {
        return false;
    };
    let UdevData {
        outputs,
        drm,
        renderer,
        ..
    } = &mut **data;
    let Some(surface) = outputs.get_mut(&crtc) else {
        return false;
    };
    if surface.enabled {
        return true;
    }
    let name = surface.output.name();
    let planes = drm.device().planes(&crtc).ok();
    match drm.initialize_output::<_, PilaRenderElement<GlesRenderer>>(
        crtc,
        surface.mode,
        &[surface.connector],
        &surface.output,
        planes,
        renderer,
        &DrmOutputRenderElements::default(),
    ) {
        Ok(drm_output) => surface.drm_output = Some(drm_output),
        Err(err) => {
            warn!(name, ?err, "failed to set up output");
            return false;
        }
    }
    surface.enabled = true;
    surface.frame_pending = false;
    surface.redraw_needed = true;
    surface.global = Some(surface.output.create_global::<Pila>(&state.display_handle));
    let output = surface.output.clone();
    let loc = output.current_location();
    data.reapply_gamma(crtc);
    state.space.map_output(&output, loc);
    layer_map_for_output(&output).arrange();
    true
}

/// Take an output out of use: its CRTC is cleared, its global removed and
/// its windows moved elsewhere. It stays known, to be enabled again.
fn disable_output(state: &mut Pila, crtc: crtc::Handle) {
    let Some(Backend::Udev(data)) = &mut state.backend else {
        return;
    };
    let Some(surface) = data.outputs.get_mut(&crtc) else {
        return;
    };
    if !surface.enabled {
        return;
    }
    surface.enabled = false;
    surface.drm_output = None;
    surface.frame_pending = false;
    surface.lock_frame = LockFrame::None;
    let output = surface.output.clone();
    if let Some(global) = surface.global.take() {
        state.display_handle.remove_global::<Pila>(global);
    }
    info!(name = output.name(), "output disabled");
    state.space.unmap_output(&output);
    state.output_removed(&output);
}

/// Turn variable refresh rate on or off where the monitor supports it.
fn set_vrr(data: &UdevData, crtc: crtc::Handle, on: bool) {
    let Some(surface) = data.outputs.get(&crtc) else {
        return;
    };
    let Some(drm_output) = &surface.drm_output else {
        return;
    };
    let result = drm_output.with_compositor(|c| {
        if c.vrr_enabled() == on {
            return Ok(());
        }
        c.use_vrr(on)
    });
    match result {
        Ok(()) => {}
        Err(err) => debug!(
            name = surface.output.name(),
            ?err,
            on,
            "couldn't set adaptive sync"
        ),
    }
}

/// `[[output]]` for `name`, with what a display tool set at runtime on top
/// (`Pila::output_overrides`).
fn effective_output_config(state: &Pila, name: &str) -> OutputConfig {
    let file = state
        .config
        .output(name)
        .cloned()
        .unwrap_or_else(|| OutputConfig::named(name));
    match state.output_overrides.get(name) {
        Some(over) => file.overridden_by(over),
        None => file,
    }
}

fn connector_disconnected(state: &mut Pila, crtc: crtc::Handle) {
    let Some(Backend::Udev(data)) = &mut state.backend else {
        return;
    };
    let Some(surface) = data.outputs.remove(&crtc) else {
        return;
    };
    info!(name = surface.output.name(), "output disconnected");
    if let Some(global) = surface.global {
        state.display_handle.remove_global::<Pila>(global);
    }
    if surface.enabled {
        state.space.unmap_output(&surface.output);
        state.output_removed(&surface.output);
    }
    state.refresh_output_heads();
    state.queue_redraw();
}

/// Whether the connector is marked `non-desktop` (VR headsets and the like).
fn is_non_desktop(drm: &PilaDrmOutputManager, connector: &connector::Info) -> bool {
    let device = drm.device();
    let Ok(props) = device.get_properties(connector.handle()) else {
        return false;
    };
    props.into_iter().any(|(handle, value)| {
        device.get_property(handle).is_ok_and(|info| {
            info.name().to_str() == Ok("non-desktop")
                && info.value_type().convert_value(value).as_boolean() == Some(true)
        })
    })
}

/// The idle callback queued by `UdevData::queue_redraw`.
fn render_all(state: &mut Pila) {
    let Some(Backend::Udev(data)) = &mut state.backend else {
        return;
    };
    data.redraw_scheduled = false;
    let crtcs: Vec<_> = data.outputs.keys().copied().collect();
    for crtc in crtcs {
        render_output(state, crtc);
    }
}

/// Draw one output, unless it has a frame in flight (then it's drawn at the
/// next vblank) or nothing changed.
fn render_output(state: &mut Pila, crtc: crtc::Handle) {
    let Some(Backend::Udev(data)) = &state.backend else {
        return;
    };
    let Some(surface) = data.outputs.get(&crtc) else {
        return;
    };
    if surface.frame_pending
        || !surface.redraw_needed
        || surface.drm_output.is_none()
        || !data.session.is_active()
    {
        return;
    }
    let output = surface.output.clone();
    let Some(output_geo) = state.space.output_geometry(&output) else {
        return;
    };

    // A destroyed cursor surface falls back to the default arrow.
    if matches!(&state.cursor_status, CursorImageStatus::Surface(s) if !s.alive()) {
        state.cursor_status = CursorImageStatus::default_named();
    }
    // Everything taken from `state` is collected before borrowing the backend.
    let pointer = state
        .seat
        .get_pointer()
        .map(|p| p.current_location())
        .unwrap_or_default();
    let pointer_here = output_geo.to_f64().contains(pointer);
    let cursor_surface = cursor::surface_position(&state.cursor_status, pointer);
    let named_cursor = match &state.cursor_status {
        CursorImageStatus::Named(icon) => Some(*icon),
        _ => None,
    };
    let scene = state.scene();
    let lock_pending = state.lock_pending();
    let scale = crate::scale::output_scale(&output);

    let Some(Backend::Udev(data)) = &mut state.backend else {
        return;
    };
    let UdevData {
        renderer,
        outputs,
        cursor_theme,
        ..
    } = &mut **data;
    let surface = outputs.get_mut(&crtc).expect("checked above");
    let drm_output = surface.drm_output.as_mut().expect("checked above");
    surface.redraw_needed = false;

    let mut elements = Vec::new();
    if pointer_here {
        elements.extend(render::cursor_elements(
            renderer,
            cursor_surface.as_ref(),
            output_geo.loc,
            scale,
        ));
        if let Some(icon) = named_cursor {
            let cursor = cursor_theme.get(icon, crate::scale::buffer_scale(scale));
            elements.extend(render::named_cursor_elements(
                renderer,
                &cursor,
                pointer,
                output_geo.loc,
                scale,
            ));
        }
    }
    elements.extend(render::scene_elements(
        renderer,
        &scene,
        output_geo.loc,
        scale,
    ));

    let rendered = drm_output.render_frame(
        renderer,
        &elements,
        render::clear_color(&scene),
        FrameFlags::DEFAULT,
    );
    let cursor_wl_surface = cursor_surface.as_ref().map(|(s, _)| s);
    match rendered {
        Ok(result) => {
            frame::update_primary_outputs(&scene, cursor_wl_surface, &output, &result.states);
            if !result.is_empty {
                let feedback = frame::presentation_feedback(
                    &scene,
                    cursor_wl_surface,
                    &output,
                    &result.states,
                );
                match drm_output.queue_frame(Some(feedback)) {
                    Ok(()) => {
                        surface.frame_pending = true;
                        if scene.locked {
                            surface.lock_frame = LockFrame::Queued;
                        }
                    }
                    Err(err) => warn!(?err, "failed to queue frame"),
                }
            } else if scene.locked {
                // Nothing changed: the screen already shows the locked scene.
                surface.lock_frame = LockFrame::Shown;
            }
        }
        Err(err) => warn!(?err, "failed to render frame"),
    }
    if !scene.locked {
        surface.lock_frame = LockFrame::None;
    }

    if lock_pending {
        maybe_confirm_lock(state);
    }
    frame::send_frames(&scene, cursor_wl_surface, &output, state.clock.now().into());
    state.post_render(&output);
}

/// A queued frame reached the screen: send presentation feedback, and draw
/// again if something changed meanwhile.
fn frame_finish(state: &mut Pila, crtc: crtc::Handle, metadata: &mut Option<DrmEventMetadata>) {
    let now = state.clock.now();
    let Some(Backend::Udev(data)) = &mut state.backend else {
        return;
    };
    let Some(surface) = data.outputs.get_mut(&crtc) else {
        return;
    };
    surface.frame_pending = false;
    if surface.lock_frame == LockFrame::Queued {
        surface.lock_frame = LockFrame::Shown;
    }

    let frame_duration = surface
        .output
        .current_mode()
        .map(|mode| Duration::from_secs_f64(1000.0 / mode.refresh as f64))
        .unwrap_or(Duration::from_millis(16));
    // The kernel's vblank timestamp, when it gives a monotonic one.
    let hw_time = metadata.as_ref().and_then(|m| match m.time {
        DrmEventTime::Monotonic(time) if !time.is_zero() => Some(time),
        _ => None,
    });
    let seq = metadata.as_ref().map_or(0, |m| m.sequence as u64);
    let (time, flags) = match hw_time {
        Some(time) => (
            time.into(),
            wp_presentation_feedback::Kind::Vsync
                | wp_presentation_feedback::Kind::HwClock
                | wp_presentation_feedback::Kind::HwCompletion,
        ),
        None => (now, wp_presentation_feedback::Kind::Vsync),
    };
    // Powered off meanwhile: the vblank belongs to the frame that was
    // dropped with the surface.
    let Some(drm_output) = &mut surface.drm_output else {
        return;
    };
    match drm_output.frame_submitted() {
        Ok(Some(Some(mut feedback))) => {
            feedback.presented(time, Refresh::fixed(frame_duration), seq, flags);
        }
        Ok(_) => {}
        Err(err) => warn!(?err, "frame submission failed"),
    }

    if state.lock_pending() {
        maybe_confirm_lock(state);
    }
    render_output(state, crtc);
}

/// Confirm a pending session lock once every output shows a locked frame.
fn maybe_confirm_lock(state: &mut Pila) {
    let Some(Backend::Udev(data)) = &state.backend else {
        return;
    };
    // A powered-off output shows nothing, locked or not.
    if data
        .outputs
        .values()
        .filter(|surface| surface.drm_output.is_some())
        .all(|surface| surface.lock_frame == LockFrame::Shown)
    {
        state.confirm_lock();
    }
}
