//! Monitors: connectors found and lost, outputs set up, enabled and disabled,
//! `[[output]]` applied, and `disable-internal-display`.

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

    // Built-in screens off for an external one keep their place.
    let held: Vec<crtc::Handle> = configs
        .iter()
        .filter(|(_, c)| off_by_rule(state, &c.name))
        .map(|(crtc, _)| *crtc)
        .collect();

    // Enabling and disabling first: they change what the space holds.
    // Enabling goes first, so a built-in screen is only turned off for an
    // external one that actually came on.
    for (crtc, config) in &configs {
        if config.enabled.unwrap_or(true) {
            applied &= enable_output(state, *crtc);
        }
    }
    // Then the others, and the built-in screens held by the rule last, once
    // it's known whether anything else is still on.
    let off: Vec<crtc::Handle> = configs
        .iter()
        .filter(|(_, config)| !config.enabled.unwrap_or(true))
        .map(|(crtc, _)| *crtc)
        .collect();
    for crtc in off.iter().filter(|crtc| !held.contains(crtc)) {
        disable_output(state, *crtc);
    }
    for crtc in off.iter().filter(|crtc| held.contains(crtc)) {
        if others_enabled(state, &held) {
            disable_output(state, *crtc);
        } else {
            // Never leave no output on (as `apply_internal_display`).
            keep_on(state, *crtc);
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
                    Some(_) => drm.use_mode(
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
            if held.contains(&crtc) {
                right_edge = right_edge.max(loc.x + width);
            }
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

/// Whether an output other than `held` (built-in screens off by
/// `disable-internal-display`) is on.
pub(super) fn others_enabled(state: &Pila, held: &[crtc::Handle]) -> bool {
    let Some(Backend::Udev(data)) = &state.backend else {
        return false;
    };
    data.outputs
        .iter()
        .any(|(crtc, s)| s.enabled && !held.contains(crtc))
}

/// Keep the built-in screen on `crtc` on (or turn it back on) because no
/// other output is.
pub(super) fn keep_on(state: &mut Pila, crtc: crtc::Handle) {
    let Some(Backend::Udev(data)) = &state.backend else {
        return;
    };
    let Some(surface) = data.outputs.get(&crtc) else {
        return;
    };
    if surface.enabled {
        return;
    }
    let name = surface.output.name();
    if enable_output(state, crtc) {
        warn!(name, "no external monitor is on: built-in screen kept on");
    } else {
        warn!(
            name,
            "no external monitor is on, and the built-in screen failed to come on"
        );
    }
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
    data.scan_pending = events
        .iter()
        .filter_map(|event| match event {
            DrmScanEvent::Connected {
                connector,
                crtc: Some(_),
            } => Some(connector_name(connector)),
            _ => None,
        })
        .collect();
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
    if let Some(Backend::Udev(data)) = &mut state.backend {
        data.scan_pending.clear();
    }
    // Once every new output is set up (or failed to be): the built-in
    // screen goes off for an external one, or stays on if none came up.
    apply_internal_display(state);
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
    info!(name, make, model, "monitor");
    match monitor {
        Some(monitor) => state.monitors.insert(name.clone(), monitor),
        None => state.monitors.remove(&name),
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

    // Where the config puts it, else right of the outputs already there
    // (a built-in screen turned off for an external one keeps its place, so
    // it doesn't overlap when it comes back).
    let (x, y) = output_config.position.unwrap_or_else(|| {
        let x = match &state.backend {
            Some(Backend::Udev(data)) => data
                .outputs
                .values()
                .filter(|s| s.enabled || off_by_rule(state, &s.output.name()))
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
        },
    );
    // Known now; it stayed pending until here so that it counted as an
    // external monitor while its place was worked out.
    data.scan_pending.retain(|n| *n != name);
    if !enabled {
        if off_by_rule(state, &name) {
            info!(
                name,
                "output connected, off while an external monitor is on"
            );
        } else {
            info!(name, "output connected, disabled by its config");
        }
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
    let planes = drm.device().planes(&crtc).ok();
    match drm.initialize_output::<_, PilaRenderElement>(
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
    surface.lock_frame = LockFrame::None;
    let output = surface.output.clone();
    if let Some(global) = surface.global.take() {
        crate::state::retire_global(&state.display_handle, &state.loop_handle, global);
    }
    info!(name = output.name(), "output disabled");
    state.space.unmap_output(&output);
    state.output_removed(&output);
}

/// Turn variable refresh rate on or off where the monitor supports it.
pub(super) fn set_vrr(data: &UdevData, crtc: crtc::Handle, on: bool) {
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

/// `[[output]]` for `name`, turned off by `disable-internal-display` if it's
/// the built-in screen and that applies now, with what a display tool set at
/// runtime on top (`Pila::output_overrides`).
pub(super) fn effective_output_config(state: &Pila, name: &str) -> OutputConfig {
    let mut file = state
        .output_section(name)
        .unwrap_or_else(|| OutputConfig::named(name));
    if off_by_rule(state, name) {
        file.enabled = Some(false);
    }
    with_overrides(state, file)
}

/// Whether `disable-internal-display` (rather than the file) turns output
/// `name` off now. Such an output keeps its place in the layout.
pub(super) fn off_by_rule(state: &Pila, name: &str) -> bool {
    state.output_section(name).and_then(|c| c.enabled).is_none()
        && internal_display_off(state, name)
}

pub(super) fn with_overrides(state: &Pila, file: OutputConfig) -> OutputConfig {
    match state.output_overrides.get(&file.name) {
        Some(over) => file.overridden_by(over),
        None => file,
    }
}

/// A laptop's built-in screen, by its connector type.
pub fn is_internal(name: &str) -> bool {
    ["eDP-", "LVDS-", "DSI-"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

/// Whether `disable-internal-display` turns the built-in screen `name` off
/// now: it's on and another connected monitor, an external one, is on
/// by its own config. That's decided from the config, not from which
/// outputs are running, so it doesn't depend on the order outputs are set
/// up in.
pub(super) fn internal_display_off(state: &Pila, name: &str) -> bool {
    let Some(Backend::Udev(data)) = &state.backend else {
        return false;
    };
    let applies = state.config.internal_display == InternalDisplay::Always;
    // Set up, or found by the scan in progress and about to be.
    let connected = data
        .outputs
        .values()
        .map(|s| s.output.name())
        .chain(data.scan_pending.iter().cloned());
    internal_off(applies, name, connected, |other| {
        let config = state
            .output_section(other)
            .unwrap_or_else(|| OutputConfig::named(other));
        with_overrides(state, config).enabled.unwrap_or(true)
    })
}

/// The decision of `internal_display_off`: the rule `applies`, `name` is a
/// built-in screen, and among the `connected` outputs another one,
/// external, is on by its own config (`enabled_by_config`).
pub(super) fn internal_off(
    applies: bool,
    name: &str,
    connected: impl IntoIterator<Item = String>,
    enabled_by_config: impl Fn(&str) -> bool,
) -> bool {
    applies
        && is_internal(name)
        && connected
            .into_iter()
            .any(|other| other != name && !is_internal(&other) && enabled_by_config(&other))
}

/// Turn built-in screens on or off as `disable-internal-display` says now
/// (an external monitor came or went). Never leaves no output on: if every
/// external one failed, the built-in screen stays.
pub(super) fn apply_internal_display(state: &mut Pila) {
    let Some(Backend::Udev(data)) = &state.backend else {
        return;
    };
    let internal: Vec<(crtc::Handle, String, bool)> = data
        .outputs
        .iter()
        .map(|(crtc, s)| (*crtc, s.output.name(), s.enabled))
        .filter(|(_, name, _)| is_internal(name))
        .collect();
    if internal.is_empty() {
        return;
    }
    for (crtc, name, _) in &internal {
        if effective_output_config(state, name).enabled.unwrap_or(true) {
            enable_output(state, *crtc);
        } else {
            disable_output(state, *crtc);
        }
    }
    let Some(Backend::Udev(data)) = &state.backend else {
        return;
    };
    if !data.outputs.values().any(|s| s.enabled) {
        warn!("no external monitor is on: keeping the built-in screen on");
        for (crtc, _, _) in &internal {
            enable_output(state, *crtc);
        }
    }
    let Some(Backend::Udev(data)) = &state.backend else {
        return;
    };
    let changed = internal
        .iter()
        .any(|(crtc, _, was)| data.outputs.get(crtc).is_some_and(|s| s.enabled != *was));
    if !changed {
        return;
    }
    for (crtc, name, was) in &internal {
        if !*was && data.outputs.get(crtc).is_some_and(|s| s.enabled) {
            info!(name, "built-in screen back on");
        }
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
    state.monitors.remove(&surface.output.name());
    // The built-in screen comes back first, so the windows of the monitor
    // that went away move onto it.
    apply_internal_display(state);
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
