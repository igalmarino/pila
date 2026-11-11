use smithay::{
    backend::{
        allocator::dmabuf::Dmabuf,
        renderer::{utils::on_commit_buffer_handler, ImportDma},
    },
    reexports::{
        calloop::{EventSource, Interest},
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
        drm_syncobj::{DrmSyncobjCachedState, DrmSyncobjHandler, DrmSyncobjState},
        shm::{ShmHandler, ShmState},
    },
};

use smithay::xwayland::XWaylandClientData;

use crate::{grabs::resize_grab, state::ClientState, Pila};

impl CompositorHandler for Pila {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.protocols.compositor
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        // Xwayland is a client too, with Smithay's own client data.
        if let Some(state) = client.get_data::<XWaylandClientData>() {
            return &state.compositor_state;
        }
        // Every client is inserted with it (`init_wayland_listener`), but
        // Xwayland, handled above.
        &client
            .get_data::<ClientState>()
            .expect("client without pila's client data")
            .compositor_state
    }

    fn new_surface(&mut self, surface: &WlSurface) {
        add_pre_commit_hook::<Self, _>(surface, |state, _, surface| {
            crate::surface_limits::check_commit(surface);
            crate::redraw::note_pending_content(surface);
            state.wait_for_client_rendering(surface);
            state.hold_commit_until_timestamp(surface);
        });
    }

    fn commit(&mut self, surface: &WlSurface) {
        // Where it showed before: a window that unmaps shows nowhere after.
        let before = self.surface_outputs(surface);
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
        self.handle_dnd_icon_commit(surface);
        self.redraw_committed(surface, before);
    }

    // Destroying a subsurface takes it off its parent at once, no commit
    // needed (and a role's destruction may come with no commit either).
    fn destroyed(&mut self, surface: &WlSurface) {
        match self.surface_outputs(surface) {
            Some(outputs) => outputs.iter().for_each(|o| self.queue_redraw_output(o)),
            None => self.queue_redraw(),
        }
    }
}

impl Pila {
    /// The drag icon moves by the offset its client gives a new buffer.
    fn handle_dnd_icon_commit(&mut self, surface: &WlSurface) {
        let Some(icon) = self
            .dnd_icon
            .as_mut()
            .filter(|icon| icon.surface == *surface)
        else {
            return;
        };
        let delta = with_states(surface, |states| {
            states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .buffer_delta
                .take()
        });
        if let Some(delta) = delta {
            icon.offset += delta;
        }
        // Its redraw is the commit's (`redraw_committed`).
    }

    /// Called before a commit applies: if it brings a dmabuf the client's GPU
    /// is still rendering to, hold the commit back until it's done: until its
    /// acquire point if it gave one (linux-drm-syncobj), else its implicit
    /// fence. Drawing it now would make pila's frame wait for the client's,
    /// and miss a refresh.
    fn wait_for_client_rendering(&mut self, surface: &WlSurface) {
        let (dmabuf, acquire_point) = with_states(surface, |states| {
            let mut attributes = states.cached_state.get::<SurfaceAttributes>();
            let dmabuf = match attributes.pending().buffer.as_ref() {
                Some(BufferAssignment::NewBuffer(buffer)) => get_dmabuf(buffer).ok().cloned(),
                _ => None,
            };
            let mut sync = states.cached_state.get::<DrmSyncobjCachedState>();
            (dmabuf, sync.pending().acquire_point.clone())
        });
        let Some(dmabuf) = dmabuf else {
            return;
        };
        if let Some(Ok((blocker, source))) = acquire_point.map(|point| point.generate_blocker()) {
            if self.apply_when_ready(surface, source) {
                add_blocker(surface, blocker);
                return;
            }
        }
        // An error: it's ready already.
        if let Ok((blocker, source)) = dmabuf.generate_blocker(Interest::READ) {
            if self.apply_when_ready(surface, source) {
                add_blocker(surface, blocker);
            }
        }
    }

    /// When `source` fires, apply the commits of `surface`'s client that were
    /// waiting for it, and draw them. Whether that could be set up.
    fn apply_when_ready<S>(&mut self, surface: &WlSurface, source: S) -> bool
    where
        S: EventSource<Event = (), Ret = std::io::Result<()>> + 'static,
    {
        let Some(client) = surface.client() else {
            return false;
        };
        self.loop_handle
            .insert_source(source, move |_, _, state| {
                let display = state.display_handle.clone();
                // The commits that apply ask for their redraws
                // (`redraw_committed`); outside a client's dispatch, the rest
                // is `after_frames`' too.
                state
                    .client_compositor_state(&client)
                    .blocker_cleared(state, &display);
                state.housekeeping = true;
                Ok(())
            })
            .is_ok()
    }
}

impl DrmSyncobjHandler for Pila {
    fn drm_syncobj_state(&mut self) -> Option<&mut DrmSyncobjState> {
        self.protocols.syncobj.as_mut()
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
        &self.protocols.shm
    }
}

impl DmabufHandler for Pila {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.protocols.dmabuf
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
            .and_then(|backend| backend.renderer())
            .is_some_and(|mut renderer| renderer.import_dmabuf(&dmabuf, None).is_ok());
        if imported {
            // Where it was imported: what lets an output of that GPU show it
            // as it is (`Backend::dmabuf_node`). A client may have said
            // (`set_sampling_device`).
            if dmabuf.node().is_none() {
                if let Some(node) = self.backend.as_ref().and_then(|b| b.dmabuf_node()) {
                    dmabuf.set_node(node);
                }
            }
            let _ = notifier.successful::<Pila>();
        } else {
            notifier.failed();
        }
    }
}
