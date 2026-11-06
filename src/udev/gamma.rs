//! Gamma ramps of a CRTC (`gamma_control.rs` sets them).

use super::*;

/// Set a CRTC's gamma ramps through the legacy ioctl, which atomic drivers
/// serve too.
pub(super) fn apply_gamma(device: &DrmDevice, crtc: crtc::Handle, ramp: &[u16], size: u32) -> bool {
    let n = size as usize;
    match device.set_gamma(crtc, &ramp[..n], &ramp[n..2 * n], &ramp[2 * n..3 * n]) {
        Ok(()) => true,
        Err(err) => {
            warn!(?err, "set_gamma failed");
            false
        }
    }
}

/// Identity ramps (red, green, blue) of `size` entries each.
pub(super) fn linear_ramp(size: u32) -> Vec<u16> {
    let one: Vec<u16> = (0..size).map(|i| linear_value(i, size)).collect();
    [one.clone(), one.clone(), one].concat()
}

/// Entry `i` of a linear ramp of `size` entries.
pub(super) fn linear_value(i: u32, size: u32) -> u16 {
    (u64::from(i) * 0xffff / u64::from(size - 1).max(1)) as u16
}

/// The ramp to remember: none if it's the linear one.
pub(super) fn ramp_if_set(ramp: &[u16], size: u32) -> Option<Vec<u16>> {
    let n = size as usize;
    let linear = (0..3 * n).all(|i| ramp[i] == linear_value((i % n) as u32, size));
    (!linear).then(|| ramp.to_vec())
}
