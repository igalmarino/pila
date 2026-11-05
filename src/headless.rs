//! Headless backend, for automated tests (`--backend headless`): virtual
//! outputs rendered offscreen on the GPU (a render node, or Mesa's software
//! device), input injected through the control socket (`test_control.rs`)
//! as `TestInput` events, which go through `Pila::process_input_event` like
//! real input. Frames are paced by a timer at each output's refresh rate,
//! since there is no vblank; only while a redraw was asked for.

use std::{
    os::unix::fs::MetadataExt,
    path::PathBuf,
    time::{Duration, Instant},
};

use smithay::backend::drm::DrmDeviceFd;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::DeviceFd;
use smithay::wayland::drm_syncobj::{supports_syncobj_eventfd, DrmSyncobjState};
use smithay::{
    backend::{
        allocator::Fourcc,
        egl::{EGLContext, EGLDevice, EGLDisplay},
        input::{
            AxisRelativeDirection, AxisSource, ButtonState, Device, DeviceCapability, Event,
            GestureBeginEvent, GestureEndEvent, GestureHoldBeginEvent, GestureHoldEndEvent,
            GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
            GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, InputBackend,
            KeyState, KeyboardKeyEvent, Keycode, PointerAxisEvent, PointerButtonEvent,
            PointerMotionEvent, UnusedEvent,
        },
        renderer::{
            damage::OutputDamageTracker,
            gles::{GlesRenderer, GlesTexture},
            Bind, ExportMem, ImportDma, Offscreen, TextureMapping,
        },
    },
    desktop::layer_map_for_output,
    input::pointer::CursorImageStatus,
    output::{Mode, Output, PhysicalProperties, Scale as OutputScale, Subpixel},
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            LoopHandle, RegistrationToken,
        },
        wayland_protocols::wp::presentation_time::server::wp_presentation_feedback,
        wayland_server::backend::GlobalId,
    },
    utils::{Physical, Point, Rectangle, Size, Transform},
    wayland::{dmabuf::DmabufFeedbackBuilder, presentation::Refresh},
};
use tracing::{error, info, warn};

use crate::{
    backend::Backend,
    config::{ModeSpec, OutputConfig},
    frame,
    output_management::{HeadInfo, ModeInfo},
    render, Pila,
};

/// The virtual outputs when `--headless-outputs` doesn't name any.
pub const DEFAULT_OUTPUTS: &str = "HEADLESS-1:1280x800@60";

/// Extra modes every virtual output offers besides its own, so that mode
/// changes (config, wlr-output-management) can be tested.
const EXTRA_MODES: [(i32, i32, i32); 2] = [(1024, 768, 60_000), (1920, 1080, 60_000)];

/// A virtual output as `--headless-outputs` gives it: `NAME:WxH[@HZ]`.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputSpec {
    pub name: String,
    pub size: (i32, i32),
    pub refresh_mhz: i32,
}

/// Parse `NAME:WxH[@HZ]`, several separated by commas.
pub fn parse_output_specs(text: &str) -> Result<Vec<OutputSpec>, String> {
    text.split(',')
        .filter(|s| !s.trim().is_empty())
        .map(|spec| parse_output_spec(spec.trim()))
        .collect()
}

pub fn parse_output_spec(spec: &str) -> Result<OutputSpec, String> {
    let (name, mode) = spec
        .split_once(':')
        .ok_or_else(|| format!("output {spec:?}: expected NAME:WxH[@HZ]"))?;
    if name.is_empty() {
        return Err(format!("output {spec:?}: empty name"));
    }
    let (size, hz) = match mode.split_once('@') {
        Some((size, hz)) => (size, Some(hz)),
        None => (mode, None),
    };
    let (w, h) = size
        .split_once('x')
        .and_then(|(w, h)| Some((w.parse::<i32>().ok()?, h.parse::<i32>().ok()?)))
        .filter(|(w, h)| *w > 0 && *h > 0)
        .ok_or_else(|| format!("output {spec:?}: bad size {size:?}"))?;
    let refresh_mhz = match hz {
        None => 60_000,
        Some(hz) => hz
            .parse::<f64>()
            .ok()
            .filter(|hz| *hz > 0.0)
            .map(|hz| (hz * 1000.0).round() as i32)
            .ok_or_else(|| format!("output {spec:?}: bad refresh {hz:?}"))?,
    };
    Ok(OutputSpec {
        name: name.to_string(),
        size: (w, h),
        refresh_mhz,
    })
}

pub struct HeadlessOutput {
    pub output: Output,
    pub modes: Vec<Mode>,
    pub enabled: bool,
    global: Option<GlobalId>,
    /// What the output shows: the last frame drawn.
    texture: Option<GlesTexture>,
    damage_tracker: OutputDamageTracker,
    /// Something may have changed since the last frame.
    redraw_needed: bool,
    /// The texture holds a frame (else the next one is drawn whole).
    drawn: bool,
    last_frame: Option<Instant>,
    /// Frames drawn, for tests (`frames` in the state).
    pub frames: u64,
    pub power: bool,
    /// The gamma ramps set (red, green, blue concatenated); `None` linear.
    pub gamma: Option<Vec<u16>>,
}

pub struct HeadlessData {
    pub renderer: GlesRenderer,
    pub outputs: Vec<HeadlessOutput>,
    loop_handle: LoopHandle<'static, Pila>,
    /// The timer of the next frame, while one is scheduled.
    timer: Option<RegistrationToken>,
}

/// Gamma ramps of virtual outputs are this long.
pub const GAMMA_SIZE: u32 = 256;

impl HeadlessData {
    pub fn queue_redraw(&mut self) {
        for o in &mut self.outputs {
            o.redraw_needed = true;
        }
        if self.timer.is_some() {
            return;
        }
        // The next frame is due one period after the last one; at once if
        // that's already past.
        let delay = self
            .outputs
            .iter()
            .filter_map(|o| {
                let last = o.last_frame?;
                let period = crate::frame_stats::refresh_period(&o.output);
                Some(period.saturating_sub(last.elapsed()))
            })
            .min()
            .unwrap_or(Duration::ZERO);
        let token = self
            .loop_handle
            .insert_source(Timer::from_duration(delay), |_, _, state| {
                render_all(state);
                TimeoutAction::Drop
            });
        match token {
            Ok(token) => self.timer = Some(token),
            Err(err) => error!(?err, "failed to schedule a frame"),
        }
    }

    pub fn output_heads(&self) -> Vec<HeadInfo> {
        self.outputs
            .iter()
            .map(|o| {
                let current = o.output.current_mode();
                let loc = o.output.current_location();
                HeadInfo {
                    name: o.output.name(),
                    make: "pila".into(),
                    model: "headless".into(),
                    serial: None,
                    physical: (0, 0),
                    modes: o
                        .modes
                        .iter()
                        .enumerate()
                        .map(|(i, m)| ModeInfo {
                            size: (m.size.w, m.size.h),
                            refresh_mhz: m.refresh,
                            preferred: i == 0,
                        })
                        .collect(),
                    current_mode: current.and_then(|c| o.modes.iter().position(|m| *m == c)),
                    enabled: o.enabled,
                    position: (loc.x, loc.y),
                    transform: o.output.current_transform(),
                    scale: o.output.current_scale().fractional_scale(),
                    adaptive_sync: Some(false),
                }
            })
            .collect()
    }

    fn find(&self, output: &Output) -> Option<&HeadlessOutput> {
        self.outputs.iter().find(|o| o.output == *output)
    }

    fn find_mut(&mut self, output: &Output) -> Option<&mut HeadlessOutput> {
        self.outputs.iter_mut().find(|o| o.output == *output)
    }

    pub fn gamma_size(&self, output: &Output) -> Option<u32> {
        self.find(output).map(|_| GAMMA_SIZE)
    }

    pub fn set_gamma(&mut self, output: &Output, ramp: Option<&[u16]>) -> bool {
        match self.find_mut(output) {
            Some(o) => {
                o.gamma = ramp.map(<[u16]>::to_vec);
                true
            }
            None => false,
        }
    }

    pub fn reset_gamma(&mut self) {
        for o in &mut self.outputs {
            o.gamma = None;
        }
    }

    pub fn output_power(&self, output: &Output) -> Option<bool> {
        self.find(output).map(|o| o.power)
    }

    pub fn set_output_power(&mut self, output: &Output, on: bool) -> bool {
        match self.find_mut(output) {
            Some(o) => {
                o.power = on;
                o.drawn = false;
                true
            }
            None => false,
        }
    }

    /// The pixels of `rect` (physical, in `output`'s buffer) as RGBA rows,
    /// top row first, from the last frame drawn.
    pub fn read_pixels(
        &mut self,
        output: &Output,
        rect: Rectangle<i32, Physical>,
    ) -> Result<Vec<u8>, String> {
        let HeadlessData {
            renderer, outputs, ..
        } = self;
        let o = outputs
            .iter_mut()
            .find(|o| o.output == *output)
            .ok_or("no such output")?;
        let texture = o.texture.as_mut().ok_or("nothing drawn yet")?;
        let size = texture_size(texture);
        let full: Rectangle<i32, Physical> = Rectangle::from_size((size.w, size.h).into());
        if !full.contains_rect(rect) {
            return Err(format!("{rect:?} is outside the output ({size:?})"));
        }
        let target = renderer.bind(texture).map_err(|e| format!("{e:?}"))?;
        // The whole frame, read back as captures do (`capture::copy_to_shm`):
        // rows bottom-up unless the mapping says they're flipped already.
        // Then the rectangle is cut out of it.
        let full_region = Rectangle::from_size((size.w, size.h).into());
        let mapping = renderer
            .copy_framebuffer(&target, full_region, Fourcc::Abgr8888)
            .map_err(|e| format!("{e:?}"))?;
        drop(target);
        let bottom_up = !mapping.flipped();
        let pixels = renderer
            .map_texture(&mapping)
            .map_err(|e| format!("{e:?}"))?;
        let stride = size.w as usize * 4;
        if pixels.len() < stride * size.h as usize {
            return Err("short read-back".into());
        }
        let row = rect.size.w as usize * 4;
        let mut out = Vec::with_capacity(row * rect.size.h as usize);
        for y in rect.loc.y..rect.loc.y + rect.size.h {
            let src_y = if bottom_up { size.h - 1 - y } else { y } as usize;
            let start = src_y * stride + rect.loc.x as usize * 4;
            out.extend_from_slice(&pixels[start..start + row]);
        }
        Ok(out)
    }
}

fn texture_size(texture: &GlesTexture) -> Size<i32, smithay::utils::Buffer> {
    use smithay::backend::renderer::Texture;
    texture.size()
}

/// The renderer, and the render node it runs on (path, device number).
type Renderer = (GlesRenderer, Option<(PathBuf, u64)>);

/// The renderer: on a render node if there is one, else on Mesa's software
/// device (llvmpipe, e.g. in CI). `PILA_HEADLESS_DEVICE=software` picks the
/// software device even where there's a GPU (to test what CI runs).
fn create_renderer() -> Result<Renderer, Box<dyn std::error::Error>> {
    let mut devices: Vec<EGLDevice> = EGLDevice::enumerate()?.collect();
    if std::env::var("PILA_HEADLESS_DEVICE").as_deref() == Ok("software") {
        devices.retain(EGLDevice::is_software);
    }
    // Hardware first.
    devices.sort_by_key(EGLDevice::is_software);
    let mut last_err: Option<Box<dyn std::error::Error>> = None;
    for device in devices {
        let node = device
            .render_device_path()
            .ok()
            .and_then(|path| Some((path.clone(), std::fs::metadata(&path).ok()?.rdev())));
        let software = device.is_software();
        if !software && node.is_none() {
            continue;
        }
        // SAFETY: the display is only used from this thread, by the renderer.
        let result = unsafe { EGLDisplay::new(device) }
            .and_then(|display| EGLContext::new(&display))
            .map_err(Box::<dyn std::error::Error>::from)
            .and_then(|context| {
                // SAFETY: the context was just created for this renderer alone.
                unsafe { GlesRenderer::new(context) }.map_err(Into::into)
            });
        match result {
            Ok(renderer) => {
                info!(
                    software,
                    device = ?node.as_ref().map(|(p, _)| p.display().to_string()),
                    "headless renderer"
                );
                return Ok((renderer, node));
            }
            Err(err) => last_err = Some(err),
        }
    }
    Err(last_err.unwrap_or_else(|| "no EGL device".into()))
}

pub fn init_headless(
    state: &mut Pila,
    specs: &[OutputSpec],
) -> Result<(), Box<dyn std::error::Error>> {
    let (mut renderer, node) = create_renderer()?;
    render::warm_up(&mut renderer);

    let formats = renderer.dmabuf_formats();
    let feedback = node.as_ref().and_then(|(_, dev)| {
        DmabufFeedbackBuilder::new(*dev, formats.clone())
            .build()
            .ok()
    });
    match feedback {
        Some(feedback) => {
            state
                .dmabuf_state
                .create_global_with_default_feedback::<Pila>(&state.display_handle, &feedback);
        }
        None => {
            state
                .dmabuf_state
                .create_global::<Pila>(&state.display_handle, formats);
        }
    }
    if let Some((path, _)) = &node {
        let device = std::fs::File::options().read(true).write(true).open(path);
        if let Ok(device) = device {
            let fd = DrmDeviceFd::new(DeviceFd::from(std::os::fd::OwnedFd::from(device)));
            if supports_syncobj_eventfd(&fd) {
                state.syncobj_state = Some(DrmSyncobjState::new::<Pila>(&state.display_handle, fd));
            }
        }
    }
    if let Some((_, dev)) = node {
        state.capture_dmabuf = Some(crate::capture::DmabufTarget::new(dev, &renderer));
    }

    state.backend = Some(Backend::Headless(Box::new(HeadlessData {
        renderer,
        outputs: Vec::new(),
        loop_handle: state.loop_handle.clone(),
        timer: None,
    })));
    for spec in specs {
        add_output(state, spec);
    }
    Ok(())
}

/// Plug a virtual output in, placed and set up by its `[[output]]` config.
pub fn add_output(state: &mut Pila, spec: &OutputSpec) -> bool {
    let Some(Backend::Headless(data)) = &mut state.backend else {
        return false;
    };
    if data.outputs.iter().any(|o| o.output.name() == spec.name) {
        warn!(name = spec.name, "an output with that name exists");
        return false;
    }
    let output = Output::new(
        spec.name.clone(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "pila".into(),
            model: "headless".into(),
        },
    );
    let own = Mode {
        size: spec.size.into(),
        refresh: spec.refresh_mhz,
    };
    let mut modes = vec![own];
    for (w, h, refresh) in EXTRA_MODES {
        let mode = Mode {
            size: (w, h).into(),
            refresh,
        };
        if !modes.contains(&mode) {
            modes.push(mode);
        }
    }
    output.set_preferred(own);
    output.change_current_state(Some(own), Some(Transform::Normal), None, None);
    data.outputs.push(HeadlessOutput {
        damage_tracker: OutputDamageTracker::from_output(&output),
        output,
        modes,
        enabled: false,
        global: None,
        texture: None,
        redraw_needed: true,
        drawn: false,
        last_frame: None,
        frames: 0,
        power: true,
        gamma: None,
    });
    info!(name = spec.name, size = ?spec.size, refresh_mhz = spec.refresh_mhz, "output connected");
    apply_output_config(state);
    state.refit_maximized();
    state.configure_lock_surfaces();
    state.refresh_output_heads();
    state.refresh_workspaces();
    state.queue_redraw();
    true
}

/// Unplug a virtual output.
pub fn remove_output(state: &mut Pila, name: &str) -> bool {
    let Some(Backend::Headless(data)) = &mut state.backend else {
        return false;
    };
    let Some(index) = data.outputs.iter().position(|o| o.output.name() == name) else {
        return false;
    };
    let removed = data.outputs.remove(index);
    if let Some(global) = removed.global {
        state.display_handle.remove_global::<Pila>(global);
    }
    info!(name, "output disconnected");
    if removed.enabled {
        state.space.unmap_output(&removed.output);
        state.output_removed(&removed.output);
    }
    state.refresh_output_heads();
    state.refresh_workspaces();
    state.queue_redraw();
    true
}

/// The `[[output]]` config of `name` with wlr-output-management's overrides
/// on top.
fn effective_output_config(state: &Pila, name: &str) -> OutputConfig {
    let mut config = state
        .config
        .output(name)
        .cloned()
        .unwrap_or_else(|| OutputConfig::named(name));
    if let Some(over) = state.output_overrides.get(name) {
        config.mode = over.mode.or(config.mode);
        config.scale = over.scale.or(config.scale);
        config.position = over.position.or(config.position);
        config.transform = over.transform.or(config.transform);
        config.enabled = over.enabled.or(config.enabled);
        config.adaptive_sync = over.adaptive_sync.or(config.adaptive_sync);
    }
    config
}

fn pick_mode(modes: &[Mode], spec: Option<ModeSpec>) -> Mode {
    let Some(spec) = spec else {
        return modes[0];
    };
    modes
        .iter()
        .filter(|m| (m.size.w, m.size.h) == (spec.width, spec.height))
        .min_by_key(|m| match spec.refresh_hz {
            Some(hz) => (m.refresh - (hz * 1000.0).round() as i32).abs(),
            None => -m.refresh,
        })
        .copied()
        .unwrap_or(modes[0])
}

/// Apply the config (and overrides) to every virtual output: enabled, mode,
/// scale, transform and position (else right of the previous ones). Never
/// leaves every output off.
pub fn apply_output_config(state: &mut Pila) {
    let Some(Backend::Headless(data)) = &state.backend else {
        return;
    };
    let configs: Vec<OutputConfig> = data
        .outputs
        .iter()
        .map(|o| effective_output_config(state, &o.output.name()))
        .collect();
    let any_on = configs.iter().any(|c| c.enabled.unwrap_or(true));
    let display_handle = state.display_handle.clone();
    let Some(Backend::Headless(data)) = &mut state.backend else {
        return;
    };
    let mut right_edge = 0;
    let mut placed = Vec::new();
    let mut disabled = Vec::new();
    for (i, (o, config)) in data.outputs.iter_mut().zip(&configs).enumerate() {
        // Never zero outputs: the first stays on.
        let enabled = config.enabled.unwrap_or(true) || (!any_on && i == 0);
        let mode = pick_mode(&o.modes, config.mode);
        let scale = config.scale.unwrap_or(1.0);
        let transform = config.transform.unwrap_or(Transform::Normal);
        let loc: Point<i32, smithay::utils::Logical> =
            config.position.unwrap_or((right_edge, 0)).into();
        if o.output.current_mode() != Some(mode) {
            info!(name = o.output.name(), mode = ?mode.size, refresh_mhz = mode.refresh, "output mode changed");
            o.texture = None;
            o.drawn = false;
        }
        o.output.change_current_state(
            Some(mode),
            Some(transform),
            Some(OutputScale::Fractional(scale)),
            Some(loc),
        );
        o.redraw_needed = true;
        if !enabled {
            if o.enabled {
                o.enabled = false;
                o.drawn = false;
                if let Some(global) = o.global.take() {
                    display_handle.remove_global::<Pila>(global);
                }
                info!(name = o.output.name(), "output disabled");
                disabled.push(o.output.clone());
            }
            continue;
        }
        if !o.enabled {
            o.enabled = true;
            o.global = Some(o.output.create_global::<Pila>(&display_handle));
            info!(name = o.output.name(), "output enabled");
        }
        let size = o
            .output
            .current_mode()
            .map(|m| {
                transform
                    .transform_size(m.size)
                    .to_f64()
                    .to_logical(scale)
                    .to_i32_round::<i32>()
            })
            .unwrap_or_default();
        right_edge = right_edge.max(loc.x + size.w);
        placed.push((o.output.clone(), loc));
    }
    for (output, loc) in placed {
        state.space.map_output(&output, loc);
        layer_map_for_output(&output).arrange();
    }
    for output in disabled {
        state.space.unmap_output(&output);
        state.output_removed(&output);
    }
}

fn render_all(state: &mut Pila) {
    let Some(Backend::Headless(data)) = &mut state.backend else {
        return;
    };
    data.timer = None;
    let lock_pending = state.lock_pending();
    let count = match &state.backend {
        Some(Backend::Headless(data)) => data.outputs.len(),
        _ => 0,
    };
    let mut locked_everywhere = true;
    for index in 0..count {
        if !render_output(state, index, lock_pending) {
            locked_everywhere = false;
        }
    }
    if lock_pending {
        if locked_everywhere {
            state.confirm_lock();
        } else {
            state.queue_redraw();
        }
    }
}

/// Draw one output if something changed. Returns whether the output now
/// shows the scene (for a pending lock: a locked frame, or nothing to show).
fn render_output(state: &mut Pila, index: usize, lock_pending: bool) -> bool {
    let Some(Backend::Headless(data)) = &state.backend else {
        return false;
    };
    let Some(o) = data.outputs.get(index) else {
        return false;
    };
    // An output that is off or asleep shows nothing, locked or not.
    if !o.enabled || !o.power {
        return true;
    }
    if !o.redraw_needed && !lock_pending {
        return true;
    }
    let output = o.output.clone();
    let Some(output_geo) = state.space.output_geometry(&output) else {
        return true;
    };

    let started = Instant::now();
    // A destroyed cursor surface falls back to the default arrow.
    if matches!(&state.cursor_status, CursorImageStatus::Surface(s) if !s.is_alive()) {
        state.cursor_status = CursorImageStatus::default_named();
    }
    state.signal_commit_timers(&output);
    let input = state.frame_input();
    let scene = &input.scene;
    let scale = crate::scale::output_scale(&output);
    let pointer_here = output_geo.to_f64().contains(input.pointer);

    let Pila { backend, blur, .. } = state;
    let Some(Backend::Headless(data)) = backend else {
        return false;
    };
    let HeadlessData {
        renderer, outputs, ..
    } = &mut **data;
    let o = &mut outputs[index];
    o.redraw_needed = false;

    let Some(mode) = output.current_mode() else {
        return false;
    };
    if o.texture.is_none() {
        match Offscreen::<GlesTexture>::create_buffer(
            renderer,
            Fourcc::Abgr8888,
            (mode.size.w, mode.size.h).into(),
        ) {
            Ok(texture) => {
                o.texture = Some(texture);
                o.drawn = false;
            }
            Err(err) => {
                error!(?err, "failed to create the output's texture");
                return false;
            }
        }
    }
    let texture = o.texture.as_mut().expect("created above");

    // Named cursors are left out, as nested (the host would draw them).
    let mut elements = Vec::new();
    if pointer_here {
        elements = render::pointer_elements(renderer, &input, None, output_geo.loc, scale);
    }
    elements.extend(render::output_elements(
        renderer,
        blur,
        scene,
        &output.name(),
        output_geo,
        output_geo.loc,
        scale,
    ));
    // The texture keeps the last frame: age 1, or 0 to draw it whole. A
    // frame confirming a lock keeps nothing of the unlocked screen.
    let age = if o.drawn && !lock_pending { 1 } else { 0 };
    let result = match renderer.bind(texture) {
        Ok(mut target) => o
            .damage_tracker
            .render_output(
                renderer,
                &mut target,
                age,
                &elements,
                render::clear_color(scene),
            )
            .map(|r| {
                let _ = r.sync.wait();
                (r.damage.is_some(), r.states)
            })
            .map_err(|err| error!(?err, "render failed"))
            .ok(),
        Err(err) => {
            error!(?err, "failed to bind the output's texture");
            None
        }
    };
    let Some((damaged, states)) = result else {
        o.redraw_needed = true;
        return false;
    };
    o.drawn = true;
    let now = Instant::now();
    let period = crate::frame_stats::refresh_period(&output);
    if damaged {
        o.frames += 1;
        o.last_frame = Some(now);
    }
    let cursor_surface = input.cursor_wl_surface();
    frame::update_primary_outputs(scene, cursor_surface, &output, &states);
    if damaged {
        frame::presentation_feedback(scene, cursor_surface, &output, &states).presented(
            state.clock.now(),
            Refresh::fixed(period),
            0,
            wp_presentation_feedback::Kind::Vsync,
        );
        state.record_frame(&output, started.elapsed());
    }
    frame::send_frames(scene, cursor_surface, &output, state.clock.now().into());
    state.post_render(&output);
    scene.locked
}

// --- Injected input ---------------------------------------------------------

/// Input injected through the control socket.
#[derive(Debug)]
pub struct TestInput;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TestDevice;

impl Device for TestDevice {
    fn id(&self) -> String {
        "pila-test".into()
    }
    fn name(&self) -> String {
        "pila test input".into()
    }
    fn has_capability(&self, capability: DeviceCapability) -> bool {
        matches!(
            capability,
            DeviceCapability::Keyboard | DeviceCapability::Pointer | DeviceCapability::Gesture
        )
    }
    fn usb_id(&self) -> Option<(u32, u32)> {
        None
    }
    fn syspath(&self) -> Option<PathBuf> {
        None
    }
}

/// Microseconds since pila started, as input event times.
pub fn event_time(state: &Pila) -> u64 {
    state.start_time.elapsed().as_micros() as u64
}

macro_rules! test_event {
    ($ty:ident) => {
        impl Event<TestInput> for $ty {
            fn time(&self) -> u64 {
                self.time
            }
            fn device(&self) -> TestDevice {
                TestDevice
            }
        }
    };
}

pub struct KeyEvent {
    pub time: u64,
    pub key: Keycode,
    pub state: KeyState,
}
test_event!(KeyEvent);
impl KeyboardKeyEvent<TestInput> for KeyEvent {
    fn key_code(&self) -> Keycode {
        self.key
    }
    fn state(&self) -> KeyState {
        self.state
    }
    fn count(&self) -> u32 {
        u32::from(self.state == KeyState::Pressed)
    }
}

pub struct MotionEvent {
    pub time: u64,
    pub delta: (f64, f64),
}
test_event!(MotionEvent);
impl PointerMotionEvent<TestInput> for MotionEvent {
    fn delta_x(&self) -> f64 {
        self.delta.0
    }
    fn delta_y(&self) -> f64 {
        self.delta.1
    }
    fn delta_x_unaccel(&self) -> f64 {
        self.delta.0
    }
    fn delta_y_unaccel(&self) -> f64 {
        self.delta.1
    }
}

pub struct ButtonEvent {
    pub time: u64,
    /// Linux input event code (BTN_LEFT = 0x110).
    pub button: u32,
    pub state: ButtonState,
}
test_event!(ButtonEvent);
impl PointerButtonEvent<TestInput> for ButtonEvent {
    fn button_code(&self) -> u32 {
        self.button
    }
    fn state(&self) -> ButtonState {
        self.state
    }
}

pub struct AxisEvent {
    pub time: u64,
    pub source: AxisSource,
    /// Horizontal, vertical: steps (v120 / 120) for a wheel, else pixels.
    pub amount: (f64, f64),
}
test_event!(AxisEvent);
impl PointerAxisEvent<TestInput> for AxisEvent {
    fn amount(&self, axis: smithay::backend::input::Axis) -> Option<f64> {
        let value = match axis {
            smithay::backend::input::Axis::Horizontal => self.amount.0,
            smithay::backend::input::Axis::Vertical => self.amount.1,
        };
        match self.source {
            AxisSource::Wheel | AxisSource::WheelTilt => Some(value * 15.0),
            _ => Some(value),
        }
    }
    fn amount_v120(&self, axis: smithay::backend::input::Axis) -> Option<f64> {
        let value = match axis {
            smithay::backend::input::Axis::Horizontal => self.amount.0,
            smithay::backend::input::Axis::Vertical => self.amount.1,
        };
        match self.source {
            AxisSource::Wheel | AxisSource::WheelTilt => Some(value * 120.0),
            _ => None,
        }
    }
    fn source(&self) -> AxisSource {
        self.source
    }
    fn relative_direction(&self, _: smithay::backend::input::Axis) -> AxisRelativeDirection {
        AxisRelativeDirection::Identical
    }
}

/// Every touchpad gesture event: begin (fingers), update (delta, scale,
/// rotation) and end (cancelled).
#[derive(Default)]
pub struct GestureEvent {
    pub time: u64,
    pub fingers: u32,
    pub delta: (f64, f64),
    pub scale: f64,
    pub rotation: f64,
    pub cancelled: bool,
}
test_event!(GestureEvent);
impl GestureBeginEvent<TestInput> for GestureEvent {
    fn fingers(&self) -> u32 {
        self.fingers
    }
}
impl GestureEndEvent<TestInput> for GestureEvent {
    fn cancelled(&self) -> bool {
        self.cancelled
    }
}
impl GestureSwipeUpdateEvent<TestInput> for GestureEvent {
    fn delta_x(&self) -> f64 {
        self.delta.0
    }
    fn delta_y(&self) -> f64 {
        self.delta.1
    }
}
impl GesturePinchUpdateEvent<TestInput> for GestureEvent {
    fn delta_x(&self) -> f64 {
        self.delta.0
    }
    fn delta_y(&self) -> f64 {
        self.delta.1
    }
    fn scale(&self) -> f64 {
        self.scale
    }
    fn rotation(&self) -> f64 {
        self.rotation
    }
}
impl GestureSwipeBeginEvent<TestInput> for GestureEvent {}
impl GestureSwipeEndEvent<TestInput> for GestureEvent {}
impl GesturePinchBeginEvent<TestInput> for GestureEvent {}
impl GesturePinchEndEvent<TestInput> for GestureEvent {}
impl GestureHoldBeginEvent<TestInput> for GestureEvent {}
impl GestureHoldEndEvent<TestInput> for GestureEvent {}

impl InputBackend for TestInput {
    type Device = TestDevice;
    type KeyboardKeyEvent = KeyEvent;
    type PointerAxisEvent = AxisEvent;
    type PointerButtonEvent = ButtonEvent;
    type PointerMotionEvent = MotionEvent;
    type PointerMotionAbsoluteEvent = UnusedEvent;
    type GestureSwipeBeginEvent = GestureEvent;
    type GestureSwipeUpdateEvent = GestureEvent;
    type GestureSwipeEndEvent = GestureEvent;
    type GesturePinchBeginEvent = GestureEvent;
    type GesturePinchUpdateEvent = GestureEvent;
    type GesturePinchEndEvent = GestureEvent;
    type GestureHoldBeginEvent = GestureEvent;
    type GestureHoldEndEvent = GestureEvent;
    type TouchDownEvent = UnusedEvent;
    type TouchUpEvent = UnusedEvent;
    type TouchMotionEvent = UnusedEvent;
    type TouchCancelEvent = UnusedEvent;
    type TouchFrameEvent = UnusedEvent;
    type TabletToolAxisEvent = UnusedEvent;
    type TabletToolProximityEvent = UnusedEvent;
    type TabletToolTipEvent = UnusedEvent;
    type TabletToolButtonEvent = UnusedEvent;
    type SwitchToggleEvent = UnusedEvent;
    type SpecialEvent = ();
}

/// Whether a frame is scheduled or an output waits to be drawn.
pub fn redraw_pending(data: &HeadlessData) -> bool {
    data.timer.is_some()
        || data
            .outputs
            .iter()
            .any(|o| o.enabled && o.power && o.redraw_needed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_specs_parse() {
        assert_eq!(
            parse_output_specs("A:800x600,B-2:1920x1080@144").unwrap(),
            vec![
                OutputSpec {
                    name: "A".into(),
                    size: (800, 600),
                    refresh_mhz: 60_000
                },
                OutputSpec {
                    name: "B-2".into(),
                    size: (1920, 1080),
                    refresh_mhz: 144_000
                },
            ]
        );
        assert!(parse_output_spec("A").is_err());
        assert!(parse_output_spec(":800x600").is_err());
        assert!(parse_output_spec("A:800").is_err());
        assert!(parse_output_spec("A:0x600").is_err());
        assert!(parse_output_spec("A:800x600@0").is_err());
    }
}
