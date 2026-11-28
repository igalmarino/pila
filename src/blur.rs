//! ext-background-effect: a surface asks for the screen behind a region of
//! it to be blurred (a translucent bar or terminal over the wallpaper and
//! windows). The region is double-buffered surface state (`BlurRegion`, applied
//! on commit by Smithay's cached state); `Pila::scene` evaluates it for each
//! layer surface (`blur_rects`) and window (`window_blur_rects`), and their
//! popups (`popup_blurs`: menus, an input method's candidate window), and the
//! backends run `BlurCache::prepare` before building an output's elements: for
//! every blurred surface it renders what's behind the region into an offscreen texture, blurs it
//! (dual Kawase: a few downsampling and upsampling passes with custom
//! texture shaders) and keeps the result, which `BlurCache::element` hands
//! to `render::scene_elements` to draw right behind that surface.
//!
//! A region is often many rectangles (clients tessellate rounded shapes into
//! strips). Everything is blurred over their bounding box, whose texture then
//! has what's outside the region cleared to transparent (`Entry::mask`).
//!
//! A backdrop's textures are a little bigger than it (`BLUR_ALLOC_STEP`) and
//! only their part in use is drawn and sampled, so a surface that changes
//! size a few pixels a frame keeps them (`fits`).
//!
//! The blur is redone only when something behind the region changed
//! (a damage tracker per region, fed the elements behind it); otherwise the
//! kept texture is drawn as is, and the output's own damage tracking skips
//! it. Upper layer surfaces, windows and their popups are blurred, not lower
//! layer surfaces, and a capture of a window has only the window.
//!
//! A window's backdrop is part of what it draws (`single_window_elements`), so
//! it goes along wherever the window is drawn into a texture: while it
//! animates, in a workspace slide, and in the picture of one that's closing.

use crate::sync::Locked;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            damage::OutputDamageTracker,
            element::{
                texture::{TextureRenderBuffer, TextureRenderElement},
                Kind,
            },
            gles::{
                ffi, GlesError, GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName,
                UniformType,
            },
            Bind, Color32F, Frame, Offscreen, Renderer, Texture,
        },
    },
    desktop::{LayerSurface, PopupKind, PopupManager, Window},
    reexports::{
        wayland_protocols::ext::background_effect::v1::server::{
            ext_background_effect_manager_v1::{
                self as manager, Capability, ExtBackgroundEffectManagerV1,
            },
            ext_background_effect_surface_v1::{self as effect, ExtBackgroundEffectSurfaceV1},
        },
        wayland_server::{
            backend::{ClientId, ObjectId},
            protocol::wl_surface::WlSurface,
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
        },
    },
    utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform},
    wayland::alpha_modifier::AlphaModifierSurfaceCachedState,
    wayland::compositor::{
        get_region_attributes, with_states, Cacheable, RectangleKind, RegionAttributes,
        SurfaceAttributes,
    },
    wayland::seat::WaylandFocus,
};
use tracing::{debug, warn};

use crate::{
    render::{self, Behind, PilaRenderElement, PopupBlur, Scene},
    rounded::{Corners, Rounding},
    Pila,
};

/// Down/up passes: each halves/doubles the size.
const BLUR_PASSES: usize = 3;
/// Sample spread per pass, in texels.
const BLUR_OFFSET: f32 = 2.5;
/// How far from what changed the passes can show it, in physical pixels: an
/// upper bound (each pass samples `BLUR_OFFSET` times two texels of its level
/// away at most, the levels doubling).
const BLUR_REACH: i32 = (2 << BLUR_PASSES) * 3;
/// A backdrop's textures are this many physical pixels bigger at a time, so
/// one that grows a few pixels a frame (a panel's animation, a window being
/// resized) is drawn in the same ones.
const BLUR_ALLOC_STEP: i32 = 128;

/// The surface's blur region, double-buffered with the surface state.
/// Shared behind an `Arc` so `commit` doesn't clone the region's rectangles
/// and `client_blur` can reuse its decomposed rectangles across frames while
/// the region and size stay the same.
#[derive(Default, Clone, Debug)]
pub struct BlurRegion(Option<Arc<RegionAttributes>>);

impl Cacheable for BlurRegion {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        self.clone()
    }
    fn merge_into(self, into: &mut Self, _dh: &DisplayHandle) {
        *into = self;
    }
}

/// Cached decomposed blur rectangles for a surface.
type BlurRects = Arc<[Rectangle<i32, Logical>]>;

/// Decomposed client blur rectangles for a surface, cached across frames
/// until a new region is committed or the surface changes size.
#[derive(Default)]
struct CachedBlurRects(Mutex<Option<BlurRectsCache>>);

struct BlurRectsCache {
    region: Arc<RegionAttributes>,
    size: Size<i32, Logical>,
    rects: BlurRects,
}

/// The surface's effect object, if it has one (one at a time).
#[derive(Default)]
struct BlurEffect(Mutex<Option<ExtBackgroundEffectSurfaceV1>>);

/// Create the global. Called once from `protocols::init_globals`.
pub fn init_global(dh: &DisplayHandle) {
    dh.create_global::<Pila, ExtBackgroundEffectManagerV1, ()>(1, ());
}

/// `surface`'s size and its decomposed client blur rectangles (clipped to the
/// surface), cached until the region or size changes; `None` without a buffer.
fn client_blur(surface: &WlSurface) -> Option<(Size<i32, Logical>, Option<BlurRects>)> {
    with_states(surface, |states| {
        let size = states
            .data_map
            .get::<smithay::backend::renderer::utils::RendererSurfaceStateUserData>()
            .and_then(|s| s.locked().surface_size())?;
        let region = states.cached_state.get::<BlurRegion>().current().0.clone();
        let Some(region) = region else {
            return Some((size, None));
        };
        let slot = states
            .data_map
            .get_or_insert_threadsafe(CachedBlurRects::default);
        let mut cache = slot.0.locked();
        if let Some(entry) = cache.as_ref() {
            if Arc::ptr_eq(&entry.region, &region) && entry.size == size {
                return Some((size, Some(Arc::clone(&entry.rects))));
            }
        }
        let rects: BlurRects = region_rects(&region, Rectangle::from_size(size)).into();
        *cache = Some(BlurRectsCache {
            region,
            size,
            rects: Arc::clone(&rects),
        });
        Some((size, Some(rects)))
    })
}

/// A client's region as rectangles, clipped to `bounds`.
/// The most rectangles a region is drawn as: past this it's drawn as its
/// bounding box.
const MAX_REGION_RECTS: usize = 64;

fn region_rects(
    region: &RegionAttributes,
    bounds: Rectangle<i32, Logical>,
) -> Vec<Rectangle<i32, Logical>> {
    // What's added, as one box: at least what the client asked for.
    let bounding = || {
        region
            .rects
            .iter()
            .filter(|(kind, _)| matches!(kind, RectangleKind::Add))
            .filter_map(|(_, rect)| rect.intersection(bounds))
            .reduce(|a, b| a.merge(b))
            .into_iter()
            .collect()
    };
    if region.rects.len() > MAX_REGION_RECTS {
        return bounding();
    }
    let mut rects: Vec<Rectangle<i32, Logical>> = Vec::new();
    for (kind, rect) in &region.rects {
        match kind {
            RectangleKind::Add => rects.extend(rect.intersection(bounds)),
            RectangleKind::Subtract => {
                rects = Rectangle::subtract_rects_many(rects, [*rect]);
                if rects.len() > MAX_REGION_RECTS {
                    return bounding();
                }
            }
        }
    }
    rects
}

/// The parts of a layer surface to blur behind, surface-local, clipped to
/// the surface. Empty without a region or a buffer.
/// Layer rules may override the client: `blur = true` blurs behind all of
/// it, `false` nowhere (`rules.rs`).
pub fn blur_rects(layer: &LayerSurface) -> Vec<Rectangle<i32, Logical>> {
    let Some((size, rects)) = client_blur(layer.wl_surface()) else {
        return Vec::new();
    };
    match crate::rules::layer_props(layer).blur {
        Some(false) => Vec::new(),
        Some(true) => vec![Rectangle::from_size(size)],
        None => rects.map_or(Vec::new(), |rects| rects.to_vec()),
    }
}

/// The parts of a window to blur behind, in space coordinates: its main surface
/// is at `render_location`, `frame` is its geometry and title bar there.
/// Window rules may override the client, as layer rules do: `blur = true` blurs
/// behind `frame`, so not behind a shadow the client draws around it.
pub fn window_blur_rects(
    window: &Window,
    render_location: Point<i32, Logical>,
    frame: Rectangle<i32, Logical>,
) -> Vec<Rectangle<i32, Logical>> {
    let Some(surface) = window.wl_surface() else {
        return Vec::new();
    };
    let rule = crate::rules::with_window_props(window, |props| props.blur);
    if rule == Some(false) {
        return Vec::new();
    }
    let Some((_, rects)) = client_blur(&surface) else {
        return Vec::new();
    };
    match (rule, rects) {
        (Some(true), _) => vec![frame],
        (_, Some(rects)) => rects
            .iter()
            .map(|rect| Rectangle::new(rect.loc + render_location, rect.size))
            .collect(),
        _ => Vec::new(),
    }
}

/// The popups of a window to blur behind (`render::PopupBlur`): those that
/// ask, unless a rule refuses it the window's blur (`blur = false`); a rule's
/// `popup-blur` decides for them alone: `true` blurs behind each one's
/// geometry (not a shadow the client draws around it) or its input region
/// there (`PopupArea::Geometry`), rounded by `popup-corner-radius`, `false`
/// behind none. `render_location` as in `window_blur_rects`.
pub fn window_popup_blurs(window: &Window, render_location: Point<i32, Logical>) -> Vec<PopupBlur> {
    let Some(toplevel) = window.toplevel() else {
        return Vec::new();
    };
    let (blur, popup_blur, radius) = crate::rules::with_window_props(window, |props| {
        (props.blur, props.popup_blur, props.popup_corner_radius)
    });
    let area = match popup_blur.or(blur.filter(|blur| !blur)) {
        Some(false) => return Vec::new(),
        Some(true) => PopupArea::Geometry {
            radius: radius.unwrap_or(0),
        },
        None => PopupArea::Client,
    };
    popup_blurs(
        toplevel.wl_surface(),
        render_location + window.geometry().loc,
        area,
    )
}

/// The popups of a layer surface at `location` (in space coordinates) that
/// ask for blur, unless a rule refuses it the layer surface's (`blur = false`).
pub fn layer_popup_blurs(layer: &LayerSurface, location: Point<i32, Logical>) -> Vec<PopupBlur> {
    if crate::rules::layer_props(layer).blur == Some(false) {
        return Vec::new();
    }
    popup_blurs(layer.wl_surface(), location, PopupArea::Client)
}

/// Where popups are blurred behind.
#[derive(Clone, Copy)]
enum PopupArea {
    /// The client's region of each (none if it asks for none).
    Client,
    /// Each one's geometry, or the part of it the popup takes input in if
    /// it set an input region (its shape: GTK's), the corners of that
    /// geometry rounded by `radius` (0: square).
    Geometry { radius: u32 },
}

/// The popups of `root` to blur behind, menus and an input method's candidate
/// window alike, their regions in space coordinates: their offsets are from
/// `origin` (a window's geometry, a layer surface's top-left), as
/// `render::popup_elements` places them. A rule's `blur = true` is for its
/// window or layer surface: a popup's is `area`.
fn popup_blurs(root: &WlSurface, origin: Point<i32, Logical>, area: PopupArea) -> Vec<PopupBlur> {
    PopupManager::popups_for_surface(root)
        .enumerate()
        .filter_map(|(index, (popup, offset))| {
            let surface = popup.wl_surface();
            let (size, rects) = client_blur(surface)?;
            // Its surface's top-left.
            let loc = origin + offset - popup.geometry().loc;
            let (rects, rounding) = match area {
                PopupArea::Client => {
                    let rects: Vec<Rectangle<i32, Logical>> = rects?
                        .iter()
                        .map(|rect| Rectangle::new(rect.loc + loc, rect.size))
                        .collect();
                    (rects, None)
                }
                PopupArea::Geometry { radius } => {
                    let frame = popup_frame(&popup, size);
                    let rects: Vec<Rectangle<i32, Logical>> = match input_region(surface) {
                        Some(input) => region_rects(&input, frame),
                        None => vec![frame],
                    }
                    .into_iter()
                    .filter(|rect| !rect.is_empty())
                    .map(|rect| Rectangle::new(rect.loc + loc, rect.size))
                    .collect();
                    let rect = Rectangle::new(loc + frame.loc, frame.size);
                    (
                        rects,
                        Rounding::new(rect, None, radius, Corners::All, false),
                    )
                }
            };
            (!rects.is_empty()).then(|| PopupBlur {
                surface: surface.clone(),
                index,
                rects,
                rounding,
            })
        })
        .collect()
}

/// The input region `surface` set, if any (unset: all of it). GTK gives a
/// popover's shape this way, arrow and rounded corners, in strips.
fn input_region(surface: &WlSurface) -> Option<RegionAttributes> {
    with_states(surface, |states| {
        states
            .cached_state
            .get::<SurfaceAttributes>()
            .current()
            .input_region
            .clone()
    })
}

/// What a popup shows, surface-local: its geometry, or all of it (`size`)
/// without one (Smithay's is then empty) or for an input method's (Smithay's
/// is the text cursor's rectangle).
fn popup_frame(popup: &PopupKind, size: Size<i32, Logical>) -> Rectangle<i32, Logical> {
    match popup {
        PopupKind::Xdg(_) => Some(popup.geometry()).filter(|geometry| !geometry.is_empty()),
        PopupKind::InputMethod(_) => None,
    }
    .unwrap_or_else(|| Rectangle::from_size(size))
}

/// Whether nothing shows through `window`'s geometry: its main surface says
/// it's opaque there, and isn't faded (wp-alpha-modifier). Such a window has
/// no use for a backdrop.
pub fn is_opaque(window: &Window) -> bool {
    let Some(surface) = window.wl_surface() else {
        return false;
    };
    let geometry = window.geometry();
    if geometry.is_empty() {
        return false;
    }
    with_states(&surface, |states| {
        let faded = states
            .cached_state
            .get::<AlphaModifierSurfaceCachedState>()
            .current()
            .multiplier()
            .is_some_and(|multiplier| multiplier < u32::MAX);
        if faded {
            return false;
        }
        let Some(state) = states
            .data_map
            .get::<smithay::backend::renderer::utils::RendererSurfaceStateUserData>()
        else {
            return false;
        };
        let state = state.locked();
        state.opaque_regions().is_some_and(|opaque| {
            Rectangle::subtract_rects_many([geometry], opaque.iter().copied()).is_empty()
        })
    })
}

/// The blur passes' own uniforms, after `render::texture_shader_prologue`,
/// and `tap`: a sample of the part of the texture in use, whose edge texels
/// go on past it (as `CLAMP_TO_EDGE` does at the texture's own edge).
const UNIFORMS: &str = "\
uniform float offset;
uniform vec2 halfpixel;
uniform vec2 uv_max;
vec4 tap(vec2 uv) {
    return texture2D(tex, min(uv, uv_max));
}
";

/// For `up_rounded`.
const SHAPE_UNIFORMS: &str = "\
uniform vec2 target_size;
uniform vec2 shape_loc;
uniform vec2 shape_size;
uniform vec4 radii;
";

const DOWN_FRAG: &str = "\
void main() {
    vec2 uv = v_coords;
    vec4 sum = tap(uv) * 4.0;
    sum += tap(uv - halfpixel * offset);
    sum += tap(uv + halfpixel * offset);
    sum += tap(uv + vec2(halfpixel.x, -halfpixel.y) * offset);
    sum += tap(uv - vec2(halfpixel.x, -halfpixel.y) * offset);
    gl_FragColor = (sum / 8.0) * alpha;
}
";

const UP_FRAG: &str = "\
void main() {
    vec2 uv = v_coords;
    vec4 sum = tap(uv + vec2(-halfpixel.x * 2.0, 0.0) * offset);
    sum += tap(uv + vec2(-halfpixel.x, halfpixel.y) * offset) * 2.0;
    sum += tap(uv + vec2(0.0, halfpixel.y * 2.0) * offset);
    sum += tap(uv + vec2(halfpixel.x, halfpixel.y) * offset) * 2.0;
    sum += tap(uv + vec2(halfpixel.x * 2.0, 0.0) * offset);
    sum += tap(uv + vec2(halfpixel.x, -halfpixel.y) * offset) * 2.0;
    sum += tap(uv + vec2(0.0, -halfpixel.y * 2.0) * offset);
    sum += tap(uv + vec2(-halfpixel.x, -halfpixel.y) * offset) * 2.0;
    gl_FragColor = (sum / 12.0) * alpha;
}
";

#[derive(Clone)]
pub struct Shaders {
    down: GlesTexProgram,
    up: GlesTexProgram,
    /// `up` for the last pass of a backdrop with rounded corners.
    up_rounded: GlesTexProgram,
}

impl Shaders {
    /// The shaders (`render::compiled_once`); `None` if they failed to
    /// compile (then there's no blur).
    pub fn get(renderer: &mut GlesRenderer) -> Option<Shaders> {
        crate::render::compiled_once(renderer, Self::compile)
    }

    fn compile(renderer: &mut GlesRenderer) -> Option<Shaders> {
        let uniforms = [
            UniformName::new("offset", UniformType::_1f),
            UniformName::new("halfpixel", UniformType::_2f),
            UniformName::new("uv_max", UniformType::_2f),
        ];
        let compile = |renderer: &mut GlesRenderer, body: &str| {
            renderer.compile_custom_texture_shader(
                format!(
                    "{}{UNIFORMS}{body}",
                    crate::render::texture_shader_prologue("mediump")
                ),
                &uniforms,
            )
        };
        let down = compile(renderer, DOWN_FRAG);
        let up = compile(renderer, UP_FRAG);
        let up_rounded = renderer.compile_custom_texture_shader(
            format!(
                "{}{UNIFORMS}{SHAPE_UNIFORMS}{}{}",
                crate::render::texture_shader_prologue("highp"),
                crate::rounded::ROUNDING_FN,
                UP_FRAG.replace(
                    "(sum / 12.0) * alpha",
                    "(sum / 12.0) * alpha * rounding_alpha(\
                     v_coords * target_size - shape_loc, shape_size, radii)",
                ),
            ),
            &[
                UniformName::new("offset", UniformType::_1f),
                UniformName::new("halfpixel", UniformType::_2f),
                UniformName::new("uv_max", UniformType::_2f),
                UniformName::new("target_size", UniformType::_2f),
                UniformName::new("shape_loc", UniformType::_2f),
                UniformName::new("shape_size", UniformType::_2f),
                UniformName::new("radii", UniformType::_4f),
            ],
        );
        match (down, up, up_rounded) {
            (Ok(down), Ok(up), Ok(up_rounded)) => Some(Shaders {
                down,
                up,
                up_rounded,
            }),
            (Err(err), _, _) | (_, Err(err), _) | (_, _, Err(err)) => {
                warn!(?err, "blur: shader compilation failed; blur disabled");
                None
            }
        }
    }
}

/// A backdrop's rounded corners, relative to its texture in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Shape {
    loc: Point<i32, Physical>,
    size: Size<i32, Physical>,
    /// Top-left, top-right, bottom-right, bottom-left.
    radii: [f32; 4],
}

/// The blurred backdrop of one surface on one output.
struct Entry {
    /// The blurred area, output-local.
    rect: Rectangle<i32, Logical>,
    /// Where it is in the global space.
    global: Point<i32, Logical>,
    scale: Scale<f64>,
    /// The region, output-local, as `inside` and `mask` were worked out from.
    rects: Vec<Rectangle<i32, Logical>>,
    /// The region in `rect`, relative to it, as drawn.
    inside: Vec<Rectangle<i32, Physical>>,
    /// What's outside the region in `rect`, relative to it: transparent.
    mask: Vec<Rectangle<i32, Physical>>,
    /// Its rounded corners, as they were cut out.
    shape: Option<Shape>,
    /// The blurred area's size in pixels: the part of level 0 in use.
    size: Size<i32, Physical>,
    /// Level 0 is at least `size`, rounded up (`capacity`); each further
    /// level half of the one before. Only the part `level_size` gives of
    /// each is drawn and sampled.
    levels: Vec<GlesTexture>,
    /// Level 0 as a render buffer: its commit is bumped when re-blurred, so
    /// the output's damage tracking repaints it.
    result: TextureRenderBuffer<GlesTexture>,
    /// What was behind the region when it was last blurred.
    tracker: OutputDamageTracker,
    /// Blurred at least once.
    drawn: bool,
    used: bool,
}

/// The output `BlurCache::prepare` is working on.
struct Target<'a> {
    renderer: &'a mut GlesRenderer,
    scene: &'a Scene,
    shaders: &'a Shaders,
    output_name: &'a str,
    output_location: Point<i32, Logical>,
    output_size: Size<i32, Logical>,
    scale: Scale<f64>,
}

/// Blurred backdrops, per surface and output (`Pila::blur`).
#[derive(Default)]
pub struct BlurCache {
    entries: HashMap<(ObjectId, String), Entry>,
    /// Backdrops allocated and passes run, for tests (`Pila::state_json`).
    allocated: u32,
    blurred: u32,
}

use crate::render::error_text as render_error;

/// The size of level `k` of a backdrop of `size`, or of its textures.
fn level_size(size: Size<i32, Physical>, k: usize) -> Size<i32, Physical> {
    ((size.w >> k).max(1), (size.h >> k).max(1)).into()
}

/// The size of the textures for a backdrop of `size`: rounded up to
/// `BLUR_ALLOC_STEP`.
fn capacity(size: Size<i32, Physical>) -> Size<i32, Physical> {
    let up = |n: i32| (n + BLUR_ALLOC_STEP - 1) / BLUR_ALLOC_STEP * BLUR_ALLOC_STEP;
    (up(size.w), up(size.h)).into()
}

/// Whether textures of `allocated` (level 0) can hold a backdrop of `size`:
/// big enough, and not over twice the size it would get now (so one that
/// shrank gives its memory back; room to grow, half as much again, always
/// fits).
fn fits(allocated: Size<i32, Physical>, size: Size<i32, Physical>) -> bool {
    let wanted = capacity(size);
    (size.w..=wanted.w * 2).contains(&allocated.w) && (size.h..=wanted.h * 2).contains(&allocated.h)
}

/// An offscreen texture that samples its edges, not black, past them.
fn blur_texture(
    renderer: &mut GlesRenderer,
    size: Size<i32, Buffer>,
) -> Result<GlesTexture, String> {
    let texture = Offscreen::<GlesTexture>::create_buffer(renderer, Fourcc::Abgr8888, size)
        .map_err(|e| render_error(&e))?;
    let id = texture.tex_id();
    renderer
        .with_context(|gl| unsafe {
            gl.BindTexture(ffi::TEXTURE_2D, id);
            gl.TexParameteri(
                ffi::TEXTURE_2D,
                ffi::TEXTURE_WRAP_S,
                ffi::CLAMP_TO_EDGE as i32,
            );
            gl.TexParameteri(
                ffi::TEXTURE_2D,
                ffi::TEXTURE_WRAP_T,
                ffi::CLAMP_TO_EDGE as i32,
            );
            // Interpolate between texels: the passes blur by sampling
            // between them.
            gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MIN_FILTER, ffi::LINEAR as i32);
            gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MAG_FILTER, ffi::LINEAR as i32);
            gl.BindTexture(ffi::TEXTURE_2D, 0);
        })
        .map_err(|e| render_error(&e))?;
    Ok(texture)
}

/// The region `rects` (relative to the output) in physical pixels, relative
/// to its bounding box `bounds`. By their edges: rounded apart from each other,
/// a rectangle's position and size would leave gaps between neighbors at a
/// fractional scale.
fn inside_region(
    rects: &[Rectangle<i32, Logical>],
    bounds: Rectangle<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<Rectangle<i32, Physical>> {
    let x = |v: i32| (v as f64 * scale.x).round() as i32;
    let y = |v: i32| (v as f64 * scale.y).round() as i32;
    rects
        .iter()
        .map(|r| {
            let loc = r.loc - bounds.loc;
            let end = loc + r.size.to_point();
            Rectangle::from_extremities((x(loc.x), y(loc.y)), (x(end.x), y(end.y)))
        })
        .collect()
}

/// Make `rects` of `texture` transparent.
fn clear_rects(
    renderer: &mut GlesRenderer,
    texture: &mut GlesTexture,
    size: Size<i32, Physical>,
    rects: &[Rectangle<i32, Physical>],
) -> Result<(), GlesError> {
    let mut target = renderer.bind(texture)?;
    let mut frame = renderer.render(&mut target, size, Transform::Normal)?;
    frame.clear(Color32F::TRANSPARENT, rects)?;
    let _ = frame.finish()?;
    Ok(())
}

/// One blur pass: draw the part of `src` in use, `src_used` from its
/// origin, into the part of `dst` in use, `dst_used`, through `program`.
fn pass(
    renderer: &mut GlesRenderer,
    program: &GlesTexProgram,
    (src, src_used): (&GlesTexture, Size<i32, Physical>),
    (dst, dst_used): (&mut GlesTexture, Size<i32, Physical>),
    shape: Option<Shape>,
) -> Result<(), GlesError> {
    // Texel spacing is the texture's; the samples stop at the last texel in
    // use, at its center.
    let src_size = src.size();
    let (tex_w, tex_h) = (src_size.w as f32, src_size.h as f32);
    let full = Rectangle::from_size(dst_used);
    let halfpixel = (0.5 / tex_w, 0.5 / tex_h);
    let uv_max = (
        (src_used.w as f32 - 0.5) / tex_w,
        (src_used.h as f32 - 0.5) / tex_h,
    );
    let mut uniforms = vec![
        Uniform::new("offset", BLUR_OFFSET),
        Uniform::new("halfpixel", halfpixel),
        Uniform::new("uv_max", uv_max),
    ];
    if let Some(shape) = shape {
        // `v_coords` are the source's texture coordinates, which reach
        // `src_used / src_size`, not 1, at the far edge of `dst_used`.
        let target_size = (
            dst_used.w as f32 * tex_w / src_used.w as f32,
            dst_used.h as f32 * tex_h / src_used.h as f32,
        );
        uniforms.extend([
            Uniform::new("target_size", target_size),
            Uniform::new("shape_loc", (shape.loc.x as f32, shape.loc.y as f32)),
            Uniform::new("shape_size", (shape.size.w as f32, shape.size.h as f32)),
            Uniform::new("radii", shape.radii),
        ]);
    }
    let mut target = renderer.bind(dst)?;
    // GL's own orientation throughout: textures drawn into a framebuffer
    // and sampled back by the renderer agree without any flip.
    let mut frame = renderer.render(&mut target, dst_used, Transform::Normal)?;
    frame.clear(Color32F::TRANSPARENT, &[full])?;
    let src_rect: Size<i32, Buffer> = (src_used.w, src_used.h).into();
    frame.render_texture_from_to(
        src,
        Rectangle::from_size(src_rect).to_f64(),
        full,
        &[full],
        &[],
        Transform::Normal,
        1.0,
        Some(program),
        &uniforms,
    )?;
    // Same GL context for every pass and for the output: ordering is
    // implicit, no fence to wait for.
    let _ = frame.finish()?;
    Ok(())
}

/// Whether `damage` is within `BLUR_REACH` of `inside`, so the passes carry it
/// there.
fn near(damage: Rectangle<i32, Physical>, inside: Rectangle<i32, Physical>) -> bool {
    let margin = Size::from((BLUR_REACH, BLUR_REACH));
    Rectangle::new(
        inside.loc - margin.to_point(),
        inside.size + margin + margin,
    )
    .overlaps(damage)
}

impl BlurCache {
    /// Blur behind every window and upper layer surface of `scene` that asks
    /// for it on the output at `output_location` of `output_size`, drawn at
    /// `scale`. Called before the output's elements are built.
    pub fn prepare(
        &mut self,
        renderer: &mut GlesRenderer,
        scene: &Scene,
        output_name: &str,
        output_location: Point<i32, Logical>,
        output_size: Size<i32, Logical>,
        scale: Scale<f64>,
    ) {
        // Only this output's: the others' are kept for their own frames.
        for (_, entry) in self.entries.iter_mut().filter(|(k, _)| k.1 == output_name) {
            entry.used = false;
        }
        let shaders = (!scene.locked).then(|| Shaders::get(renderer)).flatten();
        if let Some(shaders) = shaders {
            let mut target = Target {
                renderer,
                scene,
                shaders: &shaders,
                output_name,
                output_location,
                output_size,
                scale,
            };
            // Back to front, so a blur surface behind another is blurred
            // first and shows in the other's backdrop: a window, then its
            // popups in front of it.
            for (index, window) in scene.windows.iter().enumerate().rev() {
                // Under opaque windows there's no need to look for changes
                // behind it, frame after frame; it's kept for when it shows.
                let hidden = |rects: &[Rectangle<i32, Logical>]| {
                    let above = scene.windows[..index].iter().filter_map(|w| w.covers());
                    Rectangle::subtract_rects_many(rects.iter().copied(), above).is_empty()
                };
                if let Some(surface) = window
                    .window
                    .wl_surface()
                    .filter(|_| !window.blur.is_empty())
                {
                    if hidden(&window.blur) {
                        self.keep(&surface, output_name);
                    } else {
                        self.prepare_surface(
                            &mut target,
                            &surface,
                            Behind::Window(index),
                            &window.blur,
                            window.backdrop_rounding.as_ref(),
                        );
                    }
                }
                for popup in window.popup_blur.iter().rev() {
                    if hidden(&popup.rects) {
                        self.keep(&popup.surface, output_name);
                    } else {
                        self.prepare_surface(
                            &mut target,
                            &popup.surface,
                            Behind::WindowPopup(index, popup.index),
                            &popup.rects,
                            popup.rounding.as_ref(),
                        );
                    }
                }
            }
            for (index, layer) in scene.upper.iter().enumerate().rev() {
                if !layer.blur.is_empty() {
                    let rects: Vec<Rectangle<i32, Logical>> = layer
                        .blur
                        .iter()
                        .map(|r| Rectangle::new(r.loc + layer.location, r.size))
                        .collect();
                    self.prepare_surface(
                        &mut target,
                        layer.layer.wl_surface(),
                        Behind::Layer(index),
                        &rects,
                        None,
                    );
                }
                for popup in layer.popup_blur.iter().rev() {
                    self.prepare_surface(
                        &mut target,
                        &popup.surface,
                        Behind::LayerPopup(index, popup.index),
                        &popup.rects,
                        popup.rounding.as_ref(),
                    );
                }
            }
        }
        // A hidden window's is kept for when it shows again: made anew, its
        // first frame back allocates and blurs everything (a refresh missed on
        // battery, back from another workspace).
        let hidden = |id: &ObjectId| scene.hidden_windows.iter().any(|s| s.id() == *id);
        self.entries
            .retain(|k, e| k.1 != output_name || e.used || hidden(&k.0));
    }

    /// Blur behind `rects` (in space coordinates) of `surface`, what's on the
    /// target's output.
    fn prepare_surface(
        &mut self,
        target: &mut Target<'_>,
        surface: &WlSurface,
        behind: Behind,
        rects: &[Rectangle<i32, Logical>],
        rounding: Option<&Rounding>,
    ) {
        let rects: Vec<Rectangle<i32, Logical>> = rects
            .iter()
            .map(|r| Rectangle::new(r.loc - target.output_location, r.size))
            .collect();
        let rect = rects
            .iter()
            .copied()
            .reduce(|a, b| a.merge(b))
            .and_then(|r| r.intersection(Rectangle::from_size(target.output_size)));
        let Some(rect) = rect else {
            return;
        };
        let size: Size<i32, Physical> = rect.size.to_f64().to_physical(target.scale).to_i32_round();
        if size.w <= 0 || size.h <= 0 {
            return;
        }
        let key = (surface.id(), target.output_name.to_string());
        let shape = rounding.map(|rounding| {
            let shape = rounding.physical(target.output_location + rect.loc, target.scale);
            Shape {
                loc: shape.rect.loc,
                size: shape.rect.size,
                radii: shape.radii,
            }
        });
        if let Err(err) = self.prepare_entry(target, behind, &key, rect, size, rects, shape) {
            warn!(err, "blur failed");
            self.entries.remove(&key);
        }
    }

    /// Keep `surface`'s backdrop on the output as it is, if it has one (hidden
    /// for now).
    fn keep(&mut self, surface: &WlSurface, output_name: &str) {
        if let Some(entry) = self
            .entries
            .get_mut(&(surface.id(), output_name.to_string()))
        {
            entry.used = true;
        }
    }

    /// Drop what's kept for an output that's gone or turned off.
    pub fn forget_output(&mut self, output_name: &str) {
        self.entries.retain(|k, _| k.1 != output_name);
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_entry(
        &mut self,
        target: &mut Target<'_>,
        surface: Behind,
        key: &(ObjectId, String),
        rect: Rectangle<i32, Logical>,
        size: Size<i32, Physical>,
        rects: Vec<Rectangle<i32, Logical>>,
        shape: Option<Shape>,
    ) -> Result<(), String> {
        let Target {
            renderer,
            scene,
            shaders,
            output_location,
            scale,
            ..
        } = target;
        let renderer = &mut **renderer;
        let (scene, shaders, output_location, scale) = (*scene, *shaders, *output_location, *scale);

        // What's behind the region, relative to it; backdrops of blur
        // surfaces further back are included.
        let origin = output_location + rect.loc;
        let behind: Vec<PilaRenderElement> = {
            let cache = &*self;
            render::elements_behind(
                renderer,
                scene,
                surface,
                Rectangle::new(origin, rect.size),
                scale,
                &mut |surface, origin, scale, alpha| {
                    cache.element(surface, &key.1, origin, scale, alpha)
                },
            )
        };
        let clear = render::clear_color(scene);

        // A tracker keeps its scale: it can't draw for another one. A new
        // entry isn't `drawn`, so it's blurred below.
        let reusable = |entry: &Entry| {
            let allocated = entry.levels[0].size();
            fits((allocated.w, allocated.h).into(), size) && entry.scale == scale
        };
        if !self.entries.get(key).is_some_and(reusable) {
            // One that outgrew its textures is likely growing on (a panel
            // opening, maybe more than `BLUR_ALLOC_STEP` a frame): half as
            // much again of room where it grew.
            let room = |now: i32, before: i32| if now > before { now + now / 2 } else { now };
            let wanted = match self.entries.get(key).filter(|entry| entry.scale == scale) {
                Some(entry) => {
                    let before = entry.levels[0].size();
                    (room(size.w, before.w), room(size.h, before.h)).into()
                }
                None => size,
            };
            let allocated = capacity(wanted);
            debug!(?rect, ?size, ?allocated, "blur: allocated");
            self.allocated += 1;
            let mut levels = Vec::with_capacity(BLUR_PASSES + 1);
            for k in 0..=BLUR_PASSES {
                let level = level_size(allocated, k);
                levels.push(blur_texture(renderer, (level.w, level.h).into())?);
            }
            let result = TextureRenderBuffer::from_texture(
                renderer,
                levels[0].clone(),
                1,
                Transform::Normal,
                None,
            );
            self.entries.insert(
                key.clone(),
                Entry {
                    rect,
                    global: origin,
                    scale,
                    size,
                    rects: Vec::new(),
                    inside: Vec::new(),
                    mask: Vec::new(),
                    shape: None,
                    levels,
                    result,
                    tracker: OutputDamageTracker::new(size, scale, Transform::Normal),
                    drawn: false,
                    used: true,
                },
            );
        }
        let entry = self.entries.get_mut(key).expect("inserted above");
        entry.used = true;
        if entry.size != size {
            // Same textures, another part of them: what was behind the old
            // size tells nothing about the new one.
            entry.size = size;
            entry.tracker = OutputDamageTracker::new(size, scale, Transform::Normal);
            entry.drawn = false;
        }
        let reshaped = entry.rects != rects || entry.rect != rect || entry.shape != shape;
        if reshaped {
            entry.inside = inside_region(&rects, rect, scale);
            entry.mask = Rectangle::subtract_rects_many(
                [Rectangle::from_size(size)],
                entry.inside.iter().copied(),
            );
            entry.rects = rects;
            entry.shape = shape;
        }
        entry.rect = rect;
        entry.global = origin;

        // Only what can show in the region counts.
        let changed = match entry.tracker.damage_output(1, &behind) {
            Ok((Some(damage), _)) => damage.iter().any(|rect| {
                entry.mask.is_empty()
                    || entry
                        .inside
                        .iter()
                        .any(|inside| rect.overlaps(*inside) || near(*rect, *inside))
            }),
            Ok((None, _)) => false,
            Err(err) => return Err(render_error(&err)),
        };
        if entry.drawn && !changed && !reshaped {
            return Ok(());
        }

        self.blurred += 1;
        // The scene behind the region, then the blur passes down and back
        // up, ending in level 0.
        {
            let mut level0 = entry.levels[0].clone();
            let mut target = renderer.bind(&mut level0).map_err(|e| render_error(&e))?;
            entry
                .tracker
                .render_output(renderer, &mut target, 0, &behind, clear)
                .map_err(|e| render_error(&e))?;
        }
        let used = |k: usize| level_size(size, k);
        for k in 1..=BLUR_PASSES {
            let (before, after) = entry.levels.split_at_mut(k);
            pass(
                renderer,
                &shaders.down,
                (&before[k - 1], used(k - 1)),
                (&mut after[0], used(k)),
                None,
            )
            .map_err(|e| render_error(&e))?;
        }
        for k in (1..=BLUR_PASSES).rev() {
            let (before, after) = entry.levels.split_at_mut(k);
            // The last pass, into level 0, cuts the corners out.
            let (program, shape) = match shape.filter(|_| k == 1) {
                Some(shape) => (&shaders.up_rounded, Some(shape)),
                None => (&shaders.up, None),
            };
            pass(
                renderer,
                program,
                (&after[0], used(k)),
                (&mut before[k - 1], used(k - 1)),
                shape,
            )
            .map_err(|e| render_error(&e))?;
        }
        if !entry.mask.is_empty() {
            clear_rects(renderer, &mut entry.levels[0], size, &entry.mask)
                .map_err(|e| render_error(&e))?;
        }
        let buffer_size: Size<i32, Buffer> = (size.w, size.h).into();
        // Opaque if nothing's cleared: what's behind it isn't drawn. With a
        // mask, what's inside isn't exactly opaque either: at a fractional
        // scale, Smithay rounds an opaque region out over the cleared pixels.
        // A window's never is: what it hid would count as not shown, and its
        // client drop to a frame a second while the blur still shows it.
        let opaque =
            (entry.mask.is_empty() && matches!(surface, Behind::Layer(_)) && shape.is_none())
                .then(|| vec![Rectangle::from_size(buffer_size)]);
        let mut context = entry.result.render();
        context.update_opaque_regions(opaque);
        let _ = context.draw(|_| Ok::<_, ()>(vec![Rectangle::from_size(buffer_size)]));
        entry.drawn = true;
        Ok(())
    }

    /// Whether no backdrop entries are cached (no hidden windows to keep).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// What tests read (`Pila::state_json`): backdrops allocated, times they
    /// were blurred, and those kept now.
    pub fn stats(&self) -> (u32, u32, usize) {
        (self.allocated, self.blurred, self.entries.len())
    }

    /// The backdrop element of `surface` on the output `output_name`, as the
    /// last `prepare` left it, if it has one, relative to `origin` (global, what
    /// the elements it goes with are relative to).
    /// At `alpha`: a layer surface fading in fades its backdrop with it.
    pub fn element(
        &self,
        surface: &WlSurface,
        output_name: &str,
        origin: Point<i32, Logical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Option<PilaRenderElement> {
        let entry = self.entries.get(&(surface.id(), output_name.to_string()))?;
        if !entry.drawn {
            return None;
        }
        let location: Point<f64, Physical> = (entry.global - origin).to_f64().to_physical(scale);
        let in_use: Size<i32, Buffer> = (entry.size.w, entry.size.h).into();
        let src = Rectangle::from_size(in_use.to_logical(1, Transform::Normal).to_f64());
        Some(PilaRenderElement::Texture(
            TextureRenderElement::from_texture_render_buffer(
                location,
                &entry.result,
                (alpha < 1.0).then_some(alpha),
                Some(src),
                Some(entry.rect.size),
                Kind::Unspecified,
            ),
        ))
    }
}

// --- Protocol ----------------------------------------------------------------

impl GlobalDispatch<ExtBackgroundEffectManagerV1, ()> for Pila {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ExtBackgroundEffectManagerV1>,
        _: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let manager = data_init.init(resource, ());
        manager.capabilities(Capability::Blur);
    }
}

impl Dispatch<ExtBackgroundEffectManagerV1, ()> for Pila {
    fn request(
        _: &mut Self,
        _: &Client,
        resource: &ExtBackgroundEffectManagerV1,
        request: manager::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let manager::Request::GetBackgroundEffect { id, surface } = request else {
            return;
        };
        let exists = with_states(&surface, |states| {
            let slot = states
                .data_map
                .get_or_insert_threadsafe(BlurEffect::default);
            slot.0.locked().as_ref().is_some_and(|e| e.is_alive())
        });
        if exists {
            resource.post_error(
                manager::Error::BackgroundEffectExists,
                "the surface already has a background effect object",
            );
            return;
        }
        let effect = data_init.init(id, surface.clone());
        with_states(&surface, |states| {
            if let Some(slot) = states.data_map.get::<BlurEffect>() {
                *slot.0.locked() = Some(effect);
            }
        });
    }
}

impl Dispatch<ExtBackgroundEffectSurfaceV1, WlSurface> for Pila {
    fn request(
        _: &mut Self,
        _: &Client,
        resource: &ExtBackgroundEffectSurfaceV1,
        request: effect::Request,
        surface: &WlSurface,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let effect::Request::SetBlurRegion { region } = request else {
            return;
        };
        if !surface.is_alive() {
            resource.post_error(effect::Error::SurfaceDestroyed, "the surface is gone");
            return;
        }
        // Copy semantics: the client may destroy the wl_region right away.
        let attributes = region.as_ref().map(get_region_attributes);
        debug!(surface = ?surface.id(), rects = ?attributes.as_ref().map(|a| a.rects.len()), "blur region set");
        with_states(surface, |states| {
            *states.cached_state.get::<BlurRegion>().pending() =
                BlurRegion(attributes.map(Arc::new));
        });
    }

    fn destroyed(_: &mut Self, _: ClientId, _: &ExtBackgroundEffectSurfaceV1, surface: &WlSurface) {
        if !surface.is_alive() {
            return;
        }
        // "The effect regions will be removed on the next commit."
        with_states(surface, |states| {
            *states.cached_state.get::<BlurRegion>().pending() = BlurRegion(None);
            if let Some(slot) = states.data_map.get::<BlurEffect>() {
                *slot.0.locked() = None;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{region_rects, MAX_REGION_RECTS};
    use smithay::{
        utils::Rectangle,
        wayland::compositor::{RectangleKind, RegionAttributes},
    };

    /// A client's region is decomposed on every frame: however many
    /// rectangles it sends, what comes out stays few, covering at least what
    /// it asked for (blurring more is harmless, the frame missed isn't).
    #[test]
    fn a_region_of_many_rectangles_gives_few() {
        let bounds = Rectangle::from_size((1000, 1000).into());
        let mut rects = vec![(RectangleKind::Add, bounds)];
        for y in 0..40 {
            for x in 0..40 {
                let hole = Rectangle::new((x * 25 + 5, y * 25 + 5).into(), (10, 10).into());
                rects.push((RectangleKind::Subtract, hole));
            }
        }
        let region = RegionAttributes { rects };
        let started = std::time::Instant::now();
        let out = region_rects(&region, bounds);
        assert!(started.elapsed() < std::time::Duration::from_millis(5));
        assert!(out.len() <= MAX_REGION_RECTS, "{} rects", out.len());
        assert!(out.iter().any(|r| r.contains((1, 1))));
        assert!(out.iter().any(|r| r.contains((998, 998))));
    }

    /// A few rectangles, as clients send, are kept as they are.
    #[test]
    fn a_region_of_few_rectangles_is_kept() {
        let bounds = Rectangle::from_size((100, 100).into());
        let region = RegionAttributes {
            rects: vec![
                (RectangleKind::Add, bounds),
                (
                    RectangleKind::Subtract,
                    Rectangle::new((40, 40).into(), (20, 20).into()),
                ),
            ],
        };
        let out = region_rects(&region, bounds);
        assert_eq!(out.len(), 4);
        assert!(!out.iter().any(|r| r.contains((50, 50))));
    }
}
