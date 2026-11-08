//! Gamma ramps of a CRTC (`gamma_control.rs` sets them).
//!
//! Through the legacy ioctl: Smithay has no way to put `GAMMA_LUT` in the
//! atomic commits its DRM compositor makes, and a commit of pila's own
//! would race them. On an atomic driver the kernel turns the ioctl into a
//! blocking commit, which may hold the event loop up to a refresh: against
//! the performance pillar, if it does. How long it takes is logged
//! (`pila::frames`, and once in the plain log past the frame budget) to
//! tell whether it's worth taking to Smithay.

use std::sync::atomic::{AtomicBool, Ordering};

use super::*;

/// The CPU budget of a frame (CLAUDE.md, "Performance pillar").
const FRAME_BUDGET: Duration = Duration::from_millis(4);

/// Set a CRTC's gamma ramps through the legacy ioctl, which atomic drivers
/// serve too.
pub(super) fn apply_gamma(device: &DrmDevice, crtc: crtc::Handle, ramp: &[u16], size: u32) -> bool {
    let n = size as usize;
    let started = std::time::Instant::now();
    let result = device.set_gamma(crtc, &ramp[..n], &ramp[n..2 * n], &ramp[2 * n..3 * n]);
    let took = started.elapsed();
    let ms = took.as_secs_f64() * 1000.0;
    tracing::debug!(target: "pila::frames", ms, "gamma");
    static WARNED: AtomicBool = AtomicBool::new(false);
    if took > FRAME_BUDGET && !WARNED.swap(true, Ordering::Relaxed) {
        warn!(
            ms,
            "setting gamma held the event loop past a frame's budget (logged once)"
        );
    }
    match result {
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
