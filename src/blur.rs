//! ext-background-effect: a surface asks for the screen behind a region of
//! it to be blurred (a translucent bar over the wallpaper and windows). The
//! region is double-buffered surface state (`BlurRegion`, applied on
//! commit by Smithay's cached state); `Pila::scene` evaluates it for each
//! layer surface (`blur_rects`), and the backends run `BlurCache::prepare`
//! before building an output's elements: for every blurred layer surface it
//! renders what's behind the region into an offscreen texture, blurs it
//! (dual Kawase: a few downsampling and upsampling passes with custom
//! texture shaders) and keeps the result, which `BlurCache::element` hands
//! to `render::scene_elements` to draw right behind that surface.
//!
//! The blur is redone only when something behind the region changed
//! (a damage tracker per region, fed the elements behind it); otherwise the
//! kept texture is drawn as is, and the output's own damage tracking skips
//! it. Only layer surfaces are blurred for now, and not in captures.

use std::{collections::HashMap, sync::Mutex};

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
    desktop::LayerSurface,
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
    wayland::compositor::{
        get_region_attributes, with_states, Cacheable, RectangleKind, RegionAttributes,
    },
};
use tracing::{debug, warn};

use crate::{
    render::{self, PilaRenderElement, Scene},
    Pila,
};

/// Down/up passes: each halves/doubles the size.
const BLUR_PASSES: usize = 3;
/// Sample spread per pass, in texels.
const BLUR_OFFSET: f32 = 2.5;
/// Redo every blur every frame (to debug the change detection).
const BLUR_ALWAYS: bool = false;

/// The surface's blur region, double-buffered with the surface state.
#[derive(Default, Clone, Debug)]
pub struct BlurRegion(Option<RegionAttributes>);

impl Cacheable for BlurRegion {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        self.clone()
    }
    fn merge_into(self, into: &mut Self, _dh: &DisplayHandle) {
        *into = self;
    }
}

/// The surface's effect object, if it has one (one at a time).
#[derive(Default)]
struct BlurEffect(Mutex<Option<ExtBackgroundEffectSurfaceV1>>);

/// Create the global. Called once from `protocols::init_globals`.
pub fn init_global(dh: &DisplayHandle) {
    dh.create_global::<Pila, ExtBackgroundEffectManagerV1, ()>(1, ());
}

/// The parts of a layer surface to blur behind, surface-local, clipped to
/// the surface. Empty without a region or a buffer.
pub fn blur_rects(layer: &LayerSurface) -> Vec<Rectangle<i32, Logical>> {
    let surface = layer.wl_surface();
    let (region, size) = with_states(surface, |states| {
        let region = states.cached_state.get::<BlurRegion>().current().0.clone();
        let size = states
            .data_map
            .get::<smithay::backend::renderer::utils::RendererSurfaceStateUserData>()
            .and_then(|s| s.lock().unwrap().surface_size());
        (region, size)
    });
    let Some(region) = region else {
        return Vec::new();
    };
    let Some(size) = size else {
        return Vec::new();
    };
    let bounds = Rectangle::from_size(size);
    let mut rects: Vec<Rectangle<i32, Logical>> = Vec::new();
    for (kind, rect) in &region.rects {
        match kind {
            RectangleKind::Add => rects.extend(rect.intersection(bounds)),
            RectangleKind::Subtract => {
                rects = Rectangle::subtract_rects_many(rects, [*rect]);
            }
        }
    }
    rects
}

/// Fragment shader prologue Smithay's texture programs expect.
const PROLOGUE: &str = "\
#version 100
//_DEFINES_
#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif
precision mediump float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif
uniform float alpha;
varying vec2 v_coords;
uniform float offset;
uniform vec2 halfpixel;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif
";

const DOWN_FRAG: &str = "\
void main() {
    vec2 uv = v_coords;
    vec4 sum = texture2D(tex, uv) * 4.0;
    sum += texture2D(tex, uv - halfpixel * offset);
    sum += texture2D(tex, uv + halfpixel * offset);
    sum += texture2D(tex, uv + vec2(halfpixel.x, -halfpixel.y) * offset);
    sum += texture2D(tex, uv - vec2(halfpixel.x, -halfpixel.y) * offset);
    gl_FragColor = (sum / 8.0) * alpha;
}
";

const UP_FRAG: &str = "\
void main() {
    vec2 uv = v_coords;
    vec4 sum = texture2D(tex, uv + vec2(-halfpixel.x * 2.0, 0.0) * offset);
    sum += texture2D(tex, uv + vec2(-halfpixel.x, halfpixel.y) * offset) * 2.0;
    sum += texture2D(tex, uv + vec2(0.0, halfpixel.y * 2.0) * offset);
    sum += texture2D(tex, uv + vec2(halfpixel.x, halfpixel.y) * offset) * 2.0;
    sum += texture2D(tex, uv + vec2(halfpixel.x * 2.0, 0.0) * offset);
    sum += texture2D(tex, uv + vec2(halfpixel.x, -halfpixel.y) * offset) * 2.0;
    sum += texture2D(tex, uv + vec2(0.0, -halfpixel.y * 2.0) * offset);
    sum += texture2D(tex, uv + vec2(-halfpixel.x, -halfpixel.y) * offset) * 2.0;
    gl_FragColor = (sum / 12.0) * alpha;
}
";

struct Shaders {
    down: GlesTexProgram,
    up: GlesTexProgram,
}

/// The blurred backdrop of one surface on one output.
struct Entry {
    /// The blurred area, output-local.
    rect: Rectangle<i32, Logical>,
    size: Size<i32, Physical>,
    /// Level 0 is `size`; each further level half of the one before.
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

/// Blurred backdrops, per surface and output (`Pila::blur`).
#[derive(Default)]
pub struct BlurCache {
    shaders: Option<Shaders>,
    /// Shader compilation failed: no blur.
    broken: bool,
    entries: HashMap<(ObjectId, String), Entry>,
    /// For the output being drawn: the entry of each blurred upper layer,
    /// by its index in `Scene::upper`.
    current: Vec<(usize, (ObjectId, String))>,
}

fn render_error(err: &dyn std::fmt::Debug) -> String {
    format!("{err:?}")
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

/// One blur pass: draw `src` into `dst` through `program`.
fn pass(
    renderer: &mut GlesRenderer,
    program: &GlesTexProgram,
    src: &GlesTexture,
    dst: &mut GlesTexture,
) -> Result<(), GlesError> {
    let src_size = src.size();
    let dst_size = dst.size();
    let dst_physical: Size<i32, Physical> = (dst_size.w, dst_size.h).into();
    let full = Rectangle::from_size(dst_physical);
    let halfpixel = (0.5 / src_size.w as f32, 0.5 / src_size.h as f32);
    let mut target = renderer.bind(dst)?;
    // GL's own orientation throughout: textures drawn into a framebuffer
    // and sampled back by the renderer agree without any flip.
    let mut frame = renderer.render(&mut target, dst_physical, Transform::Normal)?;
    frame.clear(Color32F::TRANSPARENT, &[full])?;
    frame.render_texture_from_to(
        src,
        Rectangle::from_size(src_size).to_f64(),
        full,
        &[full],
        &[],
        Transform::Normal,
        1.0,
        Some(program),
        &[
            Uniform::new("offset", BLUR_OFFSET),
            Uniform::new("halfpixel", halfpixel),
        ],
    )?;
    // Same GL context for every pass and for the output: ordering is
    // implicit, no fence to wait for.
    let _ = frame.finish()?;
    Ok(())
}

impl BlurCache {
    fn shaders(&mut self, renderer: &mut GlesRenderer) -> Option<&Shaders> {
        if self.broken {
            return None;
        }
        if self.shaders.is_none() {
            let uniforms = [
                UniformName::new("offset", UniformType::_1f),
                UniformName::new("halfpixel", UniformType::_2f),
            ];
            let compile = |renderer: &mut GlesRenderer, body: &str| {
                renderer.compile_custom_texture_shader(format!("{PROLOGUE}{body}"), &uniforms)
            };
            match (compile(renderer, DOWN_FRAG), compile(renderer, UP_FRAG)) {
                (Ok(down), Ok(up)) => self.shaders = Some(Shaders { down, up }),
                (Err(err), _) | (_, Err(err)) => {
                    warn!(?err, "blur: shader compilation failed; blur disabled");
                    self.broken = true;
                    return None;
                }
            }
        }
        self.shaders.as_ref()
    }

    /// Blur behind every upper layer surface of `scene` that asks for it on
    /// the output at `output_location` of `output_size`, drawn at `scale`.
    /// Called before the output's elements are built.
    pub fn prepare(
        &mut self,
        renderer: &mut GlesRenderer,
        scene: &Scene,
        output_name: &str,
        output_location: Point<i32, Logical>,
        output_size: Size<i32, Logical>,
        scale: Scale<f64>,
    ) {
        self.current.clear();
        for entry in self.entries.values_mut() {
            entry.used = false;
        }
        if scene.locked || self.broken {
            self.entries.retain(|_, e| e.used);
            return;
        }
        // Back to front, so a blur surface behind another is blurred first
        // and shows in the other's backdrop.
        for (index, layer) in scene.upper.iter().enumerate().rev() {
            if layer.blur.is_empty() {
                continue;
            }
            let rect = layer
                .blur
                .iter()
                .map(|r| Rectangle::new(r.loc + layer.location - output_location, r.size))
                .reduce(|a, b| a.merge(b))
                .and_then(|r| r.intersection(Rectangle::from_size(output_size)));
            let Some(rect) = rect else {
                continue;
            };
            let size: Size<i32, Physical> = rect.size.to_f64().to_physical(scale).to_i32_round();
            if size.w <= 0 || size.h <= 0 {
                continue;
            }
            let key = (layer.layer.wl_surface().id(), output_name.to_string());
            if self.shaders(renderer).is_none() {
                return;
            }
            if let Err(err) = self.prepare_entry(
                renderer,
                scene,
                index,
                &key,
                rect,
                size,
                output_location,
                scale,
            ) {
                warn!(err, "blur failed");
                self.entries.remove(&key);
                continue;
            }
            self.current.push((index, key));
        }
        self.entries.retain(|_, e| e.used);
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_entry(
        &mut self,
        renderer: &mut GlesRenderer,
        scene: &Scene,
        index: usize,
        key: &(ObjectId, String),
        rect: Rectangle<i32, Logical>,
        size: Size<i32, Physical>,
        output_location: Point<i32, Logical>,
        scale: Scale<f64>,
    ) -> Result<(), String> {
        // What's behind the region, relative to it; backdrops of blur
        // surfaces further back are included.
        let origin = output_location + rect.loc;
        let behind: Vec<PilaRenderElement<GlesRenderer>> = {
            let cache = &*self;
            render::elements_behind_layer(renderer, scene, index, origin, scale, &mut |i| {
                cache.element(i, scale)
            })
        };
        let clear = render::clear_color(scene);

        let fresh = match self.entries.get(key) {
            Some(entry) if entry.size == size => false,
            _ => {
                debug!(?rect, ?size, "blur: allocated");
                let mut levels = Vec::with_capacity(BLUR_PASSES + 1);
                for k in 0..=BLUR_PASSES {
                    let level: Size<i32, Buffer> =
                        ((size.w >> k).max(1), (size.h >> k).max(1)).into();
                    levels.push(blur_texture(renderer, level)?);
                }
                let buffer_size = levels[0].size();
                let result = TextureRenderBuffer::from_texture(
                    renderer,
                    levels[0].clone(),
                    1,
                    Transform::Normal,
                    Some(vec![Rectangle::from_size(buffer_size)]),
                );
                self.entries.insert(
                    key.clone(),
                    Entry {
                        rect,
                        size,
                        levels,
                        result,
                        tracker: OutputDamageTracker::new(size, scale, Transform::Normal),
                        drawn: false,
                        used: true,
                    },
                );
                true
            }
        };
        let entry = self.entries.get_mut(key).expect("inserted above");
        entry.used = true;
        entry.rect = rect;

        let changed = match entry.tracker.damage_output(1, &behind) {
            Ok((Some(damage), _)) => !damage.is_empty(),
            Ok((None, _)) => false,
            Err(err) => return Err(render_error(&err)),
        };
        if entry.drawn && !changed && !fresh && !BLUR_ALWAYS {
            return Ok(());
        }

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
        let shaders = self.shaders.as_ref().expect("checked by the caller");
        for k in 1..=BLUR_PASSES {
            let (before, after) = entry.levels.split_at_mut(k);
            pass(renderer, &shaders.down, &before[k - 1], &mut after[0])
                .map_err(|e| render_error(&e))?;
        }
        for k in (1..=BLUR_PASSES).rev() {
            let (before, after) = entry.levels.split_at_mut(k);
            pass(renderer, &shaders.up, &after[0], &mut before[k - 1])
                .map_err(|e| render_error(&e))?;
        }
        let buffer_size = entry.levels[0].size();
        let _ = entry
            .result
            .render()
            .draw(|_| Ok::<_, ()>(vec![Rectangle::from_size(buffer_size)]));
        entry.drawn = true;
        Ok(())
    }

    /// The backdrop element of the upper layer at `index` in the scene
    /// `prepare` ran for, if it has one.
    pub fn element(
        &self,
        index: usize,
        scale: Scale<f64>,
    ) -> Option<PilaRenderElement<GlesRenderer>> {
        let (_, key) = self.current.iter().find(|(i, _)| *i == index)?;
        let entry = self.entries.get(key)?;
        if !entry.drawn {
            return None;
        }
        let location: Point<f64, Physical> = entry.rect.loc.to_f64().to_physical(scale);
        let buffer_size = entry.levels[0].size();
        let src = Rectangle::from_size(buffer_size.to_logical(1, Transform::Normal).to_f64());
        Some(PilaRenderElement::Texture(
            TextureRenderElement::from_texture_render_buffer(
                location,
                &entry.result,
                None,
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
            states
                .data_map
                .insert_if_missing_threadsafe(BlurEffect::default);
            let slot = states.data_map.get::<BlurEffect>().expect("inserted");
            slot.0
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|e| e.is_alive())
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
                *slot.0.lock().unwrap() = Some(effect);
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
            *states.cached_state.get::<BlurRegion>().pending() = BlurRegion(attributes);
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
                *slot.0.lock().unwrap() = None;
            }
        });
    }
}
