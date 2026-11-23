//! Screenshots for the desktop portal Pila serves (`portal.rs`): an app that
//! may take one unasked (it doesn't want to choose, and xdg-desktop-portal
//! says its permission store allows it) gets every output at once, the whole
//! layout as it shows, at the largest output scale; otherwise the share
//! picker asks what to take (`share_picker.rs`: an output or a window, the
//! window alone as screen captures draw it). The pointer isn't in it.
//!
//! The picture is drawn offscreen and read back (`Captures::read_back`, once
//! the GPU is done: `Pila::when_copied`), then encoded as a PNG and written
//! on a thread, into `$XDG_RUNTIME_DIR/pila-screenshots` (the last
//! `SCREENSHOTS_KEPT` kept); the app gets its `file://` URI.

use std::path::{Path, PathBuf};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{element::AsRenderElements, gles::GlesRenderer},
    },
    utils::{Logical, Physical, Point, Rectangle, Scale, Size},
};
use tracing::{info, warn};

use crate::{
    capture::{Source, Target},
    portal::{Refused, Reply},
    render::{self, PilaRenderElement},
    scale::output_scale,
    Pila,
};

/// How many screenshots are kept in their directory: the oldest go as new
/// ones come.
const SCREENSHOTS_KEPT: usize = 10;

/// A screenshot asked for, waiting for the picker.
pub struct Shot {
    /// Who asks (`portal::Request::Screenshot`), and the window it asks
    /// from.
    pub app: Option<String>,
    pub parent: Option<crate::portal::ParentWindow>,
    pub reply: Reply<Result<String, Refused>>,
}

impl Pila {
    /// The portal asks for a screenshot: taken now, of every output, or
    /// once the user picks what (`ask`).
    pub(crate) fn portal_screenshot(&mut self, ask: bool, shot: Shot) {
        if self.is_locked() {
            return shot.reply.send(Err(Refused::Cancelled));
        }
        if ask {
            self.screencast
                .waiting
                .push_back(crate::share_picker::Ask::Screenshot(shot));
            self.show_share_picker();
        } else {
            self.take_screenshot(None, shot.reply);
        }
    }

    /// Take a screenshot of `target` (`None`: every output, the whole
    /// layout) and answer `reply` with its file's URI once it's written.
    pub(crate) fn take_screenshot(
        &mut self,
        target: Option<Target>,
        reply: Reply<Result<String, Refused>>,
    ) {
        if self.is_locked() {
            return reply.send(Err(Refused::Cancelled));
        }
        let Some((size, scale)) = self.screenshot_geometry(target.as_ref()) else {
            return reply.send(Err(Refused::Cancelled));
        };
        if size.w <= 0 || size.h <= 0 {
            return reply.send(Err(Refused::Cancelled));
        }
        let input = self.frame_input();
        let outputs: Vec<(String, Rectangle<i32, Logical>)> = self
            .space
            .outputs()
            .filter_map(|o| Some((o.name(), self.space.output_geometry(o)?)))
            .collect();
        let window = match &target {
            Some(Target::Toplevel(identifier)) => {
                let Some(window) = self.window_for_identifier(identifier) else {
                    return reply.send(Err(Refused::Cancelled));
                };
                Some(window)
            }
            _ => None,
        };
        let shown: Vec<(String, Rectangle<i32, Logical>)> = match &target {
            None => outputs.clone(),
            Some(Target::Output(output)) => {
                let name = output.upgrade().map(|o| o.name());
                outputs
                    .iter()
                    .filter(|(n, _)| Some(n) == name.as_ref())
                    .cloned()
                    .collect()
            }
            // A region: its output, drawn from the region's corner.
            Some(region @ Target::Region(..)) => {
                let Some(crate::capture::TargetArea { output, geo, .. }) = self.target_area(region)
                else {
                    return reply.send(Err(Refused::Cancelled));
                };
                vec![(output.name(), geo)]
            }
            _ => Vec::new(),
        };
        let origin = match &target {
            Some(region @ Target::Region(..)) => self
                .target_area(region)
                .map(|a| a.area.loc)
                .unwrap_or_default(),
            _ => shown
                .iter()
                .map(|(_, geo)| *geo)
                .reduce(|a, b| a.merge(b))
                .map(|r| r.loc)
                .unwrap_or_default(),
        };
        let clear = match window {
            Some(_) => crate::capture::TRANSPARENT,
            None => render::clear_color(&input.scene),
        };
        let Pila {
            backend,
            blur,
            capture,
            ..
        } = self;
        let Some(backend) = backend.as_mut() else {
            return reply.send(Err(Refused::Failed));
        };
        let Some(mut renderer) = backend.renderer() else {
            return reply.send(Err(Refused::Failed));
        };
        let renderer = &mut *renderer;
        let mut elements: Vec<PilaRenderElement> = Vec::new();
        match &window {
            Some(window) => {
                // The buffer's origin is the window geometry's top-left.
                let loc: Point<i32, Physical> =
                    (Point::default() - window.geometry().loc).to_physical_precise_round(scale);
                elements.extend(AsRenderElements::<GlesRenderer>::render_elements::<
                    PilaRenderElement,
                >(window, renderer, loc, scale, 1.0));
            }
            None => {
                for (name, geo) in &shown {
                    elements.extend(render::output_elements(
                        renderer,
                        blur,
                        &input.scene,
                        name,
                        *geo,
                        origin,
                        scale,
                    ));
                }
            }
        }
        let pixels = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let filled = pixels.clone();
        let (w, h) = (size.w as usize, size.h as usize);
        let copied = capture.read_back(
            renderer,
            size,
            scale,
            Source::Elements {
                elements: &elements,
                clear,
            },
            Fourcc::Abgr8888,
            move |read| {
                let mut pixels = filled.borrow_mut();
                pixels.resize(w * h * 4, 0);
                read.write_into(&mut pixels, 0, w * 4)
            },
        );
        let copied = match copied {
            Ok(copied) => copied,
            Err(err) => {
                failed(err);
                return reply.send(Err(Refused::Failed));
            }
        };
        self.when_copied(
            copied,
            || true,
            move |_, result| {
                if let Err(err) = result {
                    failed(err);
                    return reply.send(Err(Refused::Failed));
                }
                let pixels = std::mem::take(&mut *pixels.borrow_mut());
                save_on_a_thread(pixels, (w as u32, h as u32), reply);
            },
        );
    }

    /// The size (physical pixels) and scale a screenshot of `target` is
    /// taken at (`None`: every output's layout, at the largest scale).
    fn screenshot_geometry(
        &self,
        target: Option<&Target>,
    ) -> Option<(Size<i32, Physical>, Scale<f64>)> {
        if let Some(target) = target {
            return self.capture_geometry(target, None);
        }
        let outputs: Vec<_> = self
            .space
            .outputs()
            .filter_map(|o| Some((self.space.output_geometry(o)?, output_scale(o))))
            .collect();
        let bounds = outputs
            .iter()
            .map(|(geo, _)| *geo)
            .reduce(|a, b| a.merge(b))?;
        let scale = outputs.iter().map(|(_, scale)| scale.x).fold(1.0, f64::max);
        let scale = Scale::from(scale);
        Some((
            bounds.size.to_f64().to_physical(scale).to_i32_round(),
            scale,
        ))
    }
}

/// A screenshot's drawing or reading back failed: logged.
fn failed(err: crate::capture::CopyError) {
    match err {
        crate::capture::CopyError::Render(err) => warn!(err, "portal: the screenshot failed"),
        _ => warn!("portal: the screenshot failed"),
    }
}

/// Encode `pixels` (RGBA, `size`) as a PNG and write it, on a thread, then
/// answer `reply` with its URI.
fn save_on_a_thread(pixels: Vec<u8>, size: (u32, u32), reply: Reply<Result<String, Refused>>) {
    let Some(dir) = screenshots_dir() else {
        warn!("portal: no XDG_RUNTIME_DIR to save the screenshot in");
        return reply.send(Err(Refused::Failed));
    };
    let spawned = std::thread::Builder::new()
        .name("pila-screenshot".into())
        .spawn(move || match save(&dir, &pixels, size) {
            Ok(path) => {
                info!(path = %path.display(), "portal: screenshot taken");
                reply.send(Ok(file_uri(&path)));
            }
            Err(err) => {
                warn!(%err, "portal: can't save the screenshot");
                reply.send(Err(Refused::Failed));
            }
        });
    if let Err(err) = spawned {
        warn!(?err, "portal: can't save the screenshot");
    }
}

/// Where screenshots go: `$XDG_RUNTIME_DIR/pila-screenshots`, gone at
/// logout.
fn screenshots_dir() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty())?;
    Some(Path::new(&runtime).join("pila-screenshots"))
}

/// Write `pixels` as a new PNG in `dir` (made, private, if it isn't
/// there), dropping the oldest past `SCREENSHOTS_KEPT`: its path.
fn save(dir: &Path, pixels: &[u8], (width, height): (u32, u32)) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().map_err(std::io::Error::other)?;
        writer
            .write_image_data(pixels)
            .map_err(std::io::Error::other)?;
    }
    // Unique and in order: the time, then a count where two share it.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let mut path = dir.join(format!("screenshot-{stamp}.png"));
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("screenshot-{stamp}-{n}.png"));
        n += 1;
    }
    std::fs::write(&path, png)?;
    prune(dir);
    Ok(path)
}

/// Keep the newest `SCREENSHOTS_KEPT` screenshots in `dir`.
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut shots: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("screenshot-"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    shots.sort();
    let extra = shots.len().saturating_sub(SCREENSHOTS_KEPT);
    for (_, path) in shots.into_iter().take(extra) {
        let _ = std::fs::remove_file(path);
    }
}

/// `path` as a `file://` URI, its bytes percent-encoded but for the
/// unreserved ones and `/`.
fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut uri = String::from("file://");
    for &byte in path.as_os_str().as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                uri.push(byte as char)
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_a_file_uri() {
        assert_eq!(
            file_uri(Path::new("/run/user/1000/pila-screenshots/a b.png")),
            "file:///run/user/1000/pila-screenshots/a%20b.png"
        );
        assert_eq!(file_uri(Path::new("/tmp/ñ")), "file:///tmp/%C3%B1");
    }

    /// Saved as a PNG of its size, the oldest dropped past what's kept.
    #[test]
    fn screenshots_are_saved_and_the_oldest_dropped() {
        let dir = std::env::temp_dir().join(format!("pila-shots-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let pixels = [255u8, 0, 0, 255].repeat(6);
        let first = save(&dir, &pixels, (3, 2)).unwrap();
        let decoder = png::Decoder::new(std::io::BufReader::new(
            std::fs::File::open(&first).unwrap(),
        ));
        let reader = decoder.read_info().unwrap();
        assert_eq!((reader.info().width, reader.info().height), (3, 2));
        for _ in 0..SCREENSHOTS_KEPT + 2 {
            save(&dir, &pixels, (3, 2)).unwrap();
        }
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), SCREENSHOTS_KEPT);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
