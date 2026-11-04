//! wlr-gamma-control: lets one client per output set the output's gamma
//! ramps (night light: wlsunset, Noctalia). Bare metal only: the ramps go to
//! the CRTC through the DRM device (`Backend::set_gamma`); nested, every
//! request fails, as the host owns the display.
//!
//! The ramps are applied again whenever the output's CRTC is set up anew
//! (a VT switch back, a mode change, power on), and reset to linear when
//! the control is destroyed or its client goes away. A removed output fails
//! its control.

use std::{collections::HashMap, fs::File, io::Read};

use smithay::{
    output::Output,
    reexports::{
        wayland_protocols_wlr::gamma_control::v1::server::{
            zwlr_gamma_control_manager_v1::{self as manager, ZwlrGammaControlManagerV1},
            zwlr_gamma_control_v1::{self as control, ZwlrGammaControlV1},
        },
        wayland_server::{
            backend::ClientId, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New,
            Resource,
        },
    },
};
use tracing::{debug, warn};

use crate::Pila;

const VERSION: u32 = 1;

/// Read a client's ramp from `file` into `bytes`, without ever blocking:
/// the fd may be a pipe nobody writes to, and pila reads it on the main
/// thread. Data that isn't there yet means a short ramp; more than `bytes`
/// holds is a malformed one. A pipe still open for writing after the ramp
/// is fine.
fn read_ramp(mut file: File, bytes: &mut [u8]) -> std::io::Result<()> {
    use smithay::reexports::rustix::fs::{fcntl_getfl, fcntl_setfl, OFlags};
    let flags = fcntl_getfl(&file)?;
    fcntl_setfl(&file, flags | OFlags::NONBLOCK)?;
    file.read_exact(bytes)?;
    let mut extra = [0u8; 1];
    match file.read(&mut extra) {
        Ok(0) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
        Ok(_) => Err(std::io::Error::other("ramp too long")),
        Err(err) => Err(err),
    }
}

/// The live control of each output (`Pila::gamma_controls`).
pub type GammaControls = HashMap<Output, ZwlrGammaControlV1>;

/// A control's user data: its output (`None` if it failed at creation) and
/// the ramp size it was told.
pub struct GammaData {
    output: Option<Output>,
    size: u32,
}

/// Create the global. Called once from `Pila::new`.
pub fn init_global(dh: &DisplayHandle) {
    dh.create_global::<Pila, ZwlrGammaControlManagerV1, ()>(VERSION, ());
}

impl Pila {
    /// Drop a control: back to linear gamma.
    fn drop_gamma_control(&mut self, resource: &ZwlrGammaControlV1) {
        let Some(output) = self
            .gamma_controls
            .iter()
            .find(|(_, c)| *c == resource)
            .map(|(o, _)| o.clone())
        else {
            return;
        };
        self.gamma_controls.remove(&output);
        let reset = self
            .backend
            .as_mut()
            .is_some_and(|b| b.set_gamma(&output, None));
        if !reset {
            warn!(
                output = output.name(),
                "couldn't reset gamma after its control went away"
            );
        }
    }

    /// An output went away: its control is over.
    pub fn gamma_output_removed(&mut self, output: &Output) {
        if let Some(control) = self.gamma_controls.remove(output) {
            control.failed();
        }
    }
}

impl GlobalDispatch<ZwlrGammaControlManagerV1, ()> for Pila {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ZwlrGammaControlManagerV1>,
        _: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }

    // Not for sandboxed apps (`state::is_privileged`).
    fn can_view(client: Client, _: &()) -> bool {
        crate::state::is_privileged(&client)
    }
}

impl Dispatch<ZwlrGammaControlManagerV1, ()> for Pila {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &ZwlrGammaControlManagerV1,
        request: manager::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let manager::Request::GetGammaControl { id, output } = request else {
            return;
        };
        let output = Output::from_resource(&output);
        let size = output.as_ref().and_then(|o| {
            // One control per output at a time.
            (!state.gamma_controls.contains_key(o))
                .then(|| state.backend.as_ref().and_then(|b| b.gamma_size(o)))
                .flatten()
        });
        match (output, size) {
            (Some(output), Some(size)) => {
                let control = data_init.init(
                    id,
                    GammaData {
                        output: Some(output.clone()),
                        size,
                    },
                );
                control.gamma_size(size);
                debug!(output = output.name(), size, "gamma control taken");
                state.gamma_controls.insert(output, control);
            }
            _ => {
                let control = data_init.init(
                    id,
                    GammaData {
                        output: None,
                        size: 0,
                    },
                );
                control.failed();
            }
        }
    }
}

impl Dispatch<ZwlrGammaControlV1, GammaData> for Pila {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &ZwlrGammaControlV1,
        request: control::Request,
        data: &GammaData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let control::Request::SetGamma { fd } = request else {
            return;
        };
        let Some(output) = data
            .output
            .as_ref()
            .filter(|o| state.gamma_controls.get(o) == Some(resource))
        else {
            // Failed at creation, or superseded.
            return;
        };
        // Three ramps of `size` u16s: red, green, blue.
        let mut ramp = vec![0u16; data.size as usize * 3];
        let mut bytes = vec![0u8; ramp.len() * 2];
        let read = read_ramp(File::from(fd), &mut bytes);
        if let Err(err) = read {
            resource.post_error(
                control::Error::InvalidGamma,
                format!("gamma ramp isn't {} bytes: {err}", bytes.len()),
            );
            return;
        }
        for (value, pair) in ramp.iter_mut().zip(bytes.as_chunks::<2>().0) {
            *value = u16::from_ne_bytes(*pair);
        }
        let applied = state
            .backend
            .as_mut()
            .is_some_and(|b| b.set_gamma(output, Some(&ramp)));
        if !applied {
            warn!(output = output.name(), "failed to set gamma");
            resource.failed();
            state.drop_gamma_control(resource);
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &ZwlrGammaControlV1, _: &GammaData) {
        state.drop_gamma_control(resource);
    }
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::Write, os::fd::OwnedFd};

    use super::read_ramp;

    fn pipe() -> (File, std::io::PipeWriter) {
        let (reader, writer) = std::io::pipe().unwrap();
        (File::from(OwnedFd::from(reader)), writer)
    }

    #[test]
    fn a_pipe_nobody_writes_to_fails_instead_of_blocking() {
        let (reader, _writer) = pipe();
        let mut bytes = [0u8; 6];
        assert!(read_ramp(reader, &mut bytes).is_err());
    }

    #[test]
    fn a_ramp_from_a_pipe_still_open_is_read() {
        let (reader, mut writer) = pipe();
        writer.write_all(&[1, 2, 3, 4, 5, 6]).unwrap();
        let mut bytes = [0u8; 6];
        read_ramp(reader, &mut bytes).unwrap();
        assert_eq!(bytes, [1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn short_and_long_ramps_fail() {
        let (reader, mut writer) = pipe();
        writer.write_all(&[1, 2, 3]).unwrap();
        drop(writer);
        assert!(read_ramp(reader, &mut [0u8; 6]).is_err());

        let (reader, mut writer) = pipe();
        writer.write_all(&[0u8; 7]).unwrap();
        assert!(read_ramp(reader, &mut [0u8; 6]).is_err());
    }
}
