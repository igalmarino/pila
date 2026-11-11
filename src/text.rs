//! Window title text for title bars, shaped and rasterized with cosmic-text.
//!
//! Loading the system fonts is slow (cosmic-text says up to a second in
//! release builds and ten times that in debug), so `init` does it on a
//! background thread. Until it's done `render_title` returns `None` and title
//! bars are drawn without text; then `init`'s callback asks for a redraw.

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
/// `ready` runs on that thread once they're in place (to redraw title bars).
pub fn init(ready: impl FnOnce() + Send + 'static) {
    let spawned = std::thread::Builder::new()
        .name("pila-fonts".into())
        .spawn(move || {
            let start = std::time::Instant::now();
            let system = FontSystem::new();
            let loaded = start.elapsed();
            let Some(fonts) = prepare(system) else {
                warn!("no fonts found; title bars will have no text");
                return;
            };
            let _ = FONTS.set(Mutex::new(fonts));
            info!(elapsed = ?loaded, warm_up = ?(start.elapsed() - loaded), "fonts loaded");
            ready();
        });
    if let Err(err) = spawned {
        warn!(
            ?err,
            "failed to start font loading; title bars will have no text"
        );
    }
}

/// The fonts, warmed up: the usual characters shaped and rasterized once
/// here, off the main thread, before anyone can wait for the lock (else
/// the first title bar drawn pays for it inside a frame). `None` without a
/// single font: cosmic-text panics shaping text then (a minimal system, a
/// container), so title bars go without text.
fn prepare(system: FontSystem) -> Option<Fonts> {
    if system.db().is_empty() {
        return None;
    }
    let mut fonts = Fonts {
        system,
        cache: SwashCache::new(),
    };
    for scale in [1, 2] {
        let _ = render_with(&mut fonts, WARM_UP_TEXT, [1.0; 3], 4000, 28, scale);
    }
    Some(fonts)
}

/// Characters titles commonly use, drawn once at startup to warm the
/// shaping and glyph caches.
const WARM_UP_TEXT: &str = "abcdefghijklmnopqrstuvwxyz ABCDEFGHIJKLMNOPQRSTUVWXYZ \
     0123456789 .,:;-_/\\|()[]{}<>@#%&*+=~'\"!?… áéíóúñü";

/// A title drawn by `render_title`.
pub struct Title {
    /// Premultiplied ARGB8888 (little-endian: B, G, R, A), cropped to the
    /// text.
    pub pixels: Vec<u8>,
    /// Their width in physical pixels (a multiple of the scale), so callers
    /// can position it.
    pub width: i32,
    /// The whole text fit, with room to spare: drawn at any `max_width` at
    /// least `width` (logical), it comes out the same.
    pub whole: bool,
}

/// `title` in `color` as one line at most `max_width` wide (ending in "…" if
/// it doesn't fit), vertically centered in `height` (both logical), drawn
/// at `scale` physical pixels per logical one. `None` until the fonts have
/// loaded.
pub fn render_title(
    title: &str,
    color: [f32; 3],
    max_width: i32,
    height: i32,
    scale: i32,
) -> Option<Title> {
    let fonts = FONTS.get()?;
    let mut fonts = fonts.lock().ok()?;
    render_with(&mut fonts, title, color, max_width, height, scale)
}

/// `render_title` with these fonts.
fn render_with(
    fonts: &mut Fonts,
    title: &str,
    color: [f32; 3],
    max_width: i32,
    height: i32,
    scale: i32,
) -> Option<Title> {
    // Sizes from a client's window: nothing to draw rather than an overflow.
    let width = max_width.checked_mul(scale).filter(|w| *w > 0)?;
    let height = height.checked_mul(scale).filter(|h| *h > 0)?;
    let bytes = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)?;
    let Fonts { system, cache } = fonts;

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
    let mut pixels = vec![0u8; bytes];
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
    let natural = (text_width.ceil() as i32) + 1;
    // Short of the room by more than the pixel: nothing was cut ("…").
    let whole = natural < width;
    let cropped = natural.clamp(1, width);
    let cropped = ((cropped + scale - 1) / scale * scale).min(width);
    let pixels = pixels
        .chunks_exact((width * 4) as usize)
        .flat_map(|row| &row[..(cropped * 4) as usize])
        .copied()
        .collect();
    Some(Title {
        pixels,
        width: cropped,
        whole,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_fonts_means_no_title_text_rather_than_a_panic() {
        let empty = FontSystem::new_with_locale_and_db(
            "en-US".into(),
            cosmic_text::fontdb::Database::new(),
        );
        assert!(prepare(empty).is_none());
    }

    /// A title as wide as `i32::MAX` px (scaled, it overflows) or with no
    /// room: nothing to draw, not a panic.
    #[test]
    fn a_title_too_wide_to_draw_is_nothing() {
        let system = FontSystem::new_with_locale_and_db(
            "en-US".into(),
            cosmic_text::fontdb::Database::new(),
        );
        let mut fonts = Fonts {
            system,
            cache: SwashCache::new(),
        };
        let white = [1.0; 3];
        assert!(render_with(&mut fonts, "a", white, i32::MAX, 30, 2).is_none());
        assert!(render_with(&mut fonts, "a", white, 0, 30, 1).is_none());
    }
}
