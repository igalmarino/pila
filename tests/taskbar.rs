//! What taskbars and shells use: ext-foreign-toplevel-list (read-only),
//! wlr-foreign-toplevel-management (acting on windows) and ext-workspace.

mod common;

use common::*;
use wayland_protocols::ext::{
    foreign_toplevel_list::v1::client::ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1,
    workspace::v1::client::{
        ext_workspace_handle_v1::ExtWorkspaceHandleV1,
        ext_workspace_manager_v1::ExtWorkspaceManagerV1,
    },
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
    zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1,
};

// --- ext-foreign-toplevel-list ----------------------------------------------------

#[test]
fn the_list_announces_windows_their_changes_and_their_end() {
    let mut pila = TestPila::start();
    let app = pila.client();
    let a = app.window("a").title("first").create();
    pila.wait_window("a");
    let taskbar = pila.client();
    let _list: ExtForeignToplevelListV1 = taskbar.bind("ext_foreign_toplevel_list_v1", 1);
    taskbar.wait_logged("ext_toplevel", "AppId { app_id: \"a\" }");
    assert!(taskbar.has_logged("ext_toplevel", "Title { title: \"first\" }"));
    // The identifier is the one the state reports.
    let id = pila.window("a")["id"].as_str().unwrap().to_string();
    assert!(taskbar.has_logged(
        "ext_toplevel",
        &format!("Identifier {{ identifier: \"{id}\" }}")
    ));
    app.request(a, |t| t.set_title("second".into()));
    app.surface(a).commit();
    app.flush();
    taskbar.wait_logged("ext_toplevel", "Title { title: \"second\" }");
    app.destroy(a);
    taskbar.wait_logged("ext_toplevel", "Closed");
}

#[test]
fn windows_opened_later_are_announced() {
    let mut pila = TestPila::start();
    let taskbar = pila.client();
    let _list: ExtForeignToplevelListV1 = taskbar.bind("ext_foreign_toplevel_list_v1", 1);
    taskbar.roundtrip();
    let app = pila.client();
    app.window("late").create();
    pila.wait_window("late");
    taskbar.wait_logged("ext_toplevel", "AppId { app_id: \"late\" }");
}

// --- wlr-foreign-toplevel-management ------------------------------------------------

fn handle(taskbar: &Client, app_id: &str) -> ZwlrForeignToplevelHandleV1 {
    taskbar.wait_logged("wlr_toplevel", &format!("AppId {{ app_id: \"{app_id}\" }}"));
    taskbar.object("wlr_toplevel", &format!("AppId {{ app_id: \"{app_id}\" }}"))
}

/// The handle's last `State` event (the states as bytes, as Debug prints
/// them).
fn last_state(taskbar: &Client, handle: &ZwlrForeignToplevelHandleV1) -> String {
    taskbar
        .events_of(handle)
        .into_iter()
        .rev()
        .find(|e| e.starts_with("State"))
        .unwrap_or_default()
}

/// Whether a `State` event's array holds `state` (as a little-endian u32).
fn has_state(event: &str, state: u32) -> bool {
    let Some(start) = event.find('[') else {
        return false;
    };
    let bytes: Vec<u8> = event[start + 1..event.find(']').unwrap()]
        .split(',')
        .filter_map(|b| b.trim().parse().ok())
        .collect();
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .any(|c| u32::from_le_bytes(*c) == state)
}

const MAXIMIZED: u32 = 0;
const MINIMIZED: u32 = 1;
const ACTIVATED: u32 = 2;
const FULLSCREEN: u32 = 3;

fn taskbar_with(names: &[&str]) -> (TestPila, Client, Client) {
    let mut pila = TestPila::start();
    let app = pila.client();
    for name in names {
        app.window(name).create();
        pila.wait_focus(name);
    }
    let taskbar = pila.client();
    let _manager: ZwlrForeignToplevelManagerV1 =
        taskbar.bind("zwlr_foreign_toplevel_manager_v1", 3);
    taskbar.flush();
    (pila, app, taskbar)
}

#[test]
fn handles_report_titles_states_and_outputs() {
    let (_pila, _app, taskbar) = taskbar_with(&["a", "b"]);
    let b = handle(&taskbar, "b");
    taskbar.wait("b's state", |_| true);
    taskbar.roundtrip();
    assert!(
        has_state(&last_state(&taskbar, &b), ACTIVATED),
        "{:?}",
        taskbar.events_of(&b)
    );
    let a = handle(&taskbar, "a");
    assert!(!has_state(&last_state(&taskbar, &a), ACTIVATED));
    assert!(taskbar
        .events_of(&b)
        .iter()
        .any(|e| e.starts_with("OutputEnter")));
    assert!(taskbar.events_of(&b).iter().any(|e| e == "Done"));
}

#[test]
fn activate_focuses_a_window() {
    let (mut pila, _app, taskbar) = taskbar_with(&["a", "b"]);
    let a = handle(&taskbar, "a");
    a.activate(&taskbar.seat());
    taskbar.flush();
    pila.wait_focus("a");
    taskbar.wait("a activated", |s| {
        s.log
            .iter()
            .rev()
            .find(|l| l.object == wayland_client::Proxy::id(&a) && l.event.starts_with("State"))
            .is_some_and(|l| has_state(&l.event, ACTIVATED))
    });
}

#[test]
fn minimize_and_unminimize() {
    let (mut pila, _app, taskbar) = taskbar_with(&["a"]);
    let a = handle(&taskbar, "a");
    a.set_minimized();
    taskbar.flush();
    pila.wait("minimized", |s| {
        window_in(s, "a").unwrap()["minimized"] == true
    });
    taskbar.wait("the minimized state", |s| {
        s.log.iter().any(|l| {
            l.object == wayland_client::Proxy::id(&a)
                && l.event.starts_with("State")
                && has_state(&l.event, MINIMIZED)
        })
    });
    a.unset_minimized();
    taskbar.flush();
    pila.wait("shown", |s| {
        window_in(s, "a").unwrap()["minimized"] == false
    });
}

#[test]
fn maximize_fullscreen_and_close() {
    let (mut pila, app, taskbar) = taskbar_with(&["a"]);
    let a = handle(&taskbar, "a");
    a.set_maximized();
    taskbar.flush();
    pila.wait("maximized", |s| {
        window_in(s, "a").unwrap()["maximized"] == true
    });
    taskbar.wait("the maximized state", |s| {
        s.log.iter().any(|l| {
            l.object == wayland_client::Proxy::id(&a)
                && l.event.starts_with("State")
                && has_state(&l.event, MAXIMIZED)
        })
    });
    a.unset_maximized();
    a.set_fullscreen(None);
    taskbar.flush();
    pila.wait("fullscreen", |s| {
        let w = window_in(s, "a").unwrap();
        w["fullscreen"] == true && w["maximized"] == false
    });
    taskbar.wait("the fullscreen state", |s| {
        s.log.iter().any(|l| {
            l.object == wayland_client::Proxy::id(&a)
                && l.event.starts_with("State")
                && has_state(&l.event, FULLSCREEN)
        })
    });
    a.unset_fullscreen();
    a.close();
    taskbar.flush();
    app.wait("close request", |s| s.windows[0].close_requested);
}

#[test]
fn activating_a_window_elsewhere_switches_workspace() {
    let (mut pila, _app, taskbar) = taskbar_with(&["a", "b"]);
    pila.combo("Super+Shift+3");
    pila.wait("b on 3", |s| window_in(s, "b").unwrap()["workspace"] == 3);
    let b = handle(&taskbar, "b");
    b.activate(&taskbar.seat());
    taskbar.flush();
    let state = pila.wait_focus("b");
    assert_eq!(state["workspace"], 3);
}

#[test]
fn requests_are_ignored_while_locked() {
    let (mut pila, app, taskbar) = taskbar_with(&["a"]);
    let a = handle(&taskbar, "a");
    let lock = taskbar.lock_session();
    taskbar.lock_surface(&lock, "HEADLESS-1", [255, 0, 0, 0]);
    taskbar.wait_logged("lock", "Locked");
    a.set_minimized();
    a.set_maximized();
    a.close();
    taskbar.flush();
    app.roundtrip();
    pila.stays(
        "nothing changes",
        std::time::Duration::from_millis(150),
        |s| {
            let w = window_in(s, "a").unwrap();
            w["minimized"] == false && w["maximized"] == false
        },
    );
    assert!(!app.with_window(common::client::Window(0), |w| w.close_requested));
}

// --- ext-workspace ------------------------------------------------------------------

fn workspace(client: &Client, name: &str) -> ExtWorkspaceHandleV1 {
    client.object("workspace", &format!("Name {{ name: \"{name}\" }}"))
}

/// Whether the workspace's last State event says active (bit 1).
fn active(client: &Client, workspace: &ExtWorkspaceHandleV1) -> bool {
    client
        .events_of(workspace)
        .into_iter()
        .rev()
        .find(|e| e.starts_with("State"))
        .is_some_and(|e| e.contains("Active"))
}

#[test]
fn workspaces_are_listed_with_the_active_one() {
    let pila = TestPila::with_config("workspaces = 3\n");
    let client = pila.client();
    let _manager: ExtWorkspaceManagerV1 = client.bind("ext_workspace_manager_v1", 1);
    client.wait_logged("ext_workspace_manager_v1", "Done");
    for name in ["1", "2", "3"] {
        workspace(&client, name);
    }
    assert!(client
        .objects::<ExtWorkspaceHandleV1>("workspace", "Name { name: \"4\" }")
        .is_empty());
    assert!(active(&client, &workspace(&client, "1")));
    assert!(!active(&client, &workspace(&client, "2")));
    assert!(client.has_logged("workspace_group", "OutputEnter"));
}

#[test]
fn a_client_activates_a_workspace_on_commit() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let manager: ExtWorkspaceManagerV1 = client.bind("ext_workspace_manager_v1", 1);
    client.wait_logged("ext_workspace_manager_v1", "Done");
    let two = workspace(&client, "2");
    two.activate();
    client.flush();
    client.roundtrip();
    // Nothing before the commit.
    assert_eq!(pila.state()["workspace"], 1);
    manager.commit();
    client.flush();
    pila.wait("on 2", |s| s["workspace"] == 2);
    client.wait("2 reported active", |s| {
        s.log
            .iter()
            .rev()
            .find(|l| l.object == wayland_client::Proxy::id(&two) && l.event.starts_with("State"))
            .is_some_and(|l| l.event.contains("Active"))
    });
}

#[test]
fn a_binding_switch_is_reported() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let _manager: ExtWorkspaceManagerV1 = client.bind("ext_workspace_manager_v1", 1);
    client.wait_logged("ext_workspace_manager_v1", "Done");
    let three = workspace(&client, "3");
    pila.combo("Super+3");
    client.wait("3 reported active", |s| {
        s.log
            .iter()
            .rev()
            .find(|l| l.object == wayland_client::Proxy::id(&three) && l.event.starts_with("State"))
            .is_some_and(|l| l.event.contains("Active"))
    });
}

#[test]
fn a_new_workspace_count_adds_and_removes_workspaces() {
    let mut pila = TestPila::with_config("workspaces = 2\n");
    let client = pila.client();
    let _manager: ExtWorkspaceManagerV1 = client.bind("ext_workspace_manager_v1", 1);
    client.wait_logged("ext_workspace_manager_v1", "Done");
    let two = workspace(&client, "2");
    pila.write_config("workspaces = 3\n");
    client.wait_logged("workspace", "Name { name: \"3\" }");
    pila.write_config("workspaces = 1\n");
    client.wait("2 removed", |s| {
        s.log
            .iter()
            .any(|l| l.object == wayland_client::Proxy::id(&two) && l.event == "Removed")
    });
}
