//! The backend pila runs on: nested in another session (`winit.rs`), or on
//! bare metal (DRM/KMS + libinput, `udev/`), or headless for automated
//! tests (`headless.rs`). Code outside the backends goes
//! through these methods instead of matching on the variant.

use smithay::{
    backend::{renderer::gles::GlesRenderer, winit::WinitGraphicsBackend},
    input::{keyboard::LedState, pointer::CursorImageStatus},
    output::Output,
    utils::{Monotonic, Time},
};

use crate::{
    cursor::CursorTheme,
    headless::HeadlessData,
    output_management::HeadInfo,
    udev::{PrimaryRenderer, UdevData},
};

/// The renderer a backend lends (`Backend::renderer`): its own, or on bare
/// metal the primary GPU's, which the GPU manager keeps and lends for as long
/// as this lives (`docs/MULTI_GPU.md`). Derefs to the `GlesRenderer`.
pub enum RendererRef<'a> {
    Own(&'a mut GlesRenderer),
    Primary(PrimaryRenderer<'a>),
}

impl std::ops::Deref for RendererRef<'_> {
    type Target = GlesRenderer;
    fn deref(&self) -> &GlesRenderer {
        match self {
            RendererRef::Own(renderer) => renderer,
            RendererRef::Primary(renderer) => renderer.as_ref(),
        }
    }
}

impl std::ops::DerefMut for RendererRef<'_> {
    fn deref_mut(&mut self) -> &mut GlesRenderer {
        match self {
            RendererRef::Own(renderer) => renderer,
            RendererRef::Primary(renderer) => renderer.as_mut(),
        }
    }
}

pub enum Backend {
    Winit(Box<WinitGraphicsBackend<GlesRenderer>>),
    Udev(Box<UdevData>),
    Headless(Box<HeadlessData>),
}

impl Backend {
    /// The renderer that client buffers are imported with, and offscreen
    /// work done; `None` (logged) if bare metal's GPU manager can't lend it.
    pub fn renderer(&mut self) -> Option<RendererRef<'_>> {
        match self {
            Backend::Winit(backend) => Some(RendererRef::Own(backend.renderer())),
            Backend::Udev(data) => data.renderer().map(RendererRef::Primary),
            Backend::Headless(data) => Some(RendererRef::Own(&mut data.renderer)),
        }
    }

    /// The renderer, and the cursor theme where pila draws named cursors
    /// itself (bare metal), for rendering outside the backend's own loop.
    pub fn renderer_and_cursor_theme(
        &mut self,
    ) -> Option<(RendererRef<'_>, Option<&mut CursorTheme>)> {
        match self {
            Backend::Winit(backend) => Some((RendererRef::Own(backend.renderer()), None)),
            Backend::Udev(data) => {
                let (renderer, theme) = data.renderer_and_cursor_theme();
                Some((RendererRef::Primary(renderer?), Some(theme)))
            }
            Backend::Headless(data) => Some((RendererRef::Own(&mut data.renderer), None)),
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

    /// Ask for a new frame on `output`; `content`: what changed surely shows
    /// (`redraw.rs`). Nested there's one output, the window.
    pub fn queue_redraw_output(&mut self, output: &Output, content: crate::redraw::Content) {
        match self {
            Backend::Winit(backend) => backend.window().request_redraw(),
            Backend::Udev(data) => data.queue_redraw_output(output, content),
            Backend::Headless(data) => data.queue_redraw_output(output),
        }
    }

    /// Whether Pila draws the pointer as `status` shows it (else the host
    /// does, nested a named shape, or nothing shows).
    pub fn draws_cursor(&self, status: &CursorImageStatus) -> bool {
        match self {
            Backend::Udev(_) => !matches!(status, CursorImageStatus::Hidden),
            Backend::Winit(_) | Backend::Headless(_) => {
                matches!(status, CursorImageStatus::Surface(_))
            }
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

    /// Nested in another session's window: the host owns the display, so
    /// only the scale is pila's to change.
    pub fn is_nested(&self) -> bool {
        matches!(self, Backend::Winit(_))
    }

    /// Whether pila is the session (bare metal), whose environment the
    /// session bus and its services should get.
    pub fn is_session(&self) -> bool {
        matches!(self, Backend::Udev(_))
    }

    /// Re-apply `[input]` to the connected devices. Nested, the host owns them.
    pub fn apply_input_config(&mut self, input: &crate::config::InputConfig) {
        if let Backend::Udev(data) = self {
            data.apply_input_config(input);
        }
    }
}

/// Use the seat's other GPUs for their monitors, or stop, as `other-gpus`
/// says (bare metal only).
pub fn apply_other_gpus(state: &mut crate::Pila) {
    if let Some(Backend::Udev(_)) = state.backend {
        crate::udev::apply_other_gpus(state);
    }
}

/// Apply the outputs' settings (`[[output]]` with the runtime overrides on
/// top) as the backend can. Whether it could do all of it.
pub fn apply_output_config(state: &mut crate::Pila) -> bool {
    match state.backend {
        Some(Backend::Udev(_)) => crate::udev::apply_output_config(state),
        Some(Backend::Winit(_)) => {
            state.apply_nested_scale();
            true
        }
        Some(Backend::Headless(_)) => crate::headless::apply_output_config(state),
        None => true,
    }
}
