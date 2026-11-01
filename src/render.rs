//! Building a frame's render elements, front to back: the client cursor
//! surface, then each window (with its popups) followed by its title bar.
//!
//! Smithay's `render_output` draws the whole `Space` at once, which can't
//! interleave title bars with windows in stacking order, so pila builds the
//! list itself and hands it to the damage tracker. Backend-neutral: the scene
//! is collected from `Pila` first (`Pila::scene`), so a backend can build
//! elements while it holds a mutable borrow of its renderer.

use smithay::{
    backend::renderer::{
        element::{
            solid::SolidColorRenderElement,
            surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
            AsRenderElements, Kind,
        },
        ImportAll, Renderer,
    },
    desktop::Window,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Physical, Point, Rectangle, Scale},
};

use crate::decoration::{self, TitleBarPart};

/// The nested output is always scale 1.
const SCALE: f64 = 1.0;

smithay::backend::renderer::element::render_elements! {
    pub PilaRenderElement<R> where R: ImportAll;
    Surface=WaylandSurfaceRenderElement<R>,
    Solid=SolidColorRenderElement,
}

/// A mapped window as it should be drawn this frame.
pub struct SceneWindow {
    pub window: Window,
    /// Where the window's surface tree is drawn (geometry origin minus the
    /// geometry offset), in space coordinates.
    pub render_location: Point<i32, Logical>,
    pub title_bar: Option<SceneTitleBar>,
}

pub struct SceneTitleBar {
    pub rect: Rectangle<i32, Logical>,
    pub focused: bool,
    pub hover: Option<TitleBarPart>,
}

/// Elements for the windows in `scene` (front to back), relative to an output
/// whose top-left is at `output_location`.
pub fn scene_elements<R>(
    renderer: &mut R,
    scene: &[SceneWindow],
    output_location: Point<i32, Logical>,
) -> Vec<PilaRenderElement<R>>
where
    R: Renderer + ImportAll,
    R::TextureId: Clone + 'static,
{
    let scale = Scale::from(SCALE);
    let mut elements = Vec::new();
    for entry in scene {
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
            elements.extend(
                decoration::title_bar_elements(&entry.window, rect, bar.focused, bar.hover, scale)
                    .into_iter()
                    .map(PilaRenderElement::from),
            );
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
) -> Vec<PilaRenderElement<R>>
where
    R: Renderer + ImportAll,
    R::TextureId: Clone + 'static,
{
    let Some((surface, location)) = cursor else {
        return Vec::new();
    };
    let scale = Scale::from(SCALE);
    let location: Point<i32, Physical> =
        (*location - output_location).to_physical_precise_round(scale);
    render_elements_from_surface_tree(renderer, surface, location, scale, 1.0, Kind::Cursor)
}
