//! Client-drawn cursors: surfaces set with `wl_pointer.set_cursor`, as opposed
//! to named shapes from `wp_cursor_shape_v1`.
//!
//! Backend-neutral: a backend renders the surface at `surface_position`
//! (on top of everything else, as `Kind::Cursor`) and sends it frame
//! callbacks. These are free functions over `Pila` fields so a render loop
//! can call them while it holds a mutable borrow of the backend.

use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    input::pointer::{CursorIcon, CursorImageStatus, CursorImageSurfaceData},
    reexports::wayland_server::{protocol::wl_surface::WlSurface, Resource},
    utils::{Logical, Point, Transform},
    wayland::compositor::{with_states, SurfaceAttributes},
};

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
            .map(|attrs| attrs.lock().unwrap().hotspot)
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
            attrs.lock().unwrap().hotspot -= delta;
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
pub struct CursorTheme {
    theme: xcursor::CursorTheme,
    size: u32,
    cache: std::collections::HashMap<(CursorIcon, i32), NamedCursor>,
}

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
        Self {
            theme: xcursor::CursorTheme::load(&name),
            size,
            cache: Default::default(),
        }
    }

    /// The image for `icon` at buffer scale `scale`, falling back to its
    /// alternative names, the default arrow, and finally the built-in arrow.
    pub fn get(&mut self, icon: CursorIcon, scale: i32) -> NamedCursor {
        if let Some(cursor) = self.cache.get(&(icon, scale)) {
            return cursor.clone();
        }
        let cursor = std::iter::once(icon.name())
            .chain(icon.alt_names().iter().copied())
            .chain(["default", "left_ptr"])
            .find_map(|name| self.load_image(name, scale))
            .unwrap_or_else(builtin_arrow);
        self.cache.insert((icon, scale), cursor.clone());
        cursor
    }

    fn load_image(&self, name: &str, scale: i32) -> Option<NamedCursor> {
        let path = self.theme.load_icon(name)?;
        let data = std::fs::read(path).ok()?;
        let images = xcursor::parser::parse_xcursor(&data)?;
        // The nominal size closest to the one asked for; its first frame.
        let wanted = self.size * scale as u32;
        let image = images
            .iter()
            .min_by_key(|image| image.size.abs_diff(wanted))?;
        // The buffer scale that keeps it about `size` logical pixels.
        let buffer_scale = image_buffer_scale(image.size, self.size);
        // The file's byte order is little-endian ARGB32, i.e. DRM's Argb8888.
        let buffer = MemoryRenderBuffer::from_slice(
            &image.pixels_rgba,
            Fourcc::Argb8888,
            (image.width as i32, image.height as i32),
            buffer_scale,
            Transform::Normal,
            None,
        );
        Some(NamedCursor {
            buffer,
            hotspot: (
                image.xhot as i32 / buffer_scale,
                image.yhot as i32 / buffer_scale,
            )
                .into(),
        })
    }
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
    use super::image_buffer_scale;

    #[test]
    fn cursor_images_keep_their_logical_size() {
        assert_eq!(image_buffer_scale(24, 24), 1);
        assert_eq!(image_buffer_scale(48, 24), 2);
        // A theme without 48px images: the 32px one, unscaled.
        assert_eq!(image_buffer_scale(32, 24), 1);
        assert_eq!(image_buffer_scale(64, 32), 2);
    }
}
