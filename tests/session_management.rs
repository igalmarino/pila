//! xdg-session-management-v1 (`session_management.rs`): clients keep their
//! windows' state in sessions and restore it, even after a restart of pila.

mod common;

use std::os::unix::fs::PermissionsExt;

use common::client::{Client, Window};
use common::session_protocol::{
    xdg_session_v1::XdgSessionV1, xdg_toplevel_session_v1::XdgToplevelSessionV1,
};
use common::*;

/// Centered on 1280x800: (1280 - 400) / 2, (800 - 300 - 28) / 2 + 28.
const CENTER: (i64, i64, i64, i64) = (440, 264, 400, 300);
const MOVED: (i64, i64, i64, i64) = (100, 128, 400, 300);

/// Mod+drag the window with its top left at `from` by (`dx`, `dy`).
fn drag(pila: &mut TestPila, from: (i64, i64), dx: f64, dy: f64) {
    pila.move_to((from.0 + 100) as f64, (from.1 + 100) as f64);
    pila.key_press("Super");
    pila.press("left");
    pila.wait("the move grab", |s| s["pointer"]["grab"] == "move");
    pila.move_by(dx, dy);
    pila.release("left");
    pila.key_release("Super");
    pila.wait("the grab ends", |s| s["pointer"]["grabbed"] == false);
}

/// A new window "a" of 400x300, in `session` as `name`.
fn add(client: &Client, session: &XdgSessionV1, name: &str) -> Window {
    client
        .window("a")
        .size(400, 300)
        .before_commit(|toplevel| {
            let _: XdgToplevelSessionV1 =
                session.add_toplevel(toplevel, name.into(), &client.qh, "toplevel".into());
        })
        .create()
}

/// A new window "a" of the default size asking to be restored from `session`
/// as `name`: only its state gives it 400x300.
fn restore(client: &Client, session: &XdgSessionV1, name: &str) -> Window {
    client
        .window("a")
        .size(150, 150)
        .before_commit(|toplevel| {
            let _: XdgToplevelSessionV1 =
                session.restore_toplevel(toplevel, name.into(), &client.qh, "toplevel".into());
        })
        .create()
}

fn wait_gone(pila: &mut TestPila) {
    pila.wait("the window is gone", |s| window_in(s, "a").is_none());
}

fn wait_geometry(pila: &mut TestPila, geometry: (i64, i64, i64, i64)) {
    pila.wait(&format!("the window at {geometry:?}"), |s| {
        window_in(s, "a").is_some_and(|w| rect(&w["geometry"]) == geometry)
    });
}

/// Pila with window "a" in a session, moved to `MOVED`.
fn moved_in_session() -> (TestPila, Client, XdgSessionV1, Window) {
    let mut pila = TestPila::start();
    let client = pila.client();
    let session = client.session(None, "session");
    let window = add(&client, &session, "main");
    pila.wait_focus("a");
    wait_geometry(&mut pila, CENTER);
    drag(&mut pila, (440, 264), -340.0, -136.0);
    wait_geometry(&mut pila, MOVED);
    (pila, client, session, window)
}

fn close(pila: &mut TestPila, client: &Client, window: Window) {
    client.destroy(window);
    wait_gone(pila);
}

/// The connection fails with this protocol error.
#[track_caller]
fn assert_error(client: &Client, code: u32, interface: &str) {
    let error = client.wait_error();
    let expected = format!("Protocol error {code} on object {interface}@");
    assert!(error.contains(&expected), "{error}");
}

/// A state directory that outlives a pila.
struct StateDir(std::path::PathBuf);

impl StateDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("pila-state-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        StateDir(dir)
    }

    fn start_pila(&self) -> TestPila {
        TestPila::start_with(Options {
            env: vec![("XDG_STATE_HOME".into(), self.0.display().to_string())],
            ..Options::default()
        })
    }
}

impl Drop for StateDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_new_session_gets_an_unguessable_id() {
    let pila = TestPila::start();
    let client = pila.client();
    client.session(None, "one");
    client.session(None, "two");
    let (one, two) = (client.session_id("one"), client.session_id("two"));
    assert_eq!(one.len(), 32, "{one}");
    assert_ne!(one, two);
    assert!(!client.has_logged("one", "Restored"));
}

#[test]
fn a_window_goes_back_to_its_place_and_size() {
    let (mut pila, client, session, window) = moved_in_session();
    close(&mut pila, &client, window);
    restore(&client, &session, "main");
    let w = pila.wait_window("a");
    assert_eq!(rect(&w["geometry"]), MOVED);
    pila.wait_focus("a");
}

#[test]
fn restored_comes_before_the_first_configure() {
    let (mut pila, client, session, window) = moved_in_session();
    close(&mut pila, &client, window);
    let before = client.configures(window).len();
    restore(&client, &session, "main");
    pila.wait_window("a");
    client.wait_logged("toplevel", "Restored");
    let event = &client.logged("toplevel")[0];
    assert!(
        event.ends_with(&format!("configures={before}")),
        "{event}, {before} before"
    );
}

#[test]
fn a_toplevel_just_added_isnt_told_restored() {
    let (mut pila, client, _session, _window) = moved_in_session();
    pila.settle();
    client.roundtrip();
    assert!(client.logged("toplevel").is_empty());
}

#[test]
fn a_client_unmapping_before_destroying_keeps_the_size() {
    let (mut pila, client, session, window) = moved_in_session();
    client.set_auto_draw(window, false);
    client.unmap(window);
    wait_gone(&mut pila);
    client.destroy(window);
    client.roundtrip();
    restore(&client, &session, "main");
    assert_eq!(rect(&pila.wait_window("a")["geometry"]), MOVED);
}

#[test]
fn a_window_mapped_again_without_restoring_is_a_new_one() {
    let (mut pila, client, _session, window) = moved_in_session();
    client.set_auto_draw(window, false);
    client.unmap(window);
    wait_gone(&mut pila);
    client.set_auto_draw(window, true);
    client.surface(window).commit();
    client.flush();
    assert_eq!(rect(&pila.wait_window("a")["geometry"]), CENTER);
}

#[test]
fn without_asking_to_restore_a_window_opens_as_usual() {
    let (mut pila, client, session, window) = moved_in_session();
    close(&mut pila, &client, window);
    add(&client, &session, "other");
    assert_eq!(rect(&pila.wait_window("a")["geometry"]), CENTER);
}

#[test]
fn a_window_outside_any_session_opens_as_usual() {
    let (mut pila, client, _session, window) = moved_in_session();
    close(&mut pila, &client, window);
    client.window("a").size(400, 300).create();
    assert_eq!(rect(&pila.wait_window("a")["geometry"]), CENTER);
}

#[test]
fn an_unknown_name_is_like_adding_it() {
    let pila = TestPila::start();
    let client = pila.client();
    let mut pila = pila;
    let session = client.session(None, "session");
    restore(&client, &session, "new");
    pila.wait_window("a");
    client.roundtrip();
    assert!(client.logged("toplevel").is_empty());
}

#[test]
fn a_taken_spot_cascades() {
    let (mut pila, client, session, window) = moved_in_session();
    close(&mut pila, &client, window);
    client.window("other").size(400, 300).create();
    pila.wait_focus("other");
    drag(&mut pila, (440, 264), -340.0, -136.0);
    pila.wait("other moved", |s| {
        rect(&window_in(s, "other").unwrap()["geometry"]) == MOVED
    });
    restore(&client, &session, "main");
    assert_eq!(
        rect(&pila.wait_window("a")["geometry"]),
        (132, 160, 400, 300)
    );
}

#[test]
fn a_window_partly_off_the_output_comes_back_inside() {
    let (mut pila, client, session, window) = moved_in_session();
    drag(&mut pila, (100, 128), 1000.0, 400.0);
    wait_geometry(&mut pila, (1100, 528, 400, 300));
    close(&mut pila, &client, window);
    restore(&client, &session, "main");
    assert_eq!(
        rect(&pila.wait_window("a")["geometry"]),
        (880, 500, 400, 300)
    );
}

#[test]
fn a_window_closed_maximized_comes_back_maximized() {
    let (mut pila, client, session, window) = moved_in_session();
    pila.combo("Super+m");
    pila.wait("maximized", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == (0, 28, 1280, 772)
    });
    // Saved by the next iteration; a roundtrip gets there.
    client.roundtrip();
    close(&mut pila, &client, window);
    restore(&client, &session, "main");
    wait_geometry(&mut pila, (0, 28, 1280, 772));
    // And it leaves for where it was before.
    pila.combo("Super+m");
    wait_geometry(&mut pila, MOVED);
}

#[test]
fn a_window_closed_fullscreen_comes_back_fullscreen() {
    let (mut pila, client, session, window) = moved_in_session();
    client.request(window, |t| t.set_fullscreen(None));
    pila.wait("fullscreen", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]) == (0, 0, 1280, 800)
    });
    client.roundtrip();
    close(&mut pila, &client, window);
    let restored = restore(&client, &session, "main");
    wait_geometry(&mut pila, (0, 0, 1280, 800));
    client.request(restored, |t| t.unset_fullscreen());
    wait_geometry(&mut pila, MOVED);
}

#[test]
fn a_window_closed_snapped_comes_back_where_it_was_before() {
    let (mut pila, client, session, window) = moved_in_session();
    pila.combo("Super+Right");
    pila.wait("snapped", |s| {
        rect(&window_in(s, "a").unwrap()["geometry"]).0 == 640
    });
    client.roundtrip();
    close(&mut pila, &client, window);
    restore(&client, &session, "main");
    assert_eq!(rect(&pila.wait_window("a")["geometry"]), MOVED);
}

#[test]
fn a_window_closed_on_another_workspace_comes_back_on_the_active_one() {
    let (mut pila, client, session, window) = moved_in_session();
    pila.combo("Super+2");
    pila.wait("workspace 2", |s| s["workspace"] == 2);
    close(&mut pila, &client, window);
    restore(&client, &session, "main");
    let w = pila.wait_window("a");
    assert_eq!(rect(&w["geometry"]), MOVED);
    assert_eq!(w["workspace"], 2);
}

#[test]
fn it_goes_back_to_its_output_wherever_the_pointer_is() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1024x768".into()),
        ..Options::default()
    });
    let client = pila.client();
    let session = client.session(None, "session");
    pila.move_to(1500.0, 300.0);
    let window = add(&client, &session, "main");
    wait_geometry(&mut pila, (1592, 248, 400, 300));
    drag(&mut pila, (1592, 248), 100.0, 100.0);
    wait_geometry(&mut pila, (1692, 348, 400, 300));
    close(&mut pila, &client, window);
    pila.move_to(100.0, 100.0);
    restore(&client, &session, "main");
    let w = pila.wait_window("a");
    assert_eq!(rect(&w["geometry"]), (1692, 348, 400, 300));
    assert_eq!(w["output"], "B");
}

#[test]
fn a_filled_window_goes_back_to_its_output() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1024x768".into()),
        ..Options::default()
    });
    let client = pila.client();
    let session = client.session(None, "session");
    pila.move_to(1500.0, 300.0);
    let window = add(&client, &session, "main");
    wait_geometry(&mut pila, (1592, 248, 400, 300));
    pila.combo("Super+m");
    wait_geometry(&mut pila, (1280, 28, 1024, 740));
    client.roundtrip();
    close(&mut pila, &client, window);
    pila.move_to(100.0, 100.0);
    restore(&client, &session, "main");
    wait_geometry(&mut pila, (1280, 28, 1024, 740));
}

#[test]
fn with_its_output_gone_it_opens_as_usual_with_its_size() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1024x768".into()),
        ..Options::default()
    });
    let client = pila.client();
    let session = client.session(None, "session");
    pila.move_to(1500.0, 300.0);
    let window = add(&client, &session, "main");
    wait_geometry(&mut pila, (1592, 248, 400, 300));
    close(&mut pila, &client, window);
    pila.request(serde_json::json!({ "cmd": "output_remove", "name": "B" }));
    pila.wait("B is gone", |s| s["outputs"].as_array().unwrap().len() == 1);
    restore(&client, &session, "main");
    assert_eq!(rect(&pila.wait_window("a")["geometry"]), CENTER);
}

#[test]
fn rules_win_over_the_session() {
    let mut pila = TestPila::with_config(
        r#"
        [[window-rule]]
        app-id = "a"
        position = [20, 30]
        size = [500, 350]
        "#,
    );
    let client = pila.client();
    let session = client.session(None, "session");
    let window = add(&client, &session, "main");
    pila.wait_focus("a");
    let (x, y, w, h) = rect(&pila.window("a")["geometry"]);
    assert_eq!((w, h), (500, 350));
    drag(&mut pila, (x, y), 200.0, 200.0);
    wait_geometry(&mut pila, (x + 200, y + 200, w, h));
    close(&mut pila, &client, window);
    restore(&client, &session, "main");
    assert_eq!(rect(&pila.wait_window("a")["geometry"]), (x, y, w, h));
}

#[test]
fn renaming_keeps_the_state() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let session = client.session(None, "session");
    let object = std::cell::RefCell::new(None);
    let window = client
        .window("a")
        .size(400, 300)
        .before_commit(|toplevel| {
            *object.borrow_mut() = Some(session.add_toplevel(
                toplevel,
                "old".into(),
                &client.qh,
                "toplevel".to_string(),
            ));
        })
        .create();
    pila.wait_focus("a");
    wait_geometry(&mut pila, CENTER);
    drag(&mut pila, (440, 264), -340.0, -136.0);
    wait_geometry(&mut pila, MOVED);
    let object: XdgToplevelSessionV1 = object.into_inner().unwrap();
    object.rename("new".into());
    client.flush();
    client.roundtrip();
    close(&mut pila, &client, window);
    restore(&client, &session, "new");
    assert_eq!(rect(&pila.wait_window("a")["geometry"]), MOVED);
}

#[test]
fn removing_a_toplevel_forgets_it() {
    let (mut pila, client, session, window) = moved_in_session();
    close(&mut pila, &client, window);
    session.remove_toplevel("main".into());
    client.flush();
    restore(&client, &session, "main");
    pila.wait_window("a");
    client.roundtrip();
    assert!(client.logged("toplevel").is_empty());
}

#[test]
fn removing_a_session_forgets_it() {
    let (mut pila, client, session, window) = moved_in_session();
    let id = client.session_id("session");
    close(&mut pila, &client, window);
    session.remove();
    client.flush();
    client.roundtrip();
    client.session(Some(&id), "again");
    let new_id = client.session_id("again");
    assert_ne!(id, new_id);
}

#[test]
fn a_destroyed_session_keeps_its_state_and_stops_following() {
    let (mut pila, client, session, window) = moved_in_session();
    let id = client.session_id("session");
    session.destroy();
    client.flush();
    client.roundtrip();
    drag(&mut pila, (100, 128), 50.0, 50.0);
    wait_geometry(&mut pila, (150, 178, 400, 300));
    close(&mut pila, &client, window);
    let session = client.session(Some(&id), "again");
    client.wait_logged("again", "Restored");
    restore(&client, &session, "main");
    assert_eq!(rect(&pila.wait_window("a")["geometry"]), MOVED);
}

#[test]
fn a_dead_clients_session_can_be_taken_up_again() {
    let (mut pila, mut client, _session, _window) = moved_in_session();
    let id = client.session_id("session");
    client.disconnect();
    wait_gone(&mut pila);
    let client = pila.client();
    let session = client.session(Some(&id), "again");
    client.wait_logged("again", "Restored");
    restore(&client, &session, "main");
    assert_eq!(rect(&pila.wait_window("a")["geometry"]), MOVED);
}

#[test]
fn another_client_takes_over_and_the_old_session_goes_inert() {
    let (mut pila, client, _session, _window) = moved_in_session();
    let id = client.session_id("session");
    let other = pila.client();
    let session = other.session(Some(&id), "again");
    other.wait_logged("again", "Restored");
    client.wait_logged("session", "Replaced");
    // The old client's window no longer follows into the session.
    drag(&mut pila, (100, 128), 50.0, 50.0);
    wait_geometry(&mut pila, (150, 178, 400, 300));
    other
        .window("b")
        .size(150, 150)
        .before_commit(|toplevel| {
            let _: XdgToplevelSessionV1 =
                session.restore_toplevel(toplevel, "main".into(), &other.qh, "toplevel".into());
        })
        .create();
    assert_eq!(rect(&pila.wait_window("b")["geometry"]), MOVED);
}

#[test]
fn sessions_outlive_pila() {
    let state = StateDir::new("restart");
    let mut pila = state.start_pila();
    let client = pila.client();
    let session = client.session(None, "session");
    add(&client, &session, "main");
    pila.wait_focus("a");
    wait_geometry(&mut pila, CENTER);
    drag(&mut pila, (440, 264), -340.0, -136.0);
    wait_geometry(&mut pila, MOVED);
    let id = client.session_id("session");
    drop(client);
    drop(pila);

    let mut pila = state.start_pila();
    let client = pila.client();
    let session = client.session(Some(&id), "again");
    client.wait_logged("again", "Restored");
    restore(&client, &session, "main");
    assert_eq!(rect(&pila.wait_window("a")["geometry"]), MOVED);
    client.wait_logged("toplevel", "Restored");
}

#[test]
fn sessions_are_saved_soon_and_only_for_the_user() {
    let (pila, client, _session, _window) = moved_in_session();
    let id = client.session_id("session");
    let path = pila.dir.join("state/pila/sessions.json");
    let text = pila.wait_file("state/pila/sessions.json");
    assert!(text.contains(&id), "{text}");
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn a_broken_sessions_file_is_ignored() {
    let state = StateDir::new("broken");
    std::fs::create_dir_all(state.0.join("pila")).unwrap();
    std::fs::write(state.0.join("pila/sessions.json"), "not json").unwrap();
    let mut pila = state.start_pila();
    pila.wait_log("ignoring a broken sessions file");
    let client = pila.client();
    client.session(Some("gone"), "session");
    client.session_id("session");
}

#[test]
fn an_unknown_session_id_gets_a_new_session() {
    let pila = TestPila::start();
    let client = pila.client();
    client.session(Some("unknown"), "session");
    assert_ne!(client.session_id("session"), "unknown");
}

#[test]
fn a_session_in_use_by_the_same_client_is_an_error() {
    let pila = TestPila::start();
    let client = pila.client();
    client.session(None, "session");
    let id = client.session_id("session");
    client.session(Some(&id), "again");
    assert_error(&client, 1, "xdg_session_manager_v1");
}

#[test]
fn an_empty_session_id_is_an_error() {
    let pila = TestPila::start();
    let client = pila.client();
    client.session(Some(""), "session");
    assert_error(&client, 2, "xdg_session_manager_v1");
}

#[test]
fn an_empty_toplevel_name_is_an_error() {
    let pila = TestPila::start();
    let client = pila.client();
    let session = client.session(None, "session");
    add(&client, &session, "");
    assert_error(&client, 3, "xdg_session_v1");
}

#[test]
fn adding_a_known_name_is_an_error() {
    let (mut pila, client, session, window) = moved_in_session();
    close(&mut pila, &client, window);
    add(&client, &session, "main");
    assert_error(&client, 1, "xdg_session_v1");
}

#[test]
fn restoring_a_name_in_use_is_an_error() {
    let (_pila, client, session, _window) = moved_in_session();
    restore(&client, &session, "main");
    assert_error(&client, 1, "xdg_session_v1");
}

#[test]
fn adding_a_toplevel_twice_is_an_error() {
    let pila = TestPila::start();
    let client = pila.client();
    let session = client.session(None, "session");
    let window = add(&client, &session, "main");
    client.request(window, |toplevel| {
        let _: XdgToplevelSessionV1 =
            session.add_toplevel(toplevel, "second".into(), &client.qh, "toplevel".into());
    });
    assert_error(&client, 4, "xdg_session_v1");
}

#[test]
fn restoring_after_the_first_commit_is_an_error() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let session = client.session(None, "session");
    let window = client.window("a").size(400, 300).create();
    pila.wait_window("a");
    client.request(window, |toplevel| {
        let _: XdgToplevelSessionV1 =
            session.restore_toplevel(toplevel, "main".into(), &client.qh, "toplevel".into());
    });
    assert_error(&client, 2, "xdg_session_v1");
}

#[test]
fn renaming_to_a_name_in_use_is_an_error() {
    let pila = TestPila::start();
    let client = pila.client();
    let session = client.session(None, "session");
    let object = std::cell::RefCell::new(None);
    client
        .window("a")
        .before_commit(|toplevel| {
            *object.borrow_mut() = Some(session.add_toplevel(
                toplevel,
                "one".into(),
                &client.qh,
                "toplevel".to_string(),
            ));
        })
        .create();
    client
        .window("b")
        .before_commit(|toplevel| {
            let _: XdgToplevelSessionV1 =
                session.add_toplevel(toplevel, "two".into(), &client.qh, "toplevel".into());
        })
        .create();
    let object: XdgToplevelSessionV1 = object.into_inner().unwrap();
    object.rename("two".into());
    client.flush();
    assert_error(&client, 1, "xdg_session_v1");
}
