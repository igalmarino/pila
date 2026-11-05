//! Window lifecycle, placement, focus and stacking, title bar buttons.

mod common;

use common::client::{states, Decorations, Ev};
use common::*;

/// Title bar height (`decoration::TITLE_BAR_HEIGHT`).
const BAR: i64 = 28;
/// Width of each title bar button (`decoration::BUTTON_WIDTH`).
const BUTTON: i64 = 36;

#[test]
fn a_window_opens_centered_below_its_title_bar() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).create();
    let w = pila.wait_window("a");
    // (1280 - 400) / 2; (800 - 300 - 28) / 2 + 28.
    assert_eq!(rect(&w["geometry"]), (440, 264, 400, 300));
    assert_eq!(w["ssd"], true);
    assert_eq!(w["title_bar"], true);
    assert_eq!(w["title"], "a");
    assert_eq!(w["workspace"], 1);
    assert_eq!(w["output"], "HEADLESS-1");
}

#[test]
fn a_client_side_decorated_window_has_no_bar() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client
        .window("csd")
        .size(400, 300)
        .decorations(Decorations::Client)
        .create();
    let w = pila.wait_window("csd");
    assert_eq!(w["ssd"], false);
    assert_eq!(w["title_bar"], false);
    assert_eq!(rect(&w["geometry"]), (440, 250, 400, 300));
}

#[test]
fn a_window_asking_for_client_side_gets_it() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client
        .window("ask-csd")
        .decorations(Decorations::AskClient)
        .create();
    let w = pila.wait_window("ask-csd");
    assert_eq!(w["title_bar"], false);
    client.wait("the decoration mode", |s| {
        s.windows[window.0].server_side.is_some()
    });
    assert_eq!(client.with_window(window, |w| w.server_side), Some(false));
}

#[test]
fn server_side_decorations_are_confirmed() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("ssd").create();
    pila.wait_window("ssd");
    client.wait("the decoration mode", |s| {
        s.windows[window.0].server_side.is_some()
    });
    assert_eq!(client.with_window(window, |w| w.server_side), Some(true));
}

#[test]
fn windows_on_the_same_spot_cascade() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).create();
    pila.wait_window("a");
    client.window("b").size(400, 300).create();
    let b = pila.wait_window("b");
    assert_eq!(rect(&b["geometry"]), (472, 296, 400, 300));
    client.window("c").size(400, 300).create();
    let c = pila.wait_window("c");
    assert_eq!(rect(&c["geometry"]), (504, 328, 400, 300));
}

#[test]
fn a_window_opens_on_the_output_under_the_pointer() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1024x768".into()),
        ..Options::default()
    });
    let state = pila.state();
    assert_eq!(rect(&state["outputs"][1]["geometry"]), (1280, 0, 1024, 768));
    pila.move_to(1500.0, 300.0);
    let client = pila.client();
    client.window("b").size(400, 300).create();
    let w = pila.wait_window("b");
    // 1280 + (1024 - 400) / 2; (768 - 328) / 2 + 28.
    assert_eq!(rect(&w["geometry"]), (1592, 248, 400, 300));
    assert_eq!(w["output"], "B");
}

#[test]
fn a_window_larger_than_the_output_starts_at_its_top_left() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("big").size(1500, 1000).create();
    let w = pila.wait_window("big");
    assert_eq!(rect(&w["geometry"]), (0, BAR, 1500, 1000));
}

#[test]
fn a_new_window_takes_focus_and_goes_on_top() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").create();
    pila.wait_focus("a");
    client.window("b").create();
    let state = pila.wait_focus("b");
    assert_eq!(stacking(&state), vec!["a", "b"]);
    client.wait_configure(a, "a deactivated", |c| !c.has(states::ACTIVATED));
}

#[test]
fn clicking_a_window_focuses_and_raises_it_and_the_click_reaches_it() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").size(400, 300).create();
    pila.wait_window("a");
    client.window("b").size(400, 300).create();
    pila.wait_focus("b");
    client.clear_events();
    // a is at (440, 264), b at (472, 296): (450, 270) is only a.
    pila.click_at(450.0, 270.0);
    let state = pila.wait_focus("a");
    assert_eq!(stacking(&state), vec!["b", "a"]);
    client.wait("the click", |s| {
        s.events
            .iter()
            .any(|e| matches!(e, Ev::Button { pressed: false, .. }))
    });
    assert_eq!(client.buttons(), vec![(0x110, true), (0x110, false)]);
    client.wait_configure(a, "a activated", |c| c.has(states::ACTIVATED));
}

#[test]
fn clicking_the_background_clears_focus() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").create();
    pila.wait_focus("a");
    pila.click_at(5.0, 790.0);
    pila.wait("no focus", |s| s["focused"].is_null());
    client.wait_configure(a, "a deactivated", |c| !c.has(states::ACTIVATED));
}

#[test]
fn the_pointer_enters_the_window_under_it_at_surface_coordinates() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).create();
    pila.wait_window("a");
    pila.move_to(450.0, 274.0);
    client.wait("pointer enter", |s| {
        s.events.iter().any(|e| {
            matches!(
                e,
                Ev::PointerEnter {
                    window: Some(0),
                    ..
                }
            )
        })
    });
    let last_position = client
        .events()
        .iter()
        .rev()
        .find_map(|e| match e {
            Ev::PointerEnter { x, y, .. } | Ev::Motion { x, y } => Some((*x, *y)),
            _ => None,
        })
        .unwrap();
    assert_eq!(last_position, (10.0, 10.0));
    let state = pila.state();
    assert_eq!(state["pointer"]["focus"], "a");
    pila.move_to(5.0, 5.0);
    client.wait("pointer leave", |s| {
        s.events.last() == Some(&Ev::PointerLeave(Some(0)))
    });
}

#[test]
fn closing_the_focused_window_focuses_the_one_below() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").create();
    pila.wait_window("a");
    let b = client.window("b").create();
    pila.wait_focus("b");
    client.destroy(b);
    pila.wait("b gone", |s| window_in(s, "b").is_none());
    pila.wait_focus("a");
}

#[test]
fn a_window_unmapped_by_its_client_leaves_and_maps_again() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").create();
    pila.wait_window("a");
    client.set_auto_draw(a, false);
    client.unmap(a);
    pila.wait("a unmapped", |s| {
        window_in(s, "a").is_none() && s["unmapped"].as_array().unwrap().len() == 1
    });
    // A client maps again with a new initial commit, then a buffer.
    client.set_auto_draw(a, true);
    let surface = client.surface(a);
    surface.commit();
    client.flush();
    pila.wait_window("a");
    pila.wait_focus("a");
}

#[test]
fn a_dead_client_takes_its_windows_along() {
    let mut pila = TestPila::start();
    let other = pila.client();
    other.window("other").create();
    pila.wait_window("other");
    let mut client = pila.client();
    client.window("a").create();
    client.window("b").create();
    pila.wait_window("b");
    client.disconnect();
    let state = pila.wait("a and b gone", |s| {
        window_in(s, "a").is_none() && window_in(s, "b").is_none()
    });
    assert_eq!(state["focused"], "other");
}

#[test]
fn the_title_bar_close_button_asks_the_window_to_close() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").size(400, 300).create();
    let w = pila.wait_window("a");
    let (x, y, width, _) = rect(&w["geometry"]);
    pila.click_at((x + width - BUTTON / 2) as f64, (y - BAR / 2) as f64);
    client.wait("close request", |s| s.windows[a.0].close_requested);
}

#[test]
fn the_title_bar_maximize_button_toggles_maximized() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").size(400, 300).create();
    let w = pila.wait_window("a");
    let (x, y, width, _) = rect(&w["geometry"]);
    pila.click_at(
        (x + width - BUTTON - BUTTON / 2) as f64,
        (y - BAR / 2) as f64,
    );
    let w = pila.wait("maximized", |s| {
        let w = window_in(s, "a").unwrap();
        w["maximized"] == true && w["geometry"]["w"] == 1280
    });
    let w = window_in(&w, "a").unwrap();
    // The whole output below the bar.
    assert_eq!(rect(&w["geometry"]), (0, BAR, 1280, 800 - BAR));
    client.wait_configure(a, "maximized configure", |c| {
        c.has(states::MAXIMIZED) && c.size == (1280, 772)
    });
    // The button is now at the right end of the output.
    pila.click_at((1280 - BUTTON - BUTTON / 2) as f64, (BAR / 2) as f64);
    let state = pila.wait("restored", |s| {
        let w = window_in(s, "a").unwrap();
        w["maximized"] == false && w["geometry"]["w"] == 400
    });
    assert_eq!(
        rect(&window_in(&state, "a").unwrap()["geometry"]),
        (x, y, 400, 300)
    );
}

#[test]
fn the_title_bar_minimize_button_minimizes() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).create();
    let w = pila.wait_window("a");
    let (x, y, width, _) = rect(&w["geometry"]);
    pila.click_at(
        (x + width - 2 * BUTTON - BUTTON / 2) as f64,
        (y - BAR / 2) as f64,
    );
    pila.wait("minimized", |s| {
        let w = window_in(s, "a").unwrap();
        w["minimized"] == true && w["mapped"] == false
    });
}

#[test]
fn the_title_bar_drags_the_window() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).create();
    let w = pila.wait_window("a");
    let (x, y, _, _) = rect(&w["geometry"]);
    pila.move_to((x + 20) as f64, (y - 10) as f64);
    pila.press("left");
    pila.wait("a move grab", |s| s["pointer"]["grab"] == "move");
    pila.move_by(-100.0, 50.0);
    pila.move_by(-10.0, 0.0);
    pila.release("left");
    let state = pila.wait("the grab to end", |s| s["pointer"]["grabbed"] == false);
    assert_eq!(
        rect(&window_in(&state, "a").unwrap()["geometry"]),
        (x - 110, y + 50, 400, 300)
    );
    // The client never saw the drag's click.
    assert!(client.buttons().is_empty());
}

#[test]
fn title_and_app_id_changes_show_in_the_state() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let a = client.window("a").title("first").create();
    pila.wait_window("a");
    client.request(a, |t| t.set_title("second".into()));
    client.surface(a).commit();
    client.flush();
    pila.wait("the new title", |s| {
        window_in(s, "a").unwrap()["title"] == "second"
    });
}

#[test]
fn an_unmapped_window_leaves_taskbars_and_comes_back_as_a_new_one() {
    use wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1;
    let mut pila = TestPila::start();
    let other = pila.client();
    other.window("other").create();
    pila.wait_focus("other");
    let client = pila.client();
    let a = client.window("a").create();
    pila.wait_focus("a");
    let taskbar = pila.client();
    let _list: ExtForeignToplevelListV1 = taskbar.bind("ext_foreign_toplevel_list_v1", 1);
    taskbar.wait_logged("ext_toplevel", "AppId { app_id: \"a\" }");
    client.set_auto_draw(a, false);
    client.unmap(a);
    taskbar.wait_logged("ext_toplevel", "Closed");
    // Focus goes back to the other window, and Mod+Tab has nothing to switch to.
    pila.wait_focus("other");
    pila.combo("Super+Tab");
    pila.stays("still other", std::time::Duration::from_millis(100), |s| {
        s["focused"] == "other"
    });
    client.set_auto_draw(a, true);
    client.surface(a).commit();
    client.flush();
    pila.wait_focus("a");
    taskbar.wait("a second handle", |s| {
        s.log
            .iter()
            .filter(|l| l.tag == "ext_toplevel" && l.event == "AppId { app_id: \"a\" }")
            .count()
            == 2
    });
}

// --- Transient windows (`transients.rs`) ---

/// "parent" and, set on it, "child" (focused, on top).
fn parent_and_child(
    pila: &mut TestPila,
    client: &Client,
) -> (common::client::Window, common::client::Window) {
    let parent = client.window("parent").size(400, 300).create();
    pila.wait_focus("parent");
    let toplevel = client.toplevel(parent);
    let child = client
        .window("child")
        .size(200, 100)
        .before_commit(|t| t.set_parent(Some(&toplevel)))
        .create();
    pila.wait_focus("child");
    (parent, child)
}

#[test]
fn a_child_stays_above_its_parent_when_the_parent_is_focused() {
    let mut pila = TestPila::start();
    let client = pila.client();
    parent_and_child(&mut pila, &client);
    client.window("other").size(100, 100).create();
    let state = pila.wait_focus("other");
    assert_eq!(stacking(&state), vec!["parent", "child", "other"]);
    // Clear of the child (centered on the parent) and of the other window.
    let parent = rect(&window_in(&state, "parent").unwrap()["geometry"]);
    pila.click_at(parent.0 as f64 + 10.0, parent.1 as f64 + 10.0);
    let state = pila.wait_focus("parent");
    assert_eq!(stacking(&state), vec!["other", "parent", "child"]);
}

#[test]
fn focusing_a_child_brings_its_parent_along() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let (parent, child) = parent_and_child(&mut pila, &client);
    client.window("other").size(800, 600).create();
    let state = pila.wait_focus("other");
    assert_eq!(stacking(&state), vec!["parent", "child", "other"]);
    // Mod+Tab goes back to the child, the window used before "other".
    pila.combo("Super+Tab");
    let state = pila.wait_focus("child");
    assert_eq!(stacking(&state), vec!["other", "parent", "child"]);
    client.wait_configure(child, "the child active", |c| c.has(states::ACTIVATED));
    client.wait_configure(parent, "the parent inactive", |c| !c.has(states::ACTIVATED));
}

#[test]
fn siblings_keep_their_order_with_the_focused_one_on_top() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let parent = client.window("parent").size(800, 600).create();
    pila.wait_focus("parent");
    let toplevel = client.toplevel(parent);
    for name in ["first", "second"] {
        client
            .window(name)
            .size(100, 100)
            .before_commit(|t| t.set_parent(Some(&toplevel)))
            .create();
        pila.wait_focus(name);
    }
    pila.combo("Super+Tab");
    let state = pila.wait_focus("first");
    assert_eq!(stacking(&state), vec!["parent", "second", "first"]);
}

#[test]
fn a_parent_set_late_brings_the_child_above_it_without_the_focus() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let child = client.window("child").size(100, 100).create();
    pila.wait_focus("child");
    let parent = client.window("parent").size(400, 300).create();
    let state = pila.wait_focus("parent");
    assert_eq!(stacking(&state), vec!["child", "parent"]);
    let toplevel = client.toplevel(parent);
    client.request(child, |t| t.set_parent(Some(&toplevel)));
    let state = pila.wait("the child on top", |s| stacking(s) == ["parent", "child"]);
    assert_eq!(state["focused"], "parent");
}

#[test]
fn moving_a_parent_keeps_its_child_above() {
    let mut pila = TestPila::start();
    let client = pila.client();
    parent_and_child(&mut pila, &client);
    pila.combo("Super+Left");
    let state = pila.wait("snapped", |s| {
        window_in(s, "parent").unwrap()["snapped"] != Value::Null
            || window_in(s, "child").unwrap()["snapped"] != Value::Null
    });
    assert_eq!(stacking(&state), vec!["parent", "child"]);
    // The parent moved, this time, with the child still above it.
    let parent = rect(&window_in(&state, "parent").unwrap()["geometry"]);
    pila.click_at(parent.0 as f64 + 5.0, parent.1 as f64 + 5.0);
    pila.wait_focus("parent");
    pila.combo("Super+Right");
    let state = pila.wait("the parent snapped", |s| {
        window_in(s, "parent").unwrap()["snapped"] != Value::Null
    });
    assert_eq!(stacking(&state), vec!["parent", "child"]);
}

#[test]
fn a_family_minimizes_and_comes_back_together() {
    let mut pila = TestPila::start();
    let client = pila.client();
    parent_and_child(&mut pila, &client);
    let state = pila.window("parent");
    let parent = rect(&state["geometry"]);
    pila.click_at(parent.0 as f64 + 10.0, parent.1 as f64 + 10.0);
    pila.wait_focus("parent");
    pila.combo("Super+n");
    let state = pila.wait("both minimized", |s| {
        window_in(s, "parent").unwrap()["minimized"] == true
            && window_in(s, "child").unwrap()["minimized"] == true
    });
    assert!(state["focused"].is_null());
    pila.combo("Super+Tab");
    let state = pila.wait("both back", |s| {
        window_in(s, "parent").unwrap()["minimized"] == false
            && window_in(s, "child").unwrap()["minimized"] == false
    });
    assert_eq!(stacking(&state), vec!["parent", "child"]);
}

#[test]
fn a_child_minimized_alone_stays_minimized_when_its_parent_is_focused() {
    let mut pila = TestPila::start();
    let client = pila.client();
    parent_and_child(&mut pila, &client);
    pila.combo("Super+n");
    let state = pila.wait_focus("parent");
    assert_eq!(window_in(&state, "child").unwrap()["minimized"], true);
    assert_eq!(window_in(&state, "parent").unwrap()["minimized"], false);
}

#[test]
fn a_family_changes_workspace_together() {
    let mut pila = TestPila::start();
    let client = pila.client();
    parent_and_child(&mut pila, &client);
    pila.combo("Super+Shift+2");
    let state = pila.wait("both moved", |s| {
        window_in(s, "parent").unwrap()["workspace"] == 2
            && window_in(s, "child").unwrap()["workspace"] == 2
    });
    assert!(state["focused"].is_null());
    pila.combo("Super+2");
    let state = pila.wait_focus("child");
    assert_eq!(stacking(&state), vec!["parent", "child"]);
}

#[test]
fn a_child_opens_on_its_parents_workspace() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let parent = client.window("parent").create();
    pila.wait_focus("parent");
    pila.combo("Super+Shift+2");
    pila.wait("moved", |s| {
        window_in(s, "parent").unwrap()["workspace"] == 2
    });
    let toplevel = client.toplevel(parent);
    client
        .window("child")
        .before_commit(|t| t.set_parent(Some(&toplevel)))
        .create();
    let state = pila.wait("the child on workspace 2", |s| {
        window_in(s, "child").is_some_and(|w| w["workspace"] == 2)
    });
    assert_eq!(state["workspace"], 1);
}

#[test]
fn a_child_outlives_its_parent() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let (parent, _child) = parent_and_child(&mut pila, &client);
    client.destroy(parent);
    let state = pila.wait("the parent gone", |s| window_in(s, "parent").is_none());
    assert_eq!(state["focused"], "child");
    client.window("other").create();
    let state = pila.wait_focus("other");
    assert_eq!(stacking(&state), vec!["child", "other"]);
}
