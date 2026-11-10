//! Monitors: connectors found and lost, outputs set up, enabled and disabled,
//! `[[output]]` applied.

use super::*;

/// The mode `config` asks for among a connector's `modes`, else its
/// preferred mode, else its first.
pub(super) fn pick_mode(
    name: &str,
    modes: &[DrmMode],
    config: Option<&OutputConfig>,
) -> Option<DrmMode> {
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

/// How many modes `initialize_with_fallback` tries, the one asked for
/// included: each failure costs test commits, paid when a monitor comes on.
const MAX_MODE_ATTEMPTS: usize = 8;

/// What `fallback_order` compares of a mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ModeKey {
    w: i32,
    h: i32,
    refresh_mhz: i32,
    interlaced: bool,
}

impl From<&DrmMode> for ModeKey {
    fn from(mode: &DrmMode) -> Self {
        let wl = WlMode::from(*mode);
        Self {
            w: wl.size.w,
            h: wl.size.h,
            refresh_mhz: wl.refresh,
            interlaced: mode.flags().contains(ModeFlags::INTERLACE),
        }
    }
}

impl ModeKey {
    fn area(&self) -> i64 {
        i64::from(self.w) * i64::from(self.h)
    }
}

/// The modes to try, as indices into `modes`, when `first` may not come
/// on (typically: two monitors beyond a dock's bandwidth): `first`, then the
/// same size at lower refresh rates, then smaller sizes, largest first,
/// each from its highest refresh rate down. Never one asking more than
/// `first`, nor an interlaced one or a duplicate; at most
/// `MAX_MODE_ATTEMPTS`.
pub(super) fn fallback_order(first: usize, modes: &[ModeKey]) -> Vec<usize> {
    let Some(asked) = modes.get(first) else {
        return Vec::new();
    };
    let lighter = |m: &ModeKey| {
        let same_size = (m.w, m.h) == (asked.w, asked.h);
        !m.interlaced
            && ((same_size && m.refresh_mhz < asked.refresh_mhz)
                || (!same_size && m.area() < asked.area()))
    };
    let mut rest: Vec<usize> = (0..modes.len()).filter(|&i| lighter(&modes[i])).collect();
    rest.sort_by_key(|&i| {
        let m = &modes[i];
        (
            std::cmp::Reverse(m.area()),
            std::cmp::Reverse(m.refresh_mhz),
        )
    });
    let mut order = vec![first];
    for i in rest {
        if !order.iter().any(|&j| modes[j] == modes[i]) {
            order.push(i);
        }
    }
    order.truncate(MAX_MODE_ATTEMPTS);
    order
}

/// Set `surface`'s CRTC up with its mode or, if that fails, with the first
/// of its `fallback_order` that works, which then becomes its mode (and its
/// `Output`'s). `None` if none did.
pub(super) fn initialize_with_fallback(
    drm: &mut PilaDrmOutputManager,
    renderer: &mut GlesRenderer,
    crtc: crtc::Handle,
    surface: &mut OutputSurface,
) -> Option<PilaDrmOutput> {
    let name = surface.output.name();
    let keys: Vec<ModeKey> = surface.modes.iter().map(ModeKey::from).collect();
    let candidates: Vec<DrmMode> = match surface.modes.iter().position(|m| *m == surface.mode) {
        Some(first) => fallback_order(first, &keys)
            .into_iter()
            .map(|i| surface.modes[i])
            .collect(),
        None => vec![surface.mode],
    };
    let describe = |mode: &DrmMode| {
        let wl = WlMode::from(*mode);
        format!(
            "{}x{}@{:.3}",
            wl.size.w,
            wl.size.h,
            wl.refresh as f64 / 1000.0
        )
    };
    for mode in &candidates {
        let planes = drm.device().planes(&crtc).ok();
        match drm.lock().initialize_output::<_, PilaRenderElement>(
            crtc,
            *mode,
            &[surface.connector],
            &surface.output,
            planes,
            renderer,
            &DrmOutputRenderElements::default(),
        ) {
            Ok(drm_output) => {
                if *mode != surface.mode {
                    warn!(
                        name,
                        asked = describe(&surface.mode),
                        using = describe(mode),
                        "the mode asked for didn't come on; using a lighter one"
                    );
                    surface.mode = *mode;
                    surface.output.change_current_state(
                        Some(WlMode::from(*mode)),
                        None,
                        None,
                        None,
                    );
                }
                return Some(drm_output);
            }
            Err(err) => warn!(name, mode = describe(mode), ?err, "failed to set up output"),
        }
    }
    let tried: Vec<String> = candidates.iter().map(describe).collect();
    warn!(name, ?tried, "no mode came on: the output stays off");
    None
}

/// Re-apply `[[output]]` (mode, scale, position) to the connected outputs
/// after the config changed. Outputs keep their left-to-right order; those
/// without a position go right of the ones placed before them. Whether every
/// output could be set up as asked.
pub fn apply_output_config(state: &mut Pila) -> bool {
    let mut applied = true;
    let Some(Backend::Udev(data)) = &state.backend else {
        return false;
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
        if config.enabled.unwrap_or(true) {
            applied &= enable_output(state, *crtc);
        }
    }
    for (crtc, config) in &configs {
        if !config.enabled.unwrap_or(true) {
            disable_output(state, *crtc);
        }
    }

    let Some(Backend::Udev(data)) = &mut state.backend else {
        return false;
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
                    Some(_) => drm.lock().use_mode(
                        &crtc,
                        mode,
                        renderer,
                        &DrmOutputRenderElements::<_, PilaRenderElement>::default(),
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
                        surface.first_frame = true;
                        remodeled.push(crtc);
                    }
                    Err(err) => {
                        warn!(name, ?err, "failed to change the output mode");
                        applied = false;
                    }
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
        let width = logical_width(&surface.output, transform, scale);
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
    applied
}

/// Set up newly connected monitors and drop disconnected ones.
pub(super) fn scan_connectors(state: &mut Pila) {
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
    retrain_bad_links(state);
    // Once every new output is set up (or failed to be).
    keep_a_screen_on(state);
}

/// `link-status` values (`drm_mode.h`).
const LINK_STATUS_GOOD: u64 = 0;
const LINK_STATUS_BAD: u64 = 1;

/// The connector's `link-status` property and its value, if it has one.
fn link_status(
    drm: &PilaDrmOutputManager,
    connector: connector::Handle,
) -> Option<(property::Handle, u64)> {
    let device = drm.device();
    let props = device.get_properties(connector).ok()?;
    props.into_iter().find_map(|(handle, value)| {
        let info = device.get_property(handle).ok()?;
        (info.name().to_str() == Ok("link-status")).then_some((handle, value))
    })
}

/// Outputs whose link the kernel marked bad (DisplayPort link training
/// failed, typically after a resume or through a dock) show nothing until
/// the compositor sets them up again: it sends a hotplug event and waits.
/// Checked after every connector scan. On an atomic device the property is
/// set back to good, which makes the kernel set the mode again (the same
/// one); otherwise, or if that fails, the output is turned off and on.
fn retrain_bad_links(state: &mut Pila) {
    let Some(Backend::Udev(data)) = &mut state.backend else {
        return;
    };
    let bad: Vec<(crtc::Handle, Output, Option<property::Handle>)> = data
        .outputs
        .iter()
        .filter(|(_, s)| s.enabled && s.drm_output.is_some())
        .filter_map(|(crtc, s)| {
            let (prop, value) = link_status(&data.drm, s.connector)?;
            (value == LINK_STATUS_BAD).then(|| (*crtc, s.output.clone(), Some(prop)))
        })
        .collect();
    for (crtc, output, prop) in bad {
        let name = output.name();
        warn!(name, "the monitor's link went bad: setting it up again");
        let connector = data.outputs[&crtc].connector;
        let retrained = data.drm.device().is_atomic()
            && prop.is_some_and(|prop| {
                let mut request = AtomicModeReq::new();
                request.add_property(
                    connector,
                    prop,
                    property::Value::UnsignedRange(LINK_STATUS_GOOD),
                );
                match data
                    .drm
                    .device()
                    .atomic_commit(AtomicCommitFlags::ALLOW_MODESET, request)
                {
                    Ok(()) => true,
                    Err(err) => {
                        warn!(name, ?err, "couldn't set the link back to good");
                        false
                    }
                }
            });
        if !retrained
            && !(data.set_output_power(&output, false) && data.set_output_power(&output, true))
        {
            warn!(name, "couldn't set the output up again");
            continue;
        }
        // A frame in flight still gets its vblank (`frame_finish`).
        if let Some(surface) = data.outputs.get_mut(&crtc) {
            surface.redraw_needed = true;
        }
        info!(name, "link set up again");
    }
    data.queue_redraw();
}

/// A connector's name as outputs are known by, e.g. `eDP-1`.
pub(super) fn connector_name(connector: &connector::Info) -> String {
    format!(
        "{}-{}",
        connector.interface().as_str(),
        connector.interface_id()
    )
}

/// An output's width in the global logical space, rotation and scale
/// applied.
pub(super) fn logical_width(output: &Output, transform: Transform, scale: f64) -> i32 {
    output
        .current_mode()
        .map(|m| {
            transform
                .transform_size(m.size)
                .to_f64()
                .to_logical(scale)
                .to_i32_round::<i32>()
                .w
        })
        .unwrap_or(0)
}

pub(super) fn connector_connected(
    state: &mut Pila,
    connector: connector::Info,
    crtc: crtc::Handle,
) {
    let name = connector_name(&connector);
    // Before its settings: an `[[output]]` section may name the monitor.
    let monitor = match &state.backend {
        Some(Backend::Udev(data)) => read_monitor(&data.drm, &connector),
        _ => None,
    };
    let (make, model) = match &monitor {
        Some(monitor) => (monitor.make.clone(), monitor.model.clone()),
        None => ("Unknown".to_string(), "Unknown".to_string()),
    };
    let serial_number = monitor
        .as_ref()
        .and_then(|m| m.serial.clone())
        .unwrap_or_default();
    info!(name, make, model, "monitor");
    match monitor {
        Some(monitor) => state.outputs.monitors.insert(name.clone(), monitor),
        None => state.outputs.monitors.remove(&name),
    };
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
        let x = match &state.backend {
            Some(Backend::Udev(data)) => data
                .outputs
                .values()
                .filter(|s| s.enabled)
                .map(|s| {
                    let o = &s.output;
                    let scale = o.current_scale().fractional_scale();
                    o.current_location().x + logical_width(o, o.current_transform(), scale)
                })
                .max()
                .unwrap_or(0),
            _ => 0,
        };
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
            make,
            model,
            serial_number,
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
            last_vblank: None,
            last_vblank_time: None,
            back_to_back: false,
            redraw_needed: true,
            lock_frame: LockFrame::None,
            modes,
            gamma: None,
            gamma_lut: gamma_lut_property(&data.drm, crtc),
            gamma_pending: None,
            gamma_in_flight: false,
            recovery: Recovery::default(),
            adaptive_sync: false,
            presented_variable: false,
            first_frame: true,
            idle_until: None,
            idle_timer: false,
            redraw_content: false,
            request: None,
            queued_request: None,
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
    // A lighter one if that one didn't come on.
    let wl_mode = output.current_mode().unwrap_or(wl_mode);
    info!(
        name,
        mode = ?wl_mode.size,
        refresh_mhz = wl_mode.refresh,
        scale,
        x,
        y,
        "output connected"
    );
    if let (Some(on), Some(Backend::Udev(data))) = (output_config.adaptive_sync, &mut state.backend)
    {
        set_vrr(data, crtc, on);
    }
    state.refit_maximized();
    state.configure_lock_surfaces();
    state.refresh_output_heads();
    state.queue_redraw();
}

/// Set a connected output up: its CRTC, its `wl_output` global and its
/// place in the space. `false` if the CRTC couldn't be set up.
pub(super) fn enable_output(state: &mut Pila, crtc: crtc::Handle) -> bool {
    let Some(Backend::Udev(data)) = &mut state.backend else {
        return false;
    };
    let UdevData {
        outputs,
        drm,
        renderer,
        session,
        ..
    } = &mut **data;
    let Some(surface) = outputs.get_mut(&crtc) else {
        return false;
    };
    if surface.enabled {
        return true;
    }
    let name = surface.output.name();
    // No DRM master on another VT: it comes on at `ActivateSession`.
    if !session.is_active() {
        debug!(name, "session inactive: output left off until it's back");
        return false;
    }
    let Some(drm_output) = initialize_with_fallback(drm, renderer, crtc, surface) else {
        return false;
    };
    surface.drm_output = Some(drm_output);
    surface.enabled = true;
    surface.frame_pending = false;
    surface.redraw_needed = true;
    surface.recovery = Recovery::default();
    surface.first_frame = true;
    apply_vrr(surface);
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
pub(super) fn disable_output(state: &mut Pila, crtc: crtc::Handle) {
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
    surface.gamma_in_flight = false;
    surface.lock_frame = LockFrame::None;
    surface.last_vblank = None;
    surface.presented_variable = false;
    let output = surface.output.clone();
    if let Some(global) = surface.global.take() {
        crate::state::retire_global(&state.display_handle, &state.loop_handle, global);
    }
    info!(name = output.name(), "output disabled");
    state.space.unmap_output(&output);
    state.output_removed(&output);
}

/// Turn variable refresh rate on or off where the monitor supports it,
/// now and whenever the CRTC is set up again (powered on, enabled).
pub(super) fn set_vrr(data: &mut UdevData, crtc: crtc::Handle, on: bool) {
    let Some(surface) = data.outputs.get_mut(&crtc) else {
        return;
    };
    surface.adaptive_sync = on;
    apply_vrr(surface);
}

/// Bring the CRTC's adaptive sync to what `[[output]]` asks for. A CRTC
/// may take it while the monitor can't do it (`vrr_capable`, off in a
/// monitor's own menu too): that's checked first.
pub(super) fn apply_vrr(surface: &OutputSurface) {
    let Some(drm_output) = &surface.drm_output else {
        return;
    };
    let on = surface.adaptive_sync;
    let result = drm_output.with_compositor(|c| {
        if c.vrr_enabled() == on {
            return Ok(false);
        }
        if on
            && matches!(
                c.vrr_supported(surface.connector),
                Ok(VrrSupport::NotSupported) | Err(_)
            )
        {
            return Err(None);
        }
        c.use_vrr(on).map(|()| true).map_err(Some)
    });
    let name = surface.output.name();
    match result {
        Ok(true) => info!(name, on, "adaptive sync set"),
        Ok(false) => {}
        Err(None) => info!(name, "adaptive sync not supported by the monitor"),
        Err(Some(err)) => warn!(name, ?err, on, "couldn't set adaptive sync"),
    }
}

/// `[[output]]` for `name`, with what a display tool set at runtime on top
/// (`Pila::outputs.overrides`).
pub(super) fn effective_output_config(state: &Pila, name: &str) -> OutputConfig {
    let file = state
        .output_section(name)
        .unwrap_or_else(|| OutputConfig::named(name));
    with_overrides(state, file)
}

pub(super) fn with_overrides(state: &Pila, file: OutputConfig) -> OutputConfig {
    match state.outputs.overrides.get(&file.name) {
        Some(over) => file.overridden_by(over),
        None => file,
    }
}

/// A laptop's built-in screen, by its connector type.
pub(super) fn is_internal(name: &str) -> bool {
    ["eDP-", "LVDS-", "DSI-"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

/// With no output left on (the external monitor went, and the config turns the
/// built-in screen off), the built-in screen comes back: there'd be nothing to
/// see otherwise, not even how to change that.
pub(super) fn keep_a_screen_on(state: &mut Pila) {
    let Some(Backend::Udev(data)) = &state.backend else {
        return;
    };
    if data.outputs.values().any(|s| s.enabled) {
        return;
    }
    let internal: Vec<crtc::Handle> = data
        .outputs
        .iter()
        .filter(|(_, s)| is_internal(&s.output.name()))
        .map(|(crtc, _)| *crtc)
        .collect();
    if internal.is_empty() {
        return;
    }
    warn!("no output is on: turning the built-in screen on");
    for crtc in internal {
        enable_output(state, crtc);
    }
    state.fixup_windows();
    state.refit_maximized();
    state.configure_lock_surfaces();
    state.refresh_output_heads();
    state.queue_redraw();
}

pub(super) fn connector_disconnected(state: &mut Pila, crtc: crtc::Handle) {
    let Some(Backend::Udev(data)) = &mut state.backend else {
        return;
    };
    let Some(surface) = data.outputs.remove(&crtc) else {
        return;
    };
    info!(name = surface.output.name(), "output disconnected");
    state.outputs.monitors.remove(&surface.output.name());
    // Before it's unmapped: the windows of the monitor that went away move
    // onto the built-in screen, if that has to come back.
    keep_a_screen_on(state);
    if let Some(global) = surface.global {
        crate::state::retire_global(&state.display_handle, &state.loop_handle, global);
    }
    if surface.enabled {
        state.space.unmap_output(&surface.output);
        state.output_removed(&surface.output);
    }
    state.refresh_output_heads();
    state.queue_redraw();
}

/// The monitor on the connector, from its `EDID` property.
pub(super) fn read_monitor(
    drm: &PilaDrmOutputManager,
    connector: &connector::Info,
) -> Option<Monitor> {
    let device = drm.device();
    let props = device.get_properties(connector.handle()).ok()?;
    let (info, value) = props.into_iter().find_map(|(handle, value)| {
        let info = device.get_property(handle).ok()?;
        (info.name().to_str() == Ok("EDID")).then_some((info, value))
    })?;
    let blob = info.value_type().convert_value(value).as_blob()?;
    crate::edid::parse(&device.get_property_blob(blob).ok()?)
}

/// Whether the connector is marked `non-desktop` (VR headsets and the like).
pub(super) fn is_non_desktop(drm: &PilaDrmOutputManager, connector: &connector::Info) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(w: i32, h: i32, hz: i32) -> ModeKey {
        ModeKey {
            w,
            h,
            refresh_mhz: hz * 1000,
            interlaced: false,
        }
    }

    /// A 4K high-refresh monitor's modes, as a connector lists them.
    fn monitor() -> Vec<ModeKey> {
        vec![
            mode(3840, 2160, 144),
            mode(3840, 2160, 120),
            mode(3840, 2160, 60),
            mode(2560, 1440, 165),
            mode(2560, 1440, 144),
            mode(1920, 1080, 60),
            ModeKey {
                interlaced: true,
                ..mode(1920, 1080, 60)
            },
            // Another timing of a mode already listed.
            mode(3840, 2160, 60),
        ]
    }

    #[test]
    fn the_preferred_mode_falls_back_to_lower_refresh_then_smaller_sizes() {
        assert_eq!(fallback_order(0, &monitor()), [0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn a_configured_mode_falls_back_to_nothing_heavier() {
        // 4K at 60 Hz: never 4K faster, nor larger.
        assert_eq!(fallback_order(2, &monitor()), [2, 3, 4, 5]);
        assert_eq!(fallback_order(4, &monitor()), [4, 5]);
    }

    #[test]
    fn a_lone_mode_has_nothing_to_fall_back_to() {
        assert_eq!(fallback_order(0, &[mode(1920, 1080, 60)]), [0]);
        assert_eq!(
            fallback_order(5, &[mode(1920, 1080, 60)]),
            Vec::<usize>::new()
        );
    }

    #[test]
    fn fallbacks_are_capped() {
        let modes: Vec<ModeKey> = (0..20).map(|i| mode(3840 - i * 100, 2160, 60)).collect();
        assert_eq!(
            fallback_order(0, &modes),
            (0..MAX_MODE_ATTEMPTS).collect::<Vec<_>>()
        );
    }
}
