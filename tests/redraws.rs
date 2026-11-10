//! Redraws per output: a change redraws the outputs it shows on, and only
//! those. (Every test also runs with the harness's check that nothing changes
//! on an output without a redraw being asked for it: `PILA_HEADLESS_CHECK_REDRAWS`.)

mod common;

use common::*;

/// Two outputs side by side, B right of A, both faster than 60 Hz.
fn two_fast_outputs() -> TestPila {
    TestPila::start_with(Options {
        outputs: Some("A:1280x800@144,B:1024x768@144".into()),
        ..Options::default()
    })
}

/// An output's frames drawn and redraws with nothing new.
fn redraws(state: &Value, name: &str) -> (u64, u64) {
    let output = state["outputs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["name"] == name)
        .unwrap_or_else(|| panic!("no output {name}"));
    (
        output["frames"].as_u64().unwrap(),
        output["idle"].as_u64().unwrap(),
    )
}

#[test]
fn moving_the_pointer_on_one_output_doesnt_redraw_another() {
    let mut pila = two_fast_outputs();
    let client = pila.client();
    pila.move_to(1280.0 + 500.0, 400.0);
    client.window("b").size(400, 300).create();
    assert_eq!(pila.wait_window("b")["output"], "B");
    pila.move_to(600.0, 400.0);
    let before = pila.settle();
    for i in 0..40 {
        pila.move_by(if i % 2 == 0 { 3.0 } else { -3.0 }, 0.0);
    }
    let after = pila.settle();
    assert_eq!(
        redraws(&after, "B"),
        redraws(&before, "B"),
        "B was redrawn for the pointer moving on A"
    );
}

#[test]
fn a_clients_frame_redraws_only_the_output_it_shows_on() {
    let mut pila = two_fast_outputs();
    let client = pila.client();
    pila.move_to(1280.0 + 500.0, 400.0);
    let window = client.window("b").size(400, 300).create();
    assert_eq!(pila.wait_window("b")["output"], "B");
    let before = pila.settle();
    for _ in 0..10 {
        client.draw(window, None);
        client.roundtrip();
    }
    let after = pila.wait("B to draw the client's frames", |s| {
        redraws(s, "B").0 > redraws(&before, "B").0
    });
    let after = pila.wait("pila to settle", |s| {
        s["redraw_pending"] == false && redraws(s, "B").0 >= redraws(&after, "B").0
    });
    assert_eq!(
        redraws(&after, "A"),
        redraws(&before, "A"),
        "A was redrawn for a client's frames on B"
    );
}

#[test]
fn a_window_across_two_outputs_redraws_both() {
    let mut pila = two_fast_outputs();
    let client = pila.client();
    let window = client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    // Moved right in 40 px steps until it straddles A's right edge.
    let (x, ..) = rect(&pila.window("a")["geometry"]);
    let steps = (1280 - x - 400 / 2) / 40;
    for _ in 0..steps {
        pila.combo("Super+Shift+Right");
    }
    pila.wait("the window across both outputs", |s| {
        let (x, _, w, _) = rect(&window_in(s, "a").unwrap()["geometry"]);
        x < 1280 && x + w > 1280
    });
    let before = pila.settle();
    client.draw(window, None);
    pila.wait("both outputs to draw the client's frame", |s| {
        redraws(s, "A").0 > redraws(&before, "A").0 && redraws(s, "B").0 > redraws(&before, "B").0
    });
}
