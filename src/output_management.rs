//! wlr-output-management: lists the monitors (heads) with their modes and
//! settings for display tools (wlr-randr, a shell's monitor settings) and
//! lets them change those settings: enable or disable a monitor, pick a
//! mode, position, transform, scale, adaptive sync.
//!
//! Heads are described by `HeadInfo`, a snapshot the backend provides
//! (`Pila::output_heads`); `refresh_output_heads` diffs a new snapshot
//! against the last one and sends each client only what changed, then
//! `done` with a new serial, which cancels configurations still being
//! built. A configuration is checked against the snapshot (`test`) and,
//! for `apply`, turned into `[[output]]`-style overrides
//! (`Pila::outputs.overrides`) that go through the same path as the config
//! file (`apply_outputs`); editing a monitor's `[[output]]` section drops
//! its override. Nested, only the scale can change.

use std::collections::HashMap;

use smithay::{
    reexports::{
        wayland_protocols_wlr::output_management::v1::server::{
            zwlr_output_configuration_head_v1::{
                self as config_head, ZwlrOutputConfigurationHeadV1,
            },
            zwlr_output_configuration_v1::{self as configuration, ZwlrOutputConfigurationV1},
            zwlr_output_head_v1::{self as head, AdaptiveSyncState, ZwlrOutputHeadV1},
            zwlr_output_manager_v1::{self as manager, ZwlrOutputManagerV1},
            zwlr_output_mode_v1::{self as mode, ZwlrOutputModeV1},
        },
        wayland_server::{
            backend::ClientId, protocol::wl_output, Client, DataInit, Dispatch, DisplayHandle,
            GlobalDispatch, New, Resource, WEnum,
        },
    },
    utils::Transform,
};
use tracing::{debug, info, warn};

use crate::{
    backend::Backend,
    config::{ModeSpec, OutputConfig},
    Pila,
};

const VERSION: u32 = 4;

/// One of a head's modes.
#[derive(Debug, Clone, PartialEq)]
pub struct ModeInfo {
    pub size: (i32, i32),
    pub refresh_mhz: i32,
    pub preferred: bool,
}

/// A monitor as the protocol describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct HeadInfo {
    pub name: String,
    pub make: String,
    pub model: String,
    pub serial: Option<String>,
    /// Millimetres.
    pub physical: (i32, i32),
    pub modes: Vec<ModeInfo>,
    /// Index into `modes`.
    pub current_mode: Option<usize>,
    pub enabled: bool,
    pub position: (i32, i32),
    pub transform: Transform,
    pub scale: f64,
    /// `None` where the monitor can't do it.
    pub adaptive_sync: Option<bool>,
}

/// What a client asked for one head.
#[derive(Debug, Clone, Default)]
pub struct HeadConfig {
    pub enabled: bool,
    pub mode: Option<ModeChoice>,
    pub position: Option<(i32, i32)>,
    pub transform: Option<Transform>,
    pub scale: Option<f64>,
    pub adaptive_sync: Option<bool>,
}

#[derive(Debug, Clone, Copy)]
pub enum ModeChoice {
    Index(usize),
    Custom { size: (i32, i32), refresh_mhz: i32 },
}

/// A client's manager and the head/mode objects it was given.
struct ClientHeads {
    heads: HashMap<String, (ZwlrOutputHeadV1, Vec<ZwlrOutputModeV1>)>,
}

/// A configuration being built; gone once used.
struct Configuration {
    heads: HashMap<String, HeadConfig>,
}

/// The protocol's state (`Pila::outputs.management`).
#[derive(Default)]
pub struct OutputManagement {
    serial: u32,
    heads: Vec<HeadInfo>,
    managers: HashMap<ZwlrOutputManagerV1, ClientHeads>,
    configurations: HashMap<ZwlrOutputConfigurationV1, Configuration>,
}

/// What's known and set about outputs beyond the `Space` (`Pila::outputs`).
#[derive(Default)]
pub struct Outputs {
    /// Display tools' view of the monitors, and what they set, over
    /// `[[output]]`, by output name.
    pub management: OutputManagement,
    pub overrides: HashMap<String, crate::config::OutputConfig>,
    /// The monitor on each output that says which it is, by output name
    /// (`edid.rs`).
    pub monitors: HashMap<String, crate::edid::Monitor>,
    /// The client controlling each output's gamma (`gamma_control.rs`).
    pub gamma: crate::gamma_control::GammaControls,
    /// Clients watching or setting outputs' power (`output_power.rs`).
    pub power: crate::output_power::OutputPowers,
    /// Outputs `power-off-monitors` turned off, back on with any input.
    pub off_until_input: Vec<smithay::output::Output>,
}

/// Create the global. Called once from `Pila::new`.
pub fn init_global(dh: &DisplayHandle) {
    dh.create_global::<Pila, ZwlrOutputManagerV1, ()>(VERSION, ());
}

fn adaptive_sync(on: bool) -> AdaptiveSyncState {
    if on {
        AdaptiveSyncState::Enabled
    } else {
        AdaptiveSyncState::Disabled
    }
}

/// A head's mode as `[[output]]` would ask for it.
fn mode_spec(mode: &ModeInfo) -> ModeSpec {
    ModeSpec {
        width: mode.size.0,
        height: mode.size.1,
        refresh_hz: Some(mode.refresh_mhz as f64 / 1000.0),
    }
}

fn send_mode(mode: &ZwlrOutputModeV1, info: &ModeInfo) {
    mode.size(info.size.0, info.size.1);
    mode.refresh(info.refresh_mhz);
    if info.preferred {
        mode.preferred();
    }
}

/// The settings that only an enabled head has.
fn send_enabled_state(head: &ZwlrOutputHeadV1, modes: &[ZwlrOutputModeV1], info: &HeadInfo) {
    if let Some(mode) = info.current_mode.and_then(|i| modes.get(i)) {
        head.current_mode(mode);
    }
    head.position(info.position.0, info.position.1);
    head.transform(wl_output::Transform::from(info.transform));
    head.scale(info.scale);
}

impl ClientHeads {
    /// Tell this client about a head it hasn't seen.
    fn add_head(&mut self, dh: &DisplayHandle, manager: &ZwlrOutputManagerV1, info: &HeadInfo) {
        let Some(client) = manager.client() else {
            return;
        };
        let version = manager.version();
        let Ok(head) = client.create_resource::<ZwlrOutputHeadV1, String, Pila>(
            dh,
            version,
            info.name.clone(),
        ) else {
            return;
        };
        manager.head(&head);
        head.name(info.name.clone());
        head.description(format!("{} {} ({})", info.make, info.model, info.name));
        head.physical_size(info.physical.0, info.physical.1);
        let mut modes = Vec::new();
        for (i, mode_info) in info.modes.iter().enumerate() {
            let Ok(mode) = client.create_resource::<ZwlrOutputModeV1, (String, usize), Pila>(
                dh,
                version,
                (info.name.clone(), i),
            ) else {
                continue;
            };
            head.mode(&mode);
            send_mode(&mode, mode_info);
            modes.push(mode);
        }
        head.enabled(info.enabled as i32);
        if info.enabled {
            send_enabled_state(&head, &modes, info);
        }
        if version >= 2 {
            head.make(info.make.clone());
            head.model(info.model.clone());
            if let Some(serial) = &info.serial {
                head.serial_number(serial.clone());
            }
        }
        if version >= 4 {
            if let Some(on) = info.adaptive_sync {
                head.adaptive_sync(adaptive_sync(on));
            }
        }
        self.heads.insert(info.name.clone(), (head, modes));
    }

    /// Send what changed between `old` and `new` for one head.
    fn update_head(
        &mut self,
        dh: &DisplayHandle,
        manager: &ZwlrOutputManagerV1,
        old: &HeadInfo,
        new: &HeadInfo,
    ) {
        let Some((head, modes)) = self.heads.get_mut(&new.name) else {
            return;
        };
        if old.modes != new.modes {
            // Modes change in place where they can (the nested window
            // resizing), else the list is trimmed or extended.
            for (mode, info) in modes.iter().zip(&new.modes) {
                send_mode(mode, info);
            }
            if new.modes.len() < modes.len() {
                for mode in modes.drain(new.modes.len()..) {
                    mode.finished();
                }
            } else if let Some(client) = manager.client() {
                for (i, info) in new.modes.iter().enumerate().skip(modes.len()) {
                    let Ok(mode) = client
                        .create_resource::<ZwlrOutputModeV1, (String, usize), Pila>(
                            dh,
                            manager.version(),
                            (new.name.clone(), i),
                        )
                    else {
                        continue;
                    };
                    head.mode(&mode);
                    send_mode(&mode, info);
                    modes.push(mode);
                }
            }
        }
        if old.enabled != new.enabled {
            head.enabled(new.enabled as i32);
            if new.enabled {
                send_enabled_state(head, modes, new);
            }
        } else if new.enabled {
            if old.current_mode != new.current_mode || old.modes != new.modes {
                if let Some(mode) = new.current_mode.and_then(|i| modes.get(i)) {
                    head.current_mode(mode);
                }
            }
            if old.position != new.position {
                head.position(new.position.0, new.position.1);
            }
            if old.transform != new.transform {
                head.transform(wl_output::Transform::from(new.transform));
            }
            if old.scale != new.scale {
                head.scale(new.scale);
            }
        }
        if head.version() >= 4 && old.adaptive_sync != new.adaptive_sync {
            if let Some(on) = new.adaptive_sync {
                head.adaptive_sync(adaptive_sync(on));
            }
        }
    }

    fn remove_head(&mut self, name: &str) {
        if let Some((head, modes)) = self.heads.remove(name) {
            for mode in modes {
                mode.finished();
            }
            head.finished();
        }
    }
}

impl Pila {
    /// The monitors as the protocol sees them: from the backend on bare
    /// metal; nested, the window is the one head and keeps its mode.
    pub fn output_heads(&self) -> Vec<HeadInfo> {
        let mut heads = match self.backend.as_ref().and_then(Backend::output_heads) {
            Some(heads) => heads,
            None => self
                .space
                .outputs()
                .map(|output| {
                    let mode = output.current_mode();
                    let props = output.physical_properties();
                    HeadInfo {
                        name: output.name(),
                        make: props.make,
                        model: props.model,
                        serial: None,
                        physical: (props.size.w, props.size.h),
                        modes: mode
                            .map(|m| ModeInfo {
                                size: (m.size.w, m.size.h),
                                refresh_mhz: m.refresh,
                                preferred: true,
                            })
                            .into_iter()
                            .collect(),
                        current_mode: mode.map(|_| 0),
                        enabled: true,
                        position: {
                            let loc = output.current_location();
                            (loc.x, loc.y)
                        },
                        // As wl_output tells clients (`scale.rs`): the
                        // window's own orientation is the backend's business.
                        transform: Transform::Normal,
                        scale: output.current_scale().fractional_scale(),
                        adaptive_sync: None,
                    }
                })
                .collect(),
        };
        for head in &mut heads {
            if let Some(monitor) = self.outputs.monitors.get(&head.name) {
                head.make.clone_from(&monitor.make);
                head.model.clone_from(&monitor.model);
                head.serial.clone_from(&monitor.serial);
            }
        }
        heads
    }

    /// Tell every client what changed about the monitors. Called whenever
    /// an output is connected, removed or reconfigured.
    pub fn refresh_output_heads(&mut self) {
        // Workspace listeners are told about outputs too (ext_workspace.rs).
        self.refresh_workspaces();
        let new = self.output_heads();
        let dh = self.display_handle.clone();
        let om = &mut self.outputs.management;
        if new == om.heads {
            return;
        }
        for (manager, client) in &mut om.managers {
            for old in &om.heads {
                if !new.iter().any(|h| h.name == old.name) {
                    client.remove_head(&old.name);
                }
            }
            for info in &new {
                match om.heads.iter().find(|h| h.name == info.name) {
                    Some(old) => client.update_head(&dh, manager, old, info),
                    None => client.add_head(&dh, manager, info),
                }
            }
        }
        om.heads = new;
        om.serial = om.serial.wrapping_add(1);
        for manager in om.managers.keys() {
            manager.done(om.serial);
        }
        // Built against the old state: the client has to start over.
        for (configuration, _) in om.configurations.drain() {
            configuration.cancelled();
        }
    }

    /// Check a configuration against the current monitors; the error says
    /// why it can't be applied.
    fn check_output_configuration(
        &self,
        heads: &HashMap<String, HeadConfig>,
    ) -> Result<(), String> {
        let om = &self.outputs.management;
        for info in &om.heads {
            let Some(config) = heads.get(&info.name) else {
                return Err(format!("head {} isn't configured", info.name));
            };
            if !config.enabled {
                continue;
            }
            match config.mode {
                Some(ModeChoice::Index(i)) if i >= info.modes.len() => {
                    return Err(format!("head {}: no such mode", info.name));
                }
                Some(ModeChoice::Custom { size, refresh_mhz }) => {
                    // Only modes the monitor advertises: no modelines.
                    let known = info.modes.iter().any(|m| {
                        m.size == size
                            && (refresh_mhz == 0 || (m.refresh_mhz - refresh_mhz).abs() < 1000)
                    });
                    if !known {
                        return Err(format!(
                            "head {}: custom mode {}x{}@{} isn't one the monitor offers",
                            info.name, size.0, size.1, refresh_mhz
                        ));
                    }
                }
                _ => {}
            }
            if let Some(scale) = config.scale {
                if !(0.5..=8.0).contains(&scale) {
                    return Err(format!(
                        "head {}: scale {scale} is outside 0.5..8",
                        info.name
                    ));
                }
            }
            if config.adaptive_sync.is_some() && info.adaptive_sync.is_none() {
                return Err(format!("head {}: no adaptive sync", info.name));
            }
        }
        if !om
            .heads
            .iter()
            .any(|h| heads.get(&h.name).is_some_and(|c| c.enabled))
        {
            return Err("every head would be disabled".into());
        }
        if self.backend.as_ref().is_some_and(Backend::is_nested) {
            // Nested, the host decides everything but the scale.
            for info in &om.heads {
                let config = &heads[&info.name];
                let same_mode = match config.mode {
                    None | Some(ModeChoice::Index(0)) => true,
                    Some(ModeChoice::Custom { size, .. }) => {
                        info.modes.first().is_some_and(|m| m.size == size)
                    }
                    _ => false,
                };
                if !config.enabled
                    || !same_mode
                    || config.position.is_some_and(|p| p != info.position)
                    || config.transform.is_some_and(|t| t != info.transform)
                    || config.adaptive_sync.is_some()
                {
                    return Err("nested, only the scale can change".into());
                }
            }
        }
        Ok(())
    }

    /// Apply a checked configuration: it becomes the outputs' runtime
    /// settings, over the config file's. If the backend can't do it they go
    /// back to what they were: `false`.
    fn apply_output_configuration(&mut self, heads: HashMap<String, HeadConfig>) -> bool {
        let previous = self.outputs.overrides.clone();
        for (name, config) in heads {
            let info = self
                .outputs
                .management
                .heads
                .iter()
                .find(|h| h.name == name);
            let mode = match config.mode {
                Some(ModeChoice::Index(i)) => info.and_then(|h| h.modes.get(i)).map(mode_spec),
                Some(ModeChoice::Custom { size, refresh_mhz }) => Some(ModeSpec {
                    width: size.0,
                    height: size.1,
                    refresh_hz: (refresh_mhz > 0).then(|| refresh_mhz as f64 / 1000.0),
                }),
                None => None,
            };
            // Unset properties keep their current values, so the override
            // records everything the head has now.
            let over = OutputConfig {
                name: name.clone(),
                mode: mode.or_else(|| {
                    info.and_then(|h| h.current_mode.and_then(|i| h.modes.get(i)))
                        .map(mode_spec)
                }),
                scale: config
                    .scale
                    .map(|s| (s * 120.0).round() / 120.0)
                    .or(info.map(|h| h.scale)),
                position: config.position.or(info.map(|h| h.position)),
                transform: config.transform.or(info.map(|h| h.transform)),
                enabled: Some(config.enabled),
                adaptive_sync: config.adaptive_sync.or(info.and_then(|h| h.adaptive_sync)),
            };
            info!(?over, "output configured by a client");
            self.outputs.overrides.insert(name, over);
        }
        if self.apply_outputs() {
            return true;
        }
        warn!("the backend couldn't apply an output configuration");
        self.outputs.overrides = previous;
        self.apply_outputs();
        false
    }
}

impl GlobalDispatch<ZwlrOutputManagerV1, ()> for Pila {
    fn bind(
        state: &mut Self,
        dh: &DisplayHandle,
        _: &Client,
        resource: New<ZwlrOutputManagerV1>,
        _: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let manager = data_init.init(resource, ());
        // The snapshot may be stale if nothing refreshed it yet; refreshing
        // it also tells the other managers what changed (and starts a new
        // serial) before this one is shown it.
        state.refresh_output_heads();
        let om = &mut state.outputs.management;
        let mut client = ClientHeads {
            heads: HashMap::new(),
        };
        for info in &om.heads {
            client.add_head(dh, &manager, info);
        }
        manager.done(om.serial);
        om.managers.insert(manager, client);
    }

    crate::state::privileged_global!();
}

impl Dispatch<ZwlrOutputManagerV1, ()> for Pila {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &ZwlrOutputManagerV1,
        request: manager::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            manager::Request::CreateConfiguration { id, serial } => {
                let current = state.outputs.management.serial;
                let configuration = data_init.init(id, serial);
                if serial != current {
                    debug!(serial, current, "output configuration with a stale serial");
                    configuration.cancelled();
                    return;
                }
                state.outputs.management.configurations.insert(
                    configuration,
                    Configuration {
                        heads: HashMap::new(),
                    },
                );
            }
            manager::Request::Stop => {
                state.outputs.management.managers.remove(resource);
                resource.finished();
            }
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &ZwlrOutputManagerV1, _: &()) {
        state.outputs.management.managers.remove(resource);
    }
}

impl Dispatch<ZwlrOutputHeadV1, String> for Pila {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &ZwlrOutputHeadV1,
        _: head::Request,
        _: &String,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        // Only `release`.
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &ZwlrOutputHeadV1, name: &String) {
        for client in state.outputs.management.managers.values_mut() {
            if client.heads.get(name).is_some_and(|(h, _)| h == resource) {
                client.heads.remove(name);
            }
        }
    }
}

impl Dispatch<ZwlrOutputModeV1, (String, usize)> for Pila {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &ZwlrOutputModeV1,
        _: mode::Request,
        _: &(String, usize),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        // Only `release`.
    }
}

/// A configuration head's user data: the head it configures and its
/// configuration, or `None` when the configuration was cancelled.
pub type ConfigHeadData = Option<(String, ZwlrOutputConfigurationV1)>;

impl Dispatch<ZwlrOutputConfigurationV1, u32> for Pila {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &ZwlrOutputConfigurationV1,
        request: configuration::Request,
        serial: &u32,
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let live = state
            .outputs
            .management
            .configurations
            .contains_key(resource);
        match request {
            configuration::Request::EnableHead { id, head } => {
                let name = head.data::<String>().cloned().unwrap_or_default();
                if !live {
                    // Cancelled (stale serial) or already used.
                    data_init.init(id, None::<(String, ZwlrOutputConfigurationV1)>);
                    return;
                }
                let configuration = state
                    .outputs
                    .management
                    .configurations
                    .get_mut(resource)
                    .expect("live");
                if configuration.heads.contains_key(&name) {
                    data_init.init(id, None::<(String, ZwlrOutputConfigurationV1)>);
                    resource.post_error(
                        configuration::Error::AlreadyConfiguredHead,
                        format!("head {name} configured twice"),
                    );
                    return;
                }
                configuration.heads.insert(
                    name.clone(),
                    HeadConfig {
                        enabled: true,
                        ..Default::default()
                    },
                );
                data_init.init(id, Some((name, resource.clone())));
            }
            configuration::Request::DisableHead { head } => {
                if !live {
                    return;
                }
                let name = head.data::<String>().cloned().unwrap_or_default();
                let configuration = state
                    .outputs
                    .management
                    .configurations
                    .get_mut(resource)
                    .expect("live");
                if configuration.heads.contains_key(&name) {
                    resource.post_error(
                        configuration::Error::AlreadyConfiguredHead,
                        format!("head {name} configured twice"),
                    );
                    return;
                }
                configuration.heads.insert(name, HeadConfig::default());
            }
            configuration::Request::Apply | configuration::Request::Test => {
                let Some(configuration) = state.outputs.management.configurations.remove(resource)
                else {
                    // Cancelled ones were told so; a used one is an error.
                    if *serial == state.outputs.management.serial {
                        resource.post_error(
                            configuration::Error::AlreadyUsed,
                            "configuration applied or tested twice",
                        );
                    }
                    return;
                };
                let unconfigured = state
                    .outputs
                    .management
                    .heads
                    .iter()
                    .find(|h| !configuration.heads.contains_key(&h.name));
                if let Some(missing) = unconfigured {
                    resource.post_error(
                        configuration::Error::UnconfiguredHead,
                        format!("head {} wasn't enabled or disabled", missing.name),
                    );
                    return;
                }
                match state.check_output_configuration(&configuration.heads) {
                    Ok(()) => {}
                    Err(reason) => {
                        warn!(reason, "output configuration refused");
                        resource.failed();
                        return;
                    }
                }
                let applied = !matches!(request, configuration::Request::Apply)
                    || state.apply_output_configuration(configuration.heads);
                if applied {
                    resource.succeeded();
                } else {
                    resource.failed();
                }
                // Applying changed the heads: `done` follows `succeeded`.
                state.refresh_output_heads();
            }
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &ZwlrOutputConfigurationV1, _: &u32) {
        state.outputs.management.configurations.remove(resource);
    }
}

impl Dispatch<ZwlrOutputConfigurationHeadV1, ConfigHeadData> for Pila {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &ZwlrOutputConfigurationHeadV1,
        request: config_head::Request,
        data: &ConfigHeadData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let Some((name, configuration)) = data else {
            return;
        };
        let Some(config) = state
            .outputs
            .management
            .configurations
            .get_mut(configuration)
            .and_then(|c| c.heads.get_mut(name))
        else {
            return;
        };
        // Each property is set once (a mode or a custom mode: one of them).
        let set_before = match &request {
            config_head::Request::SetMode { .. } | config_head::Request::SetCustomMode { .. } => {
                config.mode.is_some().then_some("mode")
            }
            config_head::Request::SetPosition { .. } => {
                config.position.is_some().then_some("position")
            }
            config_head::Request::SetTransform { .. } => {
                config.transform.is_some().then_some("transform")
            }
            config_head::Request::SetScale { .. } => config.scale.is_some().then_some("scale"),
            config_head::Request::SetAdaptiveSync { .. } => {
                config.adaptive_sync.is_some().then_some("adaptive sync")
            }
            _ => None,
        };
        if let Some(what) = set_before {
            resource.post_error(config_head::Error::AlreadySet, format!("{what} set twice"));
            return;
        }
        match request {
            config_head::Request::SetMode { mode } => {
                let index = mode.data::<(String, usize)>();
                match index {
                    Some((head, i)) if head == name => config.mode = Some(ModeChoice::Index(*i)),
                    _ => resource.post_error(
                        config_head::Error::InvalidMode,
                        "the mode belongs to another head",
                    ),
                }
            }
            config_head::Request::SetCustomMode {
                width,
                height,
                refresh,
            } => {
                if width <= 0 || height <= 0 || refresh < 0 {
                    resource
                        .post_error(config_head::Error::InvalidCustomMode, "invalid custom mode");
                    return;
                }
                config.mode = Some(ModeChoice::Custom {
                    size: (width, height),
                    refresh_mhz: refresh,
                });
            }
            config_head::Request::SetPosition { x, y } => config.position = Some((x, y)),
            config_head::Request::SetTransform { transform } => match transform {
                WEnum::Value(transform) => config.transform = Some(Transform::from(transform)),
                WEnum::Unknown(_) => {
                    resource.post_error(config_head::Error::InvalidTransform, "invalid transform")
                }
            },
            config_head::Request::SetScale { scale } => {
                if scale <= 0.0 {
                    resource.post_error(config_head::Error::InvalidScale, "scale must be positive");
                    return;
                }
                config.scale = Some(scale);
            }
            config_head::Request::SetAdaptiveSync { state: wanted } => match wanted {
                WEnum::Value(AdaptiveSyncState::Enabled) => config.adaptive_sync = Some(true),
                WEnum::Value(AdaptiveSyncState::Disabled) => config.adaptive_sync = Some(false),
                _ => resource.post_error(
                    config_head::Error::InvalidAdaptiveSyncState,
                    "invalid adaptive sync state",
                ),
            },
            _ => {}
        }
    }
}
