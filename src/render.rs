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
    backend::allocator::Fourcc,
    backend::renderer::{
        damage::OutputDamageTracker,
        element::texture::TextureRenderBuffer,
        element::{
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            solid::{SolidColorBuffer, SolidColorRenderElement},
            surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
            texture::TextureRenderElement,
            utils::RescaleRenderElement,
            AsRenderElements, Kind,
        },
        gles::{GlesRenderer, GlesTexture},
        Bind, Offscreen,
    },
    desktop::{LayerSurface, PopupManager, Window},
    input::pointer::CursorIcon,
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform},
    wayland::{compositor::with_states, seat::WaylandFocus},
};

use crate::{
    animation::{self, AnimTextures, SceneGhost, Snapshot, WindowEffect},
    blur::BlurCache,
    config::Colors,
    cursor::{CursorTheme, MagnifiedCursor, NamedCursor},
    decoration::{self, TitleBarPart},
    rounded::{
        self, Corners, RoundedSolid, RoundedSurface, Rounding, Shadow, ShadowElement, ShapeDamage,
        SurfaceElement,
    },
    workspace_slide::SceneSlide,
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
    Shadow=ShadowElement,
    Rescaled=RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>,
}

impl From<SurfaceElement> for PilaRenderElement {
    fn from(element: SurfaceElement) -> Self {
        match element {
            SurfaceElement::Plain(element) => Self::Surface(element),
            SurfaceElement::Rounded(element) => Self::RoundedSurface(element),
        }
    }
}

/// Shaders kept in the EGL context's user data, compiled by `compile` the
/// first time they're asked for (`warm_up` does it at startup, not in a
/// frame). `None` if they failed to compile, which is kept too: they aren't
/// tried again.
pub fn compiled_once<T: Clone + 'static>(
    renderer: &mut GlesRenderer,
    compile: impl FnOnce(&mut GlesRenderer) -> Option<T>,
) -> Option<T> {
    struct Slot<T>(Option<T>);
    if let Some(slot) = renderer.egl_context().user_data().get::<Slot<T>>() {
        return slot.0.clone();
    }
    let shaders = compile(renderer);
    renderer
        .egl_context()
        .user_data()
        .insert_if_missing(|| Slot(shaders.clone()));
    shaders
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

/// The blurred backdrop to draw behind a surface, if it has one (`blur.rs`):
/// relative to a global point, at a scale and an opacity.
pub type Backdrop<'a> = &'a mut dyn FnMut(
    &WlSurface,
    Point<i32, Logical>,
    Scale<f64>,
    f32,
) -> Option<PilaRenderElement>;

/// A surface of the scene whose backdrop is being prepared: what's behind it is
/// what the backdrop shows.
#[derive(Debug, Clone, Copy)]
pub enum Behind {
    /// The upper layer surface at this index of `Scene::upper`.
    Layer(usize),
    /// The window at this index of `Scene::windows`.
    Window(usize),
    /// A popup of the window at this index of `Scene::windows`, at this
    /// place among its popups (`PopupBlur::index`).
    WindowPopup(usize, usize),
    /// A popup of the upper layer surface at this index of `Scene::upper`,
    /// at this place among its popups.
    LayerPopup(usize, usize),
}

/// Where in the scene to start drawing from (it's drawn front to back).
#[derive(Clone, Copy)]
enum Start {
    /// The upper layer surface at this index of `Scene::upper`.
    Upper(usize),
    /// The window at this index of `Scene::windows`, with the ghosts behind it.
    Window(usize),
}

/// Background while the session is locked and the locker hasn't drawn yet.
const LOCKED_BACKGROUND: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// The color an output is cleared to before drawing `scene`.
pub fn clear_color(scene: &Scene) -> [f32; 4] {
    if scene.locked {
        LOCKED_BACKGROUND
    } else {
        scene.colors.background
    }
}

/// Everything below the cursor, front to back (see `Pila::scene`).
pub struct Scene {
    /// Session locked: only `lock_surfaces` are drawn, on black.
    pub locked: bool,
    /// While locked, each output's lock surface and its location.
    pub lock_surfaces: Vec<(WlSurface, Point<i32, Logical>)>,
    /// Layer surfaces above the windows (Overlay, Top).
    pub upper: Vec<SceneLayer>,
    /// Layer surfaces fading out, in front of `upper` (`animation.rs`).
    pub layer_ghosts: Vec<SceneGhost>,
    /// Where a dragged window would snap to (`snap.rs`): drawn just above
    /// the windows.
    pub snap_preview: Option<(SolidColorBuffer, Rectangle<i32, Logical>)>,
    pub windows: Vec<SceneWindow>,
    /// Closed windows fading out, among `windows` (`animation.rs`).
    pub ghosts: Vec<SceneGhost>,
    /// A workspace switch sliding the windows (`workspace_slide.rs`).
    pub slide: Option<SceneSlide>,
    /// Layer surfaces below the windows (Bottom, Background; also Top while
    /// a fullscreen window is on top).
    pub lower: Vec<SceneLayer>,
    /// A drag's icon and its top-left (global): above everything but the
    /// cursor, never while locked.
    pub dnd_icon: Option<(WlSurface, Point<i32, Logical>)>,
    /// The window menu, above everything but the cursor (`window_menu.rs`).
    pub window_menu: Option<crate::window_menu::SceneMenu>,
    /// Windows off the `Space` (on another workspace, minimized): their
    /// blurred backdrops are kept for when they show again (`blur.rs`).
    pub hidden_windows: Vec<WlSurface>,
    /// `corner-radius` from the config, for the snap preview.
    pub corner_radius: u32,
    /// `rounded-corners`, for the snap preview.
    pub rounded_corners: rounded::Corners,
    /// The window switcher, in front of everything but the cursor
    /// (`switcher_view.rs`).
    pub switcher: Option<crate::switcher_view::SceneSwitcher>,
    /// The shortcuts cheatsheet, in front of the switcher
    /// (`shortcuts_view.rs`).
    pub shortcuts: Option<crate::shortcuts_view::SceneShortcuts>,
    /// The overview: its windows are among `windows`, mapped onto their
    /// places; what it draws behind them is below them (`overview.rs`).
    pub overview: Option<crate::overview::SceneOverview>,
    /// The screen-share picker, in front of everything but the cursor, its
    /// dim over every output (`share_picker.rs`).
    pub share_picker: Option<crate::share_picker::ScenePicker>,
    pub colors: Colors,
}

impl Scene {
    /// Nothing to draw but the background, in `colors`.
    pub fn blank(colors: Colors) -> Self {
        Self {
            locked: false,
            lock_surfaces: Vec::new(),
            upper: Vec::new(),
            layer_ghosts: Vec::new(),
            snap_preview: None,
            windows: Vec::new(),
            ghosts: Vec::new(),
            slide: None,
            lower: Vec::new(),
            dnd_icon: None,
            window_menu: None,
            hidden_windows: Vec::new(),
            corner_radius: 0,
            rounded_corners: rounded::Corners::All,
            switcher: None,
            shortcuts: None,
            overview: None,
            share_picker: None,
            colors,
        }
    }

    /// Whether the frame of the output at `output` may tear (`tearing.rs`):
    /// the window in front there is fullscreen over all of it and its frames
    /// may tear, plainly drawn, with nothing of Pila's over it and not locked.
    pub fn tears(&self, output: Rectangle<i32, Logical>) -> bool {
        if self.locked
            || self.slide.is_some()
            || self.window_menu.is_some()
            || self.switcher.is_some()
            || self.shortcuts.is_some()
            || self.overview.is_some()
            || self.share_picker.is_some()
        {
            return false;
        }
        self.windows
            .iter()
            .find(|w| w.frame_rect().overlaps(output))
            .is_some_and(|w| {
                w.tearing && w.effect.is_none() && w.frame_rect().contains_rect(output)
            })
    }

    pub fn layers(&self) -> impl Iterator<Item = &SceneLayer> {
        self.upper.iter().chain(&self.lower)
    }
}

/// A popup (a menu, or an input method's candidate window) that asks for
/// the screen behind it to be blurred (`blur::popup_blurs`).
#[derive(Clone)]
pub struct PopupBlur {
    pub surface: WlSurface,
    /// Its place among its root's popups, front to back
    /// (`PopupManager::popups_for_surface`, as they're drawn).
    pub index: usize,
    /// Where it's blurred, in space coordinates.
    pub rects: Vec<Rectangle<i32, Logical>>,
    /// The blur's rounded corners (a rule's `popup-corner-radius`).
    pub rounding: Option<Rounding>,
}

/// A layer surface and where it's drawn, in space coordinates.
#[derive(Clone)]
pub struct SceneLayer {
    pub layer: LayerSurface,
    pub location: Point<i32, Logical>,
    /// Where the screen behind it is blurred (surface-local; `blur.rs`).
    pub blur: Vec<Rectangle<i32, Logical>>,
    /// Its popups that ask for blur (upper layer surfaces only).
    pub popup_blur: Vec<PopupBlur>,
    /// While it appears: drawn through a texture (`animation.rs`).
    pub effect: Option<WindowEffect>,
}

impl SceneLayer {
    /// Its opacity this frame: its blurred backdrop fades with it.
    pub fn alpha(&self) -> f32 {
        self.effect.as_ref().map_or(1.0, |e| e.transform.alpha)
    }
}

/// A mapped window as it should be drawn this frame.
#[derive(Clone)]
pub struct SceneWindow {
    pub window: Window,
    /// Where the window's surface tree is drawn (geometry origin minus the
    /// geometry offset), in space coordinates.
    pub render_location: Point<i32, Logical>,
    /// Where the screen behind it is blurred, in space coordinates (`blur.rs`).
    pub blur: Vec<Rectangle<i32, Logical>>,
    /// Its popups that ask for blur.
    pub popup_blur: Vec<PopupBlur>,
    /// Nothing behind its geometry shows through (`blur::is_opaque`).
    pub opaque: bool,
    pub title_bar: Option<SceneTitleBar>,
    /// The bell's flash over the window and its title bar (`bell.rs`).
    pub flash: Option<(SolidColorBuffer, Rectangle<i32, Logical>)>,
    /// Its rounded corners, if any (`rounded.rs`).
    pub rounding: Option<Rounding>,
    /// The corners of a blur behind it: `rounding`'s, or the configured
    /// radius for a window that keeps its own corners (`Pila::scene_window`).
    pub backdrop_rounding: Option<Rounding>,
    /// The shadow pila draws under it, if any (`rounded.rs`).
    pub shadow: Option<Shadow>,
    /// Its opacity (a window rule's `opacity`), title bar included.
    pub alpha: f32,
    /// While it animates: drawn through a texture (`animation.rs`).
    pub effect: Option<WindowEffect>,
    /// While a workspace slide draws it into an output's texture, the
    /// output it's on (`workspace_slide.rs`).
    pub slid: Option<Option<Output>>,
    /// Everything of it that may be drawn, global (`Pila::window_visual_rect`):
    /// an output it doesn't touch leaves it out (`window_elements`).
    pub bounds: Rectangle<i32, Logical>,
    /// Fullscreen, and its frames may tear: its app asks (`tearing.rs`) and
    /// the config allows it.
    pub tearing: bool,
}

impl SceneWindow {
    /// The output it's paced by while it's drawn through a texture (absent
    /// from the render states then; `frame.rs`), if it is.
    pub fn offscreen_output(&self) -> Option<Option<Output>> {
        self.effect
            .as_ref()
            .map(|effect| effect.output.clone())
            .or_else(|| self.slid.clone())
    }

    /// Everything it draws (surfaces, popups, title bar, flash), in space
    /// coordinates.
    fn content_rect(&self) -> Rectangle<i32, Logical> {
        let mut rect = self.window.bbox_with_popups();
        rect.loc += self.render_location;
        if let Some(bar) = &self.title_bar {
            rect = rect.merge(bar.rect);
        }
        if let Some((_, flash)) = &self.flash {
            rect = rect.merge(*flash);
        }
        if let Some(shadow) = &self.shadow {
            rect = rect.merge(shadow.rect());
        }
        rect
    }

    /// What it hides for certain, in space coordinates: nothing while it's
    /// drawn through a texture, or with corners that let things through.
    pub fn covers(&self) -> Option<Rectangle<i32, Logical>> {
        let plain = self.effect.is_none() && self.slid.is_none() && self.rounding.is_none();
        (self.opaque && plain).then(|| self.frame_rect())
    }

    /// Its geometry plus title bar: what animations scale about.
    fn frame_rect(&self) -> Rectangle<i32, Logical> {
        let geo = Rectangle::new(
            self.render_location + self.window.geometry().loc,
            self.window.geometry().size,
        );
        self.title_bar
            .as_ref()
            .map_or(geo, |bar| bar.rect.merge(geo))
    }
}

#[derive(Clone)]
pub struct SceneTitleBar {
    pub rect: Rectangle<i32, Logical>,
    pub focused: bool,
    pub hover: Option<TitleBarPart>,
    pub colors: Colors,
}

/// Elements for `scene` (front to back), relative to an output whose top-left
/// is at `output_location`, drawn at `scale` (`scale::output_scale`).
pub fn scene_elements(
    renderer: &mut GlesRenderer,
    scene: &Scene,
    output_location: Point<i32, Logical>,
    scale: Scale<f64>,
    backdrop: Backdrop<'_>,
    visible: Option<Rectangle<i32, Logical>>,
) -> Vec<PilaRenderElement> {
    scene_elements_from(
        renderer,
        scene,
        output_location,
        scale,
        backdrop,
        Start::Upper(0),
        visible,
    )
}

/// What's behind a surface of the scene, in `area` (global), relative to its
/// top-left: what isn't there is left out. A workspace slide doesn't move what's
/// behind a window: its backdrop is for where it will be, and goes with it.
pub fn elements_behind(
    renderer: &mut GlesRenderer,
    scene: &Scene,
    surface: Behind,
    area: Rectangle<i32, Logical>,
    scale: Scale<f64>,
    backdrop: Backdrop<'_>,
) -> Vec<PilaRenderElement> {
    // A popup's own window or layer surface is behind it, with its popups
    // further back.
    let mut elements = Vec::new();
    let start = match surface {
        Behind::Layer(index) => Start::Upper(index + 1),
        Behind::Window(index) => Start::Window(index + 1),
        Behind::WindowPopup(index, popup) => {
            let entry = &scene.windows[index];
            let shaders = (entry.rounding.is_some() || entry.shadow.is_some())
                .then(|| rounded::Shaders::get(renderer))
                .flatten();
            elements.extend(single_window_elements(
                renderer,
                entry,
                shaders.as_ref(),
                Surfaces::Tree,
                popup + 1,
                area.loc,
                scale,
                &mut *backdrop,
            ));
            Start::Window(index + 1)
        }
        Behind::LayerPopup(index, popup) => {
            let entry = &scene.upper[index];
            elements.extend(single_layer_elements(
                renderer,
                &entry.layer,
                entry.location,
                Surfaces::Tree,
                popup + 1,
                area.loc,
                scale,
                Some(&mut *backdrop),
            ));
            elements.extend(backdrop(
                entry.layer.wl_surface(),
                area.loc,
                scale,
                entry.alpha(),
            ));
            Start::Upper(index + 1)
        }
    };
    elements.extend(scene_elements_from(
        renderer,
        scene,
        area.loc,
        scale,
        backdrop,
        start,
        Some(area),
    ));
    elements
}

/// `scene_elements` from `start`. `visible`: the global area drawn (an
/// output's), outside which windows and layer surfaces are left out
/// (`window_elements`, `layer_elements`).
fn scene_elements_from(
    renderer: &mut GlesRenderer,
    scene: &Scene,
    output_location: Point<i32, Logical>,
    scale: Scale<f64>,
    backdrop: Backdrop<'_>,
    start: Start,
    visible: Option<Rectangle<i32, Logical>>,
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
    match start {
        Start::Upper(first_upper) => {
            if first_upper == 0 {
                elements.extend(
                    scene
                        .layer_ghosts
                        .iter()
                        .map(|ghost| ghost_element(ghost, output_location, scale)),
                );
            }
            for entry in scene.upper.iter().skip(first_upper) {
                if !layer_shows(entry, visible) {
                    continue;
                }
                elements.extend(layer_elements(
                    renderer,
                    std::slice::from_ref(entry),
                    output_location,
                    scale,
                    None,
                    Some(&mut *backdrop),
                ));
                // Right behind the surface (elements go front to back).
                elements.extend(backdrop(
                    entry.layer.wl_surface(),
                    output_location,
                    scale,
                    entry.alpha(),
                ));
            }
            if let Some((buffer, rect)) = &scene.snap_preview {
                // Shaped as the window dropped there would be.
                let radius = Rounding::new(
                    *rect,
                    None,
                    scene.corner_radius,
                    scene.rounded_corners,
                    false,
                )
                .map(|r| (r.radius, r.corners));
                elements.push(solid_element(
                    renderer,
                    buffer,
                    rect.loc - output_location,
                    radius,
                    1.0,
                    scale,
                ));
            }
            match scene
                .slide
                .as_ref()
                .and_then(|slide| Some((slide, slide.output_rect(output_location)?)))
            {
                Some((slide, rect)) => elements.extend(slide_elements(
                    renderer,
                    &scene.windows,
                    &scene.ghosts,
                    slide,
                    rect,
                    scale,
                    backdrop,
                )),
                None => elements.extend(window_elements(
                    renderer,
                    &scene.windows,
                    &scene.ghosts,
                    output_location,
                    scale,
                    backdrop,
                    visible,
                )),
            }
        }
        Start::Window(first) => {
            // A ghost `before` a window is in front of it.
            let ghosts: Vec<SceneGhost> = scene
                .ghosts
                .iter()
                .filter(|ghost| ghost.before >= first)
                .map(|ghost| SceneGhost {
                    before: ghost.before - first,
                    ..ghost.clone()
                })
                .collect();
            elements.extend(window_elements(
                renderer,
                &scene.windows[first..],
                &ghosts,
                output_location,
                scale,
                backdrop,
                visible,
            ));
        }
    }
    if let Some(view) = &scene.overview {
        elements.extend(crate::overview::overview_elements(
            renderer,
            view,
            &scene.colors,
            visible,
            output_location,
            scale,
        ));
    }
    elements.extend(layer_elements(
        renderer,
        &scene.lower,
        output_location,
        scale,
        visible,
        None,
    ));
    elements
}

/// Whether a layer surface may show in `visible` (global; everywhere if
/// `None`): one that animates where its effect puts it.
fn layer_shows(entry: &SceneLayer, visible: Option<Rectangle<i32, Logical>>) -> bool {
    let Some(area) = visible else {
        return true;
    };
    let rect = layer_content_rect(&entry.layer, entry.location);
    match &entry.effect {
        Some(effect) => effect_rect(effect, rect).overlaps(area.to_f64()),
        None => rect.overlaps(area),
    }
}

/// `visible`: as in `scene_elements_from`. `backdrop`: for the popups that
/// ask for blur.
fn layer_elements(
    renderer: &mut GlesRenderer,
    layers: &[SceneLayer],
    output_location: Point<i32, Logical>,
    scale: Scale<f64>,
    visible: Option<Rectangle<i32, Logical>>,
    mut backdrop: Option<Backdrop<'_>>,
) -> Vec<PilaRenderElement> {
    let mut elements = Vec::new();
    for entry in layers.iter().filter(|entry| layer_shows(entry, visible)) {
        let mut plain = |renderer: &mut GlesRenderer, origin: Point<i32, Logical>, scale| {
            let backdrop = match &mut backdrop {
                Some(backdrop) if !entry.popup_blur.is_empty() => Some(&mut **backdrop as _),
                _ => None,
            };
            single_layer_elements(
                renderer,
                &entry.layer,
                entry.location,
                Surfaces::Tree,
                0,
                origin,
                scale,
                backdrop,
            )
        };
        let animated = entry.effect.as_ref().and_then(|effect| {
            let rect = layer_content_rect(&entry.layer, entry.location);
            animated_element(
                renderer,
                entry.layer.user_data(),
                rect,
                effect,
                output_location,
                scale,
                &mut |r: &mut GlesRenderer, origin, s| plain(r, origin, s),
            )
        });
        match animated {
            Some(element) => elements.push(element),
            None => elements.extend(plain(renderer, output_location, scale)),
        }
    }
    elements
}

/// A layer surface's elements (with its popups from the `first_popup`th on,
/// `popup_elements`), its top-left at `location`, relative to `origin`.
#[allow(clippy::too_many_arguments)]
fn single_layer_elements(
    renderer: &mut GlesRenderer,
    layer: &LayerSurface,
    location: Point<i32, Logical>,
    surfaces: Surfaces,
    first_popup: usize,
    origin: Point<i32, Logical>,
    scale: Scale<f64>,
    backdrop: Option<Backdrop<'_>>,
) -> Vec<PilaRenderElement> {
    let location: Point<i32, Physical> = (location - origin).to_physical_precise_round(scale);
    match surfaces {
        Surfaces::Tree if first_popup == 0 && backdrop.is_none() => {
            AsRenderElements::<GlesRenderer>::render_elements::<PilaRenderElement>(
                layer, renderer, location, scale, 1.0,
            )
        }
        Surfaces::Tree => {
            let surface = layer.wl_surface();
            let mut elements = popup_elements(
                renderer,
                surface,
                location,
                Point::default(),
                first_popup,
                origin,
                scale,
                1.0,
                backdrop,
            );
            elements.extend(render_elements_from_surface_tree(
                renderer,
                surface,
                location,
                scale,
                1.0,
                Kind::Unspecified,
            ));
            elements
        }
        Surfaces::MainOnly => main_surface_element(renderer, layer.wl_surface(), location, 1.0)
            .into_iter()
            .map(PilaRenderElement::Surface)
            .collect(),
    }
}

/// Everything a layer surface at `location` draws, popups included.
fn layer_content_rect(
    layer: &LayerSurface,
    location: Point<i32, Logical>,
) -> Rectangle<i32, Logical> {
    let mut rect = layer.bbox_with_popups();
    rect.loc += location;
    rect
}

/// `visible`: the global area drawn; a window wholly outside it (its
/// `bounds`) is left out, unless it's drawn somewhere else than where it is
/// (animating, sliding).
fn window_elements(
    renderer: &mut GlesRenderer,
    windows: &[SceneWindow],
    ghosts: &[SceneGhost],
    output_location: Point<i32, Logical>,
    scale: Scale<f64>,
    backdrop: Backdrop<'_>,
    visible: Option<Rectangle<i32, Logical>>,
) -> Vec<PilaRenderElement> {
    let shaders = windows
        .iter()
        .any(|entry| entry.rounding.is_some() || entry.shadow.is_some())
        .then(|| rounded::Shaders::get(renderer))
        .flatten();
    let ghosts_before = |i: usize| {
        ghosts
            .iter()
            .filter(move |g| g.before == i || (i == windows.len() && g.before > i))
            .map(|g| ghost_element(g, output_location, scale))
    };
    let mut elements = Vec::new();
    for (i, entry) in windows.iter().enumerate() {
        elements.extend(ghosts_before(i));
        // An animating window shows where its effect puts it (drawing it
        // into a texture for an output it isn't on would be for nothing);
        // a sliding one wherever the slide is.
        let shows = |area: Rectangle<i32, Logical>| match &entry.effect {
            Some(effect) => effect_rect(effect, entry.content_rect()).overlaps(area.to_f64()),
            None => entry.slid.is_some() || entry.bounds.overlaps(area),
        };
        if !visible.is_none_or(shows) {
            continue;
        }
        match &entry.effect {
            Some(effect) => elements.extend(animated_window_element(
                renderer,
                entry,
                effect,
                shaders.as_ref(),
                output_location,
                scale,
                &mut *backdrop,
            )),
            None => elements.extend(single_window_elements(
                renderer,
                entry,
                shaders.as_ref(),
                Surfaces::Tree,
                0,
                output_location,
                scale,
                &mut *backdrop,
            )),
        }
    }
    elements.extend(ghosts_before(windows.len()));
    elements
}

/// A workspace's thumbnail for the overview's strip (`overview.rs`): the
/// output at `output` with `windows` (front to back) over `layers` (its
/// wallpaper) and `background`, drawn into `target` at `scale` (the output's,
/// times how much smaller the thumbnail is). Its own damage tracker redraws
/// only what changed. No blur behind them: a thumbnail is small.
pub(crate) fn draw_thumbnail(
    renderer: &mut GlesRenderer,
    target: &mut OffscreenWindow,
    windows: &[SceneWindow],
    layers: &[SceneLayer],
    background: &SolidColorBuffer,
    output: Rectangle<i32, Logical>,
    scale: Scale<f64>,
) -> bool {
    let mut elements = window_elements(
        renderer,
        windows,
        &[],
        output.loc,
        scale,
        &mut |_, _, _, _| None,
        Some(output),
    );
    elements.extend(layer_elements(
        renderer,
        layers,
        output.loc,
        scale,
        Some(output),
        None,
    ));
    elements.push(solid_element(
        renderer,
        background,
        Point::default(),
        None,
        1.0,
        scale,
    ));
    target.draw(renderer, &elements)
}

/// Where a window's surface elements come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surfaces {
    /// Its surface tree and popups, as usual.
    Tree,
    /// Only its main surface, straight from its renderer state: while the
    /// surface is being destroyed (its client died) the tree is already
    /// taken apart, but the textures are still there.
    MainOnly,
}

/// One window's elements (front to back): its flash, popups (each with its
/// blurred backdrop) and surfaces, its title bar, its blurred backdrop, then
/// its shadow. The last two are part of it wherever it's drawn (a texture that
/// animates, or a slide). From `first_popup` on: what's behind that popup
/// (its flash is in front of them all).
#[allow(clippy::too_many_arguments)]
fn single_window_elements(
    renderer: &mut GlesRenderer,
    entry: &SceneWindow,
    shaders: Option<&rounded::Shaders>,
    surfaces: Surfaces,
    first_popup: usize,
    output_location: Point<i32, Logical>,
    scale: Scale<f64>,
    backdrop: Backdrop<'_>,
) -> Vec<PilaRenderElement> {
    let mut elements = Vec::new();
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
    if let Some((buffer, rect)) = entry.flash.as_ref().filter(|_| first_popup == 0) {
        elements.push(solid_element(
            renderer,
            buffer,
            rect.loc - output_location,
            rounding.map(|r| (r.radius, r.corners)),
            entry.alpha,
            scale,
        ));
    }
    let location: Point<i32, Physical> =
        (entry.render_location - output_location).to_physical_precise_round(scale);
    // The paths that draw the surfaces themselves apply what
    // `Window::render_elements` would: an X11 window's own opacity.
    let surface_alpha = entry.alpha * own_opacity(&entry.window);
    match (surfaces, rounding.zip(shaders)) {
        (Surfaces::MainOnly, rounded) => {
            let main = entry
                .window
                .wl_surface()
                .map(|surface| main_surface_element(renderer, &surface, location, surface_alpha))
                .unwrap_or_default();
            match rounded {
                Some((rounding, shaders)) => elements.extend(
                    rounded::clip_surfaces(main, shaders, rounding, output_location, scale)
                        .map(PilaRenderElement::from),
                ),
                None => elements.extend(main.into_iter().map(PilaRenderElement::Surface)),
            }
        }
        // By hand when the popups need their backdrops, or a part of them.
        (Surfaces::Tree, rounded)
            if rounded.is_some() || first_popup > 0 || !entry.popup_blur.is_empty() =>
        {
            let popup_backdrop =
                (!entry.popup_blur.is_empty()).then_some(&mut *backdrop as Backdrop<'_>);
            elements.extend(window_tree_elements(
                renderer,
                &entry.window,
                location,
                first_popup,
                rounded,
                output_location,
                scale,
                surface_alpha,
                popup_backdrop,
            ))
        }
        // Smithay's own.
        (Surfaces::Tree, _) => {
            elements.extend(AsRenderElements::<GlesRenderer>::render_elements::<
                PilaRenderElement,
            >(
                &entry.window, renderer, location, scale, entry.alpha
            ))
        }
    }
    if let Some(bar) = &entry.title_bar {
        let rect = Rectangle::new(bar.rect.loc - output_location, bar.rect.size);
        elements.extend(decoration::title_bar_elements(
            renderer,
            &entry.window,
            rect,
            bar.focused,
            bar.hover,
            &bar.colors,
            rounding.map(|r| r.radius),
            entry.alpha,
            scale,
        ));
    }
    if !entry.blur.is_empty() {
        if let Some(surface) = entry.window.wl_surface() {
            // Whole whatever its opacity: that's of what it draws.
            elements.extend(backdrop(&surface, output_location, scale, 1.0));
        }
    }
    if let (Some(shadow), Some(shaders)) = (&entry.shadow, shaders) {
        elements.push(PilaRenderElement::Shadow(ShadowElement::new(
            &entry.window,
            shadow,
            shaders,
            entry.alpha,
            output_location,
            scale,
        )));
    }
    elements
}

/// The opacity an X11 window sets on itself (`_NET_WM_WINDOW_OPACITY`), as
/// Smithay's `X11Surface::render_elements` applies it; 1 for anything else.
fn own_opacity(window: &Window) -> f32 {
    window
        .x11_surface()
        .and_then(|x11| x11.opacity())
        .map_or(1.0, |opacity| opacity as f32 / u32::MAX as f32)
}

/// `window`'s main surface at `location`, from its renderer state alone
/// (`Surfaces::MainOnly`).
fn main_surface_element(
    renderer: &mut GlesRenderer,
    surface: &WlSurface,
    location: Point<i32, Physical>,
    alpha: f32,
) -> Vec<WaylandSurfaceRenderElement<GlesRenderer>> {
    with_states(surface, |states| {
        WaylandSurfaceRenderElement::from_surface(
            renderer,
            surface,
            states,
            location.to_f64(),
            alpha,
            Kind::Unspecified,
        )
    })
    .ok()
    .flatten()
    .into_iter()
    .collect()
}

/// A window drawn into a texture, so an animation can fade and scale it as
/// a whole (`animation.rs`). Kept per window and scale while it animates.
pub struct OffscreenWindow {
    scale: Scale<f64>,
    size: Size<i32, Physical>,
    texture: GlesTexture,
    buffer: TextureRenderBuffer<GlesTexture>,
    tracker: OutputDamageTracker,
    drawn: bool,
}

impl OffscreenWindow {
    pub(crate) fn new(
        renderer: &mut GlesRenderer,
        size: Size<i32, Physical>,
        scale: Scale<f64>,
    ) -> Option<Self> {
        let buffer_size: Size<i32, Buffer> = (size.w, size.h).into();
        let texture: GlesTexture =
            Offscreen::<GlesTexture>::create_buffer(renderer, Fourcc::Abgr8888, buffer_size)
                .map_err(|err| tracing::debug!(?err, "animation: no offscreen texture"))
                .ok()?;
        let buffer = TextureRenderBuffer::from_texture(
            renderer,
            texture.clone(),
            1,
            Transform::Normal,
            None,
        );
        Some(Self {
            scale,
            size,
            texture,
            buffer,
            tracker: OutputDamageTracker::new(size, scale, Transform::Normal),
            drawn: false,
        })
    }

    /// A texture for the windows of the output at `rect`, cleared: created
    /// and drawn into once, so that its first use in a frame costs no more
    /// than any other (`workspace_slide.rs`, `SlidePool`).
    pub fn for_output(
        renderer: &mut GlesRenderer,
        rect: Rectangle<i32, Logical>,
        scale: Scale<f64>,
    ) -> Option<Self> {
        let mut texture = Self::new(renderer, texture_size(rect, scale), scale)?;
        texture.draw(renderer, &[]).then_some(texture)
    }

    /// Whether it fits the output at `rect` drawn at `scale`.
    pub fn fits(&self, rect: Rectangle<i32, Logical>, scale: Scale<f64>) -> bool {
        self.scale == scale && self.size == texture_size(rect, scale)
    }

    /// Forget what was drawn: the next `draw` redraws all of it.
    pub fn reset(&mut self) {
        self.tracker = OutputDamageTracker::new(self.size, self.scale, Transform::Normal);
        self.drawn = false;
    }

    /// What's drawn in it, to show.
    pub fn buffer(&self) -> &TextureRenderBuffer<GlesTexture> {
        &self.buffer
    }

    /// Its size in pixels.
    pub fn size(&self) -> Size<i32, Physical> {
        self.size
    }

    /// Draw `elements` (relative to the texture's top-left) into it.
    pub(crate) fn draw(
        &mut self,
        renderer: &mut GlesRenderer,
        elements: &[PilaRenderElement],
    ) -> bool {
        self.draw_damaged(renderer, elements).is_some()
    }

    /// `draw`, saying whether anything changed: `None` if it failed.
    pub(crate) fn draw_damaged(
        &mut self,
        renderer: &mut GlesRenderer,
        elements: &[PilaRenderElement],
    ) -> Option<bool> {
        let age = usize::from(self.drawn);
        let damage = {
            let mut texture = self.texture.clone();
            let Ok(mut target) = renderer.bind(&mut texture) else {
                return None;
            };
            match self
                .tracker
                .render_output(renderer, &mut target, age, elements, [0.0; 4])
            {
                Ok(result) => result.damage.cloned().unwrap_or_default(),
                Err(err) => {
                    tracing::debug!(?err, "animation: offscreen render failed");
                    return None;
                }
            }
        };
        self.drawn = true;
        if damage.is_empty() {
            return Some(false);
        }
        let damage: Vec<Rectangle<i32, Buffer>> = damage
            .iter()
            .map(|r| Rectangle::new((r.loc.x, r.loc.y).into(), (r.size.w, r.size.h).into()))
            .collect();
        let _ = self.buffer.render().draw(|_| Ok::<_, ()>(damage));
        Some(true)
    }
}

/// Do once, at startup, what the first window and the first animation would
/// otherwise pay for inside a frame (performance pillar): compile the
/// rounded-corner and blur shaders, create an offscreen texture and draw into
/// it what a window is made of: a solid, a rounded one and a texture uploaded
/// from memory (a title, a button), blended. The driver builds what a kind of
/// draw needs the first time it's made, not when its shader is compiled: left
/// to the first window, that more than doubled its first frame.
pub fn warm_up(renderer: &mut GlesRenderer) {
    const SIZE: i32 = 64;
    let start = std::time::Instant::now();
    let _ = rounded::Shaders::get(renderer);
    let _ = crate::blur::Shaders::get(renderer);
    let scale = Scale::from(1.0);
    if let Some(mut target) = OffscreenWindow::new(renderer, (SIZE, SIZE).into(), scale) {
        let buffer = SolidColorBuffer::new((SIZE, SIZE), [0.0, 0.0, 0.0, 1.0]);
        // Front to back.
        let mut elements = Vec::with_capacity(3);
        let pixels = [0x80u8; 16 * 16 * 4];
        let memory = MemoryRenderBuffer::from_slice(
            &pixels,
            Fourcc::Argb8888,
            (16, 16),
            1,
            Transform::Normal,
            None,
        );
        elements.extend(
            MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                (8.0, 8.0),
                &memory,
                Some(0.5),
                None,
                None,
                Kind::Unspecified,
            )
            .ok()
            .map(PilaRenderElement::Memory),
        );
        let origin = Point::from((0, 0));
        elements.push(solid_element(
            renderer,
            &buffer,
            origin,
            Some((8.0, Corners::All)),
            0.5,
            scale,
        ));
        elements.push(solid_element(renderer, &buffer, origin, None, 1.0, scale));
        target.draw(renderer, &elements);
    }
    tracing::debug!(elapsed = ?start.elapsed(), "renderer warmed up");
}

/// The physical size of a texture holding `rect` at `scale`.
pub(crate) fn texture_size(
    rect: Rectangle<i32, Logical>,
    scale: Scale<f64>,
) -> Size<i32, Physical> {
    let size: Size<i32, Physical> = rect.size.to_f64().to_physical(scale).to_i32_ceil();
    (size.w.max(1), size.h.max(1)).into()
}

/// A texture of `snapshot`-like content drawn at `dst` (space coordinates).
pub(crate) fn texture_element(
    buffer: &TextureRenderBuffer<GlesTexture>,
    texture_size: Size<i32, Physical>,
    dst: Rectangle<f64, Logical>,
    alpha: f32,
    output_location: Point<i32, Logical>,
    scale: Scale<f64>,
) -> PilaRenderElement {
    let location = (dst.loc - output_location.to_f64()).to_physical(scale);
    let size: Size<i32, Logical> = dst.size.to_i32_round();
    // All of the texture (its pixels are its logical units, at buffer
    // scale 1): without it Smithay samples a `size`-sized corner, cropping
    // a shrunk texture and repeating a stretched one.
    let src = Rectangle::from_size((f64::from(texture_size.w), f64::from(texture_size.h)).into());
    PilaRenderElement::Texture(TextureRenderElement::from_texture_render_buffer(
        location,
        buffer,
        Some(alpha),
        Some(src),
        Some((size.w.max(1), size.h.max(1)).into()),
        Kind::Unspecified,
    ))
}

/// Where something covering `rect` is drawn while `effect` transforms it.
fn effect_rect(effect: &WindowEffect, rect: Rectangle<i32, Logical>) -> Rectangle<f64, Logical> {
    match &effect.frame {
        Some(frame) => frame.apply(rect.to_f64()),
        None => effect.transform.apply(rect.to_f64(), effect.center),
    }
}

/// Makes something's elements relative to an origin, at a scale.
type DrawAt<'a> =
    &'a mut dyn FnMut(&mut GlesRenderer, Point<i32, Logical>, Scale<f64>) -> Vec<PilaRenderElement>;

/// Something animating (a window or layer surface whose `user_data` keeps
/// its textures): `draw` makes its elements relative to an origin, which go
/// into its texture for this scale (covering `rect`), shown transformed by
/// `effect`. `None` if that fails (draw it plainly then).
#[allow(clippy::too_many_arguments)]
fn animated_element(
    renderer: &mut GlesRenderer,
    user_data: &smithay::utils::user_data::UserDataMap,
    rect: Rectangle<i32, Logical>,
    effect: &WindowEffect,
    output_location: Point<i32, Logical>,
    scale: Scale<f64>,
    draw: DrawAt<'_>,
) -> Option<PilaRenderElement> {
    let size = texture_size(rect, scale);
    let textures = user_data.get_or_insert(AnimTextures::default);
    let buffer = {
        let mut textures = textures.0.borrow_mut();
        textures.retain(|t| t.scale != scale || t.size == size);
        let index = match textures.iter().position(|t| t.scale == scale) {
            Some(index) => index,
            None => {
                textures.push(OffscreenWindow::new(renderer, size, scale)?);
                textures.len() - 1
            }
        };
        let target = &mut textures[index];
        let elements = draw(renderer, rect.loc, scale);
        if elements.is_empty() || !target.draw(renderer, &elements) {
            return None;
        }
        (target.buffer.clone(), target.size)
    };
    let (buffer, texture_size) = buffer;
    let dst = effect_rect(effect, rect);
    Some(texture_element(
        &buffer,
        texture_size,
        dst,
        effect.transform.alpha,
        output_location,
        scale,
    ))
}

/// An animating window: drawn into its texture for this scale, then shown
/// transformed. Falls back to its plain elements if that fails.
fn animated_window_element(
    renderer: &mut GlesRenderer,
    entry: &SceneWindow,
    effect: &WindowEffect,
    shaders: Option<&rounded::Shaders>,
    output_location: Point<i32, Logical>,
    scale: Scale<f64>,
    backdrop: Backdrop<'_>,
) -> Vec<PilaRenderElement> {
    // Hidden for now (a snapshot stands in while its client resizes).
    if effect.transform.alpha <= 0.0 {
        return Vec::new();
    }
    let mut draw = |r: &mut GlesRenderer, origin, s| {
        single_window_elements(
            r,
            entry,
            shaders,
            Surfaces::Tree,
            0,
            origin,
            s,
            &mut *backdrop,
        )
    };
    let user_data = entry.window.user_data();
    match animated_element(
        renderer,
        user_data,
        entry.content_rect(),
        effect,
        output_location,
        scale,
        &mut draw,
    ) {
        Some(element) => vec![element],
        None => draw(renderer, output_location, scale),
    }
}

/// The windows layer of the output at `rect` while a workspace slide runs:
/// the carried window as usual, in front; the new workspace's windows drawn
/// into the output's texture and coming in from one side; the old picture
/// leaving on the other. Both are cropped to the output.
fn slide_elements(
    renderer: &mut GlesRenderer,
    windows: &[SceneWindow],
    ghosts: &[SceneGhost],
    slide: &SceneSlide,
    rect: Rectangle<i32, Logical>,
    scale: Scale<f64>,
    backdrop: Backdrop<'_>,
) -> Vec<PilaRenderElement> {
    let is_carried = |entry: &SceneWindow| slide.carried.contains(&entry.window);
    let (carried, moving): (Vec<SceneWindow>, Vec<SceneWindow>) =
        windows.iter().cloned().partition(is_carried);
    // Ghosts are placed by index among all the windows: shift those behind
    // carried windows, which aren't among the moving ones.
    let ghosts: Vec<SceneGhost> = ghosts
        .iter()
        .map(|ghost| SceneGhost {
            before: ghost.before
                - windows
                    .iter()
                    .take(ghost.before)
                    .filter(|entry| is_carried(entry))
                    .count(),
            ..ghost.clone()
        })
        .collect();
    let mut elements = window_elements(
        renderer,
        &carried,
        &[],
        rect.loc,
        scale,
        &mut *backdrop,
        None,
    );

    let new = {
        let mut textures = slide.textures.borrow_mut();
        let size = texture_size(rect, scale);
        textures.retain(|(r, t)| *r != rect || (t.scale == scale && t.size == size));
        let index = match textures.iter().position(|(r, _)| *r == rect) {
            Some(index) => Some(index),
            None => OffscreenWindow::new(renderer, size, scale).map(|t| {
                textures.push((rect, t));
                textures.len() - 1
            }),
        };
        index.and_then(|index| {
            let target = &mut textures[index].1;
            let windows = window_elements(
                renderer,
                &moving,
                &ghosts,
                rect.loc,
                scale,
                &mut *backdrop,
                None,
            );
            // An empty workspace draws nothing: the texture is just cleared.
            target
                .draw(renderer, &windows)
                .then(|| target.buffer.clone())
        })
    };
    let width = f64::from(rect.size.w);
    let new_offset = slide.direction * width * (1.0 - slide.progress);
    let old_offset = -slide.direction * width * slide.progress;
    match new {
        Some(buffer) => elements.extend(slid_texture(&buffer, rect, new_offset, scale)),
        // No texture: the new windows as usual, unmoved.
        None => elements.extend(window_elements(
            renderer, &moving, &ghosts, rect.loc, scale, backdrop, None,
        )),
    }
    if let Some(old) = slide.old_buffer(rect) {
        elements.extend(slid_texture(old, rect, old_offset, scale));
    }
    elements
}

/// A texture of the output at `rect` drawn `offset` logical pixels to the
/// side: only the part still on the output.
fn slid_texture(
    buffer: &TextureRenderBuffer<GlesTexture>,
    rect: Rectangle<i32, Logical>,
    offset: f64,
    scale: Scale<f64>,
) -> Option<PilaRenderElement> {
    let offset = offset.round() as i32;
    let visible = rect.size.w - offset.abs();
    if visible <= 0 {
        return None;
    }
    // Relative to the output; the texture's pixels are physical.
    let dst_x = offset.max(0);
    let src_x = (-offset).max(0);
    let location: Point<f64, Physical> = Point::<i32, Logical>::from((dst_x, 0))
        .to_f64()
        .to_physical(scale);
    let src = Rectangle::new(
        (f64::from(src_x) * scale.x, 0.0).into(),
        (
            f64::from(visible) * scale.x,
            f64::from(rect.size.h) * scale.y,
        )
            .into(),
    );
    Some(PilaRenderElement::Texture(
        TextureRenderElement::from_texture_render_buffer(
            location,
            buffer,
            None,
            Some(src),
            Some((visible, rect.size.h).into()),
            Kind::Unspecified,
        ),
    ))
}

/// The windows (and ghosts) on the output at `rect`, as they look now,
/// drawn into `target` (the picture a workspace slide moves away): as a
/// slide in progress (`slide`) shows them, if one is running there, so a
/// new one starts from what's on screen. Whether it could be drawn.
#[allow(clippy::too_many_arguments)]
pub fn snapshot_windows(
    renderer: &mut GlesRenderer,
    windows: &[SceneWindow],
    ghosts: &[SceneGhost],
    slide: Option<&SceneSlide>,
    rect: Rectangle<i32, Logical>,
    scale: Scale<f64>,
    backdrop: Backdrop<'_>,
    target: &mut OffscreenWindow,
) -> bool {
    let elements = match slide.filter(|slide| slide.output_rect(rect.loc).is_some()) {
        Some(slide) => slide_elements(renderer, windows, ghosts, slide, rect, scale, backdrop),
        None => window_elements(renderer, windows, ghosts, rect.loc, scale, backdrop, None),
    };
    target.draw(renderer, &elements)
}

/// `elements` drawn into a new texture covering `rect` at `scale`.
fn snapshot(
    renderer: &mut GlesRenderer,
    rect: Rectangle<i32, Logical>,
    center: Point<f64, Logical>,
    scale: Scale<f64>,
    elements: &[PilaRenderElement],
) -> Option<Snapshot> {
    if elements.is_empty() || rect.is_empty() {
        return None;
    }
    let mut target = OffscreenWindow::new(renderer, texture_size(rect, scale), scale)?;
    target.draw(renderer, elements).then(|| Snapshot {
        buffer: target.buffer,
        size: target.size,
        rect,
        center,
    })
}

/// The layer surface `layer`, placed at `rect` (its place in the layer map),
/// as it looks now, drawn into a texture (for one about to go).
pub fn snapshot_layer(
    renderer: &mut GlesRenderer,
    layer: &LayerSurface,
    rect: Rectangle<i32, Logical>,
    surfaces: Surfaces,
    scale: Scale<f64>,
) -> Option<Snapshot> {
    // The popups too, while the tree is still there.
    let content = match surfaces {
        Surfaces::Tree => rect.merge(layer_content_rect(layer, rect.loc)),
        Surfaces::MainOnly => rect,
    };
    let elements = single_layer_elements(
        renderer,
        layer,
        rect.loc,
        surfaces,
        0,
        content.loc,
        scale,
        None,
    );
    snapshot(renderer, content, animation::center(rect), scale, &elements)
}

/// `entry` as it looks now, drawn into a texture at `scale` (for a window
/// about to go). `None` if it has nothing to draw.
pub fn snapshot_window(
    renderer: &mut GlesRenderer,
    entry: &SceneWindow,
    surfaces: Surfaces,
    scale: Scale<f64>,
    backdrop: Backdrop<'_>,
) -> Option<Snapshot> {
    let rect = entry.content_rect();
    let shaders = (entry.rounding.is_some() || entry.shadow.is_some())
        .then(|| rounded::Shaders::get(renderer))
        .flatten();
    // A window whose client already took its buffer away has nothing left.
    if entry.window.geometry().is_empty() {
        return None;
    }
    let elements = single_window_elements(
        renderer,
        entry,
        shaders.as_ref(),
        surfaces,
        0,
        rect.loc,
        scale,
        backdrop,
    );
    snapshot(
        renderer,
        rect,
        animation::center(entry.frame_rect()),
        scale,
        &elements,
    )
}

fn ghost_element(
    ghost: &SceneGhost,
    output_location: Point<i32, Logical>,
    scale: Scale<f64>,
) -> PilaRenderElement {
    let snapshot = &ghost.snapshot;
    let dst = match &ghost.frame {
        Some(frame) => frame.apply(snapshot.rect.to_f64()),
        None => ghost
            .transform
            .apply(snapshot.rect.to_f64(), snapshot.center),
    };
    texture_element(
        &snapshot.buffer,
        snapshot.size,
        dst,
        ghost.transform.alpha,
        output_location,
        scale,
    )
}

/// A window's elements by hand, as `Window::render_elements` makes them
/// (popups in front, unclipped): its popups from the `first_popup`th on
/// (`popup_elements`), then its own surfaces, clipped to `rounded` if given.
#[allow(clippy::too_many_arguments)]
fn window_tree_elements(
    renderer: &mut GlesRenderer,
    window: &Window,
    location: Point<i32, Physical>,
    first_popup: usize,
    rounded: Option<(&Rounding, &rounded::Shaders)>,
    output_location: Point<i32, Logical>,
    scale: Scale<f64>,
    alpha: f32,
    backdrop: Option<Backdrop<'_>>,
) -> Vec<PilaRenderElement> {
    let Some(surface) = window.wl_surface() else {
        return Vec::new();
    };
    let mut elements = match window.toplevel() {
        Some(toplevel) => popup_elements(
            renderer,
            toplevel.wl_surface(),
            location,
            window.geometry().loc,
            first_popup,
            output_location,
            scale,
            alpha,
            backdrop,
        ),
        None => Vec::new(),
    };
    let surfaces = render_elements_from_surface_tree::<_, WaylandSurfaceRenderElement<GlesRenderer>>(
        renderer,
        &surface,
        location,
        scale,
        alpha,
        Kind::Unspecified,
    );
    match rounded {
        Some((rounding, shaders)) => elements.extend(
            rounded::clip_surfaces(surfaces, shaders, rounding, output_location, scale)
                .map(PilaRenderElement::from),
        ),
        None => elements.extend(surfaces.into_iter().map(PilaRenderElement::Surface)),
    }
    elements
}

/// The popups of `root` (a toplevel's or a layer surface's) from the `first`th
/// on, front to back as `PopupManager::popups_for_surface` gives them and
/// Smithay's `render_elements` draw them, placed from `location` plus `base`
/// (a window's geometry offset). Each has its blurred backdrop right behind
/// it when `backdrop` is given and one was prepared (`blur.rs`).
#[allow(clippy::too_many_arguments)]
fn popup_elements(
    renderer: &mut GlesRenderer,
    root: &WlSurface,
    location: Point<i32, Physical>,
    base: Point<i32, Logical>,
    first: usize,
    output_location: Point<i32, Logical>,
    scale: Scale<f64>,
    alpha: f32,
    mut backdrop: Option<Backdrop<'_>>,
) -> Vec<PilaRenderElement> {
    let mut elements = Vec::new();
    for (popup, offset) in PopupManager::popups_for_surface(root).skip(first) {
        let offset: Point<i32, Physical> =
            (base + offset - popup.geometry().loc).to_physical_precise_round(scale);
        elements.extend(render_elements_from_surface_tree::<_, PilaRenderElement>(
            renderer,
            popup.wl_surface(),
            location + offset,
            scale,
            alpha,
            Kind::Unspecified,
        ));
        if let Some(backdrop) = backdrop.as_mut() {
            // Whole whatever its opacity, as a window's.
            elements.extend(backdrop(popup.wl_surface(), output_location, scale, 1.0));
        }
    }
    elements
}

/// A solid color at `loc` (relative to the output), with its corners
/// rounded when there's a rounding and the shaders compiled.
pub(crate) fn solid_element(
    renderer: &mut GlesRenderer,
    buffer: &SolidColorBuffer,
    loc: Point<i32, Logical>,
    rounding: Option<(f64, Corners)>,
    alpha: f32,
    scale: Scale<f64>,
) -> PilaRenderElement {
    let loc: Point<i32, Physical> = loc.to_physical_precise_round(scale);
    let element =
        SolidColorRenderElement::from_buffer(buffer, loc, scale, alpha, Kind::Unspecified);
    match rounding.zip(rounding.and_then(|_| rounded::Shaders::get(renderer))) {
        Some(((radius, corners), shaders)) => PilaRenderElement::RoundedSolid(RoundedSolid::new(
            element, &shaders, radius, corners, scale,
        )),
        None => PilaRenderElement::Solid(element),
    }
}

/// Elements for a client-drawn cursor surface whose top-left is at `location`
/// (from `cursor::surface_position`), relative to the output origin; with
/// `magnification`, that many times bigger around its hotspot at `pointer`.
pub fn cursor_elements(
    renderer: &mut GlesRenderer,
    cursor: Option<&(WlSurface, Point<i32, Logical>)>,
    pointer: Point<f64, Logical>,
    magnification: Option<f64>,
    output_location: Point<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement> {
    let Some((surface, location)) = cursor else {
        return Vec::new();
    };
    let location: Point<i32, Physical> =
        (*location - output_location).to_physical_precise_round(scale);
    let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
        render_elements_from_surface_tree(renderer, surface, location, scale, 1.0, Kind::Cursor);
    let Some(factor) = magnification else {
        return elements
            .into_iter()
            .map(PilaRenderElement::Surface)
            .collect();
    };
    // Grown around the hotspot: the rounded pointer, as `location` is.
    let hotspot = (pointer.to_i32_round() - output_location).to_physical_precise_round(scale);
    elements
        .into_iter()
        .map(|element| {
            PilaRenderElement::Rescaled(RescaleRenderElement::from_element(
                element, hotspot, factor,
            ))
        })
        .collect()
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

/// Elements for a named cursor image drawn `factor` times the theme's
/// `size`, with its hotspot at `pointer` (`shake_cursor.rs`).
pub fn magnified_cursor_elements(
    renderer: &mut GlesRenderer,
    cursor: &MagnifiedCursor,
    size: u32,
    factor: f64,
    pointer: Point<f64, Logical>,
    output_location: Point<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement> {
    let k = cursor.scale(size, factor);
    let shown = cursor.size.to_f64().upscale(k).to_i32_round();
    let location = (pointer - cursor.hotspot.to_f64().upscale(k) - output_location.to_f64())
        .to_physical(scale);
    MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        location,
        &cursor.buffer,
        None,
        // The whole image (at buffer scale 1, its pixels are its logical size).
        Some(Rectangle::from_size(cursor.size.to_f64())),
        Some(shown),
        Kind::Cursor,
    )
    .map(|element| vec![PilaRenderElement::Memory(element)])
    .unwrap_or_default()
}

/// The icon a client gave its drag (`wl_data_device.start_drag`), drawn at
/// the pointer plus `offset`, which its `wl_surface.offset`s move.
pub struct DndIcon {
    pub surface: WlSurface,
    pub offset: Point<i32, Logical>,
}

/// What drawing a frame needs from `Pila` (`Pila::frame_input`), taken
/// before the backend's renderer is borrowed.
pub struct FrameInput {
    pub scene: Scene,
    /// The pointer, in global logical coordinates.
    pub pointer: Point<f64, Logical>,
    /// The client cursor surface and its top-left (`cursor::surface_position`).
    pub cursor_surface: Option<(WlSurface, Point<i32, Logical>)>,
    /// The named cursor, if the backend draws it (bare metal; nested and
    /// headless while it's magnified).
    pub named_cursor: Option<CursorIcon>,
    /// How many times bigger the cursor is drawn (`shake_cursor.rs`);
    /// `None` at its own size.
    pub magnification: Option<f64>,
}

impl FrameInput {
    pub fn cursor_wl_surface(&self) -> Option<&WlSurface> {
        self.cursor_surface.as_ref().map(|(surface, _)| surface)
    }
}

/// The cursor's elements, relative to `origin`: the client's cursor surface,
/// or the named cursor from `theme` (if the backend draws it: see
/// `FrameInput::named_cursor`), magnified as `FrameInput::magnification` says.
pub fn pointer_elements(
    renderer: &mut GlesRenderer,
    input: &FrameInput,
    theme: &mut CursorTheme,
    origin: Point<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement> {
    let mut elements = cursor_elements(
        renderer,
        input.cursor_surface.as_ref(),
        input.pointer,
        input.magnification,
        origin,
        scale,
    );
    let Some(icon) = input.named_cursor else {
        return elements;
    };
    match input.magnification {
        None => {
            let image = theme.get(icon, crate::scale::buffer_scale(scale));
            elements.extend(named_cursor_elements(
                renderer,
                &image,
                input.pointer,
                origin,
                scale,
            ));
        }
        Some(factor) => {
            let image = theme.get_magnified(icon, scale.x, factor);
            elements.extend(magnified_cursor_elements(
                renderer,
                &image,
                theme.size(),
                factor,
                input.pointer,
                origin,
                scale,
            ));
        }
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
    let mut elements = match scene
        .switcher
        .as_ref()
        .filter(|view| !scene.locked && view.output == output_name)
    {
        Some(view) => crate::switcher_view::elements(renderer, view, &scene.colors, origin, scale),
        None => Vec::new(),
    };
    if let Some(view) = scene
        .shortcuts
        .as_ref()
        .filter(|view| !scene.locked && view.output == output_name)
    {
        // In front of the switcher: it's first, front to back.
        let mut front =
            crate::shortcuts_view::elements(renderer, view, &scene.colors, origin, scale);
        front.append(&mut elements);
        elements = front;
    }
    if let Some(view) = scene.share_picker.as_ref().filter(|_| !scene.locked) {
        // In front of everything of Pila's: it's modal.
        let mut front = crate::share_picker::elements(
            renderer,
            view,
            &scene.colors,
            output_name,
            Rectangle::new(origin, output_geo.size),
            scale,
        );
        front.append(&mut elements);
        elements = front;
    }
    if let Some(menu) = scene.window_menu.as_ref().filter(|_| !scene.locked) {
        // In front of all of them: it's first, front to back.
        let mut front = crate::window_menu::elements(renderer, menu, &scene.colors, origin, scale);
        front.append(&mut elements);
        elements = front;
    }
    elements.extend(dnd_icon_elements(renderer, scene, origin, scale));
    elements.extend(scene_elements(
        renderer,
        scene,
        origin,
        scale,
        &mut |surface, origin, scale, alpha| {
            blur.element(surface, output_name, origin, scale, alpha)
        },
        Some(output_geo),
    ));
    elements
}

/// The drag icon's elements, relative to `origin` (front of the scene).
fn dnd_icon_elements(
    renderer: &mut GlesRenderer,
    scene: &Scene,
    origin: Point<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement> {
    let Some((surface, location)) = scene.dnd_icon.as_ref().filter(|_| !scene.locked) else {
        return Vec::new();
    };
    let location = (*location - origin).to_physical_precise_round(scale);
    render_elements_from_surface_tree(
        renderer,
        surface,
        location,
        scale,
        1.0,
        Kind::ScanoutCandidate,
    )
}

/// A renderer error, as it's logged.
pub fn error_text(err: &dyn std::fmt::Debug) -> String {
    format!("{err:?}")
}
