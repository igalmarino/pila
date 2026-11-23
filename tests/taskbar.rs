//! What taskbars and shells use: ext-foreign-toplevel-list (read-only),
//! wlr-foreign-toplevel-management (acting on windows) and ext-workspace.

mod common;

use common::protocols::{LayerSpec, Placement};
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

/// A taskbar that binds an output after a window was listed (a monitor
/// plugged in, a bar per output starting late) is told the window is on it,
/// without waiting for something else to change (as niri does).
#[test]
fn an_output_bound_later_is_entered_by_its_windows() {
    let (mut pila, _app, taskbar) = taskbar_with(&["a"]);
    let a = handle(&taskbar, "a");
    let id = wayland_client::Proxy::id(&a);
    let enters = |s: &common::client::Shared| {
        s.log
            .iter()
            .filter(|l| l.object == id && l.event.starts_with("OutputEnter"))
            .count()
    };
    taskbar.wait("a's output", |s| enters(s) == 1);
    pila.settle();
    let _second = taskbar.bind_output_again();
    taskbar.wait("a enters the new wl_output too", |s| enters(s) == 2);
}

/// A window moved to another monitor leaves its old wl_output and enters the
/// new one: desktop shells filter their per-monitor taskbars and docks
/// by these.
#[test]
fn a_window_moved_to_another_output_leaves_the_old_one_and_enters_the_new() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1024x768".into()),
        ..Options::default()
    });
    pila.move_to(100.0, 100.0);
    let app = pila.client();
    app.window("a").create();
    pila.wait_focus("a");
    let taskbar = pila.client();
    let _manager: ZwlrForeignToplevelManagerV1 =
        taskbar.bind("zwlr_foreign_toplevel_manager_v1", 3);
    taskbar.flush();
    let a = handle(&taskbar, "a");
    let id = wayland_client::Proxy::id(&a);
    let output_a = wayland_client::Proxy::id(&taskbar.output("A")).to_string();
    let output_b = wayland_client::Proxy::id(&taskbar.output("B")).to_string();
    let events = move |s: &common::client::Shared, kind: &str| -> Vec<String> {
        s.log
            .iter()
            .filter(|l| l.object == id && l.event.starts_with(kind))
            .map(|l| l.event.clone())
            .collect()
    };
    let events_later = events.clone();
    let (ea, eb) = (output_a.clone(), output_b.clone());
    taskbar.wait("a is on A", move |s| {
        let enters = events(s, "OutputEnter");
        enters.len() == 1 && enters[0].contains(&ea) && !enters[0].contains(&eb)
    });
    pila.combo("Super+Shift+o");
    pila.wait("a on B", |s| {
        window_in(s, "a").is_some_and(|w| w["output"] == "B")
    });
    taskbar.wait("a left A and entered B", move |s| {
        let enters = events_later(s, "OutputEnter");
        let leaves = events_later(s, "OutputLeave");
        leaves.len() == 1
            && leaves[0].contains(&output_a)
            && enters.len() == 2
            && enters[1].contains(&output_b)
    });
}

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
    // The outputs come with the next frame's refresh.
    let id = wayland_client::Proxy::id(&b);
    taskbar.wait("b's output and done", |s| {
        let events: Vec<&str> = s
            .log
            .iter()
            .filter(|l| l.object == id)
            .map(|l| l.event.as_str())
            .collect();
        events.iter().any(|e| e.starts_with("OutputEnter")) && events.contains(&"Done")
    });
    assert!(
        has_state(&last_state(&taskbar, &b), ACTIVATED),
        "{:?}",
        taskbar.events_of(&b)
    );
    let a = handle(&taskbar, "a");
    assert!(!has_state(&last_state(&taskbar, &a), ACTIVATED));
}

/// A window's new title or app id reaches a taskbar listed it already
/// (Waybar's and shells' taskbars show the title of a browser's page
/// as it changes), each followed by `done`.
#[test]
fn a_new_title_and_app_id_reach_the_taskbar() {
    let mut pila = TestPila::start();
    let app = pila.client();
    let a = app.window("a").create();
    pila.wait_focus("a");
    let taskbar = pila.client();
    let _manager: ZwlrForeignToplevelManagerV1 =
        taskbar.bind("zwlr_foreign_toplevel_manager_v1", 3);
    taskbar.flush();
    let handle = handle(&taskbar, "a");
    let id = wayland_client::Proxy::id(&handle);
    let after = |s: &common::client::Shared, event: &str| {
        let events: Vec<&str> = s
            .log
            .iter()
            .filter(|l| l.object == id)
            .map(|l| l.event.as_str())
            .collect();
        events
            .iter()
            .position(|e| *e == event)
            .is_some_and(|i| events[i..].contains(&"Done"))
    };

    app.request(a, |t| t.set_title("a page".into()));
    taskbar.wait("the new title, then done", |s| {
        after(s, "Title { title: \"a page\" }")
    });
    app.request(a, |t| t.set_app_id("org.example.Renamed".into()));
    taskbar.wait("the new app id, then done", |s| {
        after(s, "AppId { app_id: \"org.example.Renamed\" }")
    });
    // Sent once each: a commit that changes neither sends them again.
    app.request(a, |t| t.set_title("a page".into()));
    pila.settle();
    taskbar.roundtrip();
    let titles = taskbar
        .events_of(&handle)
        .into_iter()
        .filter(|e| e.starts_with("Title"))
        .count();
    assert_eq!(titles, 2, "{:?}", taskbar.events_of(&handle));
}

/// A window off the screen (minimized, or on another workspace) still tells
/// the taskbars of ext-foreign-toplevel-list its new title.
#[test]
fn a_minimized_windows_new_title_reaches_the_toplevel_list() {
    use wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1;
    let mut pila = TestPila::start();
    let app = pila.client();
    let a = app.window("a").create();
    pila.wait_focus("a");
    let taskbar = pila.client();
    let _list: ExtForeignToplevelListV1 = taskbar.bind("ext_foreign_toplevel_list_v1", 1);
    taskbar.wait_logged("ext_toplevel", "AppId { app_id: \"a\" }");
    pila.combo("Super+n");
    pila.wait("minimized", |s| {
        window_in(s, "a").unwrap()["minimized"] == true
    });
    app.request(a, |t| t.set_title("a page".into()));
    taskbar.wait_logged("ext_toplevel", "Title { title: \"a page\" }");
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

/// pila's workspaces span every monitor: one group holds all the outputs, so
/// a pager on each monitor (a desktop shell's) shows the same workspaces,
/// a monitor plugged in later included.
#[test]
fn the_workspace_group_holds_every_output() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1024x768".into()),
        ..Options::default()
    });
    let client = pila.client();
    let _manager: ExtWorkspaceManagerV1 = client.bind("ext_workspace_manager_v1", 1);
    client.wait_logged("ext_workspace_manager_v1", "Done");
    let enters = |s: &common::client::Shared| {
        s.log
            .iter()
            .filter(|l| l.tag == "workspace_group" && l.event.starts_with("OutputEnter"))
            .count()
    };
    client.wait("the group entered A and B", |s| enters(s) == 2);
    pila.request(serde_json::json!({ "cmd": "output_add", "spec": "C:800x600" }));
    client.wait("C's wl_output global", |s| {
        s.globals
            .iter()
            .filter(|(_, i, _)| i == "wl_output")
            .count()
            == 3
    });
    client.bind_output_global(2);
    client.wait("the group entered C too", |s| enters(s) == 3);
}

/// A monitor unplugged leaves the workspace group, so a pager stops showing
/// the workspaces on it, and the manager says `done`.
#[test]
fn an_unplugged_monitor_leaves_the_workspace_group() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1024x768".into()),
        ..Options::default()
    });
    let client = pila.client();
    let _manager: ExtWorkspaceManagerV1 = client.bind("ext_workspace_manager_v1", 1);
    client.wait_logged("ext_workspace_manager_v1", "Done");
    let count = |s: &common::client::Shared, tag: &str, event: &str| {
        s.log
            .iter()
            .filter(|l| l.tag == tag && l.event.starts_with(event))
            .count()
    };
    client.wait("the group entered A and B", |s| {
        count(s, "workspace_group", "OutputEnter") == 2
    });
    let dones = count(&client.lock(), "ext_workspace_manager_v1", "Done");
    pila.request(serde_json::json!({ "cmd": "output_remove", "name": "B" }));
    client.wait("the group left B, then done", |s| {
        count(s, "workspace_group", "OutputLeave") == 1
            && count(s, "ext_workspace_manager_v1", "Done") > dones
    });
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

/// A minimized window, or one on another workspace, stays on the output it
/// comes back to: per-monitor taskbars keep its icon,
/// a minimized one's above all.
#[test]
fn a_window_off_the_screen_stays_on_its_output() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1024x768".into()),
        ..Options::default()
    });
    pila.move_to(100.0, 100.0);
    let app = pila.client();
    app.window("a").create();
    pila.wait_focus("a");
    pila.combo("Super+Shift+o");
    pila.wait("a on B", |s| {
        window_in(s, "a").is_some_and(|w| w["output"] == "B")
    });
    let taskbar = pila.client();
    let _manager: ZwlrForeignToplevelManagerV1 =
        taskbar.bind("zwlr_foreign_toplevel_manager_v1", 3);
    taskbar.flush();
    let a = handle(&taskbar, "a");
    let id = wayland_client::Proxy::id(&a);
    let output_b = wayland_client::Proxy::id(&taskbar.output("B")).to_string();
    let id_count = id.clone();
    let count = move |s: &common::client::Shared, kind: &str| {
        s.log
            .iter()
            .filter(|l| l.object == id_count && l.event.starts_with(kind))
            .count()
    };
    let (on_b, id_b) = (output_b.clone(), id.clone());
    taskbar.wait("a is on B", move |s| {
        s.log.iter().any(|l| {
            l.object == id_b && l.event.starts_with("OutputEnter") && l.event.contains(&on_b)
        })
    });

    a.set_minimized();
    taskbar.flush();
    taskbar.wait("the minimized state", |s| {
        s.log.iter().any(|l| {
            l.object == id && l.event.starts_with("State") && has_state(&l.event, MINIMIZED)
        })
    });
    a.unset_minimized();
    taskbar.flush();
    pila.wait("shown", |s| {
        window_in(s, "a").unwrap()["minimized"] == false
    });
    // On another workspace.
    a.activate(&taskbar.seat());
    taskbar.flush();
    pila.wait_focus("a");
    pila.combo("Super+Shift+Page_Down");
    pila.wait("a on 2", |s| window_in(s, "a").unwrap()["workspace"] == 2);
    pila.settle();
    taskbar.roundtrip();
    let leaves = count(&taskbar.lock(), "OutputLeave");
    assert_eq!(leaves, 0, "a left its output: {:?}", taskbar.events_of(&a));
}

/// A dock's menu (a grabbing popup of a layer surface) runs an entry, here
/// minimizing the focused window, and goes away: its grab ends with it. The
/// next motion moves the pointer (the one that ended the grab used to be
/// spent on it, so a shell's dock lost the click after "Minimise"), and the
/// keyboard goes back to a window.
#[test]
fn a_dock_menu_that_goes_away_leaves_no_grab_behind() {
    for minimize in [false, true] {
        let mut pila = TestPila::start();
        let app = pila.client();
        app.window("a").size(400, 300).create();
        pila.wait_focus("a");
        app.window("b").size(300, 200).create();
        pila.wait_focus("b");

        let shell = pila.client();
        let _manager: ZwlrForeignToplevelManagerV1 =
            shell.bind("zwlr_foreign_toplevel_manager_v1", 3);
        let (dock, _) = shell.layer_surface(
            "dock",
            &LayerSpec {
                anchor:
                    wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::Anchor::Bottom,
                size: (300, 60),
                ..LayerSpec::default()
            },
            [0, 0, 255, 255],
        );
        shell.wait_surface_configured(dock);
        pila.wait("the dock mapped", |s| {
            s["layers"].as_array().is_some_and(|layers| {
                layers
                    .iter()
                    .any(|l| l["namespace"] == "dock" && l["geometry"]["h"] == 60)
            })
        });
        let b = handle(&shell, "b");
        // A right click on the dock (centered at the bottom: 490..790,
        // 740..800) opens its menu above it, grabbing.
        pila.move_to(640.0, 770.0);
        pila.press("right");
        shell.wait("the press", |s| {
            s.events
                .iter()
                .any(|e| matches!(e, common::client::Ev::Button { pressed: true, .. }))
        });
        let placement = Placement {
            size: (200, 100),
            anchor_rect: (150, 0, 1, 1),
            anchor: wayland_protocols::xdg::shell::client::xdg_positioner::Anchor::Top,
            gravity: wayland_protocols::xdg::shell::client::xdg_positioner::Gravity::Top,
            ..Placement::default()
        };
        let (menu, popup) = shell.popup("menu", Err(dock), &placement, true, [0, 255, 0, 255]);
        shell.wait_surface_configured(menu);
        pila.release("right");
        pila.wait("the menu's grab", |s| s["pointer"]["grab"] == "popup");

        // An entry: a click in the menu, then its request, and the menu goes.
        pila.move_to(640.0, 700.0);
        pila.click("left");
        if minimize {
            b.set_minimized();
        }
        popup.destroy();
        shell.flush();
        let back_to = if minimize { "a" } else { "b" };
        pila.wait(&format!("the keyboard on {back_to}"), |s| {
            s["focused"] == back_to && s["keyboard_grabbed"] == false
        });
        pila.move_to(700.0, 770.0);
        pila.wait(
            &format!("the pointer on the dock (minimize: {minimize})"),
            |s| s["pointer"]["x"] == 700.0 && s["pointer"]["grabbed"] == false,
        );
    }
}

/// Every request for the state a window is already in changes nothing,
/// from a taskbar (here) or the app itself (`fill_states`): not its place,
/// size or focus, and not where it comes back to afterwards.
#[test]
fn taskbar_requests_for_the_current_state_change_nothing() {
    let (mut pila, _app, taskbar) = taskbar_with(&["b", "a"]);
    pila.combo("Super+Shift+Right");
    pila.combo("Super+Shift+Down");
    let place = |s: &serde_json::Value| rect(&window_in(s, "a").unwrap()["geometry"]);
    let moved = pila.wait("a moved", |s| place(s).0 != 440);
    let before = place(&moved);
    let a = handle(&taskbar, "a");
    let unchanged = |pila: &mut TestPila, what: &str| {
        pila.stays(what, std::time::Duration::from_millis(250), |s| {
            let w = window_in(s, "a").unwrap();
            place(s) == before
                && w["maximized"] == false
                && w["fullscreen"] == false
                && w["minimized"] == false
                && s["focused"] == "a"
        });
    };

    // Not filled, not minimized, focused: undoing any of it is nothing.
    a.unset_maximized();
    taskbar.flush();
    unchanged(&mut pila, "after unset_maximized");
    a.unset_fullscreen();
    taskbar.flush();
    unchanged(&mut pila, "after unset_fullscreen");
    a.unset_minimized();
    taskbar.flush();
    unchanged(&mut pila, "after unset_minimized");
    a.activate(&taskbar.seat());
    taskbar.flush();
    unchanged(&mut pila, "after activate");

    // Asked twice, then undone: back where it was.
    for (set, unset, key) in [
        ("set_maximized", "unset_maximized", "maximized"),
        ("set_fullscreen", "unset_fullscreen", "fullscreen"),
        ("set_minimized", "unset_minimized", "minimized"),
    ] {
        for _ in 0..2 {
            match set {
                "set_maximized" => a.set_maximized(),
                "set_fullscreen" => a.set_fullscreen(None),
                _ => a.set_minimized(),
            }
            taskbar.flush();
            pila.wait(&format!("{key} after {set}"), |s| {
                window_in(s, "a").unwrap()[key] == true
            });
        }
        match unset {
            "unset_maximized" => a.unset_maximized(),
            "unset_fullscreen" => a.unset_fullscreen(),
            _ => a.unset_minimized(),
        }
        taskbar.flush();
        pila.wait(&format!("back where it was after {unset}"), |s| {
            window_in(s, "a").unwrap()[key] == false && place(s) == before
        });
        if key == "minimized" {
            // Shown again, not focused: give it back the focus.
            a.activate(&taskbar.seat());
            taskbar.flush();
            pila.wait_focus("a");
        }
    }
}

/// A dock's menu dismissed by a click on the bare desktop, then destroyed by
/// its app: the keyboard isn't left with the dock, which takes none, nor
/// given back to it at the next key.
#[test]
fn a_dock_menu_dismissed_on_the_desktop_leaves_the_keyboard_to_a_window() {
    let mut pila = TestPila::start();
    let app = pila.client();
    app.window("a").size(300, 200).create();
    pila.wait_focus("a");

    let shell = pila.client();
    let (dock, _) = shell.layer_surface(
        "dock",
        &LayerSpec {
            anchor:
                wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::Anchor::Bottom,
            size: (300, 60),
            ..LayerSpec::default()
        },
        [0, 0, 255, 255],
    );
    shell.wait_surface_configured(dock);
    pila.wait("the dock mapped", |s| {
        s["layers"].as_array().is_some_and(|layers| {
            layers
                .iter()
                .any(|l| l["namespace"] == "dock" && l["geometry"]["h"] == 60)
        })
    });
    pila.move_to(640.0, 770.0);
    pila.press("right");
    shell.wait("the press", |s| {
        s.events
            .iter()
            .any(|e| matches!(e, common::client::Ev::Button { pressed: true, .. }))
    });
    let placement = Placement {
        size: (200, 100),
        anchor_rect: (150, 0, 1, 1),
        anchor: wayland_protocols::xdg::shell::client::xdg_positioner::Anchor::Top,
        gravity: wayland_protocols::xdg::shell::client::xdg_positioner::Gravity::Top,
        ..Placement::default()
    };
    let (menu, popup) = shell.popup("menu", Err(dock), &placement, true, [0, 255, 0, 255]);
    shell.wait_surface_configured(menu);
    pila.release("right");
    pila.wait("the menu's grab", |s| s["pointer"]["grab"] == "popup");

    // A click on the bare desktop, far from the window and the dock.
    pila.click_at(60.0, 60.0);
    shell.wait_surface_closed(menu);
    popup.destroy();
    shell.flush();
    pila.key("a");
    pila.wait("the keyboard on a window", |s| {
        s["keyboard_grabbed"] == false && s["keyboard_focus"] == "a"
    });
}
