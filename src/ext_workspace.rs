//! ext-workspace: lets taskbars and panels list the workspaces
//! (`workspaces.rs`), see which is active and switch to another.
//!
//! pila's workspaces span all outputs, so each client gets one workspace
//! group holding every output (the `wl_output`s it bound) and all the
//! workspaces, named "1", "2", ... with those as their ids and 1D
//! coordinates. Panels only: a sandboxed app doesn't get to see or switch
//! workspaces (`state::is_privileged`). The only capability is `activate`; requests are applied on
//! the manager's `commit` (the last activation wins) and ignored while the
//! session is locked. Listeners are kept up to date by diffing
//! (`Pila::refresh_workspaces`): after a switch, a change in the number of
//! workspaces, a change in the outputs (`refresh_output_heads`) and a client
//! binding a `wl_output` (`OutputHandler::output_bound`).

use smithay::reexports::{
    wayland_protocols::ext::workspace::v1::server::{
        ext_workspace_group_handle_v1::{
            self as group, ExtWorkspaceGroupHandleV1, GroupCapabilities,
        },
        ext_workspace_handle_v1::{self as workspace, ExtWorkspaceHandleV1, WorkspaceCapabilities},
        ext_workspace_manager_v1::{self as manager, ExtWorkspaceManagerV1},
    },
    wayland_server::{
        backend::ClientId, protocol::wl_output::WlOutput, Client, DataInit, Dispatch,
        DisplayHandle, GlobalDispatch, New, Resource,
    },
};

use crate::Pila;

const VERSION: u32 = 1;

/// The protocol's state (`Pila::ext_workspaces`).
#[derive(Default)]
pub struct ExtWorkspaces {
    managers: Vec<Manager>,
}

/// One bound manager and what its client was told.
struct Manager {
    manager: ExtWorkspaceManagerV1,
    group: ExtWorkspaceGroupHandleV1,
    /// Indexed by workspace.
    workspaces: Vec<ExtWorkspaceHandleV1>,
    active: usize,
    outputs: Vec<WlOutput>,
    /// Asked to activate this workspace, on the next `commit`.
    pending: Option<usize>,
}

/// Create the global. Called once from `Pila::new`.
pub fn init_global(dh: &DisplayHandle) {
    dh.create_global::<Pila, ExtWorkspaceManagerV1, ()>(VERSION, ());
}

fn state(active: bool) -> workspace::State {
    if active {
        workspace::State::Active
    } else {
        workspace::State::empty()
    }
}

impl Manager {
    /// Announce workspace `index` and put it in the group.
    fn add_workspace(&mut self, dh: &DisplayHandle, index: usize) {
        let Some(client) = self.manager.client() else {
            return;
        };
        let Ok(handle) = client.create_resource::<ExtWorkspaceHandleV1, usize, Pila>(
            dh,
            self.manager.version(),
            index,
        ) else {
            return;
        };
        self.manager.workspace(&handle);
        let name = (index + 1).to_string();
        handle.id(name.clone());
        handle.name(name);
        handle.coordinates((index as u32).to_ne_bytes().to_vec());
        handle.state(state(index == self.active));
        handle.capabilities(WorkspaceCapabilities::Activate);
        self.group.workspace_enter(&handle);
        self.workspaces.push(handle);
    }

    /// Send what changed; returns whether anything did.
    fn update(
        &mut self,
        dh: &DisplayHandle,
        count: usize,
        active: usize,
        outputs: &[WlOutput],
    ) -> bool {
        let mut changed = false;
        while self.workspaces.len() > count {
            let handle = self.workspaces.pop().unwrap();
            self.group.workspace_leave(&handle);
            handle.removed();
            changed = true;
        }
        if self.active != active {
            if let Some(old) = self.workspaces.get(self.active) {
                old.state(state(false));
            }
            if let Some(new) = self.workspaces.get(active) {
                new.state(state(true));
            }
            self.active = active;
            changed = true;
        }
        while self.workspaces.len() < count {
            self.add_workspace(dh, self.workspaces.len());
            changed = true;
        }
        let group = &self.group;
        self.outputs.retain(|o| {
            let keep = o.is_alive() && outputs.contains(o);
            if !keep {
                if o.is_alive() {
                    group.output_leave(o);
                }
                changed = true;
            }
            keep
        });
        for o in outputs {
            if !self.outputs.contains(o) {
                self.group.output_enter(o);
                self.outputs.push(o.clone());
                changed = true;
            }
        }
        changed
    }
}

impl Pila {
    /// The `wl_output`s `client` bound for the outputs in use.
    fn client_outputs(&self, client: &Client) -> Vec<WlOutput> {
        self.space
            .outputs()
            .flat_map(|o| o.client_outputs(client))
            .collect()
    }

    /// Bring every listener up to date.
    pub fn refresh_workspaces(&mut self) {
        if self.ext_workspaces.managers.is_empty() {
            return;
        }
        let dh = self.display_handle.clone();
        let (count, active) = (self.config.workspaces, self.workspaces.active);
        let mut managers = std::mem::take(&mut self.ext_workspaces.managers);
        for m in &mut managers {
            let outputs = match m.manager.client() {
                Some(client) => self.client_outputs(&client),
                None => Vec::new(),
            };
            if m.update(&dh, count, active, &outputs) {
                m.manager.done();
            }
        }
        // Binds can't happen while this runs, but keep any just in case.
        managers.append(&mut self.ext_workspaces.managers);
        self.ext_workspaces.managers = managers;
    }

    fn workspace_manager_mut(&mut self, resource: &ExtWorkspaceManagerV1) -> Option<&mut Manager> {
        self.ext_workspaces
            .managers
            .iter_mut()
            .find(|m| m.manager == *resource)
    }
}

impl GlobalDispatch<ExtWorkspaceManagerV1, ()> for Pila {
    crate::state::privileged_global!();

    fn bind(
        state: &mut Self,
        dh: &DisplayHandle,
        client: &Client,
        resource: New<ExtWorkspaceManagerV1>,
        _: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let manager = data_init.init(resource, ());
        let Ok(group) = client.create_resource::<ExtWorkspaceGroupHandleV1, (), Pila>(
            dh,
            manager.version(),
            (),
        ) else {
            return;
        };
        manager.workspace_group(&group);
        group.capabilities(GroupCapabilities::empty());
        let mut m = Manager {
            manager,
            group,
            workspaces: Vec::new(),
            active: state.workspaces.active,
            outputs: Vec::new(),
            pending: None,
        };
        let outputs = state.client_outputs(client);
        m.update(
            dh,
            state.config.workspaces,
            state.workspaces.active,
            &outputs,
        );
        m.manager.done();
        state.ext_workspaces.managers.push(m);
    }
}

impl Dispatch<ExtWorkspaceManagerV1, ()> for Pila {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &ExtWorkspaceManagerV1,
        request: manager::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            manager::Request::Commit => {
                let pending = state
                    .workspace_manager_mut(resource)
                    .and_then(|m| m.pending.take());
                if let Some(index) = pending {
                    state.switch_workspace(index);
                }
            }
            manager::Request::Stop => {
                state
                    .ext_workspaces
                    .managers
                    .retain(|m| m.manager != *resource);
                resource.finished();
            }
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &ExtWorkspaceManagerV1, _: &()) {
        state
            .ext_workspaces
            .managers
            .retain(|m| m.manager != *resource);
    }
}

impl Dispatch<ExtWorkspaceGroupHandleV1, ()> for Pila {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &ExtWorkspaceGroupHandleV1,
        _: group::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        // create_workspace isn't offered; destroy needs nothing.
    }
}

impl Dispatch<ExtWorkspaceHandleV1, usize> for Pila {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &ExtWorkspaceHandleV1,
        request: workspace::Request,
        index: &usize,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        // Only activate is offered; deactivating has no meaning when one
        // workspace is always shown.
        if let workspace::Request::Activate = request {
            if let Some(m) = state
                .ext_workspaces
                .managers
                .iter_mut()
                .find(|m| m.workspaces.contains(resource))
            {
                m.pending = Some(*index);
            }
        }
    }
}
