use smithay::{
    backend::{
        allocator::dmabuf::Dmabuf,
        renderer::{utils::on_commit_buffer_handler, ImportDma},
    },
    delegate_compositor, delegate_dmabuf, delegate_shm,
    reexports::wayland_server::{
        protocol::{wl_buffer, wl_surface::WlSurface},
        Client,
    },
    wayland::{
        buffer::BufferHandler,
        compositor::{
            get_parent, is_sync_subsurface, CompositorClientState, CompositorHandler,
            CompositorState,
        },
        dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
        shm::{ShmHandler, ShmState},
    },
};

use crate::{grabs::resize_grab, state::ClientState, Pila};

impl CompositorHandler for Pila {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);

        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self.window_for_surface(&root) {
                window.on_commit();
            }
        }

        self.handle_xdg_commit(surface);
        self.handle_layer_commit(surface);
        resize_grab::handle_commit(&mut self.space, surface);
        crate::cursor::handle_commit(&self.cursor_status, surface);
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
