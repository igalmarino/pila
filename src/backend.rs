//! The backend pila runs on: nested in another session (`winit.rs`), or on
//! bare metal (DRM/KMS + libinput, `udev.rs`). Code outside the backends goes
//! through these methods instead of matching on the variant.

use smithay::{
    backend::{renderer::gles::GlesRenderer, winit::WinitGraphicsBackend},
    input::{keyboard::LedState, pointer::CursorImageStatus},
};

use crate::udev::UdevData;

pub enum Backend {
    Winit(Box<WinitGraphicsBackend<GlesRenderer>>),
    Udev(Box<UdevData>),
}

impl Backend {
    /// The renderer that client buffers are imported with.
    pub fn renderer(&mut self) -> &mut GlesRenderer {
        match self {
            Backend::Winit(backend) => backend.renderer(),
            Backend::Udev(data) => &mut data.renderer,
        }
    }

    /// Ask for a new frame on every output.
    pub fn queue_redraw(&mut self) {
        match self {
            Backend::Winit(backend) => backend.window().request_redraw(),
            Backend::Udev(data) => data.queue_redraw(),
        }
    }

    /// The cursor image changed (a client or a grab set a new one).
    pub fn cursor_changed(&mut self, status: &CursorImageStatus) {
        match self {
            // The host draws named shapes; pila draws client surfaces itself.
            Backend::Winit(backend) => crate::winit::apply_cursor(backend.window(), status),
            Backend::Udev(data) => data.cursor_changed(status),
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
}
