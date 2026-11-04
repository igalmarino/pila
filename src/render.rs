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
        gles::GlesRenderer,
        ImportAll, ImportMem, Renderer, RendererSuper,
    },
    desktop::{LayerSurface, Window},
    input::pointer::CursorIcon,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Physical, Point, Rectangle, Scale},
};

use crate::{
    blur::BlurCache,
    cursor::{CursorTheme, NamedCursor},
    decoration::{self, TitleBarPart},
};

smithay::backend::renderer::element::render_elements! {
    pub PilaRenderElement<R> where R: ImportAll + ImportMem;
    Surface=WaylandSurfaceRenderElement<R>,
    Solid=SolidColorRenderElement,
    Memory=MemoryRenderBufferRenderElement<R>,
    Texture=TextureRenderElement<<R as RendererSuper>::TextureId>,
}

/// The blurred backdrop to draw behind the upper layer surface at an index
/// of `Scene::upper`, if it has one (`blur.rs`).
pub type Backdrop<'a, R> = &'a mut dyn FnMut(usize) -> Option<PilaRenderElement<R>>;

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
}

pub struct SceneTitleBar {
    pub rect: Rectangle<i32, Logical>,
    pub focused: bool,
    pub hover: Option<TitleBarPart>,
}

/// Elements for `scene` (front to back), relative to an output whose top-left
/// is at `output_location`, drawn at `scale` (`scale::output_scale`).
pub fn scene_elements<R>(
    renderer: &mut R,
    scene: &Scene,
    output_location: Point<i32, Logical>,
    scale: Scale<f64>,
    backdrop: Backdrop<'_, R>,
) -> Vec<PilaRenderElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    scene_elements_from(renderer, scene, output_location, scale, backdrop, 0)
}

/// What's behind the upper layer surface at `index`: the upper layers below
/// it and everything else, relative to `origin`.
pub fn elements_behind_layer<R>(
    renderer: &mut R,
    scene: &Scene,
    index: usize,
    origin: Point<i32, Logical>,
    scale: Scale<f64>,
    backdrop: Backdrop<'_, R>,
) -> Vec<PilaRenderElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    scene_elements_from(renderer, scene, origin, scale, backdrop, index + 1)
}

/// `scene_elements` starting from the upper layer at `first_upper`.
fn scene_elements_from<R>(
    renderer: &mut R,
    scene: &Scene,
    output_location: Point<i32, Logical>,
    scale: Scale<f64>,
    backdrop: Backdrop<'_, R>,
    first_upper: usize,
) -> Vec<PilaRenderElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
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
        let loc: Point<i32, Physical> =
            (rect.loc - output_location).to_physical_precise_round(scale);
        elements.push(PilaRenderElement::Solid(
            SolidColorRenderElement::from_buffer(buffer, loc, scale, 1.0, Kind::Unspecified),
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

fn layer_elements<R>(
    renderer: &mut R,
    layers: &[SceneLayer],
    output_location: Point<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    layers
        .iter()
        .flat_map(|entry| {
            let location: Point<i32, Physical> =
                (entry.location - output_location).to_physical_precise_round(scale);
            AsRenderElements::<R>::render_elements::<PilaRenderElement<R>>(
                &entry.layer,
                renderer,
                location,
                scale,
                1.0,
            )
        })
        .collect()
}

fn window_elements<R>(
    renderer: &mut R,
    windows: &[SceneWindow],
    output_location: Point<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    let mut elements = Vec::new();
    for entry in windows {
        // In front of this window only (elements go front to back).
        if let Some((buffer, rect)) = &entry.flash {
            let loc: Point<i32, Physical> =
                (rect.loc - output_location).to_physical_precise_round(scale);
            elements.push(PilaRenderElement::Solid(
                SolidColorRenderElement::from_buffer(buffer, loc, scale, 1.0, Kind::Unspecified),
            ));
        }
        let location: Point<i32, Physical> =
            (entry.render_location - output_location).to_physical_precise_round(scale);
        elements.extend(
            AsRenderElements::<R>::render_elements::<PilaRenderElement<R>>(
                &entry.window,
                renderer,
                location,
                scale,
                1.0,
            ),
        );
        if let Some(bar) = &entry.title_bar {
            let rect = Rectangle::new(bar.rect.loc - output_location, bar.rect.size);
            elements.extend(decoration::title_bar_elements(
                renderer,
                &entry.window,
                rect,
                bar.focused,
                bar.hover,
                scale,
            ));
        }
    }
    elements
}

/// Elements for a client-drawn cursor surface whose top-left is at `location`
/// (from `cursor::surface_position`), relative to the output origin.
pub fn cursor_elements<R>(
    renderer: &mut R,
    cursor: Option<&(WlSurface, Point<i32, Logical>)>,
    output_location: Point<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
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
pub fn named_cursor_elements<R>(
    renderer: &mut R,
    cursor: &NamedCursor,
    pointer: Point<f64, Logical>,
    output_location: Point<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
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
) -> Vec<PilaRenderElement<GlesRenderer>> {
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
) -> Vec<PilaRenderElement<GlesRenderer>> {
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
