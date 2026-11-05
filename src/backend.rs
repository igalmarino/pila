//! The backend pila runs on: nested in another session (`winit.rs`), or on
//! bare metal (DRM/KMS + libinput, `udev.rs`), or headless for automated
//! tests (`headless.rs`). Code outside the backends goes
//! through these methods instead of matching on the variant.

use smithay::{
    backend::{renderer::gles::GlesRenderer, winit::WinitGraphicsBackend},
    input::{keyboard::LedState, pointer::CursorImageStatus},
    output::Output,
    utils::{Monotonic, Time},
};

use crate::{
    cursor::CursorTheme, headless::HeadlessData, output_management::HeadInfo, udev::UdevData,
};

pub enum Backend {
    Winit(Box<WinitGraphicsBackend<GlesRenderer>>),
    Udev(Box<UdevData>),
    Headless(Box<HeadlessData>),
}

impl Backend {
    /// The renderer that client buffers are imported with.
    pub fn renderer(&mut self) -> &mut GlesRenderer {
        match self {
            Backend::Winit(backend) => backend.renderer(),
            Backend::Udev(data) => &mut data.renderer,
            Backend::Headless(data) => &mut data.renderer,
        }
    }

    /// The renderer, and the cursor theme where pila draws named cursors
    /// itself (bare metal), for rendering outside the backend's own loop.
    pub fn renderer_and_cursor_theme(&mut self) -> (&mut GlesRenderer, Option<&mut CursorTheme>) {
        match self {
            Backend::Winit(backend) => (backend.renderer(), None),
            Backend::Udev(data) => {
                let (renderer, theme) = data.renderer_and_cursor_theme();
                (renderer, Some(theme))
            }
            Backend::Headless(data) => (&mut data.renderer, None),
        }
    }

    /// When a frame of `output` built `now` would show, for commits that ask
    /// to appear no earlier than a given moment (`commit_pacing.rs`). Nested
    /// and headless have no vblank to go by.
    pub fn next_frame_time(&self, output: &Output, now: Time<Monotonic>) -> Time<Monotonic> {
        let period = crate::frame_stats::refresh_period(output);
        let vblank = match self {
            Backend::Udev(data) => data.last_vblank_time(output),
            Backend::Winit(_) | Backend::Headless(_) => None,
        };
        match vblank {
            Some(last) => crate::commit_pacing::next_vblank(last, now.into(), period).into(),
            None => (std::time::Duration::from(now) + period).into(),
        }
    }

    /// Ask for a new frame on every output.
    pub fn queue_redraw(&mut self) {
        match self {
            Backend::Winit(backend) => backend.window().request_redraw(),
            Backend::Udev(data) => data.queue_redraw(),
            Backend::Headless(data) => data.queue_redraw(),
        }
    }

    /// The cursor image changed (a client or a grab set a new one).
    pub fn cursor_changed(&mut self, status: &CursorImageStatus) {
        match self {
            // The host draws named shapes; pila draws client surfaces itself.
            Backend::Winit(backend) => crate::winit::apply_cursor(backend.window(), status),
            Backend::Udev(data) => data.cursor_changed(status),
            // Nothing to show it on.
            Backend::Headless(_) => {}
        }
    }

    /// Caps/Num/Scroll Lock changed. Nested, the host owns the keyboard.
    pub fn update_leds(&mut self, leds: LedState) {
        if let Backend::Udev(data) = self {
            data.update_leds(leds);
        }
    }

    /// Switch to another virtual terminal. Nested, the host owns the TTY.
    pub fn change_vt(&mut self, vt: i32) {
        if let Backend::Udev(data) = self {
            data.change_vt(vt);
        }
    }

    /// The monitors for wlr-output-management; `None` nested, where the
    /// window is the one head and `Pila::output_heads` describes it.
    pub fn output_heads(&self) -> Option<Vec<HeadInfo>> {
        match self {
            Backend::Winit(_) => None,
            Backend::Udev(data) => Some(data.output_heads()),
            Backend::Headless(data) => Some(data.output_heads()),
        }
    }

    /// The length of each of `output`'s gamma ramps, if it has any.
    /// Nested, the host owns the display.
    pub fn gamma_size(&self, output: &Output) -> Option<u32> {
        match self {
            Backend::Winit(_) => None,
            Backend::Udev(data) => data.gamma_size(output),
            Backend::Headless(data) => data.gamma_size(output),
        }
    }

    /// Whether `output`'s display is on; `None` nested (the host owns it).
    pub fn output_power(&self, output: &Output) -> Option<bool> {
        match self {
            Backend::Winit(_) => None,
            Backend::Udev(data) => data.output_power(output),
            Backend::Headless(data) => data.output_power(output),
        }
    }

    /// Turn `output`'s display off or on. `false` if it couldn't be done.
    pub fn set_output_power(&mut self, output: &Output, on: bool) -> bool {
        match self {
            Backend::Winit(_) => false,
            Backend::Udev(data) => data.set_output_power(output, on),
            Backend::Headless(data) => data.set_output_power(output, on),
        }
    }

    /// Linear gamma everywhere, for exit.
    pub fn reset_gamma(&mut self) {
        match self {
            Backend::Udev(data) => data.reset_gamma(),
            Backend::Headless(data) => data.reset_gamma(),
            Backend::Winit(_) => {}
        }
    }

    /// Set `output`'s gamma ramps (red, green, blue, each `gamma_size` long,
    /// concatenated), or `None` for linear. `false` if it couldn't be set.
    pub fn set_gamma(&mut self, output: &Output, ramp: Option<&[u16]>) -> bool {
        match self {
            Backend::Winit(_) => false,
            Backend::Udev(data) => data.set_gamma(output, ramp),
            Backend::Headless(data) => data.set_gamma(output, ramp),
        }
    }
}
