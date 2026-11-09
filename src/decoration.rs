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
            gles::GlesRenderer,
        },
    },
    desktop::Window,
    reexports::wayland_protocols::xdg::{
        decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
        shell::server::xdg_toplevel,
    },
    utils::{Logical, Physical, Point, Rectangle, Scale, Transform},
    wayland::{compositor::with_states, shell::xdg::XdgToplevelSurfaceData},
};
use tracing::warn;

use crate::{
    config::Colors,
    grabs::resize_grab::ResizeEdge,
    render::PilaRenderElement,
    rounded::{self, Corners, RoundedSolid},
};

pub const TITLE_BAR_HEIGHT: i32 = 28;
/// Width of each button cell (clickable and hover-highlighted), at the right
/// end of the bar. Cells are the full bar height.
const BUTTON_WIDTH: i32 = 36;
/// Minimize, maximize and close, from left to right at the bar's right end.
const BUTTONS: i32 = 3;
/// Space between the bar's left end and the title, and between the title and
/// the buttons.
const TITLE_PADDING: i32 = 10;
/// Narrower than this, the title isn't drawn at all.
const MIN_TITLE_WIDTH: i32 = 24;
/// The widest title texture: more than any screen shows. A client can make
/// its window far wider (a subsurface stretches the geometry's bounds), and
/// the texture is width x height x 4 bytes.
const MAX_TITLE_WIDTH: i32 = 8192;
/// Width of the invisible resize border outside the window and title bar.
const BORDER: i32 = 6;
/// Distance from a corner, along each edge, that resizes diagonally.
const CORNER: i32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleBarPart {
    Drag,
    Minimize,
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
    // X11 windows get pila's decorations unless they draw their own
    // (Motif hints) or are menus/tooltips.
    if let Some(x11) = window.x11_surface() {
        return !x11.is_decorated() && !x11.is_override_redirect();
    }
    window.toplevel().is_some_and(|t| {
        t.with_pending_state(|state| state.decoration_mode == Some(DecorationMode::ServerSide))
    })
}

/// Whether a window rule hides pila's title bar (`title-bar = false`): the
/// window stays server-side decorated, so the client draws none either.
fn bar_hidden(window: &Window) -> bool {
    crate::rules::with_window_props(window, |props| props.title_bar == Some(false))
}

/// Whether a title bar is drawn: the client acked server-side decorations,
/// isn't fullscreen and no rule hides it.
pub fn has_title_bar(window: &Window) -> bool {
    if bar_hidden(window) {
        return false;
    }
    if let Some(x11) = window.x11_surface() {
        return is_ssd(window) && !x11.is_fullscreen();
    }
    window.toplevel().is_some_and(|t| {
        t.with_committed_state(|state| {
            state.is_some_and(|state| {
                state.decoration_mode == Some(DecorationMode::ServerSide)
                    && !state.states.contains(xdg_toplevel::State::Fullscreen)
            })
        })
    })
}

/// Height taken above a window at `loc` (its geometry origin) by its title bar,
/// for layout: 0 unless the window is server-side decorated, not fullscreen
/// and its bar isn't hidden by a rule.
pub fn layout_offset(window: &Window, fullscreen: bool) -> i32 {
    if is_ssd(window) && !fullscreen && !bar_hidden(window) {
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
    } else if from_right <= 3.0 * BUTTON_WIDTH as f64 {
        TitleBarPart::Minimize
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
    if is_maximized(window) {
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
    Minimize,
    Maximize,
    /// Shown on the maximize button of a maximized window.
    Restore,
}

/// A rasterized button and what it was drawn for, so it's only redrawn
/// (and re-uploaded) when the icon or its colors change.
struct ButtonTexture {
    /// Icon, background, icon color, buffer scale and the rounded corner it's
    /// cut by (in buffer pixels; the close button's, at the bar's top-right).
    key: (Icon, Option<[f32; 3]>, [f32; 3], i32, i32),
    buffer: MemoryRenderBuffer,
}

/// One window's title bar buffers, kept in its user data so the damage
/// tracker sees the same elements from frame to frame. Buffers can't be
/// shared between windows: element ids must be unique within a frame.
#[derive(Default)]
struct TitleBarBuffers {
    bar: SolidColorBuffer,
    minimize: Option<ButtonTexture>,
    maximize: Option<ButtonTexture>,
    close: Option<ButtonTexture>,
    title: Option<TitleTexture>,
}

/// The rendered title and what it was rendered for (text, width, color,
/// buffer scale), so it's only re-shaped when one of those changes.
struct TitleTexture {
    key: (String, i32, [f32; 3], i32),
    buffer: MemoryRenderBuffer,
    /// Width of the rendered text in logical pixels, for centering.
    width: i32,
}

/// Where a title `text_width` wide starts in a bar at `bar_x`, `bar_width`
/// wide: centered on the bar, but slid left as needed to clear the buttons,
/// and never left of the padding.
fn title_x(bar_x: i32, bar_width: i32, text_width: i32) -> i32 {
    let centered = bar_x + (bar_width - text_width) / 2;
    let rightmost = bar_x + bar_width - BUTTONS * BUTTON_WIDTH - TITLE_PADDING - text_width;
    centered.min(rightmost).max(bar_x + TITLE_PADDING)
}

/// Whether the window is drawn maximized (its committed xdg state, or X11's
/// `_NET_WM_STATE`).
fn is_maximized(window: &Window) -> bool {
    if let Some(x11) = window.x11_surface() {
        return x11.is_maximized();
    }
    window.toplevel().is_some_and(|t| {
        t.with_committed_state(|state| {
            state.is_some_and(|s| s.states.contains(xdg_toplevel::State::Maximized))
        })
    })
}

/// The window's title as set by the client (`xdg_toplevel.set_title`, or
/// the X11 window's `_NET_WM_NAME`/`WM_NAME`).
fn window_title(window: &Window) -> Option<String> {
    if let Some(x11) = window.x11_surface() {
        return Some(x11.title()).filter(|t| !t.is_empty());
    }
    let toplevel = window.toplevel()?;
    with_states(toplevel.wl_surface(), |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()?
            .lock()
            .ok()?
            .title
            .clone()
    })
}

/// Render elements for a title bar at `rect` (relative to the output origin),
/// front to back, drawn at `scale`. The textures are rasterized at that
/// scale rounded up (`scale::buffer_scale`), so they stay sharp on HiDPI
/// outputs; a window spanning outputs of different scales re-rasterizes
/// them for each.
#[allow(clippy::too_many_arguments)]
pub fn title_bar_elements(
    renderer: &mut GlesRenderer,
    window: &Window,
    rect: Rectangle<i32, Logical>,
    focused: bool,
    hover: Option<TitleBarPart>,
    colors: &Colors,
    radius: Option<f64>,
    alpha: f32,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement> {
    // Rounded top corners, if the shaders for them work (`rounded.rs`).
    let shaders = radius.and_then(|_| rounded::Shaders::get(renderer));
    let radius = radius.filter(|_| shaders.is_some());
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
        minimize,
        maximize,
        close,
        title,
    } = &mut *buffers;

    let maximized = is_maximized(window);
    let buffer_scale = crate::scale::buffer_scale(scale);
    let icon_color = if focused {
        colors.icon
    } else {
        colors.icon_inactive
    };
    // Background (none: the bar shows through) and icon color.
    let button_colors = |part| match (hover == Some(part), part) {
        (true, TitleBarPart::Close) => (Some(colors.close_hover), colors.close_hover_icon),
        (true, _) => (Some(colors.hover), colors.hover_icon),
        (false, _) => (None, icon_color),
    };
    let close_loc = Point::from((rect.loc.x + rect.size.w - BUTTON_WIDTH, rect.loc.y));
    let maximize_loc = close_loc - Point::from((BUTTON_WIDTH, 0));
    let minimize_loc = maximize_loc - Point::from((BUTTON_WIDTH, 0));
    let maximize_icon = if maximized {
        Icon::Restore
    } else {
        Icon::Maximize
    };

    let mut elements = Vec::with_capacity(4);
    let upload =
        |renderer: &mut GlesRenderer, buffer: &MemoryRenderBuffer, loc: Point<i32, Logical>| {
            let loc: Point<f64, Physical> = loc.to_f64().to_physical(scale);
            MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                loc,
                buffer,
                (alpha < 1.0).then_some(alpha),
                None,
                None,
                Kind::Unspecified,
            )
            .map_err(|err| warn!(?err, "failed to upload title bar texture"))
            .ok()
        };

    // The title: at most the space between the left padding and the buttons,
    // centered on the bar where it fits (see `title_x`).
    let title_width =
        (rect.size.w - BUTTONS * BUTTON_WIDTH - 2 * TITLE_PADDING).min(MAX_TITLE_WIDTH);
    match window_title(window) {
        Some(text) if title_width >= MIN_TITLE_WIDTH && !text.is_empty() => {
            let key = (text, title_width, icon_color, buffer_scale);
            if title.as_ref().is_none_or(|texture| texture.key != key) {
                // None until the fonts have loaded; then the next frame draws it.
                *title = crate::text::render_title(
                    &key.0,
                    icon_color,
                    title_width,
                    TITLE_BAR_HEIGHT,
                    buffer_scale,
                )
                .map(|(pixels, width)| TitleTexture {
                    buffer: MemoryRenderBuffer::from_slice(
                        &pixels,
                        Fourcc::Argb8888,
                        (width, TITLE_BAR_HEIGHT * buffer_scale),
                        buffer_scale,
                        Transform::Normal,
                        None,
                    ),
                    width: width / buffer_scale,
                    key,
                });
            }
            if let Some(texture) = title.as_ref() {
                let x = title_x(rect.loc.x, rect.size.w, texture.width);
                let loc = Point::from((x, rect.loc.y));
                elements.extend(upload(renderer, &texture.buffer, loc).map(Into::into));
            }
        }
        _ => *title = None,
    }

    for (slot, icon, part, loc) in [
        (close, Icon::Close, TitleBarPart::Close, close_loc),
        (
            maximize,
            maximize_icon,
            TitleBarPart::Maximize,
            maximize_loc,
        ),
        (
            minimize,
            Icon::Minimize,
            TitleBarPart::Minimize,
            minimize_loc,
        ),
    ] {
        let corner = match (part, radius) {
            (TitleBarPart::Close, Some(radius)) => {
                (radius * f64::from(buffer_scale)).round() as i32
            }
            _ => 0,
        };
        let (background, color) = button_colors(part);
        let key = (icon, background, color, buffer_scale, corner);
        if slot.as_ref().is_none_or(|texture| texture.key != key) {
            *slot = Some(ButtonTexture {
                key,
                buffer: button_buffer(icon, background, color, buffer_scale, corner),
            });
        }
        let buffer = &slot.as_ref().unwrap().buffer;
        elements.extend(upload(renderer, buffer, loc).map(Into::into));
    }

    bar.update(
        rect.size,
        if focused {
            colors.bar
        } else if crate::urgency::is_urgent(window) {
            colors.bar_urgent
        } else {
            colors.bar_inactive
        },
    );
    let loc: Point<i32, Physical> = rect.loc.to_physical_precise_round(scale);
    let bar = SolidColorRenderElement::from_buffer(bar, loc, scale, alpha, Kind::Unspecified);
    elements.push(match (radius, &shaders) {
        (Some(radius), Some(shaders)) => {
            RoundedSolid::new(bar, shaders, radius, Corners::Top, scale).into()
        }
        _ => bar.into(),
    });
    elements
}

/// Draw one button cell (`BUTTON_WIDTH` x `TITLE_BAR_HEIGHT` logical pixels,
/// at `scale` physical pixels each): a background (transparent unless
/// hovered, so the bar shows through) and an anti-aliased 10px glyph in the
/// middle. A nonzero `corner` (buffer pixels) rounds off the top-right
/// corner: the close button's, when windows have rounded corners.
fn button_buffer(
    icon: Icon,
    background: Option<[f32; 3]>,
    color: [f32; 3],
    scale: i32,
    corner: i32,
) -> MemoryRenderBuffer {
    let pixels = rasterize_button(icon, background, color, scale, corner);
    MemoryRenderBuffer::from_slice(
        &pixels,
        Fourcc::Argb8888,
        (BUTTON_WIDTH * scale, TITLE_BAR_HEIGHT * scale),
        scale,
        Transform::Normal,
        None,
    )
}

/// Premultiplied ARGB8888 pixels (little-endian: B, G, R, A) for a button,
/// `scale` physical pixels per logical one, with the top-right corner
/// rounded by `corner` physical pixels.
fn rasterize_button(
    icon: Icon,
    background: Option<[f32; 3]>,
    color: [f32; 3],
    scale: i32,
    corner: i32,
) -> Vec<u8> {
    let radius = f64::from(corner) / f64::from(scale);
    let (w, h) = (BUTTON_WIDTH * scale, TITLE_BAR_HEIGHT * scale);
    let s = scale as f64;
    // Glyph center, in logical pixels. At scale 1, outline edges land on
    // pixel centers, so 1px strokes are crisp; at 2, on pixel edges, where
    // 2px strokes are.
    let (cx, cy) = (BUTTON_WIDTH as f64 / 2.0, TITLE_BAR_HEIGHT as f64 / 2.0);
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let p = ((x as f64 + 0.5) / s, (y as f64 + 0.5) / s);
            let shape = rounded::top_right_coverage(p, f64::from(BUTTON_WIDTH), radius, s) as f32;
            let coverage = glyph_coverage(icon, p, (cx, cy), s) as f32 * shape;
            let (bg, bg_alpha) = match background {
                Some(bg) => (bg, shape),
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

/// How much of the pixel centered at `p` the glyph covers (0..=1). `p` is
/// in logical pixels; a physical pixel is `1 / scale` of one.
fn glyph_coverage(icon: Icon, p: (f64, f64), (cx, cy): (f64, f64), scale: f64) -> f64 {
    // A stroke `width` wide along a shape at distance `dist` from p,
    // anti-aliased over one physical pixel.
    let stroke = |dist: f64, width: f64| ((width / 2.0 - dist) * scale + 0.5).clamp(0.0, 1.0);
    match icon {
        Icon::Close => {
            let r = 4.5;
            let d1 = segment_distance(p, (cx - r, cy - r), (cx + r, cy + r));
            let d2 = segment_distance(p, (cx + r, cy - r), (cx - r, cy + r));
            stroke(d1.min(d2), 1.4)
        }
        Icon::Maximize => stroke(box_sdf(p, (cx, cy), 4.5).abs(), 1.0),
        // A line along the bottom of where the maximize square sits.
        Icon::Minimize => stroke(
            segment_distance(p, (cx - 4.5, cy + 4.5), (cx + 4.5, cy + 4.5)),
            1.0,
        ),
        Icon::Restore => {
            // A front square, and the part of a back square (up and right)
            // that isn't hidden behind it. Together they span the same 10px
            // as the maximize square, with edges on pixel centers.
            let front = box_sdf(p, (cx - 1.5, cy + 1.5), 3.0);
            let back = box_sdf(p, (cx + 1.5, cy - 1.5), 3.0);
            let back = if front < -0.5 / scale {
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
            TitleBarPart::Minimize
        );
        assert_eq!(
            title_bar_part(bar(), (right - 3.0 * BUTTON_WIDTH as f64 - 1.0, y).into()),
            TitleBarPart::Drag
        );
        assert_eq!(title_bar_part(bar(), (101.0, y).into()), TitleBarPart::Drag);
    }

    fn alpha_at(pixels: &[u8], x: i32, y: i32) -> u8 {
        pixels[((y * BUTTON_WIDTH + x) * 4 + 3) as usize]
    }

    fn alpha_at_2x(pixels: &[u8], x: i32, y: i32) -> u8 {
        pixels[((y * BUTTON_WIDTH * 2 + x) * 4 + 3) as usize]
    }

    #[test]
    fn button_glyphs_are_crisp_and_centered() {
        let px = rasterize_button(Icon::Maximize, None, Colors::default().icon, 1, 0);
        // The square's left edge (x = 13.5) lands on pixel column 13.
        assert_eq!(alpha_at(&px, 13, TITLE_BAR_HEIGHT / 2), 255);
        assert_eq!(alpha_at(&px, 12, TITLE_BAR_HEIGHT / 2), 0);
        // Inside the outline and the cell's corner stay transparent.
        assert_eq!(alpha_at(&px, BUTTON_WIDTH / 2, TITLE_BAR_HEIGHT / 2), 0);
        assert_eq!(alpha_at(&px, 0, 0), 0);

        let px = rasterize_button(Icon::Close, None, Colors::default().icon, 1, 0);
        assert!(alpha_at(&px, BUTTON_WIDTH / 2, TITLE_BAR_HEIGHT / 2) > 200); // the cross
        assert_eq!(alpha_at(&px, 0, 0), 0);

        // Hovered buttons fill their whole cell.
        let colors = Colors::default();
        let px = rasterize_button(
            Icon::Close,
            Some(colors.close_hover),
            colors.close_hover_icon,
            1,
            0,
        );
        assert!(px.chunks(4).all(|p| p[3] == 255));
    }

    #[test]
    fn buttons_at_scale_2_have_crisp_2px_strokes() {
        let px = rasterize_button(Icon::Maximize, None, Colors::default().icon, 2, 0);
        assert_eq!(px.len() as i32, BUTTON_WIDTH * 2 * TITLE_BAR_HEIGHT * 2 * 4);
        // The left edge (logical x = 13.5, 1px wide) covers columns 26-27.
        let y = TITLE_BAR_HEIGHT;
        assert_eq!(alpha_at_2x(&px, 25, y), 0);
        assert_eq!(alpha_at_2x(&px, 26, y), 255);
        assert_eq!(alpha_at_2x(&px, 27, y), 255);
        assert_eq!(alpha_at_2x(&px, 28, y), 0);
    }

    #[test]
    fn titles_center_on_the_bar_but_clear_the_buttons() {
        let buttons = BUTTONS * BUTTON_WIDTH;
        // Short title on a wide bar: centered on the whole bar.
        assert_eq!(title_x(100, 600, 80), 100 + (600 - 80) / 2);
        // Wider title: centering would overlap the buttons, so it slides left
        // until it ends TITLE_PADDING before them.
        let x = title_x(100, 300, 140);
        assert_eq!(x + 140, 100 + 300 - buttons - TITLE_PADDING);
        assert!(x < 100 + (300 - 140) / 2);
        // A title filling all the space starts at the left padding.
        let full = 300 - buttons - 2 * TITLE_PADDING;
        assert_eq!(title_x(100, 300, full), 100 + TITLE_PADDING);
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
