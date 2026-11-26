//! What Pila's own panels share, so that they look alike: the switcher
//! (`switcher_view.rs`), the shortcuts cheatsheet and the welcome
//! (`shortcuts_view.rs`), the share picker (`share_picker.rs`) and the window
//! menu (`window_menu.rs`).
//!
//! - Their frame (`frame`): `[colors] bar-inactive`, opaque, and a pixel of
//!   `hover` around it, with the windows' corners (`rounding`); the border's
//!   a pixel wider, to follow the curve. Square where the corners' shaders
//!   didn't compile.
//! - The accent, `snap-preview` made opaque (`accent`): what's selected sits
//!   on a block of it.
//! - The dim a modal one draws over every output (`DIM`, `dim`).
//! - Their content image (`Item`, `TextKey`, `draw_text`, `ContentImage`):
//!   texts and keycaps drawn on a thread into one image the size of the
//!   panel, a single texture.
//! - One type scale (`TITLE`, `BODY`, `NOTE`, ...) and, on a panel with a
//!   title, the same room around its content (`PADDING`), the title in the
//!   same place (`title`) and a footer at its bottom (`footer`: a note).
//!   None shows key hints: their keys are in the manual. The switcher and
//!   the window menu have no title: their room is their own, tighter.

use std::collections::HashMap;

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            element::{
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                solid::SolidColorBuffer,
                Kind,
            },
            gles::GlesRenderer,
        },
    },
    utils::{Logical, Physical, Point, Rectangle, Scale, Size, Transform},
};

use crate::{
    config::{Colors, Config},
    render::PilaRenderElement,
    rounded::Corners,
};

/// A panel's border.
pub const BORDER: i32 = 1;
/// Over every output under a modal panel.
pub const DIM: [f32; 4] = [0.0, 0.0, 0.0, 0.45];
/// A keycap's height, its rounding and its padding around the label; the
/// buttons have the same rounding.
pub const CAP_HEIGHT: i32 = 22;
pub const CAP_RADIUS: i32 = 4;
const CAP_PADDING: i32 = 7;
/// Between keycaps, where a "+" goes.
const CAP_GAP: i32 = 18;

/// The room around a titled panel's content, and its title's place.
pub const PADDING: i32 = 28;
pub const TITLE_AT: i32 = 20;
pub const TITLE_HEIGHT: i32 = 34;
/// The footer: its row (a keycap's height), and the room above it.
pub const FOOTER_HEIGHT: i32 = CAP_HEIGHT;
pub const FOOTER_GAP: i32 = 32;
/// What a titled panel keeps under its content: the footer and its room.
pub const FOOTER: i32 = FOOTER_GAP + FOOTER_HEIGHT + PADDING;

/// The type scale. A panel's title; the welcome's, larger; what it says;
/// a note, dimmed; a section's heading, in the accent and capitals.
pub const TITLE: Font = Font {
    size: 22,
    bold: true,
};
pub const WELCOME_TITLE: Font = Font {
    size: 26,
    bold: true,
};
pub const BODY: Font = Font {
    size: 14,
    bold: false,
};
pub const NOTE: Font = Font {
    size: 13,
    bold: false,
};
pub const SECTION: Font = Font {
    size: 11,
    bold: true,
};
/// A keycap's label in a list of shortcuts.
pub const KEY: u8 = 13;

/// A corner radius and which corners it rounds; `None`: square.
pub type Shape = Option<(f64, Corners)>;

/// The windows' corners (`corner-radius`, `rounded-corners`), which the
/// panels have too.
pub fn rounding(config: &Config) -> Shape {
    (config.corner_radius > 0).then_some((f64::from(config.corner_radius), config.rounded_corners))
}

/// An opaque version of a premultiplied color.
pub fn opaque(color: [f32; 4]) -> [f32; 4] {
    let a = color[3];
    if a <= 0.0 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    [color[0] / a, color[1] / a, color[2] / a, 1.0]
}

/// An opaque color from an RGB one.
pub fn rgb([r, g, b]: [f32; 3]) -> [f32; 4] {
    [r, g, b, 1.0]
}

/// The accent: what's selected sits on a block of it.
pub fn accent(colors: &Colors) -> [f32; 4] {
    opaque(colors.snap_preview)
}

/// A panel's background and border, kept between frames: an unchanged
/// panel adds no damage.
#[derive(Default)]
pub struct FrameBuffers {
    background: Option<SolidColorBuffer>,
    border: Option<SolidColorBuffer>,
}

/// `slot`'s buffer, made or updated to `size` and `color`.
pub fn solid_buffer(
    slot: &mut Option<SolidColorBuffer>,
    size: Size<i32, Logical>,
    color: [f32; 4],
) -> &SolidColorBuffer {
    let buffer = slot.get_or_insert_with(|| SolidColorBuffer::new(size, color));
    buffer.update(size, color);
    buffer
}

/// The panel at `rect` (global) drawn as Pila's panels are, front to back:
/// its background, then its border around it.
pub fn frame(
    renderer: &mut GlesRenderer,
    buffers: &mut FrameBuffers,
    rect: Rectangle<i32, Logical>,
    rounding: Shape,
    colors: &Colors,
    origin: Point<i32, Logical>,
    scale: Scale<f64>,
) -> [PilaRenderElement; 2] {
    let background = solid_buffer(
        &mut buffers.background,
        rect.size,
        opaque(colors.bar_inactive),
    );
    let background = crate::render::solid_element(
        renderer,
        background,
        rect.loc - origin,
        rounding,
        1.0,
        scale,
    );
    let outer = Rectangle::new(
        rect.loc - Point::from((BORDER, BORDER)),
        rect.size + Size::from((2 * BORDER, 2 * BORDER)),
    );
    let border = solid_buffer(&mut buffers.border, outer.size, rgb(colors.hover));
    let border = crate::render::solid_element(
        renderer,
        border,
        outer.loc - origin,
        rounding.map(|(radius, corners)| (radius + f64::from(BORDER), corners)),
        1.0,
        scale,
    );
    [background, border]
}

/// One of a panel's solids: its name (its buffer's key), where it is
/// (global), its color, and its corners.
pub type Solid = (&'static str, Rectangle<i32, Logical>, [f32; 4], Shape);

/// A panel's buttons, as the cheatsheet's keys: rounded, `primary` (Share,
/// Save) in the accent, `secondary` (Cancel) with their edge and fill.
pub fn buttons(
    primary: Rectangle<i32, Logical>,
    secondary: Rectangle<i32, Logical>,
    colors: &Colors,
) -> [Solid; 3] {
    let key = Some((f64::from(CAP_RADIUS), Corners::All));
    let inner = Some((f64::from(CAP_RADIUS - 1), Corners::All));
    let inset = Rectangle::new(
        secondary.loc + Point::from((1, 1)),
        secondary.size - Size::from((2, 2)),
    );
    [
        ("primary", primary, accent(colors), key),
        ("secondary", inset, opaque(colors.bar), inner),
        ("secondary-edge", secondary, rgb(colors.hover), key),
    ]
}

/// `solids` drawn, front to back, relative to `origin`; their buffers kept
/// in `buffers` by name.
pub fn solids(
    renderer: &mut GlesRenderer,
    buffers: &mut HashMap<&'static str, SolidColorBuffer>,
    solids: impl IntoIterator<Item = Solid>,
    origin: Point<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement> {
    solids
        .into_iter()
        .map(|(name, rect, color, shape)| {
            let buffer = buffers
                .entry(name)
                .or_insert_with(|| SolidColorBuffer::new(rect.size, color));
            buffer.update(rect.size, color);
            crate::render::solid_element(renderer, buffer, rect.loc - origin, shape, 1.0, scale)
        })
        .collect()
}

/// The panel at `rect` with its border: what it covers.
pub fn with_border(rect: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
    Rectangle::new(
        rect.loc - Point::from((BORDER, BORDER)),
        rect.size + Size::from((2 * BORDER, 2 * BORDER)),
    )
}

/// The dim over the output `name` at `geo` (global), at `alpha`; its buffer
/// kept in `dims` by output.
pub fn dim(
    dims: &mut HashMap<String, SolidColorBuffer>,
    name: &str,
    geo: Rectangle<i32, Logical>,
    alpha: f32,
    origin: Point<i32, Logical>,
    scale: Scale<f64>,
) -> PilaRenderElement {
    let buffer = dims
        .entry(name.to_string())
        .or_insert_with(|| SolidColorBuffer::new(geo.size, DIM));
    buffer.update(geo.size, DIM);
    PilaRenderElement::Solid(
        smithay::backend::renderer::element::solid::SolidColorRenderElement::from_buffer(
            buffer,
            (geo.loc - origin).to_physical_precise_round(scale),
            scale,
            alpha,
            Kind::Unspecified,
        ),
    )
}

/// A font size and weight on a panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Font {
    pub size: u8,
    pub bold: bool,
}

/// How a text sits in its box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// Something drawn on a panel's content image, at a place from its
/// top-left (logical).
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// A line of text in a box `w` by `h`.
    Text {
        text: String,
        at: (i32, i32),
        w: i32,
        h: i32,
        font: Font,
        align: Align,
        color: [u32; 3],
    },
    /// Keycaps from left to right, a "+" between them, within `w`, their
    /// labels at `size` px.
    Keys {
        caps: Vec<String>,
        at: (i32, i32),
        w: i32,
        size: u8,
    },
}

/// What a content image shows: its items in the panel's palette, the
/// panel's size and the buffer scale it's drawn at.
#[derive(Debug, Clone, PartialEq)]
pub struct TextKey {
    pub items: Vec<Item>,
    /// A keycap's label, its fill, its edge and the "+" between them
    /// (`key_caps`).
    pub caps: [[u32; 3]; 4],
    pub size: (i32, i32),
    pub scale: i32,
}

/// An opaque color as the content image's key keeps it.
pub fn bits(color: [f32; 3]) -> [u32; 3] {
    color.map(f32::to_bits)
}

/// The keycaps' palette: their label, fill, edge and the "+" between them.
pub fn key_caps(colors: &Colors) -> [[u32; 3]; 4] {
    let [r, g, b, _] = opaque(colors.bar);
    [
        bits(colors.icon),
        bits([r, g, b]),
        bits(colors.hover),
        bits(colors.icon_inactive),
    ]
}

/// Draw `src` (premultiplied, its alpha `alpha`) over the pixel at `i` of
/// premultiplied BGRA `pixels`.
fn blend(pixels: &mut [u8], i: usize, bgr: [f32; 3], alpha: f32) {
    if alpha <= 0.0 {
        return;
    }
    let keep = 1.0 - alpha;
    for (c, value) in bgr.iter().enumerate() {
        let dst = f32::from(pixels[i + c]) / 255.0;
        pixels[i + c] = ((value * alpha + dst * keep) * 255.0).round() as u8;
    }
    let dst = f32::from(pixels[i + 3]) / 255.0;
    pixels[i + 3] = ((alpha + dst * keep) * 255.0).round() as u8;
}

/// A rounded rect (physical pixels) filled with `color`, anti-aliased, over
/// the image `width` wide.
fn fill_rounded(
    pixels: &mut [u8],
    width: i32,
    rect: (f32, f32, f32, f32),
    radius: f32,
    color: [f32; 3],
) {
    let (x, y, w, h) = rect;
    let height = pixels.len() as i32 / 4 / width.max(1);
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    let bgr = [color[2], color[1], color[0]];
    for py in (y.floor() as i32).max(0)..((y + h).ceil() as i32).min(height) {
        for px in (x.floor() as i32).max(0)..((x + w).ceil() as i32).min(width) {
            // Distance from the pixel's center to the rounded rect.
            let qx = (px as f32 + 0.5 - cx).abs() - w / 2.0 + radius;
            let qy = (py as f32 + 0.5 - cy).abs() - h / 2.0 + radius;
            let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
            let d = outside + qx.max(qy).min(0.0) - radius;
            let coverage = (0.5 - d).clamp(0.0, 1.0);
            blend(pixels, ((py * width + px) * 4) as usize, bgr, coverage);
        }
    }
}

/// A rendered line of text over the image at (`x`, `y`) (physical pixels).
fn blit_text(pixels: &mut [u8], width: i32, text: &crate::text::Title, x: i32, y: i32) {
    let height = pixels.len() as i32 / 4 / width.max(1);
    let rows = text.pixels.len() as i32 / 4 / text.width.max(1);
    for row in 0..rows {
        let py = y + row;
        if py < 0 || py >= height {
            continue;
        }
        for col in 0..text.width {
            let px = x + col;
            if px < 0 || px >= width {
                continue;
            }
            let from = ((row * text.width + col) * 4) as usize;
            let a = f32::from(text.pixels[from + 3]) / 255.0;
            if a <= 0.0 {
                continue;
            }
            // Premultiplied already: un-premultiply to blend.
            let bgr = [0, 1, 2].map(|c| f32::from(text.pixels[from + c]) / 255.0 / a);
            blend(pixels, ((py * width + px) * 4) as usize, bgr, a);
        }
    }
}

/// The texts' colors: text, dimmed text and the accent.
pub fn text_colors(colors: &Colors) -> ([u32; 3], [u32; 3], [u32; 3]) {
    let [r, g, b, _] = accent(colors);
    (
        bits(colors.icon),
        bits(colors.icon_inactive),
        bits([r, g, b]),
    )
}

/// A titled panel's title, at its place, within `width` (the panel's).
pub fn title(text: &str, width: i32, colors: &Colors) -> Item {
    Item::Text {
        text: text.into(),
        at: (PADDING, TITLE_AT),
        w: width - 2 * PADDING,
        h: TITLE_HEIGHT,
        font: TITLE,
        align: Align::Left,
        color: text_colors(colors).0,
    }
}

/// A titled panel's footer, its row at `y`, across a panel `width` wide: a
/// `note` at the right, dimmed (a long settings path ends in "…").
pub fn footer(note: &str, y: i32, width: i32, colors: &Colors) -> Item {
    Item::Text {
        text: note.into(),
        at: (PADDING, y + 1),
        w: width - 2 * PADDING,
        h: 20,
        font: NOTE,
        align: Align::Right,
        color: text_colors(colors).1,
    }
}

/// A keycap `cap_w` wide at (`x`, `y`) (physical pixels) with `text` on
/// it: its fill inside an edge a pixel wide.
#[allow(clippy::too_many_arguments)]
fn draw_cap(
    pixels: &mut [u8],
    width: i32,
    text: &crate::text::Title,
    x: i32,
    y: i32,
    cap_w: i32,
    scale: i32,
    [fill, edge]: [[f32; 3]; 2],
) {
    let s = scale as f32;
    let rect = (
        x as f32,
        y as f32,
        cap_w as f32,
        (CAP_HEIGHT * scale) as f32,
    );
    let radius = CAP_RADIUS as f32 * s;
    fill_rounded(pixels, width, rect, radius, edge);
    let inner = (rect.0 + s, rect.1 + s, rect.2 - 2.0 * s, rect.3 - 2.0 * s);
    fill_rounded(pixels, width, inner, radius - s, fill);
    blit_text(pixels, width, text, x + (cap_w - text.width) / 2, y);
}

/// Why a panel's content wasn't drawn (`draw_text`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoText {
    /// The fonts weren't in yet: their coming asks for it again
    /// (`Pila::draw_panels_text`).
    NoFonts,
    /// With the fonts in: not asked again until what it says changes.
    Failed,
}

impl NoText {
    /// A draw that found no fonts, when they've come since: it's asked again
    /// at once, as their coming may have found it still on its way.
    pub fn ask_again(drawn: &Result<Vec<u8>, NoText>) -> bool {
        *drawn == Err(NoText::NoFonts) && crate::text::loaded()
    }
}

impl crate::Pila {
    /// The fonts are in: the panels shown that drew no text without them
    /// draw it now (each skips one drawn or on its way).
    pub fn draw_panels_text(&mut self) {
        self.draw_shortcuts_text();
        self.draw_share_picker_text();
        self.draw_region_text();
        self.draw_color_pick_text();
        self.draw_app_shortcuts_text();
        self.draw_capture_permission_text();
        self.draw_quit_dialog_text();
    }
}

/// `key`'s items drawn into one image. On a thread: shaping and rasterizing
/// them all takes a few milliseconds.
pub fn draw_text(key: &TextKey) -> Result<Vec<u8>, NoText> {
    if !crate::text::loaded() {
        return Err(NoText::NoFonts);
    }
    draw_items(key).ok_or(NoText::Failed)
}

/// `draw_text` with the fonts loaded.
fn draw_items(key: &TextKey) -> Option<Vec<u8>> {
    use crate::text::{render_text, Style};
    let scale = key.scale;
    let (width, height) = (key.size.0 * scale, key.size.1 * scale);
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    let [label, fill, edge, plus] = key.caps.map(|c| c.map(f32::from_bits));
    for item in &key.items {
        match item {
            Item::Text {
                text,
                at,
                w,
                h,
                font,
                align,
                color,
            } => {
                let style = Style {
                    size: f32::from(font.size),
                    bold: font.bold,
                };
                let line = render_text(text, color.map(f32::from_bits), *w, *h, scale, style)?;
                let room = w * scale - line.width;
                let x = at.0 * scale
                    + match align {
                        Align::Left => 0,
                        Align::Center => room / 2,
                        Align::Right => room,
                    };
                blit_text(&mut pixels, width, &line, x, at.1 * scale);
            }
            Item::Keys { caps, at, w, size } => {
                let style = Style {
                    size: f32::from(*size),
                    bold: false,
                };
                let end = (at.0 + w) * scale;
                let mut x = at.0 * scale;
                let y = at.1 * scale;
                for (i, cap) in caps.iter().enumerate() {
                    if i > 0 {
                        let sign = render_text("+", plus, CAP_GAP, CAP_HEIGHT, scale, style)?;
                        let room = CAP_GAP * scale - sign.width;
                        blit_text(&mut pixels, width, &sign, x + room / 2, y);
                        x += CAP_GAP * scale;
                    }
                    let text = render_text(cap, label, 200, CAP_HEIGHT, scale, style)?;
                    let cap_w = (text.width + 2 * CAP_PADDING * scale).max(CAP_HEIGHT * scale);
                    // Keys that don't fit their room stop at its edge.
                    if x + cap_w > end {
                        break;
                    }
                    draw_cap(&mut pixels, width, &text, x, y, cap_w, scale, [fill, edge]);
                    x += cap_w;
                }
            }
        }
    }
    Some(pixels)
}

/// A panel's content drawn into one premultiplied ARGB8888 image
/// (`draw_text`).
pub struct ContentImage {
    pub key: TextKey,
    pixels: Vec<u8>,
    /// Made from `pixels` by the first frame that shows it.
    buffer: Option<MemoryRenderBuffer>,
}

impl ContentImage {
    pub fn new(key: TextKey, pixels: Vec<u8>) -> Self {
        Self {
            key,
            pixels,
            buffer: None,
        }
    }

    /// The image shown at `rect` (global): all of it, at its size as laid
    /// out, shown at `rect`'s size (smaller where the panel was shrunk).
    pub fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        rect: Rectangle<i32, Logical>,
        origin: Point<i32, Logical>,
        scale: Scale<f64>,
    ) -> Option<PilaRenderElement> {
        let (width, height) = self.key.size;
        let text_scale = self.key.scale;
        let pixels = &self.pixels;
        let buffer = self.buffer.get_or_insert_with(|| {
            MemoryRenderBuffer::from_slice(
                pixels,
                Fourcc::Argb8888,
                (width * text_scale, height * text_scale),
                text_scale,
                Transform::Normal,
                None,
            )
        });
        let loc: Point<i32, Physical> = (rect.loc - origin).to_physical_precise_round(scale);
        let loc = loc.to_f64();
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            loc,
            buffer,
            None,
            // All of it, at its size as laid out (without a source, Smithay
            // takes `size` for it and crops).
            Some(Rectangle::from_size(
                Size::<i32, Logical>::from((width, height)).to_f64(),
            )),
            Some(rect.size),
            Kind::Unspecified,
        )
        .ok()
        .map(PilaRenderElement::Memory)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A keycap is drawn over what's under it, its label over the cap.
    #[test]
    fn text_blends_over_what_is_under_it() {
        let mut pixels = vec![0u8; 4 * 4];
        fill_rounded(&mut pixels, 2, (0.0, 0.0, 2.0, 2.0), 0.0, [1.0, 0.0, 0.0]);
        // Opaque red, as BGRA.
        assert_eq!(&pixels[..4], &[0, 0, 255, 255]);
        let half_green = crate::text::Title {
            pixels: vec![0, 128, 0, 128],
            width: 1,
            whole: true,
        };
        blit_text(&mut pixels, 2, &half_green, 1, 1);
        let i = (2 + 1) * 4;
        assert_eq!(&pixels[i..i + 4], &[0, 128, 127, 255]);
    }
}
