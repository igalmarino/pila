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
        if let Some(backend) = &mut self.backend {
            backend.set_gamma(&output, None);
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
        let mut file = File::from(fd);
        let read = file.read_exact(&mut bytes).and_then(|()| {
            // Exactly that much: a longer file is a malformed ramp.
            let mut extra = [0u8; 1];
            match file.read(&mut extra)? {
                0 => Ok(()),
                _ => Err(std::io::Error::other("ramp too long")),
            }
        });
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
