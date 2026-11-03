//! Window title text for title bars, shaped and rasterized with cosmic-text.
//!
//! Loading the system fonts is slow (cosmic-text says up to a second in
//! release builds and ten times that in debug), so `init` does it on a
//! background thread. Until it's done `render_title` returns `None` and title
//! bars are drawn without text; the next frame tries again.

use std::sync::{Mutex, OnceLock};

use cosmic_text::{
    Attrs, Buffer, Color, Ellipsize, EllipsizeHeightLimit, Family, FontSystem, Metrics, Shaping,
    SwashCache, Wrap,
};
use tracing::{info, warn};

/// Title font size, in logical pixels.
pub const FONT_SIZE: f32 = 13.0;

struct Fonts {
    system: FontSystem,
    cache: SwashCache,
}

static FONTS: OnceLock<Mutex<Fonts>> = OnceLock::new();

/// Start loading the system fonts in the background. Call once at startup.
pub fn init() {
    let spawned = std::thread::Builder::new()
        .name("pila-fonts".into())
        .spawn(|| {
            let start = std::time::Instant::now();
            let fonts = Fonts {
                system: FontSystem::new(),
                cache: SwashCache::new(),
            };
            let _ = FONTS.set(Mutex::new(fonts));
            info!(elapsed = ?start.elapsed(), "fonts loaded");
        });
    if let Err(err) = spawned {
        warn!(
            ?err,
            "failed to start font loading; title bars will have no text"
        );
    }
}

/// `title` in `color` as one line at most `max_width` wide (ending in "…" if
/// it doesn't fit), vertically centered in `height` (both logical), drawn
/// at `scale` physical pixels per logical one. Returns premultiplied
/// ARGB8888 pixels (little-endian: B, G, R, A) cropped to the text, and
/// their width in physical pixels (a multiple of `scale`), so callers can
/// position it. `None` until the fonts have loaded.
pub fn render_title(
    title: &str,
    color: [f32; 3],
    max_width: i32,
    height: i32,
    scale: i32,
) -> Option<(Vec<u8>, i32)> {
    let width = max_width * scale;
    let height = height * scale;
    let fonts = FONTS.get()?;
    let mut fonts = fonts.lock().ok()?;
    let Fonts { system, cache } = &mut *fonts;

    // A line as tall as the bar: cosmic-text centers the glyphs within it.
    let mut buffer = Buffer::new(
        system,
        Metrics::new(FONT_SIZE * scale as f32, height as f32),
    );
    let mut buffer = buffer.borrow_with(system);
    buffer.set_wrap(Wrap::None);
    buffer.set_ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)));
    buffer.set_size(Some(width as f32), Some(height as f32));
    // Titles are one line; newlines and other control characters become spaces.
    let title: String = title
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    buffer.set_text(
        &title,
        &Attrs::new().family(Family::SansSerif),
        Shaping::Advanced,
        None,
    );

    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let text_color = Color::rgb(byte(color[0]), byte(color[1]), byte(color[2]));
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    buffer.draw(cache, text_color, |x, y, w, h, color| {
        let (r, g, b, a) = color.as_rgba_tuple();
        if a == 0 {
            return;
        }
        let a = a as f32 / 255.0;
        for py in y.max(0)..(y + h as i32).min(height) {
            for px in x.max(0)..(x + w as i32).min(width) {
                let i = ((py * width + px) * 4) as usize;
                // Source over destination, premultiplied.
                let blend = |src: u8, dst: u8| {
                    byte(src as f32 / 255.0 * a + dst as f32 / 255.0 * (1.0 - a))
                };
                pixels[i] = blend(b, pixels[i]);
                pixels[i + 1] = blend(g, pixels[i + 1]);
                pixels[i + 2] = blend(r, pixels[i + 2]);
                pixels[i + 3] = byte(a + pixels[i + 3] as f32 / 255.0 * (1.0 - a));
            }
        }
    });

    // Crop to the laid-out text, plus a pixel for anti-aliasing overhang.
    let text_width = buffer
        .layout_runs()
        .map(|run| run.line_w)
        .fold(0.0, f32::max);
    // A whole number of logical pixels, so the buffer's logical size is exact.
    let cropped = ((text_width.ceil() as i32) + 1).clamp(1, width);
    let cropped = ((cropped + scale - 1) / scale * scale).min(width);
    let pixels = pixels
        .chunks_exact((width * 4) as usize)
        .flat_map(|row| &row[..(cropped * 4) as usize])
        .copied()
        .collect();
    Some((pixels, cropped))
}
