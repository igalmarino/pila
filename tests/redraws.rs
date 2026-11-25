//! Redraws per output: a change redraws the outputs it shows on, and only
//! those. (Every test also runs with the harness's check that nothing changes
//! on an output without a redraw being asked for it: `PILA_HEADLESS_CHECK_REDRAWS`.)

mod common;

use common::protocols::LayerSpec;
use common::*;
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::Layer, zwlr_layer_surface_v1::Anchor,
};

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
    // Its frame for the configure that activates it, drawn before counting:
    // a slow client's, coming in late, would count as one for the pointer.
    client.roundtrip();
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

/// A window that fills its output has no shadow, so nothing of it reaches the
/// output next to it: its frames redraw only its own (found on bare metal: a
/// fullscreen terminal on one monitor redrew the other at every frame, and
/// held back that one's own frames).
#[test]
fn a_filled_windows_frames_dont_redraw_the_output_next_to_it() {
    for combo in ["Super+m", "Super+f"] {
        let mut pila = two_fast_outputs();
        let client = pila.client();
        let window = client.window("a").size(400, 300).create();
        assert_eq!(pila.wait_window("a")["output"], "A");
        pila.combo(combo);
        pila.wait("the window to fill A", |s| {
            let (x, _, w, _) = rect(&window_in(s, "a").unwrap()["geometry"]);
            (x, w) == (0, 1280)
        });
        let before = pila.settle();
        for _ in 0..10 {
            client.draw(window, None);
            client.roundtrip();
        }
        let after = pila.wait("A to draw the client's frames", |s| {
            redraws(s, "A").0 > redraws(&before, "A").0
        });
        let after = pila.wait("pila to settle", |s| {
            s["redraw_pending"] == false && redraws(s, "A").0 >= redraws(&after, "A").0
        });
        assert_eq!(
            redraws(&after, "B"),
            redraws(&before, "B"),
            "B was redrawn for the frames of a window filling A ({combo})"
        );
    }
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
/// a direct `space.map_element` moves a window and redraws nothing.
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
    assert!(
        direct.is_empty(),
        "{direct:?}: use Pila::map_window, unmap_window or raise_window"
    );
}

/// The event loop's iterations and those that did all of `after_frames`'
/// bookkeeping, since startup (`frame_stats::LoopStats`).
fn loop_counts(state: &Value) -> (u64, u64) {
    let counts = &state["event_loop"];
    (
        counts["iterations"].as_u64().unwrap(),
        counts["housekeeping"].as_u64().unwrap(),
    )
}

#[test]
fn the_event_loops_iterations_and_housekeeping_are_counted() {
    let mut pila = TestPila::start();
    let before = loop_counts(&pila.settle());
    for i in 0..10 {
        pila.move_by(if i % 2 == 0 { 3.0 } else { -3.0 }, 0.0);
    }
    let after = loop_counts(&pila.settle());
    assert!(
        after.0 > before.0,
        "no iteration counted: {before:?} {after:?}"
    );
    assert!(
        after.1 > before.1,
        "no housekeeping counted: {before:?} {after:?}"
    );
    assert!(
        after.1 <= after.0,
        "more housekeeping than iterations: {after:?}"
    );
}

/// Moving the pointer over a window changes nothing the housekeeping after
/// an iteration looks at (taskbars, IPC, the `Space`): it's done for the
/// frames drawn, not for each motion, which a 1000 Hz mouse sends a
/// thousand times a second.
#[test]
fn moving_the_pointer_over_a_window_doesnt_do_all_the_housekeeping_each_time() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).create();
    let (x, y, ..) = rect(&pila.wait_window("a")["geometry"]);
    pila.move_to((x + 200) as f64, (y + 150) as f64);
    let before = loop_counts(&pila.settle());
    const MOVES: u64 = 100;
    for i in 0..MOVES {
        pila.move_by(if i % 2 == 0 { 2.0 } else { -2.0 }, 0.0);
    }
    let after = loop_counts(&pila.settle());
    let done = after.1 - before.1;
    assert!(
        done < MOVES / 2,
        "{done} of {MOVES} moves did all the housekeeping: {before:?} {after:?}"
    );
}

/// A client setting the cursor it already has (some do on every motion)
/// changes nothing to draw: no redraw for it.
#[test]
fn setting_the_same_cursor_again_redraws_nothing() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    // Its frame for the configure that activates it, in before counting.
    client.roundtrip();
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    pila.move_to((x + 200) as f64, (y + 150) as f64);
    client.wait("pointer enter", |s| s.pointer_name.as_deref() == Some("a"));
    let serial = client.lock().pointer_enter_serial;
    let pointer = client.lock().pointer.clone().expect("a pointer");
    pointer.set_cursor(serial, None, 0, 0);
    client.flush();
    pila.wait("the cursor hidden", |s| s["pointer"]["cursor"] == "hidden");
    let before = redraws(&pila.settle(), "HEADLESS-1");
    for _ in 0..5 {
        pointer.set_cursor(serial, None, 0, 0);
        client.roundtrip();
        pila.settle();
    }
    let after = redraws(&pila.settle(), "HEADLESS-1");
    assert_eq!(after, before, "redrawn for the same cursor again");
}

/// A window animating on one output isn't drawn into a texture for another
/// it isn't on, which another scale would need a texture of its own for
/// (made, and drawn into whole, in that output's frame).
#[test]
fn an_animating_window_isnt_drawn_for_an_output_it_isnt_on() {
    let mut pila = TestPila::start_with(Options {
        config: "[[output]]\nname = \"B\"\nscale = 2\n\n[animations]\nenabled = true\nspeed = 4\n"
            .into(),
        outputs: Some("A:1280x800,B:1280x800".into()),
        ..Options::default()
    });
    let client = pila.client();
    pila.move_to(600.0, 400.0);
    client.window("a").size(400, 300).create();
    let opening = pila.wait("a opening on A", |s| {
        window_in(s, "a").is_some_and(|w| w["animating"] == true && w["output"] == "A")
    });
    // B redrawn while it opens (the pointer moving there).
    let (frames, idle) = redraws(&opening, "B");
    let mut i = 0.0;
    let state = loop {
        pila.move_to(1280.0 + 100.0 + i, 100.0);
        i += 1.0;
        let state = pila.state();
        let now = redraws(&state, "B");
        if now.0 + now.1 > frames + idle + 1 {
            break state;
        }
    };
    let window = window_in(&state, "a").unwrap();
    assert_eq!(window["animating"], true, "the opening ended too soon");
    assert_eq!(window["animation_textures"], 1, "{window}");
}

/// At a scale where an output's logical width isn't whole (1280 / 1.5 =
/// 853.33), a window and the pointer in its last, partial column change
/// with their redraws asked for, and the harness's redraw check finds
/// nothing amiss there. (A smoke test: the check taking the output's place
/// from the `Space` rather than rounding it is what makes it see that
/// column, which only a change with no redraw asked could show.)
#[test]
fn the_last_partial_column_of_a_scaled_output_is_checked_as_drawn() {
    let mut pila = TestPila::with_config(
        "[[output]]\nname = \"HEADLESS-1\"\nscale = 1.5\n\n\
         [[window-rule]]\napp-id = \"edge\"\nposition = [653, 100]\nsize = [201, 200]\n",
    );
    let client = pila.client();
    let window = client.window("edge").create();
    pila.wait_window("edge");
    for x in [853.2, 400.0, 853.2] {
        pila.move_to(x, 150.0);
        client.draw(window, Some((201, 200)));
        pila.settle();
    }
    pila.wait("the pointer on the window in the last column", |s| {
        s["pointer"]["x"].as_f64().is_some_and(|x| x > 853.0)
            && s["pointer"]["focus"]
                .as_str()
                .is_some_and(|f| f.contains("edge"))
    });
}

/// When a window or layer surface last appeared or a window drew at a new
/// size (`state_json`'s `settling_us`), if one has.
fn settling(state: &Value) -> Option<u64> {
    state["settling_us"].as_u64()
}

/// A window or layer surface shown for the first time, or a window drawn at a
/// new size, marks the moment (its app is likely busy on the GPU: on bare
/// metal no frame is held to render late for a while after,
/// `udev/render_late.rs`); drawing at the same size, moving the pointer and
/// clicking don't.
#[test]
fn a_window_appearing_or_drawn_at_a_new_size_starts_settling() {
    let mut pila = TestPila::start();
    let client = pila.client();
    assert_eq!(settling(&pila.settle()), None);
    let window = client.window("a").size(400, 300).create();
    pila.wait_window("a");
    let opened = settling(&pila.wait("noted", |s| settling(s).is_some()));
    for _ in 0..3 {
        client.draw(window, None);
        client.roundtrip();
    }
    pila.move_to(100.0, 100.0);
    pila.click_at(200.0, 200.0);
    assert_eq!(settling(&pila.settle()), opened, "same size, nothing new");
    client.request(window, |t| t.set_maximized());
    pila.wait("maximized", |s| {
        window_in(s, "a").is_some_and(|w| w["maximized"] == true)
    });
    let resized = settling(&pila.wait("the new size noted", |s| settling(s) > opened));
    client.layer_surface(
        "bar",
        &LayerSpec {
            layer: Layer::Top,
            anchor: Anchor::Top | Anchor::Left | Anchor::Right,
            size: (0, 30),
            ..LayerSpec::default()
        },
        [255, 0, 0, 255],
    );
    pila.wait("the bar's appearing noted", |s| settling(s) > resized);
}

/// The bell's flash shows on its window: ringing it redraws the outputs that
/// window is on, not the others.
#[test]
fn the_bell_redraws_only_its_windows_output() {
    use wayland_protocols::xdg::system_bell::v1::client::xdg_system_bell_v1::XdgSystemBellV1;
    let mut pila = two_fast_outputs();
    let client = pila.client();
    pila.move_to(1280.0 + 500.0, 400.0);
    let window = client.window("b").size(400, 300).create();
    assert_eq!(pila.wait_window("b")["output"], "B");
    client.roundtrip();
    let before = pila.settle();
    let bell: XdgSystemBellV1 = client.bind("xdg_system_bell_v1", 1);
    bell.ring(Some(&client.surface(window)));
    client.flush();
    pila.wait("the flash", |s| {
        window_in(s, "b").unwrap()["flashing"] == true
    });
    let after = pila.wait("the flash over, drawn", |s| {
        window_in(s, "b").unwrap()["flashing"] == false && s["redraw_pending"] == false
    });
    let after = pila.wait("pila to settle", |s| {
        s["redraw_pending"] == false && redraws(s, "B").0 >= redraws(&after, "B").0
    });
    assert!(
        redraws(&after, "B").0 > redraws(&before, "B").0,
        "B drew the flash"
    );
    assert_eq!(
        redraws(&after, "A"),
        redraws(&before, "A"),
        "A was redrawn for a bell on B"
    );
}

/// Opening, fullscreening (touching the neighboring output's edge), restoring,
/// minimizing and closing a window on one output — with animations and shadows
/// enabled — leaves the neighboring output untouched during open/minimize/close,
/// and avoids trailing idle redraws when fullscreen/restore shadows cross the
/// seam and pull back.
#[test]
fn a_window_opening_filling_and_closing_on_one_output_doesnt_redraw_another() {
    let mut pila = TestPila::start_with(Options {
        outputs: Some("A:1280x800@144,B:1024x768@144".into()),
        config: "[animations]\nenabled = true\n\n[shadow]\nenabled = true\n".into(),
        ..Options::default()
    });
    let client = pila.client();
    pila.move_to(600.0, 400.0);
    let before = pila.settle();

    let window = client.window("a").size(400, 300).create();
    pila.wait("a opened and animation finished", |s| {
        window_in(s, "a").is_some_and(|w| w["output"] == "A") && s["animations"]["running"] == false
    });
    let after_open = pila.settle();
    assert_eq!(
        redraws(&after_open, "B"),
        redraws(&before, "B"),
        "B was redrawn while opening on A"
    );

    // Fullscreen/restore animations stretch a shadowed snapshot across the
    // seam onto B for a few frames, then clear it without trailing idle storms.
    client.request(window, |t| t.set_fullscreen(None));
    pila.wait("a fullscreen and animation finished", |s| {
        window_in(s, "a").is_some_and(|w| w["fullscreen"] == true)
            && s["animations"]["running"] == false
    });
    client.request(window, |t| t.unset_fullscreen());
    pila.wait("a restored and animation finished", |s| {
        window_in(s, "a").is_some_and(|w| w["fullscreen"] == false)
            && s["animations"]["running"] == false
    });
    let after_restore = pila.settle();
    let idle_during_fill = redraws(&after_restore, "B").1 - redraws(&after_open, "B").1;
    assert!(
        idle_during_fill <= 3,
        "B had {idle_during_fill} trailing idle redraws during fullscreen/restore on A"
    );

    client.request(window, |t| t.set_minimized());
    pila.wait("a minimized and animation finished", |s| {
        window_in(s, "a").is_some_and(|w| w["minimized"] == true)
            && s["animations"]["running"] == false
    });
    client.destroy(window);
    pila.wait("a gone and animation finished", |s| {
        window_in(s, "a").is_none() && s["animations"]["running"] == false
    });
    let after_close = pila.settle();
    assert_eq!(
        redraws(&after_close, "B"),
        redraws(&after_restore, "B"),
        "B was redrawn while minimizing and closing on A"
    );
}

/// Moving keyboard focus between windows on one output redraws only that
/// output, not neighboring ones.
#[test]
fn changing_focus_on_one_output_doesnt_redraw_another() {
    let mut pila = two_fast_outputs();
    let client = pila.client();
    pila.move_to(600.0, 400.0);
    client.window("a1").size(400, 300).create();
    pila.wait_focus("a1");
    let a2 = client.window("a2").size(400, 300).create();
    pila.wait_focus("a2");
    client.roundtrip();
    let before = pila.settle();

    client.destroy(a2);
    pila.wait_focus("a1");
    client.roundtrip();
    let after = pila.settle();
    assert_eq!(
        redraws(&after, "B"),
        redraws(&before, "B"),
        "B was redrawn when focus changed on A"
    );
}
