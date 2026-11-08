//! wlr-output-power-management: lets a client (wlopm, a shell's idle
//! handling) turn an output's display off and back on. Bare metal only
//! (`Backend::set_output_power`: the CRTC is cleared and set up again);
//! nested, every request fails, as the host owns the display.
//!
//! An output that's off keeps its place: windows stay, the pointer can move
//! over it, and the lock screen, if any, counts as shown there. Every object
//! of an output is told its new mode; a removed output fails them.

use smithay::{
    output::Output,
    reexports::{
        wayland_protocols_wlr::output_power_management::v1::server::{
            zwlr_output_power_manager_v1::{self as manager, ZwlrOutputPowerManagerV1},
            zwlr_output_power_v1::{self as power, Mode, ZwlrOutputPowerV1},
        },
        wayland_server::{
            backend::ClientId, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New,
            Resource, WEnum,
        },
    },
};
use tracing::{debug, warn};

use crate::Pila;

const VERSION: u32 = 1;

/// The live objects and their outputs (`Pila::outputs.power`).
pub type OutputPowers = Vec<(ZwlrOutputPowerV1, Output)>;

/// Create the global. Called once from `Pila::new`.
pub fn init_global(dh: &DisplayHandle) {
    dh.create_global::<Pila, ZwlrOutputPowerManagerV1, ()>(VERSION, ());
}

fn mode(on: bool) -> Mode {
    if on {
        Mode::On
    } else {
        Mode::Off
    }
}

impl Pila {
    /// An output went away: its power objects are over.
    pub fn output_power_removed(&mut self, output: &Output) {
        self.outputs.power.retain(|(resource, o)| {
            if o == output {
                resource.failed();
            }
            o != output
        });
    }
}

impl GlobalDispatch<ZwlrOutputPowerManagerV1, ()> for Pila {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ZwlrOutputPowerManagerV1>,
        _: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }

    crate::state::privileged_global!();
}

impl Dispatch<ZwlrOutputPowerManagerV1, ()> for Pila {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &ZwlrOutputPowerManagerV1,
        request: manager::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let manager::Request::GetOutputPower { id, output } = request else {
            return;
        };
        let output = Output::from_resource(&output);
        let on = output
            .as_ref()
            .and_then(|o| state.backend.as_ref().and_then(|b| b.output_power(o)));
        match (output, on) {
            (Some(output), Some(on)) => {
                let resource = data_init.init(id, Some(output.clone()));
                resource.mode(mode(on));
                state.outputs.power.push((resource, output));
            }
            _ => data_init.init(id, None).failed(),
        }
    }
}

impl Dispatch<ZwlrOutputPowerV1, Option<Output>> for Pila {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &ZwlrOutputPowerV1,
        request: power::Request,
        output: &Option<Output>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let power::Request::SetMode { mode: wanted } = request else {
            return;
        };
        let on = match wanted {
            WEnum::Value(Mode::On) => true,
            WEnum::Value(Mode::Off) => false,
            _ => {
                resource.post_error(power::Error::InvalidMode, "invalid power mode");
                return;
            }
        };
        let Some(output) = output else {
            // Failed at creation.
            return;
        };
        if !state.outputs.power.iter().any(|(r, _)| r == resource) {
            return;
        }
        debug!(output = output.name(), on, "output power request");
        let mode_before = output.current_mode();
        let done = state
            .backend
            .as_mut()
            .is_some_and(|b| b.set_output_power(output, on));
        if !done {
            warn!(
                output = output.name(),
                on, "couldn't set the output's power"
            );
            resource.failed();
            state.outputs.power.retain(|(r, _)| r != resource);
            return;
        }
        if output.current_mode() != mode_before {
            // Back on with a lighter mode: the one it had didn't fit.
            state.fit_to_outputs();
        }
        if !on {
            // Nothing draws there until it's back; what's kept for it is big.
            state.blur.forget_output(&output.name());
        }
        for (r, o) in &state.outputs.power {
            if o == output {
                r.mode(mode(on));
            }
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &ZwlrOutputPowerV1, _: &Option<Output>) {
        state.outputs.power.retain(|(r, _)| r != resource);
    }
}
