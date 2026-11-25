//! Direct Wayland screen capture permissions (`[permissions]`, `[[permission]]`,
//! modal prompt, persistence, and `pila msg` management).

mod common;

use common::protocols::read_pixel;
use common::*;
use std::time::Duration;
use wayland_client::protocol::wl_shm::Format;
use wayland_protocols::ext::{
    image_capture_source::v1::client::{
        ext_image_capture_source_v1::ExtImageCaptureSourceV1,
        ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
    },
    image_copy_capture::v1::client::{
        ext_image_copy_capture_manager_v1::{
            ExtImageCopyCaptureManagerV1, Options as CaptureOptions,
        },
        ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1,
    },
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

const RED: [u8; 4] = [255, 255, 0, 0];

fn current_exe() -> String {
    std::env::current_exe()
        .expect("current_exe")
        .to_string_lossy()
        .into_owned()
}

fn output_source(client: &Client, output: &str) -> ExtImageCaptureSourceV1 {
    let manager: ExtOutputImageCaptureSourceManagerV1 =
        client.bind("ext_output_image_capture_source_manager_v1", 1);
    manager.create_source(&client.output(output), &client.qh, "source".into())
}

fn start_ext_session(client: &Client, output: &str, tag: &str) -> ExtImageCopyCaptureSessionV1 {
    let source = output_source(client, output);
    let manager: ExtImageCopyCaptureManagerV1 = client.bind("ext_image_copy_capture_manager_v1", 1);
    let session = manager.create_session(&source, CaptureOptions::empty(), &client.qh, tag.into());
    client.flush();
    session
}

fn start_screencopy(
    client: &Client,
    output: &str,
    tag: &str,
) -> (ZwlrScreencopyFrameV1, std::fs::File) {
    let manager: ZwlrScreencopyManagerV1 = client.bind("zwlr_screencopy_manager_v1", 3);
    let frame = manager.capture_output(0, &client.output(output), &client.qh, tag.into());
    client.flush();
    client.wait_logged(tag, "BufferDone");
    let (buffer, file) = client.capture_buffer((1280, 800), Format::Xrgb8888);
    frame.copy(&buffer);
    client.flush();
    (frame, file)
}

fn msg_json(pila: &TestPila, args: &[&str]) -> Value {
    let mut full = vec!["--json"];
    full.extend_from_slice(args);
    let out = pila.msg(&full);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{args:?}: {stdout}{stderr}");
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("bad JSON {stdout:?}: {e}"))
}

/// Wait for the prompt to be shown and take answers: Allow does nothing
/// for a moment after it appears (`permissions::ALLOW_DELAY`).
fn wait_armed(pila: &mut TestPila) -> Value {
    pila.wait("permission prompt shown and armed", |s| {
        s["capture_permission"]["shown"]["text"] == true
            && s["capture_permission"]["shown"]["armed"] == true
    })
}

fn center(rect: &Value) -> (f64, f64) {
    let (x, y, w, h) = common::rect(rect);
    (x as f64 + w as f64 / 2.0, y as f64 + h as f64 / 2.0)
}

fn click_at(pila: &mut TestPila, x: f64, y: f64) {
    pila.move_to(x, y);
    pila.click("left");
}

#[test]
fn capture_permissions_off_by_default_allows_both_protocols() {
    let mut pila = TestPila::start();
    let client = pila.client();
    pila.settle();

    let _session = start_ext_session(&client, "HEADLESS-1", "ext");
    client.wait_logged("ext", "Done");

    let (_frame, _file) = start_screencopy(&client, "HEADLESS-1", "sc");
    client.wait_logged("sc", "Ready");

    let state = pila.state();
    assert_eq!(state["capture_permission"]["enforce"], false);
    assert!(state["capture_permission"]["shown"].is_null());
}

#[test]
fn allow_and_deny_rules_control_screencopy_and_ext_capture() {
    let exe = current_exe();
    // Wildcard rule denying everything, overridden by a later allow rule matching us.
    let allow_cfg = format!(
        "[permissions]\nenforce = true\n\
         [[permission]]\nexecutable = \"*\"\naccess = \"deny\"\n\
         [[permission]]\nexecutable = {exe:?}\naccess = \"allow\"\n"
    );
    let mut pila = TestPila::with_config(&allow_cfg);
    let client = pila.client();
    pila.settle();

    let _session = start_ext_session(&client, "HEADLESS-1", "ext");
    client.wait_logged("ext", "Done");
    let (_frame, _file) = start_screencopy(&client, "HEADLESS-1", "sc");
    client.wait_logged("sc", "Ready");
    assert!(pila.state()["capture_permission"]["shown"].is_null());

    // Last matching rule wins: wildcard deny after allow denies both protocols without prompting.
    let deny_cfg = format!(
        "[permissions]\nenforce = true\n\
         [[permission]]\nexecutable = {exe:?}\naccess = \"allow\"\n\
         [[permission]]\nexecutable = \"*permissions*\"\naccess = \"deny\"\n"
    );
    let mut pila = TestPila::with_config(&deny_cfg);
    let client = pila.client();
    pila.settle();

    let _session = start_ext_session(&client, "HEADLESS-1", "ext");
    client.wait_logged("ext", "Stopped");
    let (_frame, _file) = start_screencopy(&client, "HEADLESS-1", "sc");
    client.wait_logged("sc", "Failed");
    assert!(pila.state()["capture_permission"]["shown"].is_null());
}

#[test]
fn prompt_allows_and_denies_capture_and_blocks_app_input() {
    let mut pila = TestPila::with_config(
        "[permissions]\nenforce = true\n\
         [[window-rule]]\napp-id = \"app\"\ntitle-bar = false\n\
         size = [400, 300]\nposition = [440, 250]\n",
    );
    let client = pila.client();
    client.window("app").color(RED).create();
    pila.wait_focus("app");
    pila.settle();

    // Unlisted client requests ext-image-copy-capture -> modal prompt opens.
    let _session = start_ext_session(&client, "HEADLESS-1", "denied_ext");
    let state = pila.wait("permission prompt shown", |s| {
        s["capture_permission"]["shown"]["text"] == true
    });
    assert_eq!(
        state["capture_permission"]["shown"]["executable"],
        current_exe()
    );
    assert_eq!(state["capture_permission"]["shown"]["remember"], false);
    assert!(!client.has_logged("denied_ext", "Done"));
    assert!(!client.has_logged("denied_ext", "Stopped"));

    // Modal: keys and clicks outside the buttons never reach the focused window.
    client.clear_events();
    pila.key("a");
    click_at(&mut pila, 10.0, 10.0);
    pila.settle();
    client.roundtrip();
    assert!(
        client.events().is_empty(),
        "input leaked to app while prompt was open: {:?}",
        client.events()
    );

    // Escape denies the session cleanly.
    pila.key("Escape");
    client.wait_logged("denied_ext", "Stopped");
    pila.wait("prompt closed", |s| {
        s["capture_permission"]["shown"].is_null()
    });

    // Denied for this connection: its next request fails without asking
    // again (a program retrying would bring the modal prompt back forever).
    let (_frame, _file) = start_screencopy(&client, "HEADLESS-1", "denied_again");
    client.wait_logged("denied_again", "Failed");
    assert!(pila.state()["capture_permission"]["shown"].is_null());

    // The same program connecting again is asked again.
    let client = pila.client();
    let (_frame, file) = start_screencopy(&client, "HEADLESS-1", "allowed_sc");
    wait_armed(&mut pila);

    // Enter allows it; the captured frame must show the red window cleanly (no prompt dim).
    pila.key("Return");
    client.wait_logged("allowed_sc", "Ready");
    pila.wait("prompt closed after allow", |s| {
        s["capture_permission"]["shown"].is_null()
    });
    let [r, g, b] = read_pixel(&file, 1280, 640, 400);
    assert!(
        r > 230 && g < 20 && b < 20,
        "captured pixel should be the red window without the prompt dim: {r},{g},{b}"
    );

    // Subsequent captures in the same session succeed without prompting again.
    let _session2 = start_ext_session(&client, "HEADLESS-1", "session_granted");
    client.wait_logged("session_granted", "Done");
    assert!(pila.state()["capture_permission"]["shown"].is_null());

    // Listed by IPC as a non-persistent session grant, with every field
    // (they're only added to: CONTRIBUTING.md, "Compatibility").
    let listed = msg_json(&pila, &["capture-permissions"]);
    let grants = listed["capture_permissions"].as_array().unwrap();
    assert_eq!(grants.len(), 1);
    for field in ["id", "executable", "persistent"] {
        assert!(
            grants[0].get(field).is_some(),
            "lost {field:?}: {}",
            grants[0]
        );
    }
    assert_eq!(grants[0]["executable"], current_exe());
    assert_eq!(grants[0]["persistent"], false);
}

#[test]
fn remembered_permission_persists_across_restart_and_can_be_forgotten_via_ipc() {
    let state_dir = std::env::temp_dir().join(format!(
        "pila-perm-state-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&state_dir);
    std::fs::create_dir_all(&state_dir).unwrap();
    let state_env = vec![(
        "XDG_STATE_HOME".to_string(),
        state_dir.to_string_lossy().into_owned(),
    )];

    {
        let mut pila = TestPila::start_with(Options {
            config: "[permissions]\nenforce = true\n".into(),
            env: state_env.clone(),
            ..Default::default()
        });
        let client = pila.client();
        pila.settle();

        let _session = start_ext_session(&client, "HEADLESS-1", "first");
        let state = wait_armed(&mut pila);
        let (rx, ry) = center(&state["capture_permission"]["shown"]["remember_rect"]);
        let (ax, ay) = center(&state["capture_permission"]["shown"]["allow"]);

        // Click the Remember toggle, then click Allow.
        click_at(&mut pila, rx, ry);
        pila.wait("remember checked", |s| {
            s["capture_permission"]["shown"]["remember"] == true
        });
        click_at(&mut pila, ax, ay);
        client.wait_logged("first", "Done");
        pila.wait("prompt closed", |s| {
            s["capture_permission"]["shown"].is_null()
        });

        let listed = msg_json(&pila, &["capture-permissions"]);
        let grants = listed["capture_permissions"].as_array().unwrap();
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0]["persistent"], true);
        // Wait for the background writer to flush the file before restarting.
        let file = state_dir.join("pila/capture-permissions.json");
        pila.wait("permissions file written", |_| file.exists());
    }

    // Second Pila instance sharing XDG_STATE_HOME restores the remembered permission.
    {
        let mut pila = TestPila::start_with(Options {
            config: "[permissions]\nenforce = true\n".into(),
            env: state_env,
            ..Default::default()
        });
        pila.wait("permissions loaded", |s| {
            s["capture_permission"]["loaded"] == true
        });
        let client = pila.client();
        pila.settle();

        let _active = start_ext_session(&client, "HEADLESS-1", "restored");
        client.wait_logged("restored", "Done");
        assert!(pila.state()["capture_permission"]["shown"].is_null());

        let listed = msg_json(&pila, &["capture-permissions"]);
        let id = listed["capture_permissions"][0]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Forgetting the grant stops the active session and forces a prompt next time.
        let forgot = msg_json(&pila, &["forget-capture-permission", &id]);
        assert_eq!(forgot["ok"], true);
        client.wait_logged("restored", "Stopped");

        let _after = start_ext_session(&client, "HEADLESS-1", "after_forget");
        wait_armed(&mut pila);
        pila.key("r");
        pila.wait("remember toggled by R", |s| {
            s["capture_permission"]["shown"]["remember"] == true
        });
        pila.key("Return");
        client.wait_logged("after_forget", "Done");

        let forgot_all = msg_json(&pila, &["forget-capture-permission", "all"]);
        assert_eq!(forgot_all["forgotten"], 1);
        assert!(
            msg_json(&pila, &["capture-permissions"])["capture_permissions"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    let _ = std::fs::remove_dir_all(&state_dir);
}

#[test]
fn locking_screen_cancels_open_permission_prompt() {
    let mut pila = TestPila::with_config("[permissions]\nenforce = true\n");
    let client = pila.client();
    pila.settle();

    let _session = start_ext_session(&client, "HEADLESS-1", "locked_ext");
    pila.wait("prompt shown", |s| {
        s["capture_permission"]["shown"]["text"] == true
    });

    let locker = pila.client();
    let lock = locker.lock_session();
    let surf = locker.lock_surface(&lock, "HEADLESS-1", [255, 10, 20, 30]);
    locker.wait_surface_configured(surf);
    locker.wait_logged("lock", "Locked");
    pila.wait("locked and prompt closed", |s| {
        s["locked"] == true && s["capture_permission"]["shown"].is_null()
    });
    client.wait_logged("locked_ext", "Stopped");

    // Requests arriving while locked without an allow rule are denied immediately.
    let _while_locked = start_ext_session(&client, "HEADLESS-1", "while_locked");
    client.wait_logged("while_locked", "Stopped");
    assert!(pila.state()["capture_permission"]["shown"].is_null());
}

#[test]
fn config_reload_updates_capture_permissions() {
    let exe = current_exe();
    let mut pila = TestPila::with_config("[permissions]\nenforce = true\n");
    let client = pila.client();
    pila.settle();

    // Open a prompt, then reload config allowing this executable -> prompt closes and capture proceeds.
    let _session = start_ext_session(&client, "HEADLESS-1", "reloaded");
    pila.wait("prompt shown", |s| {
        s["capture_permission"]["shown"]["text"] == true
    });
    pila.write_config(&format!(
        "[permissions]\nenforce = true\n\
         [[permission]]\nexecutable = {exe:?}\naccess = \"allow\"\n"
    ));
    client.wait_logged("reloaded", "Done");
    pila.wait("prompt closed by reload", |s| {
        s["capture_permission"]["shown"].is_null()
    });

    // Reloading with a deny rule stops the active capture session immediately.
    pila.write_config(&format!(
        "[permissions]\nenforce = true\n\
         [[permission]]\nexecutable = {exe:?}\naccess = \"deny\"\n"
    ));
    client.wait_logged("reloaded", "Stopped");
}

#[test]
fn permission_prompt_opened_before_fonts_load_gets_its_text() {
    let mut pila = TestPila::start_with(Options {
        config: "[permissions]\nenforce = true\n".into(),
        hold_fonts: true,
        ..Default::default()
    });
    let client = pila.client();
    let _session = start_ext_session(&client, "HEADLESS-1", "early");
    pila.wait("prompt open without text", |s| {
        !s["capture_permission"]["shown"].is_null()
            && s["capture_permission"]["shown"]["text"] == false
    });
    pila.release_fonts();
    pila.wait("prompt text arrived", |s| {
        s["capture_permission"]["shown"]["text"] == true
    });
    pila.key("Escape");
    client.wait_logged("early", "Stopped");
}

/// A prompt that pops up while the user is typing mustn't take an Enter
/// meant for the app as an Allow: Allow does nothing for a moment after the
/// prompt shows, by key or click; Deny works at once.
#[test]
fn allow_waits_a_moment_after_the_prompt_shows() {
    let mut pila = TestPila::with_config("[permissions]\nenforce = true\n");
    let client = pila.client();
    pila.settle();

    let _session = start_ext_session(&client, "HEADLESS-1", "early");
    let state = pila.wait("prompt shown, not armed yet", |s| {
        s["capture_permission"]["shown"]["text"] == true
    });
    assert_eq!(state["capture_permission"]["shown"]["armed"], false);
    let (ax, ay) = center(&state["capture_permission"]["shown"]["allow"]);
    pila.key("Return");
    click_at(&mut pila, ax, ay);
    pila.settle();
    client.roundtrip();
    assert!(!client.has_logged("early", "Done"));
    assert!(!pila.state()["capture_permission"]["shown"].is_null());

    wait_armed(&mut pila);
    pila.key("Return");
    client.wait_logged("early", "Done");

    // Deny needs no wait (the grant forgotten, so it asks again).
    msg_json(&pila, &["forget-capture-permission", "all"]);
    let other = pila.client();
    let _session = start_ext_session(&other, "HEADLESS-1", "denied");
    pila.wait("second prompt shown", |s| {
        s["capture_permission"]["shown"]["text"] == true
    });
    pila.key("Escape");
    other.wait_logged("denied", "Stopped");
}

/// A program capturing every output (grim) asks once: its requests share one
/// prompt, and one answer settles them all.
#[test]
fn one_prompt_for_a_programs_captures_of_several_outputs() {
    let mut pila = TestPila::start_with(Options {
        config: "[permissions]\nenforce = true\n".into(),
        outputs: Some("A:1280x800,B:1024x768".into()),
        ..Default::default()
    });
    let client = pila.client();
    pila.settle();
    let _a = start_ext_session(&client, "A", "a");
    let _b = start_ext_session(&client, "B", "b");
    let state = pila.wait("one prompt with both requests", |s| {
        s["capture_permission"]["shown"]["requests"] == 2
    });
    assert_eq!(state["capture_permission"]["waiting"], 0);
    wait_armed(&mut pila);
    pila.key("Return");
    client.wait_logged("a", "Done");
    client.wait_logged("b", "Done");
    assert!(pila.state()["capture_permission"]["shown"].is_null());
}

/// A request given up while its prompt is shown takes the prompt away; the
/// next program's prompt shows.
#[test]
fn a_request_given_up_takes_its_prompt_away() {
    let mut pila = TestPila::with_config("[permissions]\nenforce = true\n");
    let first = pila.client();
    let second = pila.client();
    pila.settle();
    let session = start_ext_session(&first, "HEADLESS-1", "first");
    pila.wait("first prompt", |s| {
        !s["capture_permission"]["shown"].is_null()
    });
    // Same program, but its own connection's prompt waits behind (the key is
    // the executable: it joins the shown one).
    let (frame, _file) = start_screencopy(&second, "HEADLESS-1", "second");
    pila.wait("both requests on the prompt", |s| {
        s["capture_permission"]["shown"]["requests"] == 2
    });
    session.destroy();
    first.flush();
    pila.wait("one request left", |s| {
        s["capture_permission"]["shown"]["requests"] == 1
    });
    frame.destroy();
    second.flush();
    pila.wait("prompt gone", |s| {
        s["capture_permission"]["shown"].is_null()
    });
    // A client gone with its request waiting takes it away too.
    let third = pila.client();
    let _session = start_ext_session(&third, "HEADLESS-1", "third");
    pila.wait("third prompt", |s| {
        !s["capture_permission"]["shown"].is_null()
    });
    drop(third);
    pila.wait("prompt gone with its client", |s| {
        s["capture_permission"]["shown"].is_null()
    });
}

/// `forget-capture-permission all` counts what `capture-permissions` lists.
#[test]
fn forgetting_all_counts_what_was_listed() {
    let mut pila = TestPila::with_config("[permissions]\nenforce = true\n");
    let client = pila.client();
    pila.settle();
    assert_eq!(
        msg_json(&pila, &["forget-capture-permission", "all"])["forgotten"],
        0
    );
    let _session = start_ext_session(&client, "HEADLESS-1", "s");
    wait_armed(&mut pila);
    pila.key("Return");
    client.wait_logged("s", "Done");
    let listed = msg_json(&pila, &["capture-permissions"]);
    let count = listed["capture_permissions"].as_array().unwrap().len();
    assert_eq!(
        msg_json(&pila, &["forget-capture-permission", "all"])["forgotten"],
        count
    );
}

/// On an output narrower than the prompt, the prompt and its buttons stay
/// on it.
#[test]
fn the_prompt_fits_a_narrow_output() {
    let mut pila = TestPila::start_with(Options {
        config: "[permissions]\nenforce = true\n".into(),
        outputs: Some("NARROW:480x800".into()),
        ..Default::default()
    });
    let client = pila.client();
    pila.settle();
    let _session = start_ext_session(&client, "NARROW", "s");
    let state = wait_armed(&mut pila);
    let shown = &state["capture_permission"]["shown"];
    for part in ["panel", "allow", "deny", "remember_rect"] {
        let (x, y, w, h) = common::rect(&shown[part]);
        assert!(
            x >= 0 && y >= 0 && x + w <= 480 && y + h <= 800,
            "{part} off the output: {:?}",
            (x, y, w, h)
        );
    }
    let (ax, ay) = center(&shown["allow"]);
    click_at(&mut pila, ax, ay);
    client.wait_logged("s", "Done");
}

/// A program allowed by a rule (a locker capturing the screen for its
/// background) still captures while locked; one that would be asked is
/// refused without a prompt (`locking_screen_cancels_open_permission_prompt`).
#[test]
fn an_allowed_program_captures_while_locked() {
    let exe = current_exe();
    let mut pila = TestPila::with_config(&format!(
        "[permissions]\nenforce = true\n\
         [[permission]]\nexecutable = {exe:?}\naccess = \"allow\"\n"
    ));
    let locker = pila.client();
    let lock = locker.lock_session();
    let surf = locker.lock_surface(&lock, "HEADLESS-1", [255, 10, 20, 30]);
    locker.wait_surface_configured(surf);
    locker.wait_logged("lock", "Locked");
    let client = pila.client();
    let _session = start_ext_session(&client, "HEADLESS-1", "locked");
    client.wait_logged("locked", "Done");
    let (_frame, _file) = start_screencopy(&client, "HEADLESS-1", "sc");
    client.wait_logged("sc", "Ready");
    assert!(pila.state()["capture_permission"]["shown"].is_null());
}

/// A program allowed once may capture the screen and any window: a window
/// capture's prompt says so, as a screen capture's does.
#[test]
fn a_window_capture_asks_about_the_screen_and_windows() {
    use wayland_protocols::ext::{
        foreign_toplevel_list::v1::client::{
            ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
            ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1,
        },
        image_capture_source::v1::client::ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1,
    };
    let mut pila = TestPila::with_config("[permissions]\nenforce = true\n");
    let client = pila.client();
    client.window("shared").color(RED).create();
    pila.wait_focus("shared");
    pila.settle();
    let _list: ExtForeignToplevelListV1 = client.bind("ext_foreign_toplevel_list_v1", 1);
    let text = "AppId { app_id: \"shared\" }";
    client.wait_logged("ext_toplevel", text);
    let handle: ExtForeignToplevelHandleV1 = client.object("ext_toplevel", text);
    let sources: ExtForeignToplevelImageCaptureSourceManagerV1 =
        client.bind("ext_foreign_toplevel_image_capture_source_manager_v1", 1);
    let source = sources.create_source(&handle, &client.qh, "source".into());
    let manager: ExtImageCopyCaptureManagerV1 = client.bind("ext_image_copy_capture_manager_v1", 1);
    let _window = manager.create_session(&source, CaptureOptions::empty(), &client.qh, "w".into());
    client.flush();
    let asks = |s: &Value| {
        s["capture_permission"]["shown"]["title"]
            .as_str()
            .is_some_and(|title| title.ends_with("to capture your screen and windows?"))
    };
    pila.wait("asked about the screen and windows", asks);
    let _screen = start_ext_session(&client, "HEADLESS-1", "screen");
    pila.wait("the same prompt", |s| {
        asks(s) && s["capture_permission"]["shown"]["requests"] == 2
    });
    wait_armed(&mut pila);
    pila.key("Return");
    client.wait_logged("w", "Done");
    client.wait_logged("screen", "Done");
}

/// A `deny` rule loaded while a screencopy waits for damage fails it.
#[test]
fn a_deny_rule_reloaded_fails_a_waiting_screencopy() {
    let exe = current_exe();
    let mut pila = TestPila::with_config(&format!(
        "[permissions]\nenforce = true\n\
         [[permission]]\nexecutable = {exe:?}\naccess = \"allow\"\n"
    ));
    let client = pila.client();
    pila.settle();
    let manager: ZwlrScreencopyManagerV1 = client.bind("zwlr_screencopy_manager_v1", 3);
    let output = client.output("HEADLESS-1");
    let first = manager.capture_output(0, &output, &client.qh, "first".into());
    client.flush();
    client.wait_logged("first", "BufferDone");
    let (buffer, _) = client.capture_buffer((1280, 800), Format::Xrgb8888);
    first.copy_with_damage(&buffer);
    client.flush();
    client.wait_logged("first", "Ready");
    // Nothing changes: the next one waits.
    let second = manager.capture_output(0, &output, &client.qh, "second".into());
    client.flush();
    client.wait_logged("second", "BufferDone");
    let (buffer, _) = client.capture_buffer((1280, 800), Format::Xrgb8888);
    second.copy_with_damage(&buffer);
    client.flush();
    pila.settle();
    client.roundtrip();
    assert!(!client.has_logged("second", "Ready"));
    pila.write_config(&format!(
        "[permissions]\nenforce = true\n\
         [[permission]]\nexecutable = {exe:?}\naccess = \"deny\"\n"
    ));
    client.wait_logged("second", "Failed");
}

/// Turning `enforce` on with a reload keeps a capture a rule allows going:
/// one started while `enforce` was off never had its executable read.
#[test]
fn enforce_turned_on_keeps_an_allowed_capture() {
    let exe = current_exe();
    let mut pila = TestPila::with_config("[permissions]\nenforce = false\n");
    let client = pila.client();
    pila.settle();
    let _session = start_ext_session(&client, "HEADLESS-1", "running");
    client.wait_logged("running", "Done");
    pila.write_config(&format!(
        "[permissions]\nenforce = true\n\
         [[permission]]\nexecutable = {exe:?}\naccess = \"allow\"\n"
    ));
    pila.wait("reloaded", |s| s["capture_permission"]["enforce"] == true);
    // The executable is read on a thread.
    pila.stays("no prompt", Duration::from_millis(300), |s| {
        s["capture_permission"]["shown"].is_null()
    });
    pila.settle();
    client.roundtrip();
    assert!(!client.has_logged("running", "Stopped"));
}

/// Turning `enforce` on with a reload stops a capture a rule denies, once
/// its executable is read.
#[test]
fn enforce_turned_on_stops_a_denied_capture() {
    let exe = current_exe();
    let mut pila = TestPila::with_config("[permissions]\nenforce = false\n");
    let client = pila.client();
    pila.settle();
    let _session = start_ext_session(&client, "HEADLESS-1", "running");
    client.wait_logged("running", "Done");
    pila.write_config(&format!(
        "[permissions]\nenforce = true\n\
         [[permission]]\nexecutable = {exe:?}\naccess = \"deny\"\n"
    ));
    client.wait_logged("running", "Stopped");
}

/// A state directory whose `capture-permissions.json` is a FIFO: Pila's
/// startup read of it waits until the test writes it (`write_held_store`).
fn held_store() -> (std::path::PathBuf, Vec<(String, String)>) {
    let state_dir = std::env::temp_dir().join(format!(
        "pila-perm-held-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let dir = state_dir.join("pila");
    std::fs::create_dir_all(&dir).unwrap();
    let fifo = dir.join("capture-permissions.json");
    let path = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0, "mkfifo");
    let env = vec![(
        "XDG_STATE_HOME".to_string(),
        state_dir.to_string_lossy().into_owned(),
    )];
    (fifo, env)
}

/// Let Pila's read of a `held_store` finish, with `grants` in it.
fn write_held_store(fifo: &std::path::Path, grants: &str) {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().write(true).open(fifo).unwrap();
    write!(file, "{{\"version\": 1, \"grants\": {grants}}}").unwrap();
}

/// Locking leaves alone a request of an allowed program still waiting for
/// the saved permissions to be read: it's decided once they are, and a rule
/// allows it.
#[test]
fn locking_keeps_an_allowed_request_waiting_for_the_saved_permissions() {
    let exe = current_exe();
    let (fifo, env) = held_store();
    let mut pila = TestPila::start_with(Options {
        config: format!(
            "[permissions]\nenforce = true\n\
             [[permission]]\nexecutable = {exe:?}\naccess = \"allow\"\n"
        ),
        env,
        ..Default::default()
    });
    let client = pila.client();
    pila.settle();
    let _session = start_ext_session(&client, "HEADLESS-1", "held");
    pila.settle();
    client.roundtrip();
    assert_eq!(pila.state()["capture_permission"]["loaded"], false);
    let locker = pila.client();
    let lock = locker.lock_session();
    let surf = locker.lock_surface(&lock, "HEADLESS-1", [255, 10, 20, 30]);
    locker.wait_surface_configured(surf);
    locker.wait_logged("lock", "Locked");
    write_held_store(&fifo, "[]");
    pila.wait("loaded", |s| s["capture_permission"]["loaded"] == true);
    client.wait_logged("held", "Done");
    assert!(!client.has_logged("held", "Stopped"));
}

/// A program can't answer its own prompt with a virtual keyboard: Enter and
/// R from one do nothing, and only the user's keys (or clicks) answer.
#[test]
fn a_virtual_keyboard_cant_answer_the_prompt() {
    use common::protocols::{keymap_of, virtual_keyboard};
    let mut pila = TestPila::with_config("[permissions]\nenforce = true\n");
    let client = pila.client();
    client.window("app").create();
    pila.wait_focus("app");
    pila.settle();
    let _session = start_ext_session(&client, "HEADLESS-1", "typed");
    wait_armed(&mut pila);
    let keyboard = virtual_keyboard(&client, &keymap_of(&client));
    // R (19), then Enter (28).
    for (time, key, state) in [(1, 19, 1), (2, 19, 0), (3, 28, 1), (4, 28, 0)] {
        keyboard.key(time, key, state);
    }
    client.flush();
    pila.stays("still asking", Duration::from_millis(300), |s| {
        s["capture_permission"]["shown"]["remember"] == false
    });
    client.roundtrip();
    assert!(!client.has_logged("typed", "Done"));
    pila.key("Escape");
    client.wait_logged("typed", "Stopped");
}

/// The prompt shown on a monitor unplugged moves to another, and Remember
/// toggled there is drawn checked.
#[test]
fn remember_toggled_after_the_prompts_monitor_is_unplugged_shows() {
    let mut pila = TestPila::start_with(Options {
        config: "[permissions]\nenforce = true\n".into(),
        outputs: Some("A:640x480,B:640x480".into()),
        ..Default::default()
    });
    let client = pila.client();
    pila.settle();
    pila.move_to(900.0, 200.0);
    let _session = start_ext_session(&client, "A", "moved");
    pila.wait("asked on B", |s| {
        s["capture_permission"]["shown"]["output"] == "B"
            && s["capture_permission"]["shown"]["text"] == true
    });
    pila.request(json!({ "cmd": "output_remove", "name": "B" }));
    pila.wait("asked on A", |s| {
        s["capture_permission"]["shown"]["output"] == "A"
    });
    let state = pila.settle();
    // The checkbox: the start of the Remember row.
    let (x, y, _, h) = rect(&state["capture_permission"]["shown"]["remember_rect"]);
    let (bx, by) = ((x + 9) as f64, (y + h / 2) as f64);
    let unchecked = pila.pixel(bx, by);
    pila.key("r");
    pila.wait_pixel_is(bx, by, "the box checked", |p| p != unchecked);
    pila.key("Escape");
    client.wait_logged("moved", "Stopped");
}
