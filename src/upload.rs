//! Clients' shm buffers uploaded when they're committed, not in the frame that
//! shows them (see "Uploads" in docs/ARCHITECTURE.md).
//!
//! A buffer in shared memory reaches the GPU as a copy the CPU makes
//! (`GlesRenderer::import_shm_buffer`: `glTexImage2D`, `glTexSubImage2D` of its
//! damage). Smithay makes it when the frame's elements are built, so a frame
//! showing a whole-screen picture (swaybg's, a fullscreen foot scrolling) pays
//! milliseconds for it on top of its own work, and on bare metal a frame held to
//! be drawn close to its vblank (`udev/render_late.rs`) may miss it. Made at the
//! commit instead, which comes early in the period, the copy is done when the
//! frame is built and finds the texture there (Smithay keeps it per renderer:
//! the one `Backend::renderer` lends is the one frames build their elements
//! with). Its time is counted apart, `uploads` on `pila::frames`.
//!
//! Only for what a frame would upload anyway: a surface shown on an output
//! (not a minimized window, nor one on another workspace; while locked, only
//! a lock surface), once between frames (a client committing faster than the
//! outputs refresh leaves the rest to the frame, as before: Smithay gives the
//! frame the damage since the last commit uploaded).

use std::cell::Cell;
use std::time::Instant;

use smithay::backend::renderer::{
    buffer_type,
    gles::GlesTexture,
    utils::{import_surface_tree, RendererSurfaceStateUserData},
    BufferType, ContextId, Renderer,
};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::{
    is_sync_subsurface, with_states, with_surface_tree_downward, TraversalAction,
};

use crate::{sync::Locked, Pila};

/// The frame (`Pila::frames_drawn`) after which a surface's buffer was last
/// uploaded at its commit: one upload per surface between frames.
#[derive(Default)]
struct UploadedAfter(Cell<Option<u64>>);

impl Pila {
    /// Upload the shm buffers `surface`'s commit brought, if a frame will
    /// show them (see the module docs). Called from the commit handler.
    pub fn upload_committed(&mut self, surface: &WlSurface) {
        // A sync subsurface's state applies with its parent's commit.
        if is_sync_subsurface(surface) {
            return;
        }
        if self.is_locked() && !self.is_lock_surface(surface) {
            return;
        }
        if self
            .surface_outputs(surface)
            .is_none_or(|outputs| outputs.is_empty())
        {
            return;
        }
        let frame = self.frames_drawn;
        let uploaded = with_states(surface, |states| {
            states
                .data_map
                .get::<UploadedAfter>()
                .is_some_and(|after| after.0.get() == Some(frame))
        });
        if uploaded {
            return;
        }
        // Most clients' buffers are dmabufs: no renderer asked for them.
        if !needs_upload(surface, None) {
            return;
        }
        let Some(backend) = self.backend.as_mut() else {
            return;
        };
        let Some(mut renderer) = backend.renderer() else {
            return;
        };
        if !needs_upload(surface, Some(&renderer.context_id())) {
            return;
        }
        let size = with_states(surface, |states| {
            states
                .data_map
                .get::<RendererSurfaceStateUserData>()
                .and_then(|s| s.locked().buffer_size())
                .map(|sz| (sz.w, sz.h))
        });
        let started = Instant::now();
        if let Err(err) = import_surface_tree(&mut *renderer, surface) {
            // The frame tries again.
            tracing::debug!(%err, "a committed buffer couldn't be uploaded");
        }
        let cpu = started.elapsed();
        drop(renderer);
        with_states(surface, |states| {
            states
                .data_map
                .get_or_insert(UploadedAfter::default)
                .0
                .set(Some(frame));
        });
        self.early_uploads += 1;
        self.record_upload(cpu, size);
    }
}

/// Whether a surface of `surface`'s tree shows an shm buffer that has no
/// texture yet on the renderer of `context` (any shm buffer, with `None`).
fn needs_upload(surface: &WlSurface, context: Option<&ContextId<GlesTexture>>) -> bool {
    let needed = Cell::new(false);
    with_surface_tree_downward(
        surface,
        (),
        |_, _, _| TraversalAction::DoChildren(()),
        |_, states, _| {
            if needed.get() {
                return;
            }
            let Some(state) = states.data_map.get::<RendererSurfaceStateUserData>() else {
                return;
            };
            let state = state.locked();
            needed.set(
                state
                    .buffer()
                    .is_some_and(|buffer| matches!(buffer_type(buffer), Some(BufferType::Shm)))
                    && context.is_none_or(|context| {
                        state.texture::<GlesTexture>(context.clone()).is_none()
                    }),
            );
        },
        |_, _, _| !needed.get(),
    );
    needed.get()
}
