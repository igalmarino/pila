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
use tracing::{debug, info, warn};

/// Title font size, in logical pixels.
pub const FONT_SIZE: f32 = 14.0;

struct Fonts {
    system: FontSystem,
    cache: SwashCache,
}

static FONTS: OnceLock<Mutex<Fonts>> = OnceLock::new();

/// Set once loading the fonts found none (or couldn't start): no text will
/// ever be drawn, so what's only text (the shortcuts panel) isn't shown.
static UNAVAILABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The tests' way to open panels before the fonts are in
/// (`PILA_HEADLESS_HOLD_FONTS=1`): the loaded fonts wait here until
/// `release_held` (the control socket's `fonts_release`).
static HELD: (Mutex<bool>, std::sync::Condvar) = (Mutex::new(true), std::sync::Condvar::new());

/// Let fonts held by `PILA_HEADLESS_HOLD_FONTS=1` be put in place.
pub fn release_held() {
    if let Ok(mut held) = HELD.0.lock() {
        *held = false;
    }
    HELD.1.notify_all();
}

/// Wait for `release_held` if the tests asked to hold the fonts back.
fn wait_if_held() {
    if !std::env::var_os("PILA_HEADLESS_HOLD_FONTS").is_some_and(|v| v == "1") {
        return;
    }
    debug!("fonts held until the tests release them");
    let Ok(held) = HELD.0.lock() else {
        return;
    };
    drop(HELD.1.wait_while(held, |held| *held));
}

/// Start loading the system fonts in the background. Call once at startup.
/// `ready` runs on that thread once they're in place (to redraw title bars),
/// or once it's known there are none (`unavailable`).
pub fn init(ready: impl FnOnce() + Send + 'static) {
    let spawned = std::thread::Builder::new()
        .name("pila-fonts".into())
        .spawn(move || {
            let start = std::time::Instant::now();
            let none = || {
                UNAVAILABLE.store(true, std::sync::atomic::Ordering::Relaxed);
                warn!("no fonts found; title bars will have no text");
            };
            // The tests' way to a system with no fonts (CI's has none).
            if std::env::var_os("PILA_HEADLESS_NO_FONTS").is_some_and(|v| v == "1") {
                none();
                ready();
                return;
            }
            let mut system = FontSystem::new();
            let family = follow_fontconfig(&mut system);
            let loaded = start.elapsed();
            let Some(fonts) = prepare(system) else {
                none();
                ready();
                return;
            };
            wait_if_held();
            let _ = FONTS.set(Mutex::new(fonts));
            info!(
                elapsed = ?loaded,
                warm_up = ?(start.elapsed() - loaded),
                family,
                "fonts loaded"
            );
            ready();
        });
    if let Err(err) = spawned {
        UNAVAILABLE.store(true, std::sync::atomic::Ordering::Relaxed);
        warn!(
            ?err,
            "failed to start font loading; title bars will have no text"
        );
    }
}

/// Make `system`'s sans-serif the family fontconfig gives (`fc-match`, the
/// one the tools Pila starts draw their text in: waybar, fuzzel, mako,
/// swaylock), not cosmic-text's own ("Open Sans", whether installed or not);
/// the family it then has. Without `fc-match` (package fontconfig), or with a
/// family the database lacks, it stays cosmic-text's. On the fonts' thread.
fn follow_fontconfig(system: &mut FontSystem) -> String {
    let matched = std::process::Command::new("fc-match")
        .args(["--format=%{family[0]}", "sans-serif"])
        .stderr(std::process::Stdio::null())
        .output();
    match matched {
        Ok(out) if out.status.success() => {
            let family = String::from_utf8_lossy(&out.stdout);
            if !use_sans_serif(system, family.trim()) {
                debug!(family = %family.trim(), "fontconfig's sans-serif isn't in the font database");
            }
        }
        Ok(out) => debug!(status = %out.status, "fc-match failed; cosmic-text's sans-serif"),
        Err(err) => debug!(%err, "no fc-match (package fontconfig); cosmic-text's sans-serif"),
    }
    system.db().family_name(&Family::SansSerif).to_string()
}

/// Make `family` `system`'s sans-serif if the database has it; whether it
/// did.
fn use_sans_serif(system: &mut FontSystem, family: &str) -> bool {
    let known = !family.is_empty()
        && system.db().faces().any(|face| {
            face.families
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(family))
        });
    if known {
        system.db_mut().set_sans_serif_family(family);
    }
    known
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
        let _ = render_with(
            &mut fonts,
            WARM_UP_TEXT,
            [1.0; 3],
            4000,
            28,
            scale,
            Style::default(),
        );
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

impl Title {
    /// Its texture, `height` high (logical) at `scale`.
    pub fn buffer(
        &self,
        height: i32,
        scale: i32,
    ) -> smithay::backend::renderer::element::memory::MemoryRenderBuffer {
        smithay::backend::renderer::element::memory::MemoryRenderBuffer::from_slice(
            &self.pixels,
            smithay::backend::allocator::Fourcc::Argb8888,
            (self.width, height * scale),
            scale,
            smithay::utils::Transform::Normal,
            None,
        )
    }
}

/// Draw `bgr` (premultiplied, its alpha `alpha`) over the pixel at `i` of
/// premultiplied BGRA `pixels`: source over destination.
#[inline]
pub(crate) fn blend(pixels: &mut [u8], i: usize, bgr: [f32; 3], alpha: f32) {
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

/// Whether the fonts have loaded: until then nothing draws text.
pub fn loaded() -> bool {
    FONTS.get().is_some()
}

/// Whether it's known there are no fonts: no text will ever be drawn.
pub fn unavailable() -> bool {
    UNAVAILABLE.load(std::sync::atomic::Ordering::Relaxed)
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
    render_text(title, color, max_width, height, scale, Style::default())
}

/// How `render_text` draws: the font's size in logical pixels and whether
/// bold. The default is the title bars'.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub size: f32,
    pub bold: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            size: FONT_SIZE,
            bold: false,
        }
    }
}

/// `render_title` in `style`.
pub fn render_text(
    text: &str,
    color: [f32; 3],
    max_width: i32,
    height: i32,
    scale: i32,
    style: Style,
) -> Option<Title> {
    let fonts = FONTS.get()?;
    let mut fonts = fonts.lock().ok()?;
    render_with(&mut fonts, text, color, max_width, height, scale, style)
}

/// `render_text` with these fonts.
fn render_with(
    fonts: &mut Fonts,
    title: &str,
    color: [f32; 3],
    max_width: i32,
    height: i32,
    scale: i32,
    style: Style,
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
        Metrics::new(style.size * scale as f32, height as f32),
    );
    let mut buffer = buffer.borrow_with(system);
    buffer.set_wrap(Wrap::None);
    buffer.set_ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)));
    // First with no width: how wide the whole text is, which the line cut
    // to fit ("…", a few pixels short of the room) doesn't tell.
    buffer.set_size(None, Some(height as f32));
    // Titles are one line; newlines and other control characters become spaces.
    let title: String = title
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    buffer.set_text(
        &title,
        &Attrs::new()
            .family(Family::SansSerif)
            .weight(if style.bold {
                cosmic_text::Weight::BOLD
            } else {
                cosmic_text::Weight::NORMAL
            }),
        Shaping::Advanced,
        None,
    );
    let line_width = |buffer: &mut cosmic_text::BorrowedWithFontSystem<'_, Buffer>| {
        buffer
            .layout_runs()
            .map(|run| run.line_w)
            .fold(0.0, f32::max)
    };
    let full_width = line_width(&mut buffer);
    buffer.set_size(Some(width as f32), Some(height as f32));

    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let text_color = Color::rgb(byte(color[0]), byte(color[1]), byte(color[2]));
    let mut pixels = vec![0u8; bytes];
    buffer.draw(cache, text_color, |x, y, w, h, color| {
        let (r, g, b, a) = color.as_rgba_tuple();
        if a == 0 {
            return;
        }
        let a = a as f32 / 255.0;
        let bgr = [b, g, r].map(|c| c as f32 / 255.0);
        for py in y.max(0)..(y + h as i32).min(height) {
            for px in x.max(0)..(x + w as i32).min(width) {
                let i = ((py * width + px) * 4) as usize;
                blend(&mut pixels, i, bgr, a);
            }
        }
    });

    // Crop to the laid-out text, plus a pixel for anti-aliasing overhang.
    let text_width = line_width(&mut buffer);
    // A whole number of logical pixels, so the buffer's logical size is exact.
    let natural = (text_width.ceil() as i32) + 1;
    // The whole text fits, with the pixel: nothing was cut ("…").
    let whole = (full_width.ceil() as i32) < width;
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

/// Whether `text` in `style` fits on one line `max_width` logical pixels
/// wide, uncut ("…"), in fontconfig's sans-serif: for tests of text a panel
/// must show whole. `None` without fonts.
#[cfg(test)]
pub(crate) fn fits(text: &str, style: Style, max_width: i32) -> Option<bool> {
    let mut system = FontSystem::new();
    if system.db().is_empty() {
        return None;
    }
    follow_fontconfig(&mut system);
    let mut fonts = prepare(system)?;
    render_with(&mut fonts, text, [1.0; 3], max_width, 40, 1, style).map(|t| t.whole)
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

    /// Title bars draw in the sans-serif family the tools Pila starts do:
    /// fontconfig's, not cosmic-text's "Open Sans".
    #[test]
    fn the_title_font_is_fontconfigs_sans_serif() {
        let Ok(out) = std::process::Command::new("fc-match")
            .args(["--format=%{family[0]}", "sans-serif"])
            .output()
        else {
            eprintln!("no fc-match: skipped");
            return;
        };
        let expected = String::from_utf8(out.stdout).unwrap();
        let mut system = FontSystem::new();
        if system.db().is_empty() {
            eprintln!("no fonts: skipped");
            return;
        }
        assert_eq!(follow_fontconfig(&mut system), expected.trim());
        assert_eq!(system.db().family_name(&Family::SansSerif), expected.trim());
    }

    /// A family the database doesn't have leaves the one it had.
    #[test]
    fn a_family_not_installed_is_not_used() {
        let mut system = FontSystem::new_with_locale_and_db(
            "en-US".into(),
            cosmic_text::fontdb::Database::new(),
        );
        let before = system.db().family_name(&Family::SansSerif).to_string();
        assert!(!use_sans_serif(&mut system, "No Such Family"));
        assert!(!use_sans_serif(&mut system, ""));
        assert_eq!(system.db().family_name(&Family::SansSerif), before);
    }

    /// A text cut to fit ("…") isn't whole: a title bar keeps a whole title
    /// when its window widens (`decoration.rs`), and a cut one would stay cut.
    #[test]
    fn a_text_cut_to_fit_is_not_whole() {
        let style = Style {
            size: 22.0,
            bold: true,
        };
        let long = "Allow wf-recorder to capture your screen and windows?";
        let (Some(cut), Some(roomy)) = (fits(long, style, 400), fits(long, style, 2000)) else {
            eprintln!("no fonts: skipped");
            return;
        };
        assert!(!cut, "cut to 400 px, yet whole");
        assert!(roomy);
        assert_eq!(fits("short", style, 400), Some(true));
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
        assert!(render_with(&mut fonts, "a", white, i32::MAX, 30, 2, Style::default()).is_none());
        assert!(render_with(&mut fonts, "a", white, 0, 30, 1, Style::default()).is_none());
    }
}
