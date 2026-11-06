//! wlr-foreign-toplevel-management: lets taskbars, docks and window
//! switchers list the open windows with their state and act on them
//! (activate, minimize, maximize, fullscreen, close). The standard
//! ext-foreign-toplevel-list (`foreign_toplevel.rs`) only lists; windows
//! here are named by the identifier of their handle there, so a window has
//! both once it's mapped.
//!
//! Listeners are kept up to date by diffing (`Pila::refresh_toplevels`,
//! after every rendered frame): each window's title, app id, states and
//! outputs are compared with what was last sent to each handle, and only
//! the changes go out, followed by one `done`. "Activated" means keyboard
//! focus. Requests are ignored while the session is locked.

use std::collections::{HashMap, HashSet};

use smithay::{
    desktop::Window,
    output::Output,
    reexports::{
        wayland_protocols_wlr::foreign_toplevel::v1::server::{
            zwlr_foreign_toplevel_handle_v1::{self as handle, State, ZwlrForeignToplevelHandleV1},
            zwlr_foreign_toplevel_manager_v1::{self as manager, ZwlrForeignToplevelManagerV1},
        },
        wayland_server::{
            backend::ClientId, protocol::wl_output::WlOutput, Client, DataInit, Dispatch,
            DisplayHandle, GlobalDispatch, New, Resource,
        },
    },
    utils::SERIAL_COUNTER,
};
use tracing::debug;

use crate::{
    foreign_toplevel::{identifier, title_and_app_id},
    state::{close_window, fill_state},
    Pila,
};

const VERSION: u32 = 3;

/// The protocol's state (`Pila::toplevel_management`).
#[derive(Default)]
pub struct ToplevelManagement {
    managers: Vec<ZwlrForeignToplevelManagerV1>,
    /// By the window's identifier.
    entries: HashMap<String, Entry>,
}

/// What listeners were last told about a window, and their handles with
/// the `wl_output`s each was told the window is on.
struct Entry {
    identifier: String,
    title: String,
    app_id: String,
    states: Vec<u32>,
    outputs: Vec<Output>,
    handles: HashMap<ZwlrForeignToplevelHandleV1, Vec<WlOutput>>,
}

/// A window's current state, as listeners see it.
struct Info {
    identifier: String,
    title: String,
    app_id: String,
    states: Vec<u32>,
    outputs: Vec<Output>,
}

/// Create the global. Called once from `Pila::new`.
pub fn init_global(dh: &DisplayHandle) {
    dh.create_global::<Pila, ZwlrForeignToplevelManagerV1, ()>(VERSION, ());
}

fn state_bytes(states: &[u32]) -> Vec<u8> {
    states.iter().flat_map(|s| s.to_ne_bytes()).collect()
}

impl Entry {
    /// Make a handle for `manager`'s client and tell it everything.
    fn add_handle(&mut self, dh: &DisplayHandle, manager: &ZwlrForeignToplevelManagerV1) {
        let Some(client) = manager.client() else {
            return;
        };
        let Ok(handle) = client.create_resource::<ZwlrForeignToplevelHandleV1, String, Pila>(
            dh,
            manager.version(),
            self.identifier.clone(),
        ) else {
            return;
        };
        manager.toplevel(&handle);
        handle.title(self.title.clone());
        handle.app_id(self.app_id.clone());
        handle.state(state_bytes(&self.states));
        let mut sent = Vec::new();
        for output in &self.outputs {
            for wl_output in output.client_outputs(&client) {
                handle.output_enter(&wl_output);
                sent.push(wl_output);
            }
        }
        handle.done();
        self.handles.insert(handle, sent);
    }

    /// Send what changed since the last refresh, if anything.
    fn update(&mut self, info: Info) {
        let title = (self.title != info.title).then(|| info.title.clone());
        let app_id = (self.app_id != info.app_id).then(|| info.app_id.clone());
        let states = (self.states != info.states).then(|| state_bytes(&info.states));
        self.title = info.title;
        self.app_id = info.app_id;
        self.states = info.states;
        self.outputs = info.outputs;
        for (handle, sent) in &mut self.handles {
            let mut changed = title.is_some() || app_id.is_some() || states.is_some();
            if let Some(title) = &title {
                handle.title(title.clone());
            }
            if let Some(app_id) = &app_id {
                handle.app_id(app_id.clone());
            }
            if let Some(states) = &states {
                handle.state(states.clone());
            }
            // Outputs: compared per handle, since a client may bind a
            // wl_output at any time.
            let wanted: Vec<WlOutput> = match handle.client() {
                Some(client) => self
                    .outputs
                    .iter()
                    .flat_map(|o| o.client_outputs(&client))
                    .collect(),
                None => Vec::new(),
            };
            sent.retain(|o| {
                if !o.is_alive() {
                    return false;
                }
                let keep = wanted.contains(o);
                if !keep {
                    handle.output_leave(o);
                    changed = true;
                }
                keep
            });
            for o in wanted {
                if !sent.contains(&o) {
                    handle.output_enter(&o);
                    sent.push(o);
                    changed = true;
                }
            }
            if changed {
                handle.done();
            }
        }
    }

    fn close(&self) {
        for handle in self.handles.keys() {
            handle.closed();
        }
    }
}

impl ToplevelManagement {
    /// Tell listeners a window is gone.
    pub fn closed(&mut self, window: &Window) {
        let Some(identifier) = identifier(window) else {
            return;
        };
        if let Some(entry) = self.entries.remove(&identifier) {
            entry.close();
        }
    }

    fn forget_handle(&mut self, handle: &ZwlrForeignToplevelHandleV1) {
        for entry in self.entries.values_mut() {
            entry.handles.remove(handle);
        }
    }
}

impl Pila {
    /// A window's state as listeners see it; `None` before it's listed.
    fn toplevel_info(&self, window: &Window, focused: bool) -> Option<Info> {
        let identifier = identifier(window)?;
        let (title, app_id) = title_and_app_id(window);
        let (maximized, fullscreen) = fill_state(window);
        let minimized = self.is_minimized(window);
        let mut states = Vec::new();
        if maximized {
            states.push(State::Maximized as u32);
        }
        if minimized {
            states.push(State::Minimized as u32);
        }
        if focused {
            states.push(State::Activated as u32);
        }
        if fullscreen {
            states.push(State::Fullscreen as u32);
        }
        let outputs = if minimized {
            Vec::new()
        } else {
            self.space.outputs_for_element(window)
        };
        Some(Info {
            identifier,
            title,
            app_id,
            states,
            outputs,
        })
    }

    /// Bring every listener up to date. Called after each rendered frame.
    pub fn refresh_toplevels(&mut self) {
        if self.toplevel_management.managers.is_empty()
            && self.toplevel_management.entries.is_empty()
        {
            return;
        }
        let focused = self.focused_window();
        // The focused window last: a listener sees the old one deactivate
        // before the new one activates.
        let mut windows: Vec<Window> = self
            .space
            .elements()
            .chain(self.off_space_windows())
            .filter(|w| Some(*w) != focused.as_ref())
            .cloned()
            .collect();
        windows.extend(focused.clone());
        let infos: Vec<Info> = windows
            .iter()
            .filter_map(|w| self.toplevel_info(w, Some(w) == focused.as_ref()))
            .collect();

        let dh = self.display_handle.clone();
        let tm = &mut self.toplevel_management;
        let live: HashSet<&str> = infos.iter().map(|i| i.identifier.as_str()).collect();
        tm.entries.retain(|identifier, entry| {
            let keep = live.contains(identifier.as_str());
            if !keep {
                entry.close();
            }
            keep
        });
        for info in infos {
            match tm.entries.get_mut(&info.identifier) {
                Some(entry) => entry.update(info),
                None => {
                    let mut entry = Entry {
                        identifier: info.identifier.clone(),
                        title: info.title,
                        app_id: info.app_id,
                        states: info.states,
                        outputs: info.outputs,
                        handles: HashMap::new(),
                    };
                    for manager in &tm.managers {
                        entry.add_handle(&dh, manager);
                    }
                    tm.entries.insert(info.identifier, entry);
                }
            }
        }
    }

    /// Bring a window to the front with keyboard focus, as a taskbar asks.
    fn activate_toplevel(&mut self, window: &Window) {
        if self.windows.switch.is_some() {
            self.end_switch();
        }
        self.reveal_workspace_of(window);
        if self.is_minimized(window) {
            self.restore_window(window);
        } else {
            self.focus_window(Some(window.clone()), SERIAL_COUNTER.next_serial());
        }
        self.queue_redraw();
    }
}

impl GlobalDispatch<ZwlrForeignToplevelManagerV1, ()> for Pila {
    fn bind(
        state: &mut Self,
        dh: &DisplayHandle,
        _: &Client,
        resource: New<ZwlrForeignToplevelManagerV1>,
        _: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let manager = data_init.init(resource, ());
        // Everything listed so far, then whatever isn't listed yet (nothing
        // is tracked until the first manager binds).
        for entry in state.toplevel_management.entries.values_mut() {
            entry.add_handle(dh, &manager);
        }
        state.toplevel_management.managers.push(manager);
        state.refresh_toplevels();
    }

    crate::state::privileged_global!();
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for Pila {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &ZwlrForeignToplevelManagerV1,
        request: manager::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let manager::Request::Stop = request {
            state.toplevel_management.managers.retain(|m| m != resource);
            resource.finished();
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, resource: &ZwlrForeignToplevelManagerV1, _: &()) {
        state.toplevel_management.managers.retain(|m| m != resource);
    }
}

impl Dispatch<ZwlrForeignToplevelHandleV1, String> for Pila {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &ZwlrForeignToplevelHandleV1,
        request: handle::Request,
        identifier: &String,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if matches!(request, handle::Request::Destroy) || state.is_locked() {
            return;
        }
        let Some(window) = state.window_for_identifier(identifier) else {
            return;
        };
        debug!(?request, identifier, "toplevel management request");
        match request {
            handle::Request::Activate { .. } => state.activate_toplevel(&window),
            handle::Request::Close => close_window(&window),
            handle::Request::SetMinimized => state.minimize_window(&window),
            handle::Request::UnsetMinimized => state.show_window(&window),
            handle::Request::SetMaximized => {
                let (_, fullscreen) = fill_state(&window);
                state.set_fill_state(&window, true, fullscreen);
            }
            handle::Request::UnsetMaximized => {
                let (_, fullscreen) = fill_state(&window);
                state.set_fill_state(&window, false, fullscreen);
            }
            handle::Request::SetFullscreen { output } => {
                let (maximized, _) = fill_state(&window);
                let output = output.as_ref().and_then(Output::from_resource);
                state.set_fill_state_on(&window, maximized, true, output.as_ref());
            }
            handle::Request::UnsetFullscreen => {
                let (maximized, _) = fill_state(&window);
                state.set_fill_state(&window, maximized, false);
            }
            // Where a taskbar shows the window's icon: only for minimize
            // animations, which pila doesn't do.
            handle::Request::SetRectangle { .. } => {}
            _ => {}
        }
    }

    fn destroyed(
        state: &mut Self,
        _: ClientId,
        resource: &ZwlrForeignToplevelHandleV1,
        _: &String,
    ) {
        state.toplevel_management.forget_handle(resource);
    }
}
