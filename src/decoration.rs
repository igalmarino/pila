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
    backend::{
        allocator::Fourcc,
        renderer::{
            element::{
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                solid::{SolidColorBuffer, SolidColorRenderElement},
                Kind,
            },
            ImportAll, ImportMem, Renderer,
        },
    },
    desktop::Window,
    reexports::wayland_protocols::xdg::{
        decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
        shell::server::xdg_toplevel,
    },
    utils::{Logical, Physical, Point, Rectangle, Scale, Transform},
};
use tracing::warn;

use crate::{grabs::resize_grab::ResizeEdge, render::PilaRenderElement};

pub const TITLE_BAR_HEIGHT: i32 = 28;
/// Width of each button cell (clickable and hover-highlighted), at the right
/// end of the bar. Cells are the full bar height.
const BUTTON_WIDTH: i32 = 36;
/// Width of the invisible resize border outside the window and title bar.
const BORDER: i32 = 6;
/// Distance from a corner, along each edge, that resizes diagonally.
const CORNER: i32 = 20;

// Neutral dark greys; focus is shown by the bar and icon brightness.
const BAR_FOCUSED: [f32; 4] = [0.184, 0.192, 0.212, 1.0]; // #2f3136
const BAR_UNFOCUSED: [f32; 4] = [0.149, 0.153, 0.169, 1.0]; // #26272b
const ICON_FOCUSED: [f32; 3] = [0.875, 0.882, 0.898]; // #dfe1e5
const ICON_UNFOCUSED: [f32; 3] = [0.502, 0.514, 0.541]; // #80838a
const HOVER_BG: [f32; 3] = [0.247, 0.259, 0.286]; // #3f4249
const HOVER_ICON: [f32; 3] = [0.949, 0.953, 0.961]; // #f2f3f5
const CLOSE_HOVER_BG: [f32; 3] = [0.769, 0.169, 0.110]; // #c42b1c
const CLOSE_HOVER_ICON: [f32; 3] = [1.0, 1.0, 1.0];

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

/// Which glyph a button shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Icon {
    Close,
    Maximize,
    /// Shown on the maximize button of a maximized window.
    Restore,
}

/// A button's colors: depends on window focus and pointer hover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Look {
    Focused,
    Unfocused,
    Hover,
}

/// A rasterized button and what it was drawn for, so it's only redrawn
/// (and re-uploaded) when the icon or look changes.
struct ButtonTexture {
    key: (Icon, Look),
    buffer: MemoryRenderBuffer,
}

/// One window's title bar buffers, kept in its user data so the damage
/// tracker sees the same elements from frame to frame. Buffers can't be
/// shared between windows: element ids must be unique within a frame.
#[derive(Default)]
struct TitleBarBuffers {
    bar: SolidColorBuffer,
    maximize: Option<ButtonTexture>,
    close: Option<ButtonTexture>,
}

/// Render elements for a title bar at `rect` (relative to the output origin),
/// front to back.
pub fn title_bar_elements<R>(
    renderer: &mut R,
    window: &Window,
    rect: Rectangle<i32, Logical>,
    focused: bool,
    hover: Option<TitleBarPart>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    window
        .user_data()
        .insert_if_missing(|| RefCell::new(TitleBarBuffers::default()));
    let mut buffers = window
        .user_data()
        .get::<RefCell<TitleBarBuffers>>()
        .unwrap()
        .borrow_mut();
    let TitleBarBuffers {
        bar,
        maximize,
        close,
    } = &mut *buffers;

    let maximized = window.toplevel().is_some_and(|t| {
        t.current_state()
            .states
            .contains(xdg_toplevel::State::Maximized)
    });
    let look = |part| match (hover == Some(part), focused) {
        (true, _) => Look::Hover,
        (false, true) => Look::Focused,
        (false, false) => Look::Unfocused,
    };
    let close_loc = Point::from((rect.loc.x + rect.size.w - BUTTON_WIDTH, rect.loc.y));
    let maximize_loc = close_loc - Point::from((BUTTON_WIDTH, 0));
    let maximize_icon = if maximized {
        Icon::Restore
    } else {
        Icon::Maximize
    };

    let mut elements = Vec::with_capacity(3);
    for (slot, icon, part, loc) in [
        (close, Icon::Close, TitleBarPart::Close, close_loc),
        (
            maximize,
            maximize_icon,
            TitleBarPart::Maximize,
            maximize_loc,
        ),
    ] {
        let key = (icon, look(part));
        if slot.as_ref().is_none_or(|texture| texture.key != key) {
            *slot = Some(ButtonTexture {
                key,
                buffer: button_buffer(icon, key.1),
            });
        }
        let buffer = &slot.as_ref().unwrap().buffer;
        let loc: Point<f64, Physical> = loc.to_f64().to_physical(scale);
        match MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            loc,
            buffer,
            None,
            None,
            None,
            Kind::Unspecified,
        ) {
            Ok(element) => elements.push(element.into()),
            Err(err) => warn!(?err, "failed to upload title bar button"),
        }
    }

    bar.update(rect.size, if focused { BAR_FOCUSED } else { BAR_UNFOCUSED });
    let loc: Point<i32, Physical> = rect.loc.to_physical_precise_round(scale);
    elements
        .push(SolidColorRenderElement::from_buffer(bar, loc, scale, 1.0, Kind::Unspecified).into());
    elements
}

/// Draw one button cell (`BUTTON_WIDTH` x `TITLE_BAR_HEIGHT`): a background
/// (transparent unless hovered, so the bar shows through) and an
/// anti-aliased 10px glyph in the middle.
fn button_buffer(icon: Icon, look: Look) -> MemoryRenderBuffer {
    let (background, color) = match (look, icon) {
        (Look::Hover, Icon::Close) => (Some(CLOSE_HOVER_BG), CLOSE_HOVER_ICON),
        (Look::Hover, _) => (Some(HOVER_BG), HOVER_ICON),
        (Look::Focused, _) => (None, ICON_FOCUSED),
        (Look::Unfocused, _) => (None, ICON_UNFOCUSED),
    };
    let pixels = rasterize_button(icon, background, color);
    MemoryRenderBuffer::from_slice(
        &pixels,
        Fourcc::Argb8888,
        (BUTTON_WIDTH, TITLE_BAR_HEIGHT),
        1,
        Transform::Normal,
        None,
    )
}

/// Premultiplied ARGB8888 pixels (little-endian: B, G, R, A) for a button.
fn rasterize_button(icon: Icon, background: Option<[f32; 3]>, color: [f32; 3]) -> Vec<u8> {
    let (w, h) = (BUTTON_WIDTH, TITLE_BAR_HEIGHT);
    // Glyph center. Outline edges land on pixel centers, so 1px strokes are crisp.
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let p = (x as f64 + 0.5, y as f64 + 0.5);
            let coverage = glyph_coverage(icon, p, (cx, cy)) as f32;
            let (bg, bg_alpha) = match background {
                Some(bg) => (bg, 1.0),
                None => ([0.0; 3], 0.0),
            };
            let channel = |i: usize| color[i] * coverage + bg[i] * bg_alpha * (1.0 - coverage);
            let alpha = coverage + bg_alpha * (1.0 - coverage);
            let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            pixels.extend([
                byte(channel(2)),
                byte(channel(1)),
                byte(channel(0)),
                byte(alpha),
            ]);
        }
    }
    pixels
}

/// How much of the pixel at `p` the glyph covers (0..=1).
fn glyph_coverage(icon: Icon, p: (f64, f64), (cx, cy): (f64, f64)) -> f64 {
    // A stroke `width` wide along a shape at distance `dist` from p.
    let stroke = |dist: f64, width: f64| (width / 2.0 + 0.5 - dist).clamp(0.0, 1.0);
    match icon {
        Icon::Close => {
            let r = 4.5;
            let d1 = segment_distance(p, (cx - r, cy - r), (cx + r, cy + r));
            let d2 = segment_distance(p, (cx + r, cy - r), (cx - r, cy + r));
            stroke(d1.min(d2), 1.4)
        }
        Icon::Maximize => stroke(box_sdf(p, (cx, cy), 4.5).abs(), 1.0),
        Icon::Restore => {
            // A front square, and the part of a back square (up and right)
            // that isn't hidden behind it. Together they span the same 10px
            // as the maximize square, with edges on pixel centers.
            let front = box_sdf(p, (cx - 1.5, cy + 1.5), 3.0);
            let back = box_sdf(p, (cx + 1.5, cy - 1.5), 3.0);
            let back = if front < -0.5 {
                0.0
            } else {
                stroke(back.abs(), 1.0)
            };
            stroke(front.abs(), 1.0).max(back)
        }
    }
}

/// Distance from `p` to the segment `a`-`b`.
fn segment_distance(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (abx, aby) = (b.0 - a.0, b.1 - a.1);
    let (apx, apy) = (p.0 - a.0, p.1 - a.1);
    let t = ((apx * abx + apy * aby) / (abx * abx + aby * aby)).clamp(0.0, 1.0);
    let (dx, dy) = (apx - abx * t, apy - aby * t);
    (dx * dx + dy * dy).sqrt()
}

/// Signed distance from `p` to a square centered at `c` with half-size
/// `half` (negative inside).
fn box_sdf(p: (f64, f64), c: (f64, f64), half: f64) -> f64 {
    let qx = (p.0 - c.0).abs() - half;
    let qy = (p.1 - c.1).abs() - half;
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0)
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

    fn alpha_at(pixels: &[u8], x: i32, y: i32) -> u8 {
        pixels[((y * BUTTON_WIDTH + x) * 4 + 3) as usize]
    }

    #[test]
    fn button_glyphs_are_crisp_and_centered() {
        let px = rasterize_button(Icon::Maximize, None, ICON_FOCUSED);
        // The square's left edge (x = 13.5) lands on pixel column 13.
        assert_eq!(alpha_at(&px, 13, TITLE_BAR_HEIGHT / 2), 255);
        assert_eq!(alpha_at(&px, 12, TITLE_BAR_HEIGHT / 2), 0);
        // Inside the outline and the cell's corner stay transparent.
        assert_eq!(alpha_at(&px, BUTTON_WIDTH / 2, TITLE_BAR_HEIGHT / 2), 0);
        assert_eq!(alpha_at(&px, 0, 0), 0);

        let px = rasterize_button(Icon::Close, None, ICON_FOCUSED);
        assert!(alpha_at(&px, BUTTON_WIDTH / 2, TITLE_BAR_HEIGHT / 2) > 200); // the cross
        assert_eq!(alpha_at(&px, 0, 0), 0);

        // Hovered buttons fill their whole cell.
        let px = rasterize_button(Icon::Close, Some(CLOSE_HOVER_BG), CLOSE_HOVER_ICON);
        assert!(px.chunks(4).all(|p| p[3] == 255));
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
