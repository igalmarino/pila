//! Outputs: plugging and unplugging, where windows go, wlr-output-management
//! (heads, modes, applying and testing configurations), output power and
//! gamma.

mod common;

use common::*;
use wayland_client::Proxy;
use wayland_protocols_wlr::{
    gamma_control::v1::client::zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1,
    output_management::v1::client::{
        zwlr_output_configuration_v1::ZwlrOutputConfigurationV1,
        zwlr_output_head_v1::ZwlrOutputHeadV1, zwlr_output_manager_v1::ZwlrOutputManagerV1,
        zwlr_output_mode_v1::ZwlrOutputModeV1,
    },
    output_power_management::v1::client::{
        zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1, zwlr_output_power_v1::Mode,
    },
};

const BAR: i64 = 28;

fn two_outputs() -> TestPila {
    TestPila::start_with(Options {
        outputs: Some("A:1280x800,B:1024x768".into()),
        ..Options::default()
    })
}

// --- Plugging and unplugging --------------------------------------------------

#[test]
fn a_new_output_goes_right_of_the_others_and_clients_see_it() {
    let mut pila = TestPila::start();
    let client = pila.client();
    pila.request(json!({ "cmd": "output_add", "spec": "NEW:800x600@75" }));
    let state = pila.wait("two outputs", |s| {
        s["outputs"].as_array().unwrap().len() == 2
    });
    let new = &state["outputs"][1];
    assert_eq!(rect(&new["geometry"]), (1280, 0, 800, 600));
    assert_eq!(new["mode"]["refresh_mhz"], 75000);
    client.wait("a wl_output global", |s| {
        s.globals
            .iter()
            .filter(|(_, i, _)| i == "wl_output")
            .count()
            == 2
    });
}

#[test]
fn unplugging_an_output_moves_its_windows_and_the_pointer() {
    let mut pila = two_outputs();
    pila.move_to(1500.0, 300.0);
    let client = pila.client();
    client.window("b").size(400, 300).create();
    let w = pila.wait_window("b");
    assert_eq!(w["output"], "B");
    pila.request(json!({ "cmd": "output_remove", "name": "B" }));
    let state = pila.wait("b moved to A", |s| {
        window_in(s, "b").unwrap()["output"] == "A"
    });
    // Centered on A (where the pointer went).
    assert_eq!(
        rect(&window_in(&state, "b").unwrap()["geometry"]),
        (440, 264, 400, 300)
    );
    let px = state["pointer"]["x"].as_f64().unwrap();
    assert!(px < 1280.0, "the pointer is still off every output: {px}");
}

#[test]
fn binding_an_output_as_it_goes_away_doesnt_kill_the_client() {
    use wayland_client::protocol::{wl_output::WlOutput, wl_registry::WlRegistry};
    let mut pila = two_outputs();
    let client = pila.client();
    let (registry, name): (WlRegistry, u32) = {
        let shared = client.lock();
        let name = shared
            .globals
            .iter()
            .rev()
            .find(|(_, interface, _)| interface == "wl_output")
            .map(|(name, _, _)| *name)
            .unwrap();
        (shared.registry.clone().unwrap(), name)
    };
    pila.request(json!({ "cmd": "output_remove", "name": "B" }));
    // Its `global_remove` was on its way when the client asked for this.
    let _: WlOutput = registry.bind(name, 4, &client.qh, ());
    client.flush();
    client.roundtrip();
    assert_eq!(client.error(), None);
}

#[test]
fn a_maximized_window_on_an_unplugged_output_refits_and_restores_onto_another() {
    let mut pila = two_outputs();
    pila.move_to(1500.0, 300.0);
    let client = pila.client();
    client.window("b").size(400, 300).create();
    pila.wait_focus("b");
    pila.combo("Super+m");
    pila.wait("maximized on B", |s| {
        rect(&window_in(s, "b").unwrap()["geometry"]) == (1280, BAR, 1024, 768 - BAR)
    });
    pila.request(json!({ "cmd": "output_remove", "name": "B" }));
    pila.wait("maximized on A", |s| {
        rect(&window_in(s, "b").unwrap()["geometry"]) == (0, BAR, 1280, 800 - BAR)
    });
    pila.combo("Super+m");
    let state = pila.wait("restored", |s| {
        window_in(s, "b").unwrap()["geometry"]["w"] == 400
    });
    let (x, y, w, h) = rect(&window_in(&state, "b").unwrap()["geometry"]);
    assert!(
        x >= 0 && x + w <= 1280 && y >= BAR && y + h <= 800,
        "{x},{y} {w}x{h}"
    );
}

#[test]
fn a_window_on_another_workspace_comes_back_onto_a_remaining_output() {
    let mut pila = two_outputs();
    pila.move_to(1500.0, 300.0);
    let client = pila.client();
    client.window("b").size(400, 300).create();
    pila.wait_focus("b");
    pila.combo("Super+Shift+2");
    pila.wait("b on 2", |s| window_in(s, "b").unwrap()["workspace"] == 2);
    pila.request(json!({ "cmd": "output_remove", "name": "B" }));
    pila.combo("Super+2");
    let state = pila.wait_focus("b");
    assert_eq!(window_in(&state, "b").unwrap()["output"], "A");
}

#[test]
fn a_window_belongs_to_the_output_holding_most_of_it() {
    let mut pila = two_outputs();
    let client = pila.client();
    client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    // Moved right in 40 px steps until most of it is on B.
    let (x, ..) = rect(&pila.window("a")["geometry"]);
    let steps = (1280 - x - 400 / 2 + 40) / 40;
    for _ in 0..steps {
        pila.combo("Super+Shift+Right");
    }
    let state = pila.wait("moved", |s| {
        window_in(s, "a").unwrap()["geometry"]["x"] == x + 40 * steps
    });
    assert_eq!(window_in(&state, "a").unwrap()["output"], "B");
}

#[test]
fn unplugging_the_last_output_and_plugging_one_back_works() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").create();
    pila.wait_focus("a");
    pila.request(json!({ "cmd": "output_remove", "name": "HEADLESS-1" }));
    pila.wait("no output", |s| s["outputs"].as_array().unwrap().is_empty());
    pila.request(json!({ "cmd": "output_add", "spec": "OTHER:1024x768" }));
    let state = pila.wait("a back on an output", |s| {
        window_in(s, "a").unwrap()["output"] == "OTHER"
    });
    assert!(pila.alive());
    let _ = state;
}

// --- wlr-output-management ------------------------------------------------------

fn last_serial(client: &Client) -> u32 {
    serial_in(&client.lock()).expect("a done event")
}

/// The serial of the last `done` (usable inside `Client::wait`, which
/// holds the client's lock).
fn serial_in(s: &common::client::Shared) -> Option<u32> {
    s.logged("zwlr_output_manager_v1")
        .iter()
        .rev()
        .find_map(|e| {
            e.strip_prefix("Done { serial: ")
                .and_then(|r| r.trim_end_matches(" }").parse().ok())
        })
}

fn head(client: &Client, name: &str) -> ZwlrOutputHeadV1 {
    client.object("head", &format!("Name {{ name: \"{name}\" }}"))
}

/// The mode of `head` with this size.
fn mode(client: &Client, head: &ZwlrOutputHeadV1, size: (i32, i32)) -> ZwlrOutputModeV1 {
    let ids: Vec<u32> = client
        .events_of(head)
        .iter()
        .filter(|e| e.starts_with("Mode "))
        .filter_map(|e| {
            let at = e.find("zwlr_output_mode_v1@")? + "zwlr_output_mode_v1@".len();
            e[at..]
                .split(|c: char| !c.is_ascii_digit())
                .next()?
                .parse()
                .ok()
        })
        .collect();
    let text = format!("Size {{ width: {}, height: {} }}", size.0, size.1);
    client
        .objects::<ZwlrOutputModeV1>("mode", &text)
        .into_iter()
        .find(|m| ids.contains(&m.id().protocol_id()))
        .unwrap_or_else(|| panic!("no mode {size:?}"))
}

fn manager(pila: &TestPila) -> (Client, ZwlrOutputManagerV1) {
    let client = pila.client();
    let manager: ZwlrOutputManagerV1 = client.bind("zwlr_output_manager_v1", 4);
    client.wait_logged("zwlr_output_manager_v1", "Done");
    (client, manager)
}

fn configuration(client: &Client, manager: &ZwlrOutputManagerV1) -> ZwlrOutputConfigurationV1 {
    manager.create_configuration(last_serial(client), &client.qh, "config".into())
}

#[test]
fn heads_describe_the_outputs() {
    let pila = two_outputs();
    let (client, _manager) = manager(&pila);
    let b = head(&client, "B");
    let events = client.events_of(&b);
    assert!(
        events.iter().any(|e| e == "Enabled { enabled: 1 }"),
        "{events:?}"
    );
    assert!(
        events.iter().any(|e| e == "Position { x: 1280, y: 0 }"),
        "{events:?}"
    );
    assert!(
        events.iter().any(|e| e == "Scale { scale: 1.0 }"),
        "{events:?}"
    );
    // Its own mode (1024x768) and the extra one it doesn't already have.
    assert_eq!(events.iter().filter(|e| e.starts_with("Mode ")).count(), 2);
    let a = head(&client, "A");
    assert_eq!(
        client
            .events_of(&a)
            .iter()
            .filter(|e| e.starts_with("Mode "))
            .count(),
        3
    );
    mode(&client, &b, (1024, 768));
    mode(&client, &b, (1920, 1080));
}

#[test]
fn applying_a_configuration_changes_the_outputs() {
    let mut pila = two_outputs();
    let (client, manager) = manager(&pila);
    let b = head(&client, "B");
    let big = mode(&client, &b, (1920, 1080));
    let a = head(&client, "A");
    let config = configuration(&client, &manager);
    let a_config = config.enable_head(&a, &client.qh, "config_head".into());
    a_config.set_scale(2.0);
    let b_config = config.enable_head(&b, &client.qh, "config_head".into());
    b_config.set_mode(&big);
    b_config.set_position(640, 0);
    config.apply();
    client.flush();
    client.wait_logged("config", "Succeeded");
    let state = pila.wait("the new layout", |s| s["outputs"][1]["mode"]["w"] == 1920);
    assert_eq!(rect(&state["outputs"][0]["geometry"]), (0, 0, 640, 400));
    assert_eq!(rect(&state["outputs"][1]["geometry"]), (640, 0, 1920, 1080));
    // Listeners hear about it, with a new serial.
    client.wait("a new done", |s| {
        s.logged("zwlr_output_manager_v1")
            .iter()
            .filter(|e| e.starts_with("Done"))
            .count()
            >= 2
    });
    assert!(client
        .events_of(&a)
        .iter()
        .any(|e| e == "Scale { scale: 2.0 }"));
}

#[test]
fn testing_a_configuration_changes_nothing() {
    let mut pila = two_outputs();
    let (client, manager) = manager(&pila);
    let b = head(&client, "B");
    let config = configuration(&client, &manager);
    let b_config = config.enable_head(&b, &client.qh, "config_head".into());
    b_config.set_scale(2.0);
    config.enable_head(&head(&client, "A"), &client.qh, "config_head".into());
    config.test();
    client.flush();
    client.wait_logged("config", "Succeeded");
    let state = pila.settle();
    assert_eq!(state["outputs"][1]["scale"], 1.0);
}

#[test]
fn bad_configurations_fail() {
    let pila = two_outputs();
    let (client, manager) = manager(&pila);
    // Every head off.
    let config = configuration(&client, &manager);
    config.disable_head(&head(&client, "A"));
    config.disable_head(&head(&client, "B"));
    config.apply();
    client.flush();
    client.wait_logged("config", "Failed");
    // A scale out of range.
    let config = manager.create_configuration(last_serial(&client), &client.qh, "config2".into());
    let a = config.enable_head(&head(&client, "A"), &client.qh, "config_head".into());
    a.set_scale(20.0);
    config.enable_head(&head(&client, "B"), &client.qh, "config_head".into());
    config.apply();
    client.flush();
    client.wait_logged("config2", "Failed");
    // A custom mode the output doesn't have.
    let config = manager.create_configuration(last_serial(&client), &client.qh, "config3".into());
    let a = config.enable_head(&head(&client, "A"), &client.qh, "config_head".into());
    a.set_custom_mode(333, 222, 0);
    config.enable_head(&head(&client, "B"), &client.qh, "config_head".into());
    config.apply();
    client.flush();
    client.wait_logged("config3", "Failed");
}

#[test]
fn a_configuration_the_backend_cant_apply_fails_and_changes_nothing() {
    let mut pila = two_outputs();
    let (client, manager) = manager(&pila);
    pila.request(json!({ "cmd": "output_fail_next" }));
    let apply = |tag: &str| {
        let config = manager.create_configuration(last_serial(&client), &client.qh, tag.into());
        let b = config.enable_head(&head(&client, "B"), &client.qh, "config_head".into());
        b.set_scale(2.0);
        config.enable_head(&head(&client, "A"), &client.qh, "config_head".into());
        config.apply();
        client.flush();
    };
    apply("config");
    client.wait_logged("config", "Failed");
    assert!(!client.has_logged("config", "Succeeded"));
    let state = pila.settle();
    assert_eq!(state["outputs"][1]["scale"], 1.0);
    // What failed isn't kept for later: the next one starts clean, and works.
    apply("config2");
    client.wait_logged("config2", "Succeeded");
    pila.wait("the scale changed", |s| s["outputs"][1]["scale"] == 2.0);
}

#[test]
fn a_configuration_for_an_old_serial_is_cancelled() {
    let mut pila = two_outputs();
    let (client, manager) = manager(&pila);
    let old = last_serial(&client);
    pila.request(json!({ "cmd": "output_add", "spec": "C:800x600" }));
    client.wait("a new serial", |s| serial_in(s) != Some(old));
    let config = manager.create_configuration(old, &client.qh, "config".into());
    config.enable_head(&head(&client, "A"), &client.qh, "config_head".into());
    config.apply();
    client.flush();
    client.wait_logged("config", "Cancelled");
}

#[test]
fn disabling_a_head_removes_the_output_but_keeps_the_head() {
    let mut pila = two_outputs();
    pila.move_to(1500.0, 300.0);
    let app = pila.client();
    app.window("b").create();
    pila.wait_window("b");
    let (client, manager) = manager(&pila);
    let b = head(&client, "B");
    let config = configuration(&client, &manager);
    config.enable_head(&head(&client, "A"), &client.qh, "config_head".into());
    config.disable_head(&b);
    config.apply();
    client.flush();
    client.wait_logged("config", "Succeeded");
    let state = pila.wait("B off", |s| s["outputs"][1]["enabled"] == false);
    assert_eq!(window_in(&state, "b").unwrap()["output"], "A");
    client.wait("B listed as off", |s| {
        s.log
            .iter()
            .any(|l| l.object == b.id() && l.event == "Enabled { enabled: 0 }")
    });
    // And back on.
    let config = manager.create_configuration(last_serial(&client), &client.qh, "config2".into());
    config.enable_head(&head(&client, "A"), &client.qh, "config_head".into());
    config.enable_head(&b, &client.qh, "config_head".into());
    config.apply();
    client.flush();
    client.wait_logged("config2", "Succeeded");
    pila.wait("B on", |s| s["outputs"][1]["enabled"] == true);
}

#[test]
fn editing_an_outputs_config_section_drops_its_override() {
    let mut pila = two_outputs();
    let (client, manager) = manager(&pila);
    let config = configuration(&client, &manager);
    let b = config.enable_head(&head(&client, "B"), &client.qh, "config_head".into());
    b.set_scale(2.0);
    config.enable_head(&head(&client, "A"), &client.qh, "config_head".into());
    config.apply();
    client.flush();
    client.wait_logged("config", "Succeeded");
    pila.wait("B at 2", |s| s["outputs"][1]["scale"] == 2.0);
    // An unrelated edit keeps it.
    pila.write_config("workspaces = 3\n");
    pila.wait_log("config reloaded");
    assert_eq!(pila.settle()["outputs"][1]["scale"], 2.0);
    // B's section edited: the file takes over again.
    pila.write_config("workspaces = 3\n\n[[output]]\nname = \"B\"\nscale = 1.5\n");
    pila.wait("B at 1.5", |s| s["outputs"][1]["scale"] == 1.5);
}

// --- Monitors (`edid.rs`) ---------------------------------------------------------

const MONITOR: &str = "=LEN/P27h-30/ABC";

fn scale_of(state: &Value, output: &str) -> f64 {
    let outputs = state["outputs"].as_array().unwrap();
    let found = outputs.iter().find(|o| o["name"] == output).unwrap();
    found["scale"].as_f64().unwrap()
}

fn with_monitor_on(connector: &str, config: &str) -> TestPila {
    let outputs = format!("A:1280x800,{connector}:1024x768{MONITOR}");
    let mut pila = TestPila::start_with(Options {
        outputs: Some(outputs),
        ..Options::default()
    });
    pila.write_config(config);
    pila.wait_log("config reloaded");
    pila
}

#[test]
fn a_section_naming_a_model_follows_the_monitor_to_any_connector() {
    for connector in ["B", "C"] {
        let mut pila = with_monitor_on(
            connector,
            "[[output]]\nname = \"LEN P27h-30\"\nscale = 2.0\n",
        );
        let state = pila.wait("the monitor at 2", |s| scale_of(s, connector) == 2.0);
        assert_eq!(scale_of(&state, "A"), 1.0);
    }
}

#[test]
fn a_section_naming_the_monitor_itself_wins_over_its_model_and_connector() {
    let mut pila = with_monitor_on(
        "B",
        "[[output]]\nname = \"B\"\nscale = 1.25\n\
         [[output]]\nname = \"LEN P27h-30\"\nscale = 1.5\n\
         [[output]]\nname = \"LEN P27h-30 ABC\"\nscale = 2.0\n",
    );
    pila.wait("the monitor's section", |s| scale_of(s, "B") == 2.0);
}

#[test]
fn a_section_for_another_monitor_doesnt_apply() {
    let mut pila = with_monitor_on(
        "B",
        "[[output]]\nname = \"LEN Other\"\nscale = 2.0\n\
         [[output]]\nname = \"LEN P27h-30 XYZ\"\nscale = 2.0\n",
    );
    assert_eq!(scale_of(&pila.settle(), "B"), 1.0);
}

#[test]
fn a_monitor_going_away_takes_its_section_with_it() {
    let mut pila = with_monitor_on("B", "[[output]]\nname = \"LEN P27h-30\"\nscale = 2.0\n");
    pila.wait("the monitor at 2", |s| scale_of(s, "B") == 2.0);
    pila.request(json!({ "cmd": "output_remove", "name": "B" }));
    pila.wait("gone", |s| s["outputs"].as_array().unwrap().len() == 1);
    pila.request(json!({ "cmd": "output_add", "spec": "B:1024x768=DEL/U2720Q" }));
    let state = pila.wait("another monitor", |s| {
        s["outputs"].as_array().unwrap().len() == 2
    });
    assert_eq!(scale_of(&state, "B"), 1.0);
}

#[test]
fn a_tools_setting_stays_until_the_monitors_section_is_edited() {
    let mut pila = with_monitor_on("B", "[[output]]\nname = \"LEN P27h-30\"\nscale = 2.0\n");
    pila.wait("the monitor at 2", |s| scale_of(s, "B") == 2.0);
    let (client, manager) = manager(&pila);
    let configuration = configuration(&client, &manager);
    let setup = configuration.enable_head(&head(&client, "B"), &client.qh, "config_head".into());
    setup.set_scale(1.5);
    configuration.enable_head(&head(&client, "A"), &client.qh, "config_head".into());
    configuration.apply();
    client.flush();
    client.wait_logged("config", "Succeeded");
    pila.wait("B at 1.5", |s| scale_of(s, "B") == 1.5);
    // Its section edited: the file takes over again.
    pila.write_config("[[output]]\nname = \"LEN P27h-30\"\nscale = 1.25\n");
    pila.wait("B at 1.25", |s| scale_of(s, "B") == 1.25);
}

#[test]
fn clients_get_the_monitors_make_model_and_serial() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some(format!("A:1280x800,B:1024x768{MONITOR}")),
        ..Options::default()
    });
    let state = pila.state();
    let outputs = state["outputs"].as_array().unwrap();
    assert_eq!(outputs[1]["make"], "LEN");
    assert_eq!(outputs[1]["model"], "P27h-30");
    assert_eq!(outputs[0]["make"], "pila");
    let (client, _manager) = manager(&pila);
    client.wait_logged("head", "Make { make: \"LEN\" }");
    client.wait_logged("head", "Model { model: \"P27h-30\" }");
    client.wait_logged("head", "SerialNumber { serial_number: \"ABC\" }");
    client.wait_logged("head", "Make { make: \"pila\" }");
}

// --- Power and gamma -------------------------------------------------------------

#[test]
fn an_output_can_be_turned_off_and_on() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let manager: ZwlrOutputPowerManagerV1 = client.bind("zwlr_output_power_manager_v1", 1);
    let power = manager.get_output_power(&client.output("HEADLESS-1"), &client.qh, "power".into());
    client.flush();
    client.wait_logged("power", "Mode { mode: Value(On) }");
    power.set_mode(Mode::Off);
    client.flush();
    pila.wait("off", |s| s["outputs"][0]["power"] == false);
    client.wait_logged("power", "Mode { mode: Value(Off) }");
    power.set_mode(Mode::On);
    client.flush();
    pila.wait("on", |s| s["outputs"][0]["power"] == true);
}

#[test]
fn a_lock_is_confirmed_with_an_output_turned_off() {
    let mut pila = two_outputs();
    let client = pila.client();
    let manager: ZwlrOutputPowerManagerV1 = client.bind("zwlr_output_power_manager_v1", 1);
    let power = manager.get_output_power(&client.output("B"), &client.qh, "power".into());
    power.set_mode(Mode::Off);
    client.flush();
    pila.wait("B off", |s| s["outputs"][1]["power"] == false);
    let lock = client.lock_session();
    client.lock_surface(&lock, "A", [255, 0, 0, 0]);
    client.wait_logged("lock", "Locked");
}

#[test]
fn gamma_ramps_are_set_and_reset() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let manager: ZwlrGammaControlManagerV1 = client.bind("zwlr_gamma_control_manager_v1", 1);
    let control =
        manager.get_gamma_control(&client.output("HEADLESS-1"), &client.qh, "gamma".into());
    client.flush();
    client.wait_logged("gamma", "GammaSize { size: 256 }");
    let ramps = memfd(&vec![0x40u8; 256 * 3 * 2]);
    use std::os::fd::AsFd;
    control.set_gamma(ramps.as_fd());
    client.flush();
    pila.wait("gamma set", |s| s["outputs"][0]["gamma_set"] == true);
    control.destroy();
    client.flush();
    pila.wait("gamma reset", |s| s["outputs"][0]["gamma_set"] == false);
}

#[test]
fn a_second_gamma_control_or_a_short_ramp_fails() {
    let pila = TestPila::start();
    let client = pila.client();
    let manager: ZwlrGammaControlManagerV1 = client.bind("zwlr_gamma_control_manager_v1", 1);
    let control =
        manager.get_gamma_control(&client.output("HEADLESS-1"), &client.qh, "gamma".into());
    client.flush();
    client.wait_logged("gamma", "GammaSize");
    let _second =
        manager.get_gamma_control(&client.output("HEADLESS-1"), &client.qh, "gamma2".into());
    client.flush();
    client.wait_logged("gamma2", "Failed");
    // A ramp of the wrong size is a protocol error (invalid_gamma).
    let short = memfd(&[0u8; 10]);
    use std::os::fd::AsFd;
    client.expect_error();
    control.set_gamma(short.as_fd());
    client.flush();
    let error = client.wait_error();
    assert!(error.contains("zwlr_gamma_control_v1"), "{error}");
}

fn memfd(data: &[u8]) -> std::os::fd::OwnedFd {
    use std::io::{Seek, Write};
    use std::os::fd::FromRawFd;
    // SAFETY: a new fd, owned by the File.
    let fd = unsafe { libc::memfd_create(c"ramp".as_ptr(), libc::MFD_CLOEXEC) };
    assert!(fd >= 0);
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    file.write_all(data).unwrap();
    file.rewind().unwrap();
    file.into()
}
