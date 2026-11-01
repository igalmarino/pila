//! Server-side decorations: a title bar above the window (drag to move,
//! maximize and close buttons) and an invisible resize border around it.
//!
//! Only windows that agreed to server-side decorations get these (see
//! `XdgDecorationHandler` in `handlers/xdg_shell.rs`); fullscreen windows get
//! neither, maximized windows get no resize border. The window's position in
//! the `Space` stays its geometry origin; the title bar sits directly above it.
//!
//! Layout decisions (placement, maximize) use the *pending* decoration mode,
//! which pila decides; drawing and hit testing use the *committed* state, so a
//! bar only appears once the client has agreed to stop drawing its own.

use std::cell::RefCell;

use smithay::{
    backend::renderer::element::{
        solid::{SolidColorBuffer, SolidColorRenderElement},
        Kind,
    },
    desktop::Window,
    reexports::wayland_protocols::xdg::{
        decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
        shell::server::xdg_toplevel,
    },
    utils::{Logical, Physical, Point, Rectangle, Scale},
};

use crate::grabs::resize_grab::ResizeEdge;

pub const TITLE_BAR_HEIGHT: i32 = 28;
/// Each button's clickable cell is square, at the right end of the bar.
const BUTTON_WIDTH: i32 = TITLE_BAR_HEIGHT;
/// The visible button is inset this much inside its cell.
const BUTTON_INSET: i32 = 8;
/// Width of the invisible resize border outside the window and title bar.
const BORDER: i32 = 6;
/// Distance from a corner, along each edge, that resizes diagonally.
const CORNER: i32 = 20;

const BAR_FOCUSED: [f32; 4] = [0.21, 0.36, 0.58, 1.0];
const BAR_UNFOCUSED: [f32; 4] = [0.24, 0.25, 0.28, 1.0];
const MAXIMIZE: [f32; 4] = [0.80, 0.66, 0.26, 1.0];
const MAXIMIZE_HOVER: [f32; 4] = [0.98, 0.82, 0.35, 1.0];
const CLOSE: [f32; 4] = [0.78, 0.30, 0.28, 1.0];
const CLOSE_HOVER: [f32; 4] = [0.97, 0.38, 0.34, 1.0];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleBarPart {
    Drag,
    Maximize,
    Close,
}

/// A pila-drawn (or pila-handled) part of a window, as opposed to a client surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decoration {
    TitleBar(TitleBarPart),
    Border(ResizeEdge),
}

/// Whether pila has chosen server-side decorations for this window. For
/// layout; may be ahead of what the client has acked.
pub fn is_ssd(window: &Window) -> bool {
    window.toplevel().is_some_and(|t| {
        t.with_pending_state(|state| state.decoration_mode == Some(DecorationMode::ServerSide))
    })
}

/// Whether a title bar is drawn: the client acked server-side decorations
/// and isn't fullscreen.
pub fn has_title_bar(window: &Window) -> bool {
    window.toplevel().is_some_and(|t| {
        let state = t.current_state();
        state.decoration_mode == Some(DecorationMode::ServerSide)
            && !state.states.contains(xdg_toplevel::State::Fullscreen)
    })
}

/// Height taken above a window at `loc` (its geometry origin) by its title bar,
/// for layout: 0 unless the window is server-side decorated and not fullscreen.
pub fn layout_offset(window: &Window, fullscreen: bool) -> i32 {
    if is_ssd(window) && !fullscreen {
        TITLE_BAR_HEIGHT
    } else {
        0
    }
}

/// The title bar of a window whose geometry origin is at `loc`, if it has one.
pub fn title_bar_rect(
    window: &Window,
    loc: Point<i32, Logical>,
) -> Option<Rectangle<i32, Logical>> {
    has_title_bar(window).then(|| {
        Rectangle::new(
            (loc.x, loc.y - TITLE_BAR_HEIGHT).into(),
            (window.geometry().size.w, TITLE_BAR_HEIGHT).into(),
        )
    })
}

/// Which part of a title bar `pos` is on. `pos` must be inside `bar`.
pub fn title_bar_part(bar: Rectangle<i32, Logical>, pos: Point<f64, Logical>) -> TitleBarPart {
    let from_right = (bar.loc.x + bar.size.w) as f64 - pos.x;
    if from_right <= BUTTON_WIDTH as f64 {
        TitleBarPart::Close
    } else if from_right <= 2.0 * BUTTON_WIDTH as f64 {
        TitleBarPart::Maximize
    } else {
        TitleBarPart::Drag
    }
}

/// The resize edge(s) for `pos` if it's on the invisible border around a
/// decorated, non-maximized window whose geometry origin is at `loc`.
pub fn border_edges(
    window: &Window,
    loc: Point<i32, Logical>,
    pos: Point<f64, Logical>,
) -> Option<ResizeEdge> {
    let bar = title_bar_rect(window, loc)?;
    let maximized = window.toplevel().is_some_and(|t| {
        t.current_state()
            .states
            .contains(xdg_toplevel::State::Maximized)
    });
    if maximized {
        return None;
    }
    // The window plus its title bar.
    let frame = Rectangle::new(
        bar.loc,
        (bar.size.w, bar.size.h + window.geometry().size.h).into(),
    )
    .to_f64();
    let outer = Rectangle::new(
        frame.loc - Point::from((BORDER as f64, BORDER as f64)),
        frame.size + (2.0 * BORDER as f64, 2.0 * BORDER as f64).into(),
    );
    if !outer.contains(pos) || frame.contains(pos) {
        return None;
    }

    // The band the pointer is in gives one edge; being within CORNER of a
    // corner along that band adds the other.
    Some(
        band_edges(
            pos.x,
            frame.loc.x,
            frame.size.w,
            ResizeEdge::LEFT,
            ResizeEdge::RIGHT,
        ) | band_edges(
            pos.y,
            frame.loc.y,
            frame.size.h,
            ResizeEdge::TOP,
            ResizeEdge::BOTTOM,
        ),
    )
}

/// One axis of `border_edges`: `low` if `p` is before the frame (or within
/// CORNER of its start), `high` if past it (or within CORNER of its end).
fn band_edges(p: f64, start: f64, len: f64, low: ResizeEdge, high: ResizeEdge) -> ResizeEdge {
    let end = start + len;
    let corner = CORNER as f64;
    if p < start + corner {
        low
    } else if p >= end - corner {
        high
    } else {
        ResizeEdge::empty()
    }
}

/// Solid color buffers for one window's title bar, kept in its user data so
/// the damage tracker sees the same elements from frame to frame.
#[derive(Default)]
struct TitleBarBuffers {
    bar: SolidColorBuffer,
    maximize: SolidColorBuffer,
    close: SolidColorBuffer,
}

/// Render elements for a title bar at `rect` (relative to the output origin),
/// front to back.
pub fn title_bar_elements(
    window: &Window,
    rect: Rectangle<i32, Logical>,
    focused: bool,
    hover: Option<TitleBarPart>,
    scale: Scale<f64>,
) -> Vec<SolidColorRenderElement> {
    window
        .user_data()
        .insert_if_missing(|| RefCell::new(TitleBarBuffers::default()));
    let mut buffers = window
        .user_data()
        .get::<RefCell<TitleBarBuffers>>()
        .unwrap()
        .borrow_mut();

    let button_size = (
        BUTTON_WIDTH - 2 * BUTTON_INSET,
        TITLE_BAR_HEIGHT - 2 * BUTTON_INSET,
    );
    let pick = |part, normal, hovered| {
        if hover == Some(part) {
            hovered
        } else {
            normal
        }
    };
    buffers
        .bar
        .update(rect.size, if focused { BAR_FOCUSED } else { BAR_UNFOCUSED });
    buffers
        .close
        .update(button_size, pick(TitleBarPart::Close, CLOSE, CLOSE_HOVER));
    buffers.maximize.update(
        button_size,
        pick(TitleBarPart::Maximize, MAXIMIZE, MAXIMIZE_HOVER),
    );

    let close_loc = Point::from((
        rect.loc.x + rect.size.w - BUTTON_WIDTH + BUTTON_INSET,
        rect.loc.y + BUTTON_INSET,
    ));
    let maximize_loc = close_loc - Point::from((BUTTON_WIDTH, 0));
    let element = |buffer: &SolidColorBuffer, loc: Point<i32, Logical>| {
        let loc: Point<i32, Physical> = loc.to_physical_precise_round(scale);
        SolidColorRenderElement::from_buffer(buffer, loc, scale, 1.0, Kind::Unspecified)
    };
    vec![
        element(&buffers.close, close_loc),
        element(&buffers.maximize, maximize_loc),
        element(&buffers.bar, rect.loc),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar() -> Rectangle<i32, Logical> {
        // A 300px-wide bar at (100, 50).
        Rectangle::new((100, 50).into(), (300, TITLE_BAR_HEIGHT).into())
    }

    #[test]
    fn title_bar_buttons_are_at_the_right_end() {
        let y = 50.0 + TITLE_BAR_HEIGHT as f64 / 2.0;
        let right = 400.0;
        assert_eq!(
            title_bar_part(bar(), (right - 1.0, y).into()),
            TitleBarPart::Close
        );
        assert_eq!(
            title_bar_part(bar(), (right - BUTTON_WIDTH as f64 - 1.0, y).into()),
            TitleBarPart::Maximize
        );
        assert_eq!(
            title_bar_part(bar(), (right - 2.0 * BUTTON_WIDTH as f64 - 1.0, y).into()),
            TitleBarPart::Drag
        );
        assert_eq!(title_bar_part(bar(), (101.0, y).into()), TitleBarPart::Drag);
    }

    #[test]
    fn band_edges_pick_side_and_corners() {
        let (low, high) = (ResizeEdge::LEFT, ResizeEdge::RIGHT);
        // Frame spans 100..400 on this axis.
        assert_eq!(band_edges(97.0, 100.0, 300.0, low, high), low); // in the border, before the frame
        assert_eq!(band_edges(403.0, 100.0, 300.0, low, high), high); // past the frame
        assert_eq!(band_edges(110.0, 100.0, 300.0, low, high), low); // within CORNER of the start
        assert_eq!(band_edges(390.0, 100.0, 300.0, low, high), high); // within CORNER of the end
        assert_eq!(
            band_edges(250.0, 100.0, 300.0, low, high),
            ResizeEdge::empty()
        ); // middle
    }
}
