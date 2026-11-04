//! Building a frame's render elements, front to back: the client cursor
//! surface, layer surfaces above the windows, each window (with its popups)
//! followed by its title bar, then layer surfaces below the windows.
//!
//! Smithay's `render_output` draws the whole `Space` at once, which can't
//! interleave title bars with windows in stacking order, so pila builds the
//! list itself and hands it to the damage tracker. Backend-neutral: the scene
//! is collected from `Pila` first (`Pila::scene`), so a backend can build
//! elements while it holds a mutable borrow of its renderer.

use smithay::{
    backend::renderer::{
        element::{
            memory::MemoryRenderBufferRenderElement,
            solid::{SolidColorBuffer, SolidColorRenderElement},
            surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
            texture::TextureRenderElement,
            AsRenderElements, Kind,
        },
        gles::{GlesRenderer, GlesTexture},
    },
    desktop::{LayerSurface, PopupManager, Window},
    input::pointer::CursorIcon,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Physical, Point, Rectangle, Scale},
    wayland::seat::WaylandFocus,
};

use crate::{
    blur::BlurCache,
    cursor::{CursorTheme, NamedCursor},
    decoration::{self, TitleBarPart},
    rounded::{self, Corners, RoundedSolid, RoundedSurface, Rounding, ShapeDamage, SurfaceElement},
};

smithay::backend::renderer::element::render_elements! {
    pub PilaRenderElement<=GlesRenderer>;
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
    Solid=SolidColorRenderElement,
    Memory=MemoryRenderBufferRenderElement<GlesRenderer>,
    Texture=TextureRenderElement<GlesTexture>,
    RoundedSurface=RoundedSurface,
    RoundedSolid=RoundedSolid,
    ShapeDamage=ShapeDamage,
}

impl From<SurfaceElement> for PilaRenderElement {
    fn from(element: SurfaceElement) -> Self {
        match element {
            SurfaceElement::Plain(element) => Self::Surface(element),
            SurfaceElement::Rounded(element) => Self::RoundedSurface(element),
        }
    }
}

/// The start of a custom texture shader's source
/// (`GlesRenderer::compile_custom_texture_shader`): what Smithay's texture
/// programs declare, with floats at `precision`. Its own uniforms and
/// `main` follow.
pub fn texture_shader_prologue(precision: &str) -> String {
    format!(
        "\
#version 100
//_DEFINES_
#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif
precision {precision} float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif
uniform float alpha;
varying vec2 v_coords;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif
"
    )
}

/// The blurred backdrop to draw behind the upper layer surface at an index
/// of `Scene::upper`, if it has one (`blur.rs`).
pub type Backdrop<'a> = &'a mut dyn FnMut(usize) -> Option<PilaRenderElement>;

/// Behind all windows and layer surfaces.
const BACKGROUND: [f32; 4] = [0.12, 0.13, 0.15, 1.0];
/// Background while the session is locked and the locker hasn't drawn yet.
const LOCKED_BACKGROUND: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// The color an output is cleared to before drawing `scene`.
pub fn clear_color(scene: &Scene) -> [f32; 4] {
    if scene.locked {
        LOCKED_BACKGROUND
    } else {
        BACKGROUND
    }
}

/// Everything below the cursor, front to back (see `Pila::scene`).
#[derive(Default)]
pub struct Scene {
    /// Session locked: only `lock_surfaces` are drawn, on black.
    pub locked: bool,
    /// While locked, each output's lock surface and its location.
    pub lock_surfaces: Vec<(WlSurface, Point<i32, Logical>)>,
    /// Layer surfaces above the windows (Overlay, Top).
    pub upper: Vec<SceneLayer>,
    /// Where a dragged window would snap to (`snap.rs`): drawn just above
    /// the windows.
    pub snap_preview: Option<(SolidColorBuffer, Rectangle<i32, Logical>)>,
    pub windows: Vec<SceneWindow>,
    /// Layer surfaces below the windows (Bottom, Background; also Top while
    /// a fullscreen window is on top).
    pub lower: Vec<SceneLayer>,
    /// `corner-radius` from the config, for the snap preview.
    pub corner_radius: u32,
}

impl Scene {
    pub fn layers(&self) -> impl Iterator<Item = &SceneLayer> {
        self.upper.iter().chain(&self.lower)
    }
}

/// A layer surface and where it's drawn, in space coordinates.
pub struct SceneLayer {
    pub layer: LayerSurface,
    pub location: Point<i32, Logical>,
    /// Where the screen behind it is blurred (surface-local; `blur.rs`).
    pub blur: Vec<Rectangle<i32, Logical>>,
}

/// A mapped window as it should be drawn this frame.
pub struct SceneWindow {
    pub window: Window,
    /// Where the window's surface tree is drawn (geometry origin minus the
    /// geometry offset), in space coordinates.
    pub render_location: Point<i32, Logical>,
    pub title_bar: Option<SceneTitleBar>,
    /// The bell's flash over the window and its title bar (`bell.rs`).
    pub flash: Option<(SolidColorBuffer, Rectangle<i32, Logical>)>,
    /// Its rounded corners, if any (`rounded.rs`).
    pub rounding: Option<Rounding>,
}

pub struct SceneTitleBar {
    pub rect: Rectangle<i32, Logical>,
    pub focused: bool,
    pub hover: Option<TitleBarPart>,
}

/// Elements for `scene` (front to back), relative to an output whose top-left
/// is at `output_location`, drawn at `scale` (`scale::output_scale`).
pub fn scene_elements(
    renderer: &mut GlesRenderer,
    scene: &Scene,
    output_location: Point<i32, Logical>,
    scale: Scale<f64>,
    backdrop: Backdrop<'_>,
) -> Vec<PilaRenderElement> {
    scene_elements_from(renderer, scene, output_location, scale, backdrop, 0)
}

/// What's behind the upper layer surface at `index`: the upper layers below
/// it and everything else, relative to `origin`.
pub fn elements_behind_layer(
    renderer: &mut GlesRenderer,
    scene: &Scene,
    index: usize,
    origin: Point<i32, Logical>,
    scale: Scale<f64>,
    backdrop: Backdrop<'_>,
) -> Vec<PilaRenderElement> {
    scene_elements_from(renderer, scene, origin, scale, backdrop, index + 1)
}

/// `scene_elements` starting from the upper layer at `first_upper`.
fn scene_elements_from(
    renderer: &mut GlesRenderer,
    scene: &Scene,
    output_location: Point<i32, Logical>,
    scale: Scale<f64>,
    backdrop: Backdrop<'_>,
    first_upper: usize,
) -> Vec<PilaRenderElement> {
    if scene.locked {
        return scene
            .lock_surfaces
            .iter()
            .flat_map(|(surface, location)| {
                let location: Point<i32, Physical> =
                    (*location - output_location).to_physical_precise_round(scale);
                render_elements_from_surface_tree(
                    renderer,
                    surface,
                    location,
                    scale,
                    1.0,
                    Kind::Unspecified,
                )
            })
            .collect();
    }
    let mut elements = Vec::new();
    for (i, entry) in scene.upper.iter().enumerate().skip(first_upper) {
        elements.extend(layer_elements(
            renderer,
            std::slice::from_ref(entry),
            output_location,
            scale,
        ));
        // Right behind the surface (elements go front to back).
        elements.extend(backdrop(i));
    }
    if let Some((buffer, rect)) = &scene.snap_preview {
        let radius = Rounding::new(*rect, None, scene.corner_radius, false).map(|r| r.radius);
        elements.push(solid_element(
            renderer,
            buffer,
            rect.loc - output_location,
            radius,
            scale,
        ));
    }
    elements.extend(window_elements(
        renderer,
        &scene.windows,
        output_location,
        scale,
    ));
    elements.extend(layer_elements(
        renderer,
        &scene.lower,
        output_location,
        scale,
    ));
    elements
}

fn layer_elements(
    renderer: &mut GlesRenderer,
    layers: &[SceneLayer],
    output_location: Point<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement> {
    layers
        .iter()
        .flat_map(|entry| {
            let location: Point<i32, Physical> =
                (entry.location - output_location).to_physical_precise_round(scale);
            AsRenderElements::<GlesRenderer>::render_elements::<PilaRenderElement>(
                &entry.layer,
                renderer,
                location,
                scale,
                1.0,
            )
        })
        .collect()
}

fn window_elements(
    renderer: &mut GlesRenderer,
    windows: &[SceneWindow],
    output_location: Point<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement> {
    let shaders = windows
        .iter()
        .any(|entry| entry.rounding.is_some())
        .then(|| rounded::Shaders::get(renderer))
        .flatten();
    let mut elements = Vec::new();
    for entry in windows {
        let rounding = entry.rounding.as_ref().filter(|_| shaders.is_some());
        if let Some(rounding) = rounding {
            // In front of everything of this window, so its opaque regions
            // can't hide the damage.
            elements.push(PilaRenderElement::ShapeDamage(ShapeDamage::new(
                &entry.window,
                rounding,
                output_location,
                scale,
            )));
        }
        // In front of this window only (elements go front to back).
        if let Some((buffer, rect)) = &entry.flash {
            elements.push(solid_element(
                renderer,
                buffer,
                rect.loc - output_location,
                rounding.map(|r| r.radius),
                scale,
            ));
        }
        let location: Point<i32, Physical> =
            (entry.render_location - output_location).to_physical_precise_round(scale);
        match (rounding, &shaders) {
            (Some(rounding), Some(shaders)) => elements.extend(rounded_window_elements(
                renderer,
                &entry.window,
                location,
                rounding,
                shaders,
                output_location,
                scale,
            )),
            _ => elements.extend(AsRenderElements::<GlesRenderer>::render_elements::<
                PilaRenderElement,
            >(&entry.window, renderer, location, scale, 1.0)),
        }
        if let Some(bar) = &entry.title_bar {
            let rect = Rectangle::new(bar.rect.loc - output_location, bar.rect.size);
            elements.extend(decoration::title_bar_elements(
                renderer,
                &entry.window,
                rect,
                bar.focused,
                bar.hover,
                rounding.map(|r| r.radius),
                scale,
            ));
        }
    }
    elements
}

/// A window's elements, as `Window::render_elements` makes them (popups in
/// front, unclipped), with its own surfaces clipped to `rounding`.
fn rounded_window_elements(
    renderer: &mut GlesRenderer,
    window: &Window,
    location: Point<i32, Physical>,
    rounding: &Rounding,
    shaders: &rounded::Shaders,
    output_location: Point<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement> {
    let Some(surface) = window.wl_surface() else {
        return Vec::new();
    };
    let mut elements: Vec<PilaRenderElement> = Vec::new();
    if let Some(toplevel) = window.toplevel() {
        for (popup, popup_offset) in PopupManager::popups_for_surface(toplevel.wl_surface()) {
            let offset: Point<i32, Physical> = (window.geometry().loc + popup_offset
                - popup.geometry().loc)
                .to_physical_precise_round(scale);
            elements.extend(render_elements_from_surface_tree::<_, PilaRenderElement>(
                renderer,
                popup.wl_surface(),
                location + offset,
                scale,
                1.0,
                Kind::Unspecified,
            ));
        }
    }
    let surfaces = render_elements_from_surface_tree::<_, WaylandSurfaceRenderElement<GlesRenderer>>(
        renderer,
        &surface,
        location,
        scale,
        1.0,
        Kind::Unspecified,
    );
    elements.extend(
        rounded::clip_surfaces(surfaces, shaders, rounding, output_location, scale)
            .map(PilaRenderElement::from),
    );
    elements
}

/// A solid color at `loc` (relative to the output), with all four corners
/// rounded by `radius` when there is one and the shaders compiled.
fn solid_element(
    renderer: &mut GlesRenderer,
    buffer: &SolidColorBuffer,
    loc: Point<i32, Logical>,
    radius: Option<f64>,
    scale: Scale<f64>,
) -> PilaRenderElement {
    let loc: Point<i32, Physical> = loc.to_physical_precise_round(scale);
    let element = SolidColorRenderElement::from_buffer(buffer, loc, scale, 1.0, Kind::Unspecified);
    match radius.zip(radius.and_then(|_| rounded::Shaders::get(renderer))) {
        Some((radius, shaders)) => PilaRenderElement::RoundedSolid(RoundedSolid::new(
            element,
            &shaders,
            radius,
            Corners::All,
            scale,
        )),
        None => PilaRenderElement::Solid(element),
    }
}

/// Elements for a client-drawn cursor surface whose top-left is at `location`
/// (from `cursor::surface_position`), relative to the output origin.
pub fn cursor_elements(
    renderer: &mut GlesRenderer,
    cursor: Option<&(WlSurface, Point<i32, Logical>)>,
    output_location: Point<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement> {
    let Some((surface, location)) = cursor else {
        return Vec::new();
    };
    let location: Point<i32, Physical> =
        (*location - output_location).to_physical_precise_round(scale);
    render_elements_from_surface_tree(renderer, surface, location, scale, 1.0, Kind::Cursor)
}

/// Elements for a named cursor image (drawn by pila on bare metal, loaded
/// for this output's `scale`) with its hotspot at `pointer`, relative to an
/// output whose top-left is at `output_location`.
pub fn named_cursor_elements(
    renderer: &mut GlesRenderer,
    cursor: &NamedCursor,
    pointer: Point<f64, Logical>,
    output_location: Point<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement> {
    let location =
        (pointer - cursor.hotspot.to_f64() - output_location.to_f64()).to_physical(scale);
    MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        location,
        &cursor.buffer,
        None,
        None,
        None,
        Kind::Cursor,
    )
    .map(|element| vec![PilaRenderElement::Memory(element)])
    .unwrap_or_default()
}

/// What drawing a frame needs from `Pila` (`Pila::frame_input`), taken
/// before the backend's renderer is borrowed.
pub struct FrameInput {
    pub scene: Scene,
    /// The pointer, in global logical coordinates.
    pub pointer: Point<f64, Logical>,
    /// The client cursor surface and its top-left (`cursor::surface_position`).
    pub cursor_surface: Option<(WlSurface, Point<i32, Logical>)>,
    /// The named cursor, for backends that draw it themselves.
    pub named_cursor: Option<CursorIcon>,
}

impl FrameInput {
    pub fn cursor_wl_surface(&self) -> Option<&WlSurface> {
        self.cursor_surface.as_ref().map(|(surface, _)| surface)
    }
}

/// The cursor's elements, relative to `origin`: the client's cursor surface,
/// or the named cursor from `theme` (none without one: nested, the host draws it).
pub fn pointer_elements(
    renderer: &mut GlesRenderer,
    input: &FrameInput,
    theme: Option<&mut CursorTheme>,
    origin: Point<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement> {
    let mut elements = cursor_elements(renderer, input.cursor_surface.as_ref(), origin, scale);
    if let (Some(icon), Some(theme)) = (input.named_cursor, theme) {
        let image = theme.get(icon, crate::scale::buffer_scale(scale));
        elements.extend(named_cursor_elements(
            renderer,
            &image,
            input.pointer,
            origin,
            scale,
        ));
    }
    elements
}

/// The scene's elements on the output `output_name` at `output_geo`,
/// relative to `origin` (the output's top-left, or a region's), with the
/// blurred backdrops prepared for that output first.
pub fn output_elements(
    renderer: &mut GlesRenderer,
    blur: &mut BlurCache,
    scene: &Scene,
    output_name: &str,
    output_geo: Rectangle<i32, Logical>,
    origin: Point<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement> {
    blur.prepare(
        renderer,
        scene,
        output_name,
        output_geo.loc,
        output_geo.size,
        scale,
    );
    scene_elements(renderer, scene, origin, scale, &mut |i| {
        blur.element(i, scale)
    })
}
