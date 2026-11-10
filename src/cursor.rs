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
    utils::{Logical, Point, Transform},
    wayland::compositor::{with_states, SurfaceAttributes},
};
use std::sync::{Arc, OnceLock};
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
/// from the xcursor theme, for backends that draw the cursor themselves. The
/// nested backend doesn't need it: the host draws named shapes.
///
/// The theme and size come from `XCURSOR_THEME` (default `default`) and
/// `XCURSOR_SIZE` (default 24). Only the first frame of animated cursors is
/// used. Without a usable theme, a built-in arrow stands in for every shape.
///
/// On a scaled output the image is loaded at the size times the scale
/// (rounded up, `scale::buffer_scale`), so it keeps its logical size and
/// stays sharp; if the theme lacks that size, the nearest one is scaled.
///
/// Reading and parsing the files is a one-time cost paid off the event loop:
/// `load` starts a thread that reads every shape at buffer scales up to
/// `PRELOADED_SCALES`. Until it's done (and for a larger scale, at the
/// nearest loaded one) the built-in arrow is drawn; a frame never reads a file.
pub struct CursorTheme {
    loaded: Arc<OnceLock<Loaded>>,
    cache: std::collections::HashMap<(CursorIcon, i32), NamedCursor>,
}

/// The buffer scales whose images are read at startup.
const PRELOADED_SCALES: i32 = 3;

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

/// A cursor image as read from the theme, before it's a render buffer.
struct RawCursor {
    pixels: Vec<u8>,
    size: (i32, i32),
    buffer_scale: i32,
    hotspot: (i32, i32),
}

/// What the loading thread read: per shape and buffer scale, `None` where the
/// theme has nothing for it.
type Loaded = std::collections::HashMap<(CursorIcon, i32), Option<RawCursor>>;

/// A cursor image and its hotspot.
#[derive(Clone)]
pub struct NamedCursor {
    pub buffer: MemoryRenderBuffer,
    pub hotspot: Point<i32, Logical>,
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
        Self {
            loaded,
            cache: Default::default(),
        }
    }

    /// The image for `icon` at buffer scale `scale`, falling back to its
    /// alternative names, the default arrow, and finally the built-in arrow.
    pub fn get(&mut self, icon: CursorIcon, scale: i32) -> NamedCursor {
        if let Some(cursor) = self.cache.get(&(icon, scale)) {
            return cursor.clone();
        }
        // Not loaded yet: the arrow for now, not kept.
        let Some(loaded) = self.loaded.get() else {
            return builtin_arrow();
        };
        let at = scale.clamp(1, PRELOADED_SCALES);
        let cursor = loaded
            .get(&(icon, at))
            .or_else(|| loaded.get(&(CursorIcon::Default, at)))
            .and_then(Option::as_ref)
            .map(RawCursor::to_cursor)
            .unwrap_or_else(builtin_arrow);
        self.cache.insert((icon, scale), cursor.clone());
        cursor
    }
}

impl RawCursor {
    fn to_cursor(&self) -> NamedCursor {
        // The file's byte order is little-endian ARGB32, i.e. DRM's Argb8888.
        let buffer = MemoryRenderBuffer::from_slice(
            &self.pixels,
            Fourcc::Argb8888,
            self.size,
            self.buffer_scale,
            Transform::Normal,
            None,
        );
        NamedCursor {
            buffer,
            hotspot: self.hotspot.into(),
        }
    }
}

/// Every shape of theme `name` at `size`, at every preloaded buffer scale
/// (on the loading thread).
fn read_theme(name: &str, size: u32) -> Loaded {
    let theme = xcursor::CursorTheme::load(name);
    let mut files: std::collections::HashMap<String, Option<Vec<xcursor::parser::Image>>> =
        Default::default();
    let mut loaded = Loaded::default();
    for icon in SHAPES {
        for scale in 1..=PRELOADED_SCALES {
            let image = std::iter::once(icon.name())
                .chain(icon.alt_names().iter().copied())
                .chain(["default", "left_ptr"])
                .find_map(|name| {
                    let images = files.entry(name.to_string()).or_insert_with(|| {
                        let data = std::fs::read(theme.load_icon(name)?).ok()?;
                        xcursor::parser::parse_xcursor(&data)
                    });
                    pick_image(images.as_deref()?, size, scale)
                });
            loaded.insert((icon, scale), image);
        }
    }
    loaded
}

/// The image of a file's `images` for `size` at buffer scale `scale`: the
/// nominal size closest to the one asked for, its first frame.
fn pick_image(images: &[xcursor::parser::Image], size: u32, scale: i32) -> Option<RawCursor> {
    let wanted = size * scale as u32;
    let image = images
        .iter()
        .min_by_key(|image| image.size.abs_diff(wanted))?;
    // The buffer scale that keeps it about `size` logical pixels.
    let buffer_scale = image_buffer_scale(image.size, size);
    Some(RawCursor {
        pixels: image.pixels_rgba.clone(),
        size: (image.width as i32, image.height as i32),
        buffer_scale,
        hotspot: (
            image.xhot as i32 / buffer_scale,
            image.yhot as i32 / buffer_scale,
        ),
    })
}

/// The buffer scale for an xcursor image of nominal size `image_size` meant
/// to be `size` logical pixels: a 48px image for size 24 is scale 2.
fn image_buffer_scale(image_size: u32, size: u32) -> i32 {
    ((image_size as f64 / size.max(1) as f64).round() as i32).max(1)
}

/// A plain black-and-white arrow, for systems without a cursor theme.
fn builtin_arrow() -> NamedCursor {
    // `X` outline, `.` fill, ` ` transparent; the hotspot is the tip.
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
    const W: usize = 11;
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

    /// A theme whose loading thread hasn't finished.
    fn unloaded() -> CursorTheme {
        CursorTheme {
            loaded: Arc::new(OnceLock::new()),
            cache: Default::default(),
        }
    }

    #[test]
    fn until_the_theme_is_loaded_the_arrow_stands_in_and_isnt_kept() {
        let mut theme = unloaded();
        let cursor = theme.get(CursorIcon::Text, 1);
        assert_eq!(cursor.hotspot, (0, 0).into());
        assert!(theme.cache.is_empty());
        let mut loaded = Loaded::default();
        loaded.insert(
            (CursorIcon::Text, 1),
            Some(RawCursor {
                pixels: vec![0; 4 * 4 * 4],
                size: (4, 4),
                buffer_scale: 1,
                hotspot: (2, 3),
            }),
        );
        loaded.insert(
            (CursorIcon::Text, PRELOADED_SCALES),
            Some(RawCursor {
                pixels: vec![0; 4 * 4 * 4],
                size: (4, 4),
                buffer_scale: 1,
                hotspot: (4, 5),
            }),
        );
        let _ = theme.loaded.set(loaded);
        assert_eq!(theme.get(CursorIcon::Text, 1).hotspot, (2, 3).into());
        // A larger scale than was read: the largest one.
        assert_eq!(theme.get(CursorIcon::Text, 4).hotspot, (4, 5).into());
        // A shape the theme lacks: the arrow, kept.
        assert_eq!(theme.get(CursorIcon::Wait, 1).hotspot, (0, 0).into());
        assert_eq!(theme.cache.len(), 3);
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
