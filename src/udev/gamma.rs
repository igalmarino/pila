//! Gamma ramps of a CRTC (`gamma_control.rs` sets them).
//!
//! A client's change (a night light, every step of its fade) goes out as an
//! atomic commit of the CRTC's `GAMMA_LUT` alone, without waiting
//! (`commit_pending_gamma`): sent while no frame is on its way, it takes the
//! next vblank as a frame would, and drawing resumes at that vblank's event
//! (`frame_finish`). Smithay's DRM compositor has no way to put it in a frame's
//! own commit. The legacy ioctl, which atomic drivers turn into a blocking
//! commit that holds the event loop for a whole refresh (measured: 16.7 ms
//! at 60 Hz), stays for drivers without `GAMMA_LUT`, a commit the driver
//! refuses, and setting gamma again after a modeset, which blocks anyway.
//! Either way how long it took is logged (`pila::frames`, and once in the
//! plain log past the frame budget).

use std::{
    os::fd::AsFd,
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

use super::*;

/// The CPU budget of a frame (CLAUDE.md, "Performance pillar").
const FRAME_BUDGET: Duration = Duration::from_millis(4);

/// Set a CRTC's gamma ramps through the legacy ioctl, which atomic drivers
/// serve too, as a blocking commit.
pub(super) fn apply_gamma(device: &DrmDevice, crtc: crtc::Handle, ramp: &[u16], size: u32) -> bool {
    let n = size as usize;
    let started = Instant::now();
    let result = device.set_gamma(crtc, &ramp[..n], &ramp[n..2 * n], &ramp[2 * n..3 * n]);
    record_time(started, "ioctl");
    match result {
        Ok(()) => true,
        Err(err) => {
            warn!(?err, "set_gamma failed");
            false
        }
    }
}

/// Log how long setting gamma held the event loop, and which way it went
/// (`commit` or the blocking `ioctl`).
fn record_time(started: Instant, how: &str) {
    let took = started.elapsed();
    let ms = took.as_secs_f64() * 1000.0;
    tracing::debug!(target: "pila::frames", ms, how, "gamma");
    static WARNED: AtomicBool = AtomicBool::new(false);
    if took > FRAME_BUDGET && !WARNED.swap(true, Ordering::Relaxed) {
        warn!(
            ms,
            "setting gamma held the event loop past a frame's budget (logged once)"
        );
    }
}

/// The CRTC's `GAMMA_LUT` property, on an atomic device that has one.
pub(super) fn gamma_lut_property(
    drm: &PilaDrmOutputManager,
    crtc: crtc::Handle,
) -> Option<property::Handle> {
    let device = drm.device();
    if !device.is_atomic() {
        return None;
    }
    let props = device.get_properties(crtc).ok()?;
    props.into_iter().find_map(|(handle, _)| {
        let info = device.get_property(handle).ok()?;
        (info.name().to_str() == Ok("GAMMA_LUT")).then_some(handle)
    })
}

/// Ramps (red, green, blue, `size` entries each) as a `GAMMA_LUT` blob: an
/// array of `struct drm_color_lut { __u16 red, green, blue, reserved; }`.
/// The legacy ramp size is one every driver takes: it's what the kernel's
/// own legacy gamma path puts in `GAMMA_LUT`.
fn lut_bytes(ramp: &[u16], size: u32) -> Vec<u8> {
    let n = size as usize;
    (0..n)
        .flat_map(|i| [ramp[i], ramp[n + i], ramp[2 * n + i], 0])
        .flat_map(u16::to_ne_bytes)
        .collect()
}

/// Commit `ramp` as the CRTC's `GAMMA_LUT` without waiting: the kernel sets
/// it at the next vblank and sends that vblank's event, as for a frame.
/// `false` if it wasn't taken.
fn commit_gamma_lut(
    device: &DrmDevice,
    crtc: crtc::Handle,
    prop: property::Handle,
    ramp: &[u16],
    size: u32,
) -> bool {
    let started = Instant::now();
    let mut bytes = lut_bytes(ramp, size);
    let blob = match drm_ffi::mode::create_property_blob(device.as_fd(), &mut bytes) {
        Ok(blob) => u64::from(blob.blob_id),
        Err(err) => {
            debug!(?err, "couldn't make a GAMMA_LUT blob");
            return false;
        }
    };
    let mut request = AtomicModeReq::new();
    request.add_property(crtc, prop, property::Value::Blob(blob));
    let result = device.atomic_commit(
        AtomicCommitFlags::NONBLOCK | AtomicCommitFlags::PAGE_FLIP_EVENT,
        request,
    );
    // The CRTC's state holds a reference of its own.
    if let Err(err) = device.destroy_property_blob(blob) {
        debug!(?err, "couldn't destroy a GAMMA_LUT blob");
    }
    record_time(started, "commit");
    match result {
        Ok(()) => true,
        Err(err) => {
            debug!(?err, "GAMMA_LUT commit refused");
            false
        }
    }
}

/// Send the output's pending gamma, if it has some and the CRTC is free (no
/// frame on its way). `true` if it went out as a commit of its own: drawing
/// waits for its vblank. Refused, it's set the blocking way, and drawing
/// goes on.
pub(super) fn commit_pending_gamma(data: &mut UdevData, crtc: crtc::Handle) -> bool {
    let active = data.session.is_active();
    let UdevData { outputs, drm, .. } = data;
    let Some(surface) = outputs.get_mut(&crtc) else {
        return false;
    };
    if surface.gamma_pending.is_none()
        || surface.frame_pending
        || surface.drm_output.is_none()
        || !active
        || surface.recovery.blocks_render()
    {
        return false;
    }
    let Some(ramp) = surface.gamma_pending.take() else {
        return false;
    };
    let size = (ramp.len() / 3) as u32;
    if let Some(prop) = surface.gamma_lut {
        if commit_gamma_lut(drm.device(), crtc, prop, &ramp, size) {
            surface.frame_pending = true;
            surface.gamma_in_flight = true;
            return true;
        }
    }
    if !apply_gamma(drm.device(), crtc, &ramp, size) {
        warn!(name = surface.output.name(), "failed to set gamma");
    }
    false
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gamma_lut_interleaves_the_ramps_with_a_padding_entry() {
        // Two entries: red 1, 2; green 3, 4; blue 5, 6.
        let bytes = lut_bytes(&[1, 2, 3, 4, 5, 6], 2);
        let entries: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_ne_bytes(*b))
            .collect();
        assert_eq!(entries, [1, 3, 5, 0, 2, 4, 6, 0]);
        // `struct drm_color_lut` is 8 bytes.
        assert_eq!(bytes.len(), 2 * 8);
    }
}
