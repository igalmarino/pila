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
        drm::control::{connector, crtc, Device as _, ModeTypeFlags},
        input::{self as libinput, Libinput},
        rustix::fs::OFlags,
        wayland_protocols::wp::presentation_time::server::wp_presentation_feedback,
        wayland_server::backend::GlobalId,
    },
    utils::{DeviceFd, IsAlive},
    wayland::{dmabuf::DmabufFeedbackBuilder, presentation::Refresh},
};
use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};
use tracing::{debug, error, info, warn};

use crate::{
    backend::Backend,
    cursor::{self, CursorTheme},
    frame,
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
    cursor_theme: CursorTheme,
    /// A render pass is already queued for this loop iteration.
    redraw_scheduled: bool,
    loop_handle: LoopHandle<'static, Pila>,
    /// Last: see the struct docs.
    pub session: LibSeatSession,
}

struct OutputSurface {
    output: Output,
    drm_output: PilaDrmOutput,
    global: GlobalId,
    /// A frame was queued and its vblank hasn't arrived yet.
    frame_pending: bool,
    /// Something changed since the last frame.
    redraw_needed: bool,
    /// Progress of the frame that confirms a session lock.
    lock_frame: LockFrame,
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
                    for surface in data.outputs.values_mut() {
                        surface.frame_pending = false;
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
/// tap-to-click on touchpads.
fn on_input_device(state: &mut Pila, event: &mut InputEvent<LibinputInputBackend>) {
    let Some(Backend::Udev(data)) = &mut state.backend else {
        return;
    };
    match event {
        InputEvent::DeviceAdded { device } => {
            if device.has_capability(libinput::DeviceCapability::Keyboard) {
                if let Some(keyboard) = state.seat.get_keyboard() {
                    device.led_update(keyboard.led_state().into());
                }
                data.keyboards.push(device.clone());
            }
            if device.config_tap_finger_count() > 0 {
                let _ = device.config_tap_set_enabled(true);
            }
        }
        InputEvent::DeviceRemoved { device } => {
            data.keyboards.retain(|keyboard| keyboard != device);
        }
        _ => {}
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
    let output_config = state.config.output(&name).cloned();
    let modes = connector.modes();
    let preferred = modes
        .iter()
        .position(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED));
    let configured = output_config
        .as_ref()
        .and_then(|c| c.mode)
        .and_then(|spec| {
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
    let Some(drm_mode) = configured
        .or(preferred)
        .and_then(|i| modes.get(i))
        .or_else(|| modes.first())
        .copied()
    else {
        warn!(name, "connector has no modes");
        return;
    };
    let scale = output_config.as_ref().and_then(|c| c.scale).unwrap_or(1.0);

    // Where the config puts it, else right of the outputs already there.
    let (x, y) = output_config
        .as_ref()
        .and_then(|c| c.position)
        .unwrap_or_else(|| {
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
        None,
        Some(OutputScale::Fractional(scale)),
        Some((x, y).into()),
    );

    let planes = data.drm.device().planes(&crtc).ok();
    let drm_output = match data
        .drm
        .initialize_output::<_, PilaRenderElement<GlesRenderer>>(
            crtc,
            drm_mode,
            &[connector.handle()],
            &output,
            planes,
            &mut data.renderer,
            &DrmOutputRenderElements::default(),
        ) {
        Ok(drm_output) => drm_output,
        Err(err) => {
            warn!(name, ?err, "failed to set up output");
            return;
        }
    };
    let global = output.create_global::<Pila>(&state.display_handle);
    info!(
        name,
        mode = ?wl_mode.size,
        refresh_mhz = wl_mode.refresh,
        scale,
        x,
        y,
        "output connected"
    );
    data.outputs.insert(
        crtc,
        OutputSurface {
            output: output.clone(),
            drm_output,
            global,
            frame_pending: false,
            redraw_needed: true,
            lock_frame: LockFrame::None,
        },
    );

    state.space.map_output(&output, (x, y));
    layer_map_for_output(&output).arrange();
    state.refit_maximized();
    state.configure_lock_surfaces();
    state.queue_redraw();
}

fn connector_disconnected(state: &mut Pila, crtc: crtc::Handle) {
    let Some(Backend::Udev(data)) = &mut state.backend else {
        return;
    };
    let Some(surface) = data.outputs.remove(&crtc) else {
        return;
    };
    info!(name = surface.output.name(), "output disconnected");
    state.display_handle.remove_global::<Pila>(surface.global);
    state.space.unmap_output(&surface.output);
    state.output_removed(&surface.output);
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
    if surface.frame_pending || !surface.redraw_needed || !data.session.is_active() {
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

    let rendered = surface.drm_output.render_frame(
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
                match surface.drm_output.queue_frame(Some(feedback)) {
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
    match surface.drm_output.frame_submitted() {
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
    if data
        .outputs
        .values()
        .all(|surface| surface.lock_frame == LockFrame::Shown)
    {
        state.confirm_lock();
    }
}
