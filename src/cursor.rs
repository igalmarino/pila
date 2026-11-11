//! Client-drawn cursors: surfaces set with `wl_pointer.set_cursor`, as opposed
//! to named shapes from `wp_cursor_shape_v1`.
//!
//! Backend-neutral: a backend renders the surface at `surface_position`
//! (on top of everything else, as `Kind::Cursor`) and sends it frame
//! callbacks. These are free functions over `Pila` fields so a render loop
//! can call them while it holds a mutable borrow of the backend.

use crate::sync::Locked;
use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    input::pointer::{CursorIcon, CursorImageStatus, CursorImageSurfaceData},
    reexports::wayland_server::{protocol::wl_surface::WlSurface, Resource},
    utils::{Logical, Point, Size, Transform},
    wayland::compositor::{with_states, SurfaceAttributes},
};
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};
use tracing::{debug, warn};

/// The cursor surface to draw and its top-left corner in global logical
/// coordinates (pointer position minus the hotspot), if the client set one.
pub fn surface_position(
    status: &CursorImageStatus,
    pointer: Point<f64, Logical>,
) -> Option<(WlSurface, Point<i32, Logical>)> {
    let CursorImageStatus::Surface(surface) = status else {
        return None;
    };
    if !surface.is_alive() {
        return None;
    }
    let hotspot = with_states(surface, |states| {
        states
            .data_map
            .get::<CursorImageSurfaceData>()
            .map(|attrs| attrs.locked().hotspot)
            .unwrap_or_default()
    });
    Some((surface.clone(), pointer.to_i32_round() - hotspot))
}

/// Keep the hotspot under the pointer when the client moves its buffer
/// (`wl_surface.offset` or an attach offset). Call on every commit.
pub fn handle_commit(status: &CursorImageStatus, surface: &WlSurface) {
    if !matches!(status, CursorImageStatus::Surface(cursor) if cursor == surface) {
        return;
    }
    with_states(surface, |states| {
        let Some(attrs) = states.data_map.get::<CursorImageSurfaceData>() else {
            return;
        };
        let delta = states
            .cached_state
            .get::<SurfaceAttributes>()
            .current()
            .buffer_delta
            .take();
        if let Some(delta) = delta {
            attrs.locked().hotspot -= delta;
        }
    });
}

/// Named cursor images (`wp_cursor_shape`, pila's own move/resize cursors)
/// from the xcursor theme, for backends that draw the cursor themselves:
/// bare metal always, nested and headless while it's magnified
/// (`shake_cursor.rs`; otherwise the host draws named shapes).
///
/// The theme and size come from `XCURSOR_THEME` (default `default`) and
/// `XCURSOR_SIZE` (default 24). Only the first frame of animated cursors is
/// used. Without a usable theme, a built-in arrow stands in for every shape.
///
/// On a scaled output the image is loaded at the size times the scale
/// (rounded up, `scale::buffer_scale`), so it keeps its logical size and
/// stays sharp; if the theme lacks that size, the nearest one is scaled.
/// A magnified cursor is drawn from the smallest image at least as large as
/// it shows (`get_magnified`), so it stays sharp too as far as the theme goes.
///
/// Reading and parsing the files is a one-time cost paid off the event loop:
/// `load` starts a thread that reads every shape at every nominal size up to
/// `LARGEST_IMAGE` times the size. Until it's done the built-in arrow is
/// drawn; a frame never reads a file.
pub struct CursorTheme {
    loaded: Arc<OnceLock<Loaded>>,
    /// The size asked for (`XCURSOR_SIZE`), in logical pixels.
    size: u32,
    cache: HashMap<(CursorIcon, i32), NamedCursor>,
    /// Images for magnified cursors, by shape and nominal size.
    magnified: HashMap<(CursorIcon, u32), MagnifiedCursor>,
    /// The built-in arrow, made once: a buffer made again would be damage.
    arrow: NamedCursor,
}

/// The largest images kept, in times the size: buffer scale 3 at the
/// largest magnification.
const LARGEST_IMAGE: f64 = 3.0 * *crate::config::SHAKE_MAGNIFICATIONS.end();

/// Every shape `wp_cursor_shape` has.
const SHAPES: [CursorIcon; 35] = [
    CursorIcon::Default,
    CursorIcon::ContextMenu,
    CursorIcon::Help,
    CursorIcon::Pointer,
    CursorIcon::Progress,
    CursorIcon::Wait,
    CursorIcon::Cell,
    CursorIcon::Crosshair,
    CursorIcon::Text,
    CursorIcon::VerticalText,
    CursorIcon::Alias,
    CursorIcon::Copy,
    CursorIcon::Move,
    CursorIcon::NoDrop,
    CursorIcon::NotAllowed,
    CursorIcon::Grab,
    CursorIcon::Grabbing,
    CursorIcon::EResize,
    CursorIcon::NResize,
    CursorIcon::NeResize,
    CursorIcon::NwResize,
    CursorIcon::SResize,
    CursorIcon::SeResize,
    CursorIcon::SwResize,
    CursorIcon::WResize,
    CursorIcon::EwResize,
    CursorIcon::NsResize,
    CursorIcon::NeswResize,
    CursorIcon::NwseResize,
    CursorIcon::ColResize,
    CursorIcon::RowResize,
    CursorIcon::AllScroll,
    CursorIcon::ZoomIn,
    CursorIcon::ZoomOut,
    CursorIcon::DndAsk,
];

/// A cursor image as read from the theme (the first frame of one nominal
/// size), before it's a render buffer.
struct RawImage {
    pixels: Vec<u8>,
    size: (i32, i32),
    /// The size the theme made it for, in pixels.
    nominal: u32,
    hotspot: (i32, i32),
}

/// What the loading thread read: per shape, its file's images smallest
/// first (shared by the shapes read from the same file); a shape the theme
/// has nothing for isn't there.
type Loaded = HashMap<CursorIcon, Arc<Vec<RawImage>>>;

/// A cursor image and its hotspot.
#[derive(Clone)]
pub struct NamedCursor {
    pub buffer: MemoryRenderBuffer,
    pub hotspot: Point<i32, Logical>,
}

/// A cursor image for drawing magnified, at buffer scale 1: it's shown at
/// the theme's size times the magnification (`render::named_cursor_elements`).
#[derive(Clone)]
pub struct MagnifiedCursor {
    pub buffer: MemoryRenderBuffer,
    /// In pixels of the image.
    pub size: Size<i32, Logical>,
    pub hotspot: Point<i32, Logical>,
    /// The size the theme made it for, in pixels: shown at `scale` logical
    /// pixels per pixel of the image (`MagnifiedCursor::scale`).
    nominal: u32,
}

impl MagnifiedCursor {
    /// Logical pixels per pixel of the image, for a theme of `size` drawn
    /// `factor` times bigger.
    pub fn scale(&self, size: u32, factor: f64) -> f64 {
        f64::from(size) * factor / f64::from(self.nominal.max(1))
    }
}

impl CursorTheme {
    pub fn load() -> Self {
        let name = std::env::var("XCURSOR_THEME")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "default".into());
        let size = std::env::var("XCURSOR_SIZE")
            .ok()
            .and_then(|s| s.parse().ok())
            .filter(|&size| size > 0)
            .unwrap_or(24);
        let loaded = Arc::new(OnceLock::new());
        let result = loaded.clone();
        let spawned = std::thread::Builder::new()
            .name("pila-cursors".into())
            .spawn(move || {
                let start = std::time::Instant::now();
                let _ = result.set(read_theme(&name, size));
                debug!(elapsed = ?start.elapsed(), theme = name, "cursor theme loaded");
            });
        if let Err(err) = spawned {
            warn!(?err, "can't load the cursor theme; using a built-in arrow");
        }
        Self::new(loaded, size)
    }

    fn new(loaded: Arc<OnceLock<Loaded>>, size: u32) -> Self {
        Self {
            loaded,
            size,
            cache: Default::default(),
            magnified: Default::default(),
            arrow: builtin_arrow(),
        }
    }

    /// The size asked for (`XCURSOR_SIZE`), in logical pixels.
    pub fn size(&self) -> u32 {
        self.size
    }

    /// The images of `icon`, else of the default arrow; `None` until the
    /// theme is loaded or if it has neither.
    fn images(&self, icon: CursorIcon) -> Option<&[RawImage]> {
        let loaded = self.loaded.get()?;
        loaded
            .get(&icon)
            .or_else(|| loaded.get(&CursorIcon::Default))
            .map(|images| images.as_slice())
    }

    /// The image for `icon` at buffer scale `scale`, falling back to its
    /// alternative names, the default arrow, and finally the built-in arrow.
    pub fn get(&mut self, icon: CursorIcon, scale: i32) -> NamedCursor {
        if let Some(cursor) = self.cache.get(&(icon, scale)) {
            return cursor.clone();
        }
        // Not loaded yet: the arrow for now, not kept.
        if self.loaded.get().is_none() {
            return self.arrow.clone();
        }
        let size = self.size;
        let cursor = self
            .images(icon)
            .and_then(|images| pick_image(images, size, scale))
            .map(|image| image.to_cursor(size))
            .unwrap_or_else(|| self.arrow.clone());
        self.cache.insert((icon, scale), cursor.clone());
        cursor
    }

    /// The image for `icon` drawn `factor` times bigger on an output of
    /// `scale`: the smallest at least that large, else the largest, with the
    /// same fallbacks as `get`.
    pub fn get_magnified(&mut self, icon: CursorIcon, scale: f64, factor: f64) -> MagnifiedCursor {
        let wanted = (f64::from(self.size) * scale * factor).ceil() as u32;
        let Some(image) = self.images(icon).and_then(|images| {
            images
                .iter()
                .find(|image| image.nominal >= wanted)
                .or(images.last())
        }) else {
            // The built-in arrow, made for the size.
            return MagnifiedCursor {
                buffer: self.arrow.buffer.clone(),
                size: arrow_size(),
                hotspot: self.arrow.hotspot,
                nominal: self.size,
            };
        };
        let nominal = image.nominal;
        if let Some(cursor) = self.magnified.get(&(icon, nominal)) {
            return cursor.clone();
        }
        let cursor = image.to_magnified();
        self.magnified.insert((icon, nominal), cursor.clone());
        cursor
    }
}

impl RawImage {
    fn buffer(&self, buffer_scale: i32) -> MemoryRenderBuffer {
        // The file's byte order is little-endian ARGB32, i.e. DRM's Argb8888.
        MemoryRenderBuffer::from_slice(
            &self.pixels,
            Fourcc::Argb8888,
            self.size,
            buffer_scale,
            Transform::Normal,
            None,
        )
    }

    /// As a cursor of `size` logical pixels: an image of twice the size is
    /// buffer scale 2.
    fn to_cursor(&self, size: u32) -> NamedCursor {
        let buffer_scale = image_buffer_scale(self.nominal, size);
        NamedCursor {
            buffer: self.buffer(buffer_scale),
            hotspot: (self.hotspot.0 / buffer_scale, self.hotspot.1 / buffer_scale).into(),
        }
    }

    fn to_magnified(&self) -> MagnifiedCursor {
        MagnifiedCursor {
            buffer: self.buffer(1),
            size: self.size.into(),
            hotspot: self.hotspot.into(),
            nominal: self.nominal,
        }
    }
}

/// Every shape of theme `name` at `size`, at every nominal size up to
/// `LARGEST_IMAGE` times it (on the loading thread).
fn read_theme(name: &str, size: u32) -> Loaded {
    let theme = xcursor::CursorTheme::load(name);
    let largest = (f64::from(size) * LARGEST_IMAGE).ceil() as u32;
    let mut files: HashMap<String, Option<Arc<Vec<RawImage>>>> = Default::default();
    let mut loaded = Loaded::default();
    for icon in SHAPES {
        let images = std::iter::once(icon.name())
            .chain(icon.alt_names().iter().copied())
            .chain(["default", "left_ptr"])
            .find_map(|name| {
                files
                    .entry(name.to_string())
                    .or_insert_with(|| {
                        let data = std::fs::read(theme.load_icon(name)?).ok()?;
                        let images = first_frames(&xcursor::parser::parse_xcursor(&data)?, largest);
                        (!images.is_empty()).then(|| Arc::new(images))
                    })
                    .clone()
            });
        if let Some(images) = images {
            loaded.insert(icon, images);
        }
    }
    loaded
}

/// The first frame of each nominal size of a file's `images`, up to
/// `largest`, smallest first.
fn first_frames(images: &[xcursor::parser::Image], largest: u32) -> Vec<RawImage> {
    let mut frames: Vec<RawImage> = Vec::new();
    for image in images {
        if image.size > largest || frames.iter().any(|f| f.nominal == image.size) {
            continue;
        }
        frames.push(RawImage {
            pixels: image.pixels_rgba.clone(),
            size: (image.width as i32, image.height as i32),
            nominal: image.size,
            hotspot: (image.xhot as i32, image.yhot as i32),
        });
    }
    // A file with only larger images: its smallest.
    if frames.is_empty() {
        if let Some(image) = images.iter().min_by_key(|image| image.size) {
            frames.extend(first_frames(images, image.size));
        }
    }
    frames.sort_by_key(|frame| frame.nominal);
    frames
}

/// The image of `images` (smallest first) for `size` at buffer scale
/// `scale`: the nominal size closest to the one asked for.
fn pick_image(images: &[RawImage], size: u32, scale: i32) -> Option<&RawImage> {
    let wanted = size * scale.max(1) as u32;
    images
        .iter()
        .min_by_key(|image| image.nominal.abs_diff(wanted))
}

/// The buffer scale for an xcursor image of nominal size `image_size` meant
/// to be `size` logical pixels: a 48px image for size 24 is scale 2.
fn image_buffer_scale(image_size: u32, size: u32) -> i32 {
    ((image_size as f64 / size.max(1) as f64).round() as i32).max(1)
}

/// `builtin_arrow`'s rows: `X` outline, `.` fill, ` ` transparent; the
/// hotspot is the tip.
const ARROW: [&str; 17] = [
    "X",
    "XX",
    "X.X",
    "X..X",
    "X...X",
    "X....X",
    "X.....X",
    "X......X",
    "X.......X",
    "X........X",
    "X.....XXXXX",
    "X..X..X",
    "X.X X..X",
    "XX  X..X",
    "X    X..X",
    "     X..X",
    "      XX",
];
/// `ARROW`'s width.
const ARROW_WIDTH: usize = 11;

/// The size of `builtin_arrow`, in pixels.
fn arrow_size() -> Size<i32, Logical> {
    (ARROW_WIDTH as i32, ARROW.len() as i32).into()
}

/// A plain black-and-white arrow, for systems without a cursor theme.
fn builtin_arrow() -> NamedCursor {
    const W: usize = ARROW_WIDTH;
    let mut pixels = vec![0u8; W * ARROW.len() * 4];
    for (y, row) in ARROW.iter().enumerate() {
        for (x, c) in row.bytes().enumerate() {
            // Argb8888 is little-endian: B, G, R, A.
            let bgra = match c {
                b'X' => [0x00, 0x00, 0x00, 0xff],
                b'.' => [0xff, 0xff, 0xff, 0xff],
                _ => continue,
            };
            let i = (y * W + x) * 4;
            pixels[i..i + 4].copy_from_slice(&bgra);
        }
    }
    NamedCursor {
        buffer: MemoryRenderBuffer::from_slice(
            &pixels,
            Fourcc::Argb8888,
            (W as i32, ARROW.len() as i32),
            1,
            Transform::Normal,
            None,
        ),
        hotspot: (0, 0).into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A theme of size 24 whose loading thread hasn't finished.
    fn unloaded() -> CursorTheme {
        CursorTheme::new(Arc::new(OnceLock::new()), 24)
    }

    /// An image of nominal size `nominal` with its hotspot at `hotspot`.
    fn image(nominal: u32, hotspot: (i32, i32)) -> RawImage {
        let side = nominal as i32;
        RawImage {
            pixels: vec![0; (side * side * 4) as usize],
            size: (side, side),
            nominal,
            hotspot,
        }
    }

    /// `unloaded`, loaded with a text cursor of 24, 48 and 72 pixels.
    fn loaded() -> CursorTheme {
        let theme = unloaded();
        let mut loaded = Loaded::default();
        let images = vec![image(24, (2, 3)), image(48, (6, 8)), image(72, (12, 15))];
        loaded.insert(CursorIcon::Text, Arc::new(images));
        let _ = theme.loaded.set(loaded);
        theme
    }

    #[test]
    fn until_the_theme_is_loaded_the_arrow_stands_in_and_isnt_kept() {
        let mut theme = unloaded();
        let cursor = theme.get(CursorIcon::Text, 1);
        assert_eq!(cursor.hotspot, (0, 0).into());
        assert!(theme.cache.is_empty());
        let mut theme = loaded();
        assert_eq!(theme.get(CursorIcon::Text, 1).hotspot, (2, 3).into());
        // At buffer scale 2 the hotspot is in logical pixels still.
        assert_eq!(theme.get(CursorIcon::Text, 2).hotspot, (3, 4).into());
        // A larger scale than the theme has: the largest image.
        assert_eq!(theme.get(CursorIcon::Text, 4).hotspot, (4, 5).into());
        // A shape the theme lacks, and no default arrow: the built-in one, kept.
        assert_eq!(theme.get(CursorIcon::Wait, 1).hotspot, (0, 0).into());
        assert_eq!(theme.cache.len(), 4);
    }

    #[test]
    fn a_magnified_cursor_is_drawn_from_an_image_at_least_as_large() {
        let mut theme = loaded();
        // 24 x 1.5 = 36: the 48px image, a 0.75 logical pixel per pixel.
        let cursor = theme.get_magnified(CursorIcon::Text, 1.0, 1.5);
        assert_eq!(cursor.size, (48, 48).into());
        assert_eq!(cursor.hotspot, (6, 8).into());
        assert_eq!(cursor.scale(theme.size(), 1.5), 0.75);
        // 24 x 2 = 48, exactly.
        assert_eq!(theme.get_magnified(CursorIcon::Text, 1.0, 2.0).size.w, 48);
        // 24 x 2 x 3 = 144: larger than any, the largest, scaled up.
        let cursor = theme.get_magnified(CursorIcon::Text, 2.0, 3.0);
        assert_eq!(cursor.size.w, 72);
        assert_eq!(cursor.scale(theme.size(), 3.0), 1.0);
        assert_eq!(theme.magnified.len(), 2);
        // Without an image, the built-in arrow at the theme's size times it.
        let arrow = theme.get_magnified(CursorIcon::Wait, 1.0, 3.0);
        assert_eq!(arrow.size, arrow_size());
        assert_eq!(arrow.scale(theme.size(), 3.0), 3.0);
    }

    #[test]
    fn a_file_keeps_the_first_frame_of_each_size_up_to_the_largest() {
        let frame = |size: u32, xhot: u32| xcursor::parser::Image {
            size,
            width: size,
            height: size,
            xhot,
            yhot: 0,
            delay: 0,
            pixels_rgba: vec![0; (size * size * 4) as usize],
            pixels_argb: vec![0; (size * size * 4) as usize],
        };
        let images = [frame(48, 1), frame(24, 1), frame(24, 2), frame(96, 1)];
        let frames = first_frames(&images, 72);
        let kept: Vec<(u32, i32)> = frames.iter().map(|f| (f.nominal, f.hotspot.0)).collect();
        assert_eq!(kept, [(24, 1), (48, 1)]);
        // Only larger ones than that: the smallest.
        let frames = first_frames(&images, 10);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].nominal, 24);
    }

    #[test]
    fn cursor_images_keep_their_logical_size() {
        assert_eq!(image_buffer_scale(24, 24), 1);
        assert_eq!(image_buffer_scale(48, 24), 2);
        // A theme without 48px images: the 32px one, unscaled.
        assert_eq!(image_buffer_scale(32, 24), 1);
        assert_eq!(image_buffer_scale(64, 32), 2);
    }
}
