//! Popups (menus): placement against the parent, keeping them on the
//! output, grabs and what dismisses them, nested popups, popups of layer
//! surfaces.

mod common;

use common::client::Ev;
use common::protocols::{LayerSpec, Placement};
use common::*;
use wayland_protocols::xdg::shell::client::xdg_positioner::{
    Anchor, ConstraintAdjustment, Gravity,
};

const BLUE: [u8; 4] = [255, 0, 0, 255];

fn popup_of<'a>(state: &'a Value, owner: &str) -> Option<&'a Value> {
    state["popups"]
        .as_array()?
        .iter()
        .find(|p| p["owner"] == owner)
}

/// Whether `owner` has a popup with a buffer (a size).
fn mapped_popup(state: &Value, owner: &str) -> bool {
    popup_of(state, owner).is_some_and(|p| p["geometry"]["w"] != 0)
}

fn window_at_its_place() -> (
    TestPila,
    Client,
    common::client::Window,
    (i64, i64, i64, i64),
) {
    let mut pila = TestPila::start();
    let client = pila.client();
    let window = client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let geometry = rect(&pila.window("a")["geometry"]);
    (pila, client, window, geometry)
}

/// Press (and hold) the left button inside the window, for a grab serial.
fn press_in(pila: &mut TestPila, client: &Client, x: i64, y: i64) {
    pila.move_to(x as f64, y as f64);
    pila.press("left");
    client.wait("the press", |s| {
        s.events
            .iter()
            .any(|e| matches!(e, Ev::Button { pressed: true, .. }))
    });
}

#[test]
fn a_popup_over_the_title_bar_gets_the_pointer() {
    let (mut pila, client, window, (x, y, ..)) = window_at_its_place();
    let placement = Placement {
        anchor_rect: (10, -20, 1, 1),
        ..Placement::default()
    };
    let (menu, _) = client.popup("menu", Ok(window), &placement, false, BLUE);
    client.wait_surface_configured(menu);
    pila.wait("the popup", |s| mapped_popup(s, "a"));
    pila.settle();
    // Drawn over the title bar, so that's what the pointer is on.
    pila.move_to((x + 20) as f64, (y - 10) as f64);
    client.wait("the pointer on the menu", |s| {
        s.pointer_name.as_deref() == Some("menu")
    });
}

#[test]
fn a_popup_goes_where_its_positioner_says() {
    let (mut pila, client, window, (x, y, ..)) = window_at_its_place();
    let (menu, _) = client.popup("menu", Ok(window), &Placement::default(), false, BLUE);
    assert_eq!(client.wait_surface_configured(menu), (10, 10, 100, 80));
    let state = pila.wait("the popup", |s| mapped_popup(s, "a"));
    assert_eq!(
        rect(&popup_of(&state, "a").unwrap()["geometry"]),
        (x + 10, y + 10, 100, 80)
    );
    pila.settle();
    assert_eq!(
        pila.pixel((x + 50) as f64, (y + 50) as f64),
        [0, 0, 255, 255]
    );
}

/// `xdg_popup.reposition`, as GTK 4 moves a popover that stays open: the
/// popup is told it was repositioned (the client's token), configured at its
/// new place and shown there.
#[test]
fn a_popup_can_be_repositioned() {
    let (mut pila, client, window, (x, y, ..)) = window_at_its_place();
    let (menu, popup) = client.popup("menu", Ok(window), &Placement::default(), false, BLUE);
    assert_eq!(client.wait_surface_configured(menu), (10, 10, 100, 80));
    pila.wait("the popup", |s| mapped_popup(s, "a"));

    let moved = Placement {
        size: (120, 90),
        anchor_rect: (60, 40, 1, 1),
        ..Placement::default()
    };
    client.reposition_popup(&popup, &moved, 7);
    client.wait_logged("popup:menu", "Repositioned { token: 7 }");
    client.wait("configured at its new place", |s| {
        s.surfaces[menu.0].configures.last() == Some(&(60, 40, 120, 90))
    });
    let state = pila.wait("shown there", |s| {
        popup_of(s, "a").is_some_and(|p| rect(&p["geometry"]) == (x + 60, y + 40, 120, 90))
    });
    assert!(mapped_popup(&state, "a"));
    pila.settle();
    assert_eq!(
        pila.pixel((x + 170) as f64, (y + 120) as f64),
        [0, 0, 255, 255]
    );
}

#[test]
fn a_popup_slides_back_onto_the_output_when_allowed() {
    let (_pila, client, window, (x, ..)) = window_at_its_place();
    // Anchored near the window's right edge, 500 wide: past the output.
    let placement = Placement {
        size: (500, 80),
        anchor_rect: (380, 10, 1, 1),
        constraints: ConstraintAdjustment::SlideX,
        ..Placement::default()
    };
    let (menu, _) = client.popup("menu", Ok(window), &placement, false, BLUE);
    let (px, ..) = client.wait_surface_configured(menu);
    assert_eq!(
        x + px as i64 + 500,
        1280,
        "slid to end at the output's edge"
    );
}

#[test]
fn a_popup_flips_when_allowed_and_that_fits() {
    let (_pila, client, window, (_, y, _, h)) = window_at_its_place();
    // From the window's bottom edge (y + h - 1 = 563) down, 300 tall: past
    // the output's bottom (800). Flipped up, it fits (263..563).
    let placement = Placement {
        size: (100, 300),
        anchor_rect: (10, h as i32 - 1, 1, 1),
        anchor: Anchor::BottomLeft,
        gravity: Gravity::BottomRight,
        constraints: ConstraintAdjustment::FlipY,
    };
    let (menu, _) = client.popup("menu", Ok(window), &placement, false, BLUE);
    assert_eq!(
        client.wait_surface_configured(menu),
        (10, h as i32 - 1 - 300, 100, 300)
    );
    assert!(y + h - 1 - 300 >= 0);
}

#[test]
fn a_popup_doesnt_flip_when_flipped_wouldnt_fit_either() {
    let (_pila, client, window, (_, _, _, h)) = window_at_its_place();
    // 600 tall: out of the output down (to 1163) and up (to -37).
    let placement = Placement {
        size: (100, 600),
        anchor_rect: (10, h as i32 - 1, 1, 1),
        anchor: Anchor::BottomLeft,
        gravity: Gravity::BottomRight,
        constraints: ConstraintAdjustment::FlipY,
    };
    let (menu, _) = client.popup("menu", Ok(window), &placement, false, BLUE);
    assert_eq!(
        client.wait_surface_configured(menu),
        (10, h as i32, 100, 600)
    );
}

#[test]
fn without_adjustments_a_popup_stays_where_asked() {
    let (_pila, client, window, _) = window_at_its_place();
    let placement = Placement {
        size: (500, 80),
        anchor_rect: (380, 10, 1, 1),
        ..Placement::default()
    };
    let (menu, _) = client.popup("menu", Ok(window), &placement, false, BLUE);
    assert_eq!(client.wait_surface_configured(menu), (380, 10, 500, 80));
}

#[test]
fn a_grabbing_popup_takes_the_keyboard_and_a_click_outside_dismisses_it() {
    let (mut pila, client, window, (x, y, ..)) = window_at_its_place();
    press_in(&mut pila, &client, x + 20, y + 20);
    let (menu, _) = client.popup("menu", Ok(window), &Placement::default(), true, BLUE);
    client.wait_surface_configured(menu);
    pila.release("left");
    let state = pila.wait("the grab", |s| s["keyboard_grabbed"] == true);
    assert_eq!(state["keyboard_focus"], "popup:a");
    client.wait("the popup has the keyboard", |s| {
        s.keyboard_name.as_deref() == Some("menu")
    });
    pila.click_at(5.0, 795.0);
    client.wait_surface_closed(menu);
    pila.wait("the grab is gone", |s| s["keyboard_grabbed"] == false);
}

#[test]
fn a_new_window_dismisses_a_grabbing_popup() {
    let (mut pila, client, window, (x, y, ..)) = window_at_its_place();
    press_in(&mut pila, &client, x + 20, y + 20);
    let (menu, _) = client.popup("menu", Ok(window), &Placement::default(), true, BLUE);
    client.wait_surface_configured(menu);
    pila.release("left");
    pila.wait("the grab", |s| s["keyboard_grabbed"] == true);
    let other = pila.client();
    other.window("b").create();
    pila.wait_focus("b");
    client.wait_surface_closed(menu);
}

#[test]
fn mod_tab_dismisses_a_grabbing_popup() {
    let mut pila = TestPila::start();
    let client = pila.client();
    client.window("b").create();
    pila.wait_focus("b");
    let window = client.window("a").size(400, 300).create();
    pila.wait_focus("a");
    let (x, y, ..) = rect(&pila.window("a")["geometry"]);
    press_in(&mut pila, &client, x + 20, y + 20);
    let (menu, _) = client.popup("menu", Ok(window), &Placement::default(), true, BLUE);
    client.wait_surface_configured(menu);
    pila.release("left");
    pila.wait("the grab", |s| s["keyboard_grabbed"] == true);
    pila.combo("Super+Tab");
    // Bindings still run during a menu grab: Mod+Tab switches and closes it.
    pila.wait_focus("b");
    client.wait_surface_closed(menu);
}

#[test]
fn a_popup_of_a_popup_is_placed_against_its_parent() {
    let (mut pila, client, window, (x, y, ..)) = window_at_its_place();
    let (menu, _) = client.popup("menu", Ok(window), &Placement::default(), false, BLUE);
    client.wait_surface_configured(menu);
    let placement = Placement {
        size: (60, 40),
        anchor_rect: (100, 0, 1, 1),
        ..Placement::default()
    };
    let (submenu, _) = client.popup("submenu", Err(menu), &placement, false, BLUE);
    assert_eq!(client.wait_surface_configured(submenu), (100, 0, 60, 40));
    let state = pila.wait("both popups", |s| {
        s["popups"]
            .as_array()
            .is_some_and(|p| p.len() == 2 && p.iter().all(|p| p["geometry"]["w"] != 0))
    });
    let rects: Vec<_> = state["popups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| rect(&p["geometry"]))
        .collect();
    assert!(rects.contains(&(x + 10, y + 10, 100, 80)), "{rects:?}");
    assert!(rects.contains(&(x + 110, y + 10, 60, 40)), "{rects:?}");
}

#[test]
fn a_layer_surface_can_have_popups() {
    let mut pila = TestPila::start();
    let client = pila.client();
    let (panel, _) = client.layer_surface(
        "panel",
        &LayerSpec {
            anchor:
                wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::Anchor::Top,
            size: (300, 40),
            ..LayerSpec::default()
        },
        BLUE,
    );
    client.wait_surface_configured(panel);
    let placement = Placement {
        size: (100, 200),
        anchor_rect: (0, 40, 1, 1),
        ..Placement::default()
    };
    let (menu, _) = client.popup("menu", Err(panel), &placement, false, BLUE);
    client.wait_surface_configured(menu);
    let state = pila.wait("the popup", |s| mapped_popup(s, "panel"));
    // The panel is centered at the top: x = (1280 - 300) / 2.
    assert_eq!(
        rect(&popup_of(&state, "panel").unwrap()["geometry"]),
        (490, 40, 100, 200)
    );
}

#[test]
fn a_popup_closes_with_its_window() {
    let (mut pila, client, window, _) = window_at_its_place();
    let (menu, popup) = client.popup("menu", Ok(window), &Placement::default(), false, BLUE);
    client.wait_surface_configured(menu);
    pila.wait("the popup", |s| mapped_popup(s, "a"));
    popup.destroy();
    client.destroy(window);
    pila.wait("nothing left", |s| {
        s["popups"].as_array().unwrap().is_empty() && window_in(s, "a").is_none()
    });
}

/// A popup hidden with a null commit is gone (Smithay drops its parent, so
/// it can't map again): its hiding commit isn't answered with a configure.
#[test]
fn a_popup_hidden_with_a_null_commit_isnt_configured_again() {
    let (mut pila, client, window, _) = window_at_its_place();
    let (menu, _) = client.popup("menu", Ok(window), &Placement::default(), false, BLUE);
    client.wait_surface_configured(menu);
    pila.wait("the popup", |s| mapped_popup(s, "a"));
    let surface = client.wl_surface_of(menu);
    client.surface_data(menu, |s| s.auto_draw = false);
    surface.attach(None, 0, 0);
    surface.commit();
    client.roundtrip();
    client.roundtrip();
    pila.wait("no popup shown", |s| !mapped_popup(s, "a"));
    let configures = client.surface_data(menu, |s| s.configures.len());
    assert_eq!(configures, 1, "a configure answered the hiding commit");
    assert_eq!(client.error(), None);
}

/// GTK 4 keeps a popover's surface when it hides it (role objects
/// destroyed, a null commit) and opens the next popover on it: that popup
/// is new, gets its initial configure and shows, and a click outside ends
/// its grab. It once got no configure and never showed, and its grab kept
/// every click from the app until a key was pressed.
#[test]
fn a_popup_on_the_surface_of_a_hidden_one_is_configured_and_shows() {
    let (mut pila, client, window, (x, y, ..)) = window_at_its_place();
    press_in(&mut pila, &client, x + 20, y + 20);
    let (menu, popup) = client.popup("menu", Ok(window), &Placement::default(), true, BLUE);
    client.wait_surface_configured(menu);
    pila.release("left");
    pila.wait("the first popup, with its grab", |s| {
        mapped_popup(s, "a") && s["pointer"]["grab"] == "popup"
    });
    client.hide_popup_keeping_surface(menu, &popup);
    pila.wait("the first popup hidden, no grab", |s| {
        popup_of(s, "a").is_none()
            && s["pointer"]["grabbed"] == false
            && s["keyboard_grabbed"] == false
    });

    client.clear_events();
    press_in(&mut pila, &client, x + 20, y + 20);
    client.popup_on(menu, Ok(window), &Placement::default(), true);
    client.wait("the second popup's configure", |s| {
        s.surfaces[menu.0].configures.len() == 2
    });
    pila.release("left");
    pila.wait("the second popup, with its grab", |s| {
        mapped_popup(s, "a") && s["pointer"]["grab"] == "popup"
    });
    pila.click_at(5.0, 795.0);
    client.wait_surface_closed(menu);
    pila.wait("the grab is gone", |s| {
        s["pointer"]["grabbed"] == false && s["keyboard_grabbed"] == false
    });
    assert_eq!(client.error(), None);
}

/// While a menu is open (its grab holds the pointer), a scroll over it and a
/// touchpad pinch reach it: a long menu scrolls, and the grab passes them on.
#[test]
fn a_menu_open_gets_the_scroll_and_gestures_over_it() {
    use wayland_protocols::wp::pointer_gestures::zv1::client::zwp_pointer_gestures_v1::ZwpPointerGesturesV1;
    let (mut pila, client, window, (x, y, ..)) = window_at_its_place();
    let gestures: ZwpPointerGesturesV1 = client.bind("zwp_pointer_gestures_v1", 3);
    let pointer = client.lock().pointer.clone().expect("a pointer");
    gestures.get_pinch_gesture(&pointer, &client.qh, "pinch".into());
    client.flush();
    client.roundtrip();

    press_in(&mut pila, &client, x + 20, y + 20);
    let (menu, _) = client.popup("menu", Ok(window), &Placement::default(), true, BLUE);
    client.wait_surface_configured(menu);
    pila.release("left");
    let state = pila.wait("the menu's grab", |s| {
        s["pointer"]["grab"] == "popup" && mapped_popup(s, "a")
    });
    let (mx, my, mw, mh) = rect(&popup_of(&state, "a").unwrap()["geometry"]);
    pila.move_to((mx + mw / 2) as f64, (my + mh / 2) as f64);
    client.wait("the pointer on the menu", |s| {
        s.pointer_name.as_deref() == Some("menu")
    });

    client.clear_events();
    pila.scroll("wheel", 0.0, 15.0);
    client.wait("the menu's scroll", |s| {
        s.events
            .iter()
            .any(|e| matches!(e, Ev::Axis { horizontal: false, value } if *value > 0.0))
    });
    pila.gesture(json!({ "kind": "pinch", "phase": "begin", "fingers": 2 }));
    pila.gesture(json!({ "kind": "pinch", "phase": "update", "scale": 1.5, "rotation": 0.0 }));
    pila.gesture(json!({ "kind": "pinch", "phase": "end" }));
    client.wait_logged("pinch", "End");
    assert!(client.has_logged("pinch", "scale: 1.5"));
    assert_eq!(
        pila.state()["pointer"]["grab"],
        "popup",
        "the menu is still open"
    );
}
