//! Input devices on bare metal: keyboards for their LEDs, `[input]` on
//! touchpads and mice.

use super::*;

/// Device setup before an input event is processed: keyboards for LEDs,
/// `[input]` settings on touchpads and mice.
pub(super) fn on_input_device(state: &mut Pila, event: &mut InputEvent<LibinputInputBackend>) {
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
pub(super) fn pointer_config<'a>(
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
pub(super) fn configure_pointer(device: &mut libinput::Device, config: &PointerConfig) {
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
