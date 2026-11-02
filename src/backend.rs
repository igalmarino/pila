//! The backend pila runs on: nested in another session (`winit.rs`), or on
//! bare metal (DRM/KMS + libinput, `udev.rs`). Code outside the backends goes
//! through these methods instead of matching on the variant.

use smithay::{
    backend::{renderer::gles::GlesRenderer, winit::WinitGraphicsBackend},
    input::pointer::CursorImageStatus,
};

pub enum Backend {
    Winit(WinitGraphicsBackend<GlesRenderer>),
}

impl Backend {
    /// The renderer that client buffers are imported with.
    pub fn renderer(&mut self) -> &mut GlesRenderer {
        match self {
            Backend::Winit(backend) => backend.renderer(),
        }
    }

    /// Ask for a new frame on every output.
    pub fn queue_redraw(&mut self) {
        match self {
            Backend::Winit(backend) => backend.window().request_redraw(),
        }
    }

    /// The cursor image changed (a client or a grab set a new one).
    pub fn cursor_changed(&mut self, status: &CursorImageStatus) {
        match self {
            // The host draws named shapes; pila draws client surfaces itself.
            Backend::Winit(backend) => crate::winit::apply_cursor(backend.window(), status),
        }
    }
}
