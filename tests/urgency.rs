//! Urgency: a window asking for attention (an activation token without a
//! serial) is marked urgent until it's focused: shown by its title bar and by
//! ext-workspace on its workspace.

mod common;

use common::*;
use wayland_protocols::{
    ext::workspace::v1::client::{
        ext_workspace_handle_v1::ExtWorkspaceHandleV1,
        ext_workspace_manager_v1::ExtWorkspaceManagerV1,
    },
    xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1,
};

/// `client` asks for attention for `window`: a token with no serial (as
/// chat apps do for a new message), then an activation with it.
fn ask_for_attention(client: &Client, window: common::client::Window) {
    let activation: XdgActivationV1 = client.bind("xdg_activation_v1", 1);
    let token = activation.get_activation_token(&client.qh, "token".into());
    token.commit();
    client.flush();
    client.wait_logged("token", "Done");
    let text = client.logged("token").last().unwrap().clone();
    let token = text.split('"').nth(1).expect("a token string").to_string();
    activation.activate(token, &client.surface(window));
    client.flush();
}

/// The last State a workspace handle got.
fn last_state(client: &Client, workspace: &ExtWorkspaceHandleV1) -> String {
    client
        .events_of(workspace)
        .into_iter()
        .rev()
        .find(|e| e.starts_with("State"))
        .unwrap_or_default()
}

#[test]
fn a_window_asking_for_attention_is_urgent_until_focused() {
    let mut pila =
        TestPila::with_config("[[window-rule]]\napp-id = \"chat\"\nopen-on-workspace = 2\n");
    let editor = pila.client();
    editor.window("editor").create();
    pila.wait_focus("editor");
    let chat = pila.client();
    let window = chat.window("chat").create();
    pila.wait("chat on 2", |s| {
        window_in(s, "chat").is_some_and(|w| w["workspace"] == 2)
    });
    let bar = pila.client();
    let _manager: ExtWorkspaceManagerV1 = bar.bind("ext_workspace_manager_v1", 1);
    bar.wait_logged("ext_workspace_manager_v1", "Done");
    let two: ExtWorkspaceHandleV1 = bar.object("workspace", "Name { name: \"2\" }");

    ask_for_attention(&chat, window);
    let state = pila.wait("chat urgent", |s| {
        window_in(s, "chat").unwrap()["urgent"] == true
    });
    // Not focused: the user is busy elsewhere.
    assert_eq!(state["focused"], "editor");
    let two_id = wayland_client::Proxy::id(&two);
    bar.wait("workspace 2 urgent", |s| {
        s.log
            .iter()
            .rev()
            .find(|l| l.object == two_id && l.event.starts_with("State"))
            .is_some_and(|l| l.event.contains("Urgent"))
    });

    // The user goes there: it's focused, no longer urgent.
    pila.combo("Super+2");
    pila.wait_focus("chat");
    pila.wait("chat no longer urgent", |s| {
        window_in(s, "chat").unwrap()["urgent"] == false
    });
    bar.wait("workspace 2 no longer urgent", |s| {
        s.log
            .iter()
            .rev()
            .find(|l| l.object == two_id && l.event.starts_with("State"))
            .is_some_and(|l| !l.event.contains("Urgent"))
    });
    assert!(last_state(&bar, &two).contains("Active"));
}

#[test]
fn an_urgent_window_has_the_urgent_title_bar() {
    let mut pila = TestPila::with_config("[colors]\nbar-urgent = \"#ff0000\"\n");
    let client = pila.client();
    let a = client.window("a").size(400, 300).create();
    let state = pila.wait_focus("a");
    let (x, y, ..) = rect(&window_in(&state, "a").unwrap()["geometry"]);
    let other = pila.client();
    other.window("b").size(400, 300).create();
    pila.wait_focus("b");
    pila.settle();
    // Its title bar's corner, left of the title, uncovered by b (cascaded).
    let corner = (x as f64 + 3.0, y as f64 - 25.0);
    assert_ne!(pila.pixel(corner.0, corner.1), [255, 0, 0, 255]);
    ask_for_attention(&client, a);
    pila.wait("a urgent", |s| window_in(s, "a").unwrap()["urgent"] == true);
    pila.settle();
    assert_eq!(pila.pixel(corner.0, corner.1), [255, 0, 0, 255]);
}
