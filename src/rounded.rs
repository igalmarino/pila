//! Rounded window corners (`corner-radius` in the config).
//!
//! A window's shape is its geometry, together with its title bar when pila
//! draws one (`Rounding`), and is decided in `Pila::scene`. Maximized and
//! fullscreen windows stay square. Drawing it:
//!
//! - Surface elements of the window's own tree (not its popups) that reach
//!   into a corner are wrapped in `RoundedSurface`, which draws them with a
//!   texture shader that fades out what lies outside the rounded corner.
//!   Elements away from the corners stay as they are, so they can still be
//!   scanned out directly. Content outside the geometry (a client-drawn
//!   shadow) is left alone.
//! - The title bar background, the bell flash and the snap preview are
//!   solid colors, drawn by `RoundedSolid`'s pixel shader.
//! - The close button, at the bar's top-right corner, is rasterized with
//!   that corner cut out (`decoration.rs`).
//! - `ShapeDamage`, an element that draws nothing, damages the window when
//!   its shape changes (e.g. the radius in a reloaded config), since no
//!   client commit says so.
//!
//! - A window's shadow (`[shadow]` in the config, `Shadow`) is `ShadowElement`'s
//!   pixel shader, from its distance to the shape. It leaves the shape itself
//!   clear: what's behind a translucent window shows through without it.
//!
//! The shaders are compiled on first use and kept in the renderer's EGL
//! context; if one fails to compile, windows are drawn square and no shadows.

use std::cell::RefCell;

use smithay::{
    backend::renderer::{
        element::{
            solid::SolidColorRenderElement,
            surface::{WaylandSurfaceRenderElement, WaylandSurfaceTexture},
            Element, Id, Kind, RenderElement, UnderlyingStorage,
        },
        gles::{
            GlesError, GlesFrame, GlesPixelProgram, GlesRenderer, GlesTexProgram, Uniform,
            UniformName, UniformType, UniformValue,
        },
        utils::{CommitCounter, DamageSet, OpaqueRegions},
        Texture,
    },
    desktop::Window,
    utils::user_data::UserDataMap,
    utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform},
};
use tracing::warn;

use crate::decoration::TITLE_BAR_HEIGHT;

/// A window's rounded shape, in space coordinates (logical).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rounding {
    /// The window geometry, with the title bar on top if it has one.
    pub rect: Rectangle<i32, Logical>,
    pub radius: f64,
    /// Whether the top of `rect` is pila's title bar.
    pub title_bar: bool,
}

impl Rounding {
    /// The shape for a window whose geometry is `geometry` (and title bar
    /// `bar`), with `radius` from the config; `None` when it's square.
    pub fn new(
        geometry: Rectangle<i32, Logical>,
        bar: Option<Rectangle<i32, Logical>>,
        radius: u32,
        filled: bool,
    ) -> Option<Self> {
        if radius == 0 || filled {
            return None;
        }
        let rect = bar.map_or(geometry, |bar| bar.merge(geometry));
        let mut limit = f64::from(rect.size.w.min(rect.size.h)) / 2.0;
        if bar.is_some() {
            // The bar's corners stay within the bar.
            limit = limit.min(f64::from(TITLE_BAR_HEIGHT));
        }
        let radius = f64::from(radius).min(limit);
        (radius >= 1.0).then_some(Self {
            rect,
            radius,
            title_bar: bar.is_some(),
        })
    }

    /// `rect` relative to an output at `origin`, in physical pixels, and the
    /// radius scaled to match.
    pub(crate) fn physical(&self, origin: Point<i32, Logical>, scale: Scale<f64>) -> PhysicalShape {
        PhysicalShape {
            rect: Rectangle::new(self.rect.loc - origin, self.rect.size)
                .to_physical_precise_round(scale),
            radius: (self.radius * scale.x) as f32,
        }
    }
}

/// A rounded rect in an output's physical coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PhysicalShape {
    pub rect: Rectangle<i32, Physical>,
    pub radius: f32,
}

impl PhysicalShape {
    /// The squares at the corners that the rounding cuts into: top-left,
    /// top-right, bottom-right, bottom-left (`radii` of each; 0 for none).
    fn corner_squares(&self, radii: [f32; 4]) -> Vec<Rectangle<i32, Physical>> {
        let Rectangle { loc, size } = self.rect;
        let side = |r: f32| r.ceil() as i32;
        let [tl, tr, br, bl] = radii.map(side);
        [
            Rectangle::new(loc, (tl, tl).into()),
            Rectangle::new((loc.x + size.w - tr, loc.y).into(), (tr, tr).into()),
            Rectangle::new(
                (loc.x + size.w - br, loc.y + size.h - br).into(),
                (br, br).into(),
            ),
            Rectangle::new((loc.x, loc.y + size.h - bl).into(), (bl, bl).into()),
        ]
        .into_iter()
        .filter(|square| !square.is_empty())
        .collect()
    }
}

/// Whether an element at `element` (physical) reaches into a corner of
/// `shape` that's rounded off, and so must be clipped.
fn needs_clip(element: Rectangle<i32, Physical>, shape: &PhysicalShape) -> bool {
    shape
        .corner_squares([shape.radius; 4])
        .iter()
        .any(|square| square.overlaps(element))
}

/// `regions` (relative to an element at `element_loc`) without the corner
/// squares of `shape`: those pixels aren't opaque anymore.
fn without_corners(
    regions: OpaqueRegions<i32, Physical>,
    element_loc: Point<i32, Physical>,
    shape: &PhysicalShape,
    radii: [f32; 4],
) -> OpaqueRegions<i32, Physical> {
    let squares = shape.corner_squares(radii).into_iter().map(|mut square| {
        square.loc -= element_loc;
        square
    });
    OpaqueRegions::from_slice(&Rectangle::subtract_rects_many(regions, squares))
}

/// Coverage of a rounded rect, shared by both shaders: 1 inside, 0 outside
/// a rounded corner, anti-aliased over a physical pixel. `p` and `size` are
/// in physical pixels; `radii` are top-left, top-right, bottom-right,
/// bottom-left. Outside the rect it's 1: clipping stops at the geometry.
pub(crate) const ROUNDING_FN: &str = "
float rounding_alpha(vec2 p, vec2 size, vec4 radii) {
    float r;
    vec2 center;
    if (p.x < radii.x && p.y < radii.x) {
        r = radii.x;
        center = vec2(r, r);
    } else if (p.x > size.x - radii.y && p.y < radii.y) {
        r = radii.y;
        center = vec2(size.x - r, r);
    } else if (p.x > size.x - radii.z && p.y > size.y - radii.z) {
        r = radii.z;
        center = vec2(size.x - r, size.y - r);
    } else if (p.x < radii.w && p.y > size.y - radii.w) {
        r = radii.w;
        center = vec2(r, size.y - r);
    } else {
        return 1.0;
    }
    if (p.x < 0.0 || p.y < 0.0 || p.x > size.x || p.y > size.y) {
        return 1.0;
    }
    return clamp(r - distance(p, center) + 0.5, 0.0, 1.0);
}
";

/// Texture shader body: Smithay's default one times the rounding.
/// `input_to_geo` maps texture coordinates to physical pixels relative to
/// the shape's top-left (`RoundedSurface::input_to_geo`).
const TEXTURE_FRAG: &str = "
uniform vec2 geo_size;
uniform vec4 radii;
uniform mat3 input_to_geo;

void main() {
    vec4 color = texture2D(tex, v_coords);
#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0);
#endif
    vec2 p = (input_to_geo * vec3(v_coords, 1.0)).xy;
    color = color * rounding_alpha(p, geo_size, radii) * alpha;
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif
    gl_FragColor = color;
}
";

/// Pixel shader: a solid color (premultiplied) with rounded corners.
/// `v_coords` runs 0..1 over the element and `size` is its physical size.
const SOLID_FRAG: &str = "
precision highp float;
uniform float alpha;
uniform vec2 size;
varying vec2 v_coords;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif
uniform vec4 color;
uniform vec4 radii;
";

const SOLID_MAIN: &str = "
void main() {
    vec4 c = color * rounding_alpha(v_coords * size, size, radii) * alpha;
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        c = vec4(0.0, 0.2, 0.0, 0.2) + c * 0.8;
#endif
    gl_FragColor = c;
}
";

/// Pixel shader: a window's shadow. `v_coords` runs 0..1 over the element and
/// `size` is its physical size; the rest is in physical pixels from its corner.
const SHADOW_FRAG: &str = "
precision highp float;
uniform float alpha;
uniform vec2 size;
varying vec2 v_coords;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif
uniform vec4 color;
uniform vec2 frame_center;
uniform vec2 frame_half;
uniform vec2 offset;
uniform float radius;
uniform float extent;

// Distance from p to the frame's rounded rect: negative inside.
float distance_to_frame(vec2 p) {
    vec2 q = abs(p - frame_center) - frame_half + radius;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
}

void main() {
    vec2 p = v_coords * size;
    float t = clamp(1.0 - distance_to_frame(p - offset) / extent, 0.0, 1.0);
    float clear = clamp(distance_to_frame(p) + 0.5, 0.0, 1.0);
    vec4 c = color * t * t * (3.0 - 2.0 * t) * clear * alpha;
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        c = vec4(0.0, 0.2, 0.0, 0.2) + c * 0.8;
#endif
    gl_FragColor = c;
}
";

/// The compiled shaders, kept in the EGL context's user data.
#[derive(Clone)]
pub struct Shaders {
    texture: GlesTexProgram,
    solid: GlesPixelProgram,
    shadow: GlesPixelProgram,
}

/// `None` if the shaders failed to compile (then windows are drawn square).
struct ShaderSlot(Option<Shaders>);

impl Shaders {
    /// The shaders, compiled on first use.
    pub fn get(renderer: &mut GlesRenderer) -> Option<Shaders> {
        if let Some(slot) = renderer.egl_context().user_data().get::<ShaderSlot>() {
            return slot.0.clone();
        }
        let shaders = Self::compile(renderer);
        renderer
            .egl_context()
            .user_data()
            .insert_if_missing(|| ShaderSlot(shaders.clone()));
        shaders
    }

    fn compile(renderer: &mut GlesRenderer) -> Option<Shaders> {
        let texture = renderer.compile_custom_texture_shader(
            format!(
                "{}{ROUNDING_FN}{TEXTURE_FRAG}",
                crate::render::texture_shader_prologue("highp")
            ),
            &[
                UniformName::new("geo_size", UniformType::_2f),
                UniformName::new("radii", UniformType::_4f),
                UniformName::new("input_to_geo", UniformType::Matrix3x3),
            ],
        );
        let solid = renderer.compile_custom_pixel_shader(
            format!("{SOLID_FRAG}{ROUNDING_FN}{SOLID_MAIN}"),
            &[
                UniformName::new("color", UniformType::_4f),
                UniformName::new("radii", UniformType::_4f),
            ],
        );
        let shadow = renderer.compile_custom_pixel_shader(
            SHADOW_FRAG,
            &[
                UniformName::new("color", UniformType::_4f),
                UniformName::new("frame_center", UniformType::_2f),
                UniformName::new("frame_half", UniformType::_2f),
                UniformName::new("offset", UniformType::_2f),
                UniformName::new("radius", UniformType::_1f),
                UniformName::new("extent", UniformType::_1f),
            ],
        );
        match (texture, solid, shadow) {
            (Ok(texture), Ok(solid), Ok(shadow)) => Some(Shaders {
                texture,
                solid,
                shadow,
            }),
            (Err(err), _, _) | (_, Err(err), _) | (_, _, Err(err)) => {
                warn!(
                    ?err,
                    "rounded corners: shader compilation failed; corners stay square"
                );
                None
            }
        }
    }
}

/// Wrap the surface elements of a window's own tree (`elements`, not its
/// popups) that reach into a rounded corner.
pub fn clip_surfaces(
    elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>>,
    shaders: &Shaders,
    rounding: &Rounding,
    origin: Point<i32, Logical>,
    scale: Scale<f64>,
) -> impl Iterator<Item = SurfaceElement> {
    let shape = rounding.physical(origin, scale);
    let program = shaders.texture.clone();
    elements.into_iter().map(move |element| {
        let clip = needs_clip(element.geometry(scale), &shape)
            && matches!(element.texture(), WaylandSurfaceTexture::Texture(_));
        if clip {
            SurfaceElement::Rounded(RoundedSurface {
                inner: element,
                program: program.clone(),
                shape,
            })
        } else {
            SurfaceElement::Plain(element)
        }
    })
}

/// A window surface element, clipped to the window's shape or not.
pub enum SurfaceElement {
    Plain(WaylandSurfaceRenderElement<GlesRenderer>),
    Rounded(RoundedSurface),
}

/// A surface element drawn through the rounding texture shader.
pub struct RoundedSurface {
    inner: WaylandSurfaceRenderElement<GlesRenderer>,
    program: GlesTexProgram,
    shape: PhysicalShape,
}

impl RoundedSurface {
    /// The matrix from the texture coordinates Smithay's vertex shader
    /// produces for this draw to physical pixels relative to the shape.
    /// Smithay builds `v_coords` from the position inside `dst`
    /// (`build_texture_mat` in its GLES renderer, then a flip for y-inverted
    /// textures); this is that matrix inverted, then moved to the shape.
    fn input_to_geo(
        &self,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        texture_size: Size<i32, Buffer>,
        y_inverted: bool,
    ) -> Option<[f32; 9]> {
        let tex = texture_matrix(src, dst, texture_size, self.inner.transform(), y_inverted);
        let to_dst = invert(tex)?;
        let offset = dst.loc - self.shape.rect.loc;
        Some(to_column_major(multiply(
            translation(offset.x as f32, offset.y as f32),
            to_dst,
        )))
    }
}

impl Element for RoundedSurface {
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.inner.location(scale)
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.inner.damage_since(scale, commit)
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        without_corners(
            self.inner.opaque_regions(scale),
            self.geometry(scale).loc,
            &self.shape,
            [self.shape.radius; 4],
        )
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }

    // A wrapper passes the framebuffer-effect protocol through (Smithay's
    // changelog: hand-written wrappers must).
    fn is_framebuffer_effect(&self) -> bool {
        self.inner.is_framebuffer_effect()
    }
}

impl RenderElement<GlesRenderer> for RoundedSurface {
    fn capture_framebuffer(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        cache: &UserDataMap,
    ) -> Result<(), GlesError> {
        self.inner.capture_framebuffer(frame, src, dst, cache)
    }

    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        let matrix = match self.inner.texture() {
            WaylandSurfaceTexture::Texture(texture) => {
                self.input_to_geo(src, dst, texture.size(), texture.is_y_inverted())
            }
            WaylandSurfaceTexture::SolidColor(_) => None,
        };
        let Some(matrix) = matrix else {
            return self
                .inner
                .draw(frame, src, dst, damage, opaque_regions, cache);
        };
        let size = self.shape.rect.size;
        frame.override_default_tex_program(
            self.program.clone(),
            vec![
                Uniform::new("geo_size", (size.w as f32, size.h as f32)),
                Uniform::new("radii", [self.shape.radius; 4]),
                Uniform::new(
                    "input_to_geo",
                    UniformValue::Matrix3x3 {
                        matrices: vec![matrix],
                        transpose: false,
                    },
                ),
            ],
        );
        let result = self
            .inner
            .draw(frame, src, dst, damage, opaque_regions, cache);
        frame.clear_tex_program_override();
        result
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        // Scanned out, the corners would show.
        None
    }
}

/// A solid color with some corners rounded: the title bar (top corners),
/// the bell flash and the snap preview (all four). It keeps the id and
/// commits of the `SolidColorBuffer` it's made from.
pub struct RoundedSolid {
    inner: SolidColorRenderElement,
    program: GlesPixelProgram,
    shape: PhysicalShape,
    /// Top-left, top-right, bottom-right, bottom-left (physical pixels).
    radii: [f32; 4],
}

/// Which corners of a solid are rounded.
#[derive(Debug, Clone, Copy)]
pub enum Corners {
    All,
    Top,
}

impl RoundedSolid {
    /// `inner` drawn with `corners` rounded by `radius` (logical pixels).
    pub fn new(
        inner: SolidColorRenderElement,
        shaders: &Shaders,
        radius: f64,
        corners: Corners,
        scale: Scale<f64>,
    ) -> Self {
        let radius = (radius * scale.x) as f32;
        let radii = match corners {
            Corners::All => [radius; 4],
            Corners::Top => [radius, radius, 0.0, 0.0],
        };
        let rect = inner.geometry(scale);
        Self {
            inner,
            program: shaders.solid.clone(),
            shape: PhysicalShape { rect, radius },
            radii,
        }
    }
}

impl Element for RoundedSolid {
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.inner.damage_since(scale, commit)
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        without_corners(
            self.inner.opaque_regions(scale),
            self.geometry(scale).loc,
            &self.shape,
            self.radii,
        )
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl RenderElement<GlesRenderer> for RoundedSolid {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        _src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        // With src the whole buffer and size = dst's, `v_coords` runs 0..1.
        let size: Size<i32, Buffer> = (dst.size.w, dst.size.h).into();
        let color = self.inner.color();
        frame.render_pixel_shader_to(
            &self.program,
            Rectangle::from_size(size.to_f64()),
            dst,
            size,
            Some(damage),
            self.inner.alpha(),
            &[
                Uniform::new("color", color.components()),
                Uniform::new("radii", self.radii),
            ],
        )
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

/// The shadow under a window, in space coordinates (logical).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shadow {
    /// The window geometry, with the title bar on top if it has one.
    pub frame: Rectangle<i32, Logical>,
    /// Of the frame's corners.
    pub radius: f64,
    /// How far it reaches past the frame.
    pub size: i32,
    pub offset: Point<i32, Logical>,
    /// Premultiplied.
    pub color: [f32; 4],
}

impl Shadow {
    /// All it can draw on.
    pub fn rect(&self) -> Rectangle<i32, Logical> {
        let moved = Rectangle::new(self.frame.loc + self.offset, self.frame.size);
        Rectangle::new(
            moved.loc - Point::from((self.size, self.size)),
            (moved.size.w + 2 * self.size, moved.size.h + 2 * self.size).into(),
        )
    }
}

/// Per-window record of the shadow last drawn, kept in its user data.
struct ShadowState {
    id: Id,
    commit: CommitCounter,
    /// The shadow wherever it is, and the alpha.
    last: Option<(Shadow, f32)>,
}

/// A window's shadow, behind everything of it. What changes it other than by
/// moving is a new commit, and only that damages it: it takes a frame's work
/// only where something over it or under it changed.
pub struct ShadowElement {
    id: Id,
    commit: CommitCounter,
    program: GlesPixelProgram,
    rect: Rectangle<i32, Physical>,
    color: [f32; 4],
    frame_center: [f32; 2],
    frame_half: [f32; 2],
    offset: [f32; 2],
    radius: f32,
    extent: f32,
    alpha: f32,
}

impl ShadowElement {
    pub fn new(
        window: &Window,
        shadow: &Shadow,
        shaders: &Shaders,
        alpha: f32,
        origin: Point<i32, Logical>,
        scale: Scale<f64>,
    ) -> Self {
        let mut state = window
            .user_data()
            .get_or_insert(|| {
                RefCell::new(ShadowState {
                    id: Id::new(),
                    commit: CommitCounter::default(),
                    last: None,
                })
            })
            .borrow_mut();
        let mut look = *shadow;
        look.frame.loc = Point::default();
        let look = Some((look, alpha));
        if state.last != look {
            state.last = look;
            state.commit.increment();
        }
        let physical = |rect: Rectangle<i32, Logical>| -> Rectangle<i32, Physical> {
            Rectangle::new(rect.loc - origin, rect.size).to_physical_precise_round(scale)
        };
        let rect = physical(shadow.rect());
        let frame = physical(shadow.frame);
        let half = [frame.size.w as f32 / 2.0, frame.size.h as f32 / 2.0];
        let offset = shadow.offset.to_f64().to_physical(scale);
        Self {
            id: state.id.clone(),
            commit: state.commit,
            program: shaders.shadow.clone(),
            rect,
            color: shadow.color,
            frame_center: [
                (frame.loc.x - rect.loc.x) as f32 + half[0],
                (frame.loc.y - rect.loc.y) as f32 + half[1],
            ],
            frame_half: half,
            offset: [offset.x as f32, offset.y as f32],
            radius: (shadow.radius * scale.x) as f32,
            extent: ((f64::from(shadow.size) * scale.x) as f32).max(1.0),
            alpha,
        }
    }
}

impl Element for ShadowElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        Rectangle::from_size((self.rect.size.w, self.rect.size.h).into()).to_f64()
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.rect
    }

    fn damage_since(
        &self,
        _scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        if commit == Some(self.commit) {
            DamageSet::default()
        } else {
            DamageSet::from_slice(&[Rectangle::from_size(self.rect.size)])
        }
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for ShadowElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        _src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        // With src the whole buffer and size = dst's, `v_coords` runs 0..1.
        let size: Size<i32, Buffer> = (dst.size.w, dst.size.h).into();
        frame.render_pixel_shader_to(
            &self.program,
            Rectangle::from_size(size.to_f64()),
            dst,
            size,
            Some(damage),
            self.alpha,
            &[
                Uniform::new("color", self.color),
                Uniform::new("frame_center", self.frame_center),
                Uniform::new("frame_half", self.frame_half),
                Uniform::new("offset", self.offset),
                Uniform::new("radius", self.radius),
                Uniform::new("extent", self.extent),
            ],
        )
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

/// Per-window record of the shape last drawn, kept in its user data.
struct ShapeState {
    id: Id,
    commit: CommitCounter,
    last: Option<(Size<i32, Logical>, f64, bool)>,
}

/// An element that draws nothing and damages a rounded window's whole shape
/// whenever the shape changes other than by moving (the damage tracker
/// handles moves itself, and it appearing or going away). Placed in front
/// of the window, so the window's opaque regions don't hide the damage.
pub struct ShapeDamage {
    id: Id,
    commit: CommitCounter,
    rect: Rectangle<i32, Physical>,
}

impl ShapeDamage {
    pub fn new(
        window: &Window,
        rounding: &Rounding,
        origin: Point<i32, Logical>,
        scale: Scale<f64>,
    ) -> Self {
        let mut state = window
            .user_data()
            .get_or_insert(|| {
                RefCell::new(ShapeState {
                    id: Id::new(),
                    commit: CommitCounter::default(),
                    last: None,
                })
            })
            .borrow_mut();
        let shape = Some((rounding.rect.size, rounding.radius, rounding.title_bar));
        if state.last != shape {
            state.last = shape;
            state.commit.increment();
        }
        Self {
            id: state.id.clone(),
            commit: state.commit,
            rect: rounding.physical(origin, scale).rect,
        }
    }
}

impl Element for ShapeDamage {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        Rectangle::from_size((1.0, 1.0).into())
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.rect
    }

    fn damage_since(
        &self,
        _scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        if commit == Some(self.commit) {
            DamageSet::default()
        } else {
            DamageSet::from_slice(&[Rectangle::from_size(self.rect.size)])
        }
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for ShapeDamage {
    fn draw(
        &self,
        _frame: &mut GlesFrame<'_, '_>,
        _src: Rectangle<f64, Buffer>,
        _dst: Rectangle<i32, Physical>,
        _damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        Ok(())
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

/// Alpha (0..=1) of the close button's pixel centered at `p` (logical,
/// relative to the button cell, whose top-right corner is the bar's) with
/// that corner rounded by `radius`.
pub fn top_right_coverage(p: (f64, f64), cell_width: f64, radius: f64, scale: f64) -> f64 {
    if radius <= 0.0 {
        return 1.0;
    }
    let center = (cell_width - radius, radius);
    if p.0 <= center.0 || p.1 >= center.1 {
        return 1.0;
    }
    let distance = ((p.0 - center.0).powi(2) + (p.1 - center.1).powi(2)).sqrt();
    // Anti-aliased over one physical pixel.
    ((radius - distance) * scale + 0.5).clamp(0.0, 1.0)
}

// 3x3 matrices, row-major, for 2D affine maps of column vectors.
type Matrix = [[f32; 3]; 3];

fn multiply(a: Matrix, b: Matrix) -> Matrix {
    let mut m = [[0.0; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    m
}

fn translation(x: f32, y: f32) -> Matrix {
    [[1.0, 0.0, x], [0.0, 1.0, y], [0.0, 0.0, 1.0]]
}

fn scaling(x: f32, y: f32) -> Matrix {
    [[x, 0.0, 0.0], [0.0, y, 0.0], [0.0, 0.0, 1.0]]
}

/// The inverse of an affine matrix (last row 0, 0, 1).
fn invert(m: Matrix) -> Option<Matrix> {
    let det = m[0][0] * m[1][1] - m[0][1] * m[1][0];
    if det.abs() < f32::EPSILON {
        return None;
    }
    let (a, b, c, d) = (m[1][1] / det, -m[0][1] / det, -m[1][0] / det, m[0][0] / det);
    Some([
        [a, b, -(a * m[0][2] + b * m[1][2])],
        [c, d, -(c * m[0][2] + d * m[1][2])],
        [0.0, 0.0, 1.0],
    ])
}

/// GL's layout for `uniform mat3` with `transpose: false`.
fn to_column_major(m: Matrix) -> [f32; 9] {
    [
        m[0][0], m[1][0], m[2][0], m[0][1], m[1][1], m[2][1], m[0][2], m[1][2], m[2][2],
    ]
}

/// The linear part of `Transform::matrix` in Smithay, row-major (it's
/// written there column by column, as cgmath takes it).
fn transform_matrix(transform: Transform) -> Matrix {
    let (a, b, c, d) = match transform {
        Transform::Normal => (1.0, 0.0, 0.0, 1.0),
        Transform::_90 => (0.0, 1.0, -1.0, 0.0),
        Transform::_180 => (-1.0, 0.0, 0.0, -1.0),
        Transform::_270 => (0.0, -1.0, 1.0, 0.0),
        Transform::Flipped => (-1.0, 0.0, 0.0, 1.0),
        Transform::Flipped90 => (0.0, 1.0, 1.0, 0.0),
        Transform::Flipped180 => (1.0, 0.0, 0.0, -1.0),
        Transform::Flipped270 => (0.0, -1.0, -1.0, 0.0),
    };
    [[a, b, 0.0], [c, d, 0.0], [0.0, 0.0, 1.0]]
}

/// Smithay's `build_texture_mat` (GLES renderer): from a position in
/// `dst` (physical, relative to its top-left) to texture coordinates, plus
/// the flip `render_texture_from_to` applies to y-inverted textures.
fn texture_matrix(
    src: Rectangle<f64, Buffer>,
    dst: Rectangle<i32, Physical>,
    texture: Size<i32, Buffer>,
    transform: Transform,
    y_inverted: bool,
) -> Matrix {
    let src_size = transform.transform_size(src.size);
    let to_src = scaling(
        (src_size.w / f64::from(dst.size.w)) as f32,
        (src_size.h / f64::from(dst.size.h)) as f32,
    );
    let (w, h) = (src_size.w as f32, src_size.h as f32);
    let shift = match transform {
        Transform::Normal | Transform::Flipped90 => (0.0, 0.0),
        Transform::_90 => (0.0, w),
        Transform::_180 => (w, h),
        Transform::_270 => (h, 0.0),
        Transform::Flipped => (w, 0.0),
        Transform::Flipped180 => (0.0, h),
        Transform::Flipped270 => (h, w),
    };
    let mut m = multiply(transform_matrix(transform), to_src);
    m = multiply(translation(shift.0, shift.1), m);
    m = multiply(translation(src.loc.x as f32, src.loc.y as f32), m);
    m = multiply(scaling(1.0 / texture.w as f32, 1.0 / texture.h as f32), m);
    if y_inverted {
        m = multiply(scaling(1.0, -1.0), m);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(m: Matrix, p: (f32, f32)) -> (f32, f32) {
        (
            m[0][0] * p.0 + m[0][1] * p.1 + m[0][2],
            m[1][0] * p.0 + m[1][1] * p.1 + m[1][2],
        )
    }

    fn close(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 1e-4 && (a.1 - b.1).abs() < 1e-4
    }

    #[test]
    fn square_when_off_filled_or_tiny() {
        let geo = Rectangle::new((10, 40).into(), (400, 300).into());
        assert_eq!(Rounding::new(geo, None, 0, false), None);
        assert_eq!(Rounding::new(geo, None, 12, true), None);
        let tiny = Rectangle::new((0, 0).into(), (1, 1).into());
        assert_eq!(Rounding::new(tiny, None, 12, false), None);
    }

    #[test]
    fn radius_is_limited_by_size_and_title_bar() {
        let geo = Rectangle::new((10, 40).into(), (400, 30).into());
        assert_eq!(Rounding::new(geo, None, 64, false).unwrap().radius, 15.0);
        let geo = Rectangle::new((10, 40).into(), (400, 300).into());
        let bar = Rectangle::new((10, 12).into(), (400, TITLE_BAR_HEIGHT).into());
        let rounding = Rounding::new(geo, Some(bar), 64, false).unwrap();
        assert_eq!(rounding.radius, f64::from(TITLE_BAR_HEIGHT));
        // The shape is the bar and the window together.
        assert_eq!(
            rounding.rect,
            Rectangle::new((10, 12).into(), (400, 328).into())
        );
    }

    #[test]
    fn only_elements_in_a_corner_are_clipped() {
        let shape = PhysicalShape {
            rect: Rectangle::new((100, 100).into(), (400, 300).into()),
            radius: 10.0,
        };
        // The whole window, and a subsurface at its top-left corner.
        assert!(needs_clip(shape.rect, &shape));
        assert!(needs_clip(
            Rectangle::new((95, 95).into(), (20, 20).into()),
            &shape
        ));
        // A subsurface in the middle, one along an edge between corners.
        assert!(!needs_clip(
            Rectangle::new((200, 200).into(), (50, 50).into()),
            &shape
        ));
        assert!(!needs_clip(
            Rectangle::new((150, 100).into(), (300, 20).into()),
            &shape
        ));
    }

    #[test]
    fn opaque_regions_lose_the_corners() {
        let shape = PhysicalShape {
            rect: Rectangle::new((100, 100).into(), (40, 30).into()),
            radius: 4.5,
        };
        let full = OpaqueRegions::from_slice(&[Rectangle::from_size((40, 30).into())]);
        let regions = without_corners(full, shape.rect.loc, &shape, [4.5, 4.5, 0.0, 0.0]);
        let area: i32 = regions.iter().map(|r| r.size.w * r.size.h).sum();
        // Two 5x5 squares (the radius rounded up) at the top.
        assert_eq!(area, 40 * 30 - 2 * 25);
        assert!(regions
            .iter()
            .all(|r| !r.overlaps(Rectangle::from_size((5, 5).into()))));
    }

    #[test]
    fn texture_matrix_inverts_back_to_the_shape() {
        // A 200x100 buffer at scale 2 drawn into a 100x50 dst at (30, 20),
        // with the shape at (10, 10).
        let src = Rectangle::from_size((200.0, 100.0).into());
        let dst = Rectangle::new((30, 20).into(), (100, 50).into());
        let texture = Size::from((200, 100));
        for transform in [Transform::Normal, Transform::_90, Transform::Flipped180] {
            let src = Rectangle::from_size(transform.transform_size(src.size));
            let size = Size::from((src.size.w as i32, src.size.h as i32));
            let forward = texture_matrix(src, dst, size, transform, false);
            let back = invert(forward).unwrap();
            for p in [(0.0, 0.0), (100.0, 50.0), (25.0, 10.0)] {
                assert!(close(apply(back, apply(forward, p)), p));
            }
        }
        // Normal: dst's corners are the texture's.
        let forward = texture_matrix(src, dst, texture, Transform::Normal, false);
        assert!(close(apply(forward, (0.0, 0.0)), (0.0, 0.0)));
        assert!(close(apply(forward, (100.0, 50.0)), (1.0, 1.0)));
        // Y-inverted: the flip comes back out.
        let forward = texture_matrix(src, dst, texture, Transform::Normal, true);
        assert!(close(apply(forward, (100.0, 50.0)), (1.0, -1.0)));
        assert!(close(
            apply(invert(forward).unwrap(), (1.0, -1.0)),
            (100.0, 50.0)
        ));
    }

    const TRANSFORMS: [Transform; 8] = [
        Transform::Normal,
        Transform::_90,
        Transform::_180,
        Transform::_270,
        Transform::Flipped,
        Transform::Flipped90,
        Transform::Flipped180,
        Transform::Flipped270,
    ];

    #[test]
    fn transform_matrices_match_smithays() {
        for transform in TRANSFORMS {
            // glam: columns `x_axis`, `y_axis`, each with rows `x`, `y`.
            let theirs = transform.matrix().matrix2;
            let ours = transform_matrix(transform);
            assert_eq!(
                [ours[0][0], ours[0][1], ours[1][0], ours[1][1]],
                [
                    theirs.x_axis.x,
                    theirs.y_axis.x,
                    theirs.x_axis.y,
                    theirs.y_axis.y
                ],
                "{transform:?}"
            );
        }
    }

    #[test]
    fn every_transform_maps_dst_onto_the_whole_texture() {
        // A buffer drawn rotated: its size is dst's, transformed.
        let dst = Rectangle::new((30, 20).into(), (100, 50).into());
        let corners = [(0.0, 0.0), (100.0, 0.0), (0.0, 50.0), (100.0, 50.0)];
        for transform in TRANSFORMS {
            let size = transform.transform_size(Size::<i32, Buffer>::from((100, 50)));
            let src = Rectangle::from_size(size.to_f64());
            let m = texture_matrix(src, dst, size, transform, false);
            let mut mapped: Vec<(i32, i32)> = corners
                .iter()
                .map(|&p| {
                    let (x, y) = apply(m, p);
                    assert!(close((x, y), (x.round(), y.round())), "{transform:?}");
                    (x.round() as i32, y.round() as i32)
                })
                .collect();
            mapped.sort();
            assert_eq!(mapped, [(0, 0), (0, 1), (1, 0), (1, 1)], "{transform:?}");
        }
    }

    #[test]
    fn column_major_puts_the_translation_last() {
        let m = to_column_major(translation(3.0, 4.0));
        assert_eq!(&m[6..], &[3.0, 4.0, 1.0]);
    }

    #[test]
    fn close_button_corner_is_cut() {
        let (w, r) = (36.0, 10.0);
        // The very corner is gone, the inside and the left half aren't.
        assert_eq!(top_right_coverage((35.5, 0.5), w, r, 1.0), 0.0);
        assert_eq!(top_right_coverage((20.0, 0.5), w, r, 1.0), 1.0);
        assert_eq!(top_right_coverage((30.0, 8.0), w, r, 1.0), 1.0);
        assert_eq!(top_right_coverage((35.5, 0.5), w, 0.0, 1.0), 1.0);
    }
}
