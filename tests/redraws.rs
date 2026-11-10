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

/// A window's title bar is rasterized per output scale (`decoration.rs`):
/// drawing another output, of another scale, leaves this one's textures
/// alone. (It used to swap them for the other scale's, so the output the
/// window is on found new elements with no redraw asked for it.)
#[test]
fn drawing_an_output_of_another_scale_leaves_a_windows_title_bar_alone() {
    let mut pila = TestPila::start_with(Options {
        config: "[[output]]\nname = \"B\"\nscale = 2\n".into(),
        outputs: Some("A:1280x800,B:1280x800".into()),
        ..Options::default()
    });
    let client = pila.client();
    client.window("a").size(400, 300).create();
    assert_eq!(pila.wait_window("a")["output"], "A");
    pila.settle();
    // Redraws of B alone, with the window's title bar in the scene.
    for i in 0..20 {
        pila.move_to(1280.0 + 100.0 + f64::from(i), 100.0);
    }
    pila.stays(
        "pila alive, A untouched",
        std::time::Duration::from_millis(300),
        |s| s["locked"] == false,
    );
}

/// An animation redraws the outputs it shows on, frame after frame, not
/// every output: a window closing on one output, fading out, leaves the
/// other alone after the start. (Each step asked every output; one with
/// nothing new there, queuing no frame, was redrawn as fast as the loop
/// went, held back by nothing.)
#[test]
fn an_animation_on_one_output_doesnt_redraw_another_for_each_frame() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800@144,B:1024x768@144".into()),
        config: "[animations]\nenabled = true\n".into(),
        ..Options::default()
    });
    let client = pila.client();
    pila.move_to(600.0, 400.0);
    let mut window = Some(client.window("a").size(400, 300).create());
    assert_eq!(pila.wait_window("a")["output"], "A");
    let before = pila.settle();
    client.destroy(window.take().unwrap());
    let after = pila.wait("the close to be over", |s| {
        s["animations"]["running"] == false && window_in(s, "a").is_none()
    });
    let after = pila.wait("pila to settle", |s| {
        s["redraw_pending"] == false && redraws(s, "A").0 >= redraws(&after, "A").0
    });
    let (frames_a, _) = redraws(&after, "A");
    assert!(
        frames_a > redraws(&before, "A").0 + 5,
        "A drew the fade-out: {frames_a}"
    );
    let (frames_b, idle_b) = redraws(&after, "B");
    let (frames_b0, idle_b0) = redraws(&before, "B");
    assert!(
        (frames_b + idle_b) - (frames_b0 + idle_b0) <= 3,
        "B redrawn {} times during A's animation",
        (frames_b + idle_b) - (frames_b0 + idle_b0)
    );
}

/// A window dragged partly onto another output, the pointer staying on the
/// first: the other output follows the drag. (The move grab moved the window
/// without asking for its redraws; only the pointer's output was redrawn, and
/// the other showed the window wherever it was at its last redraw for
/// something else.)
#[test]
fn a_window_dragged_onto_another_output_is_redrawn_there() {
    const RED: [u8; 4] = [255, 255, 0, 0];
    let mut pila = two_fast_outputs();
    let client = pila.client();
    pila.move_to(1280.0 + 500.0, 400.0);
    client.window("a").size(400, 300).color(RED).create();
    let state = pila.wait_focus("a");
    let (x, y, ..) = rect(&window_in(&state, "a").unwrap()["geometry"]);
    assert!(x >= 1280, "the window opens on B");
    // Grabbed near its right edge, dragged left until 200 px of it are on A:
    // the pointer ends 150 px into B.
    pila.move_to((x + 350) as f64, (y + 150) as f64);
    pila.key_press("Super");
    pila.press("left");
    // Settled before and between the steps: the redraw of every output that
    // the press asks for would otherwise still be due, and draw the last
    // place.
    pila.settle();
    let distance = x - 1280 + 200;
    for _ in 0..distance / 20 {
        pila.move_by(-20.0, 0.0);
        pila.settle();
    }
    pila.wait("the window across both outputs", |s| {
        let (x, ..) = rect(&window_in(s, "a").unwrap()["geometry"]);
        x <= 1280 - 180
    });
    assert!(pila.state()["pointer"]["x"].as_f64().unwrap() > 1280.0);
    // On A, 100 px left of B, with nothing else happening.
    pila.wait_pixel(1180.0, (y + 150) as f64, [255, 0, 0, 255]);
    pila.release("left");
    pila.key_release("Super");
}

/// Nothing changes the `Space`'s windows but through `map_window`,
/// `unmap_window` and `raise_window` (`redraw.rs`), which ask for the redraws:
/// a direct `space.map_element` moves a window and redraws nothing. (One is
/// allowed: `resize_grab::handle_commit`, inside a commit, which asks for its
/// surface's outputs before and after.)
#[test]
fn the_spaces_windows_change_only_through_the_helpers_that_redraw() {
    fn sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&src, &mut files);
    let mut direct = Vec::new();
    for path in files {
        let name = path.strip_prefix(&src).unwrap().display().to_string();
        if name == "redraw.rs" {
            continue;
        }
        // Without whitespace: a call may be split over lines.
        let code: String = std::fs::read_to_string(&path)
            .unwrap()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        for call in [
            "space.map_element(",
            "space.unmap_elem(",
            "space.raise_element(",
        ] {
            for _ in 0..code.matches(call).count() {
                direct.push(format!("{name}: {call}"));
            }
        }
    }
    assert_eq!(
        direct,
        ["grabs/resize_grab.rs: space.map_element("],
        "use Pila::map_window, unmap_window or raise_window"
    );
}
