use smithay::{
    backend::{
        allocator::dmabuf::Dmabuf,
        renderer::{utils::on_commit_buffer_handler, ImportDma},
    },
    delegate_compositor, delegate_dmabuf, delegate_shm,
    reexports::{
        calloop::Interest,
        wayland_server::{
            protocol::{wl_buffer, wl_surface::WlSurface},
            Client, Resource,
        },
    },
    wayland::{
        buffer::BufferHandler,
        compositor::{
            add_blocker, add_pre_commit_hook, get_parent, is_sync_subsurface, with_states,
            BufferAssignment, CompositorClientState, CompositorHandler, CompositorState,
            SurfaceAttributes,
        },
        dmabuf::{get_dmabuf, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
        shm::{ShmHandler, ShmState},
    },
};

use smithay::xwayland::XWaylandClientData;

use crate::{grabs::resize_grab, state::ClientState, Pila};

impl CompositorHandler for Pila {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        // Xwayland is a client too, with Smithay's own client data.
        if let Some(state) = client.get_data::<XWaylandClientData>() {
            return &state.compositor_state;
        }
        &client
            .get_data::<ClientState>()
            .expect("client without pila's client data")
            .compositor_state
    }

    fn new_surface(&mut self, surface: &WlSurface) {
        add_pre_commit_hook::<Self, _>(surface, |state, _, surface| {
            state.wait_for_client_rendering(surface);
            state.hold_commit_until_timestamp(surface);
        });
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        self.commit_paced(surface);

        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self.window_for_surface(&root) {
                window.on_commit();
                log_size_change(&window);
                self.geometry_committed(&window);
            }
        }

        self.handle_xdg_commit(surface);
        self.handle_layer_commit(surface);
        self.update_foreign_toplevel(surface);
        self.handle_lock_commit(surface);
        if let Some(moved) = resize_grab::handle_commit(&mut self.space, surface) {
            self.keep_transients_above(&moved);
        }
        crate::cursor::handle_commit(&self.cursor_status, surface);
    }
}

impl Pila {
    /// Called before a commit applies: if it brings a dmabuf the client's GPU
    /// is still rendering to, hold the commit back until it's done. Drawing
    /// it now would make pila's frame wait for the client's, and miss a
    /// refresh.
    fn wait_for_client_rendering(&mut self, surface: &WlSurface) {
        let dmabuf = with_states(surface, |states| {
            let mut attributes = states.cached_state.get::<SurfaceAttributes>();
            match attributes.pending().buffer.as_ref() {
                Some(BufferAssignment::NewBuffer(buffer)) => get_dmabuf(buffer).ok().cloned(),
                _ => None,
            }
        });
        let (Some(dmabuf), Some(client)) = (dmabuf, surface.client()) else {
            return;
        };
        // An error: it's ready already.
        let Ok((blocker, source)) = dmabuf.generate_blocker(Interest::READ) else {
            return;
        };
        let inserted = self.loop_handle.insert_source(source, move |_, _, state| {
            let display = state.display_handle.clone();
            state
                .client_compositor_state(&client)
                .blocker_cleared(state, &display);
            // Outside a client's dispatch: nothing else asks for this redraw.
            state.queue_redraw();
            Ok(())
        });
        if inserted.is_ok() {
            add_blocker(surface, blocker);
        }
    }
}

/// A window's size at its last commit, to log when it changes.
#[derive(Default)]
struct LastSize(std::cell::Cell<Option<smithay::utils::Size<i32, smithay::utils::Logical>>>);

/// Log a window whose size changed with a commit (debug, `pila::state`):
/// a client resizing itself, or answering a configure. `scripts/check`
/// reads these.
fn log_size_change(window: &smithay::desktop::Window) {
    window.user_data().insert_if_missing(LastSize::default);
    let Some(last) = window.user_data().get::<LastSize>() else {
        return;
    };
    // Empty before the first buffer and after the last: not a size.
    let size = window.geometry().size;
    if size.is_empty() {
        return;
    }
    if last.0.replace(Some(size)) != Some(size) {
        let app_id = crate::foreign_toplevel::title_and_app_id(window).1;
        tracing::debug!(target: "pila::state", app_id, w = size.w, h = size.h, "window size");
    }
}

impl BufferHandler for Pila {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for Pila {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl DmabufHandler for Pila {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        // Test-import now so a bad buffer fails at creation (the client gets
        // `failed`) rather than silently at render time.
        let imported = self
            .backend
            .as_mut()
            .is_some_and(|backend| backend.renderer().import_dmabuf(&dmabuf, None).is_ok());
        if imported {
            let _ = notifier.successful::<Pila>();
        } else {
            notifier.failed();
        }
    }
}

delegate_compositor!(Pila);
delegate_dmabuf!(Pila);
delegate_shm!(Pila);
