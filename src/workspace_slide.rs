//! The workspace switch animation: the windows of the workspace being left
//! slide out sideways while the new one's slide in from the other side (from
//! the right when going to a higher workspace), on each output separately,
//! so nothing crosses onto a neighboring one. Panels and the wallpaper stay
//! put; a window carried along by a drag is drawn as usual, in front.
//!
//! When the switch starts (`start_workspace_slide`, before the windows are
//! swapped) each output's windows are drawn into a texture as they look.
//! Then, each frame, `render.rs` draws the new workspace's windows into
//! another texture per output (`SlideTextures`) and shows both, shifted and
//! cropped to the output (`render::slide_elements`). Input sees the new
//! workspace at once; the animation is only drawn (`[animations]`
//! `workspace`).

use std::{cell::RefCell, rc::Rc, time::Duration};

use smithay::{
    backend::renderer::{element::texture::TextureRenderBuffer, gles::GlesTexture},
    desktop::Window,
    utils::{Logical, Rectangle},
};
use tracing::debug;

use crate::{
    animation::{Animation, Curve},
    config::AnimationKind,
    render::{self, OffscreenWindow},
    Pila,
};

/// The new workspace's windows drawn into a texture, per output (where it
/// is), reused from frame to frame while the slide lasts.
pub type SlideTextures = Rc<RefCell<Vec<(Rectangle<i32, Logical>, OffscreenWindow)>>>;

/// A workspace switch being animated.
pub struct WorkspaceSlide {
    progress: Animation,
    /// 1: the new workspace comes in from the right; -1: from the left.
    direction: f64,
    /// Per output, where it is and its windows as they were.
    old: Vec<(Rectangle<i32, Logical>, TextureRenderBuffer<GlesTexture>)>,
    /// A window dragged along to the new workspace, and those on all of them:
    /// drawn as usual.
    carried: Vec<Window>,
    textures: SlideTextures,
}

/// The slide as it's drawn this frame (`Scene::slide`).
#[derive(Clone)]
pub struct SceneSlide {
    /// How far the new workspace has come in, 0 to 1.
    pub progress: f64,
    pub direction: f64,
    pub old: Vec<(Rectangle<i32, Logical>, TextureRenderBuffer<GlesTexture>)>,
    pub carried: Vec<Window>,
    pub textures: SlideTextures,
}

impl SceneSlide {
    /// Where the output whose top-left is at `output_location` is, if the
    /// slide covers it (an output added meanwhile isn't).
    pub fn output_rect(
        &self,
        output_location: smithay::utils::Point<i32, Logical>,
    ) -> Option<Rectangle<i32, Logical>> {
        self.old
            .iter()
            .map(|(rect, _)| *rect)
            .find(|rect| rect.loc == output_location)
    }

    /// The old workspace's picture of the output at `rect`.
    pub fn old_buffer(
        &self,
        rect: Rectangle<i32, Logical>,
    ) -> Option<&TextureRenderBuffer<GlesTexture>> {
        self.old.iter().find(|(r, _)| *r == rect).map(|(_, b)| b)
    }
}

impl Pila {
    /// The active workspace is about to become `index`: take each output's
    /// picture of the windows now, for them to slide out. Call before the
    /// windows are swapped; `carried` stay out of the picture.
    pub fn start_workspace_slide(&mut self, index: usize, carried: Vec<Window>) {
        // A slide still running: the new one starts from what it shows.
        let running = self.scene_slide(self.animation_time());
        self.workspace_slide = None;
        let Some(duration) = self
            .config
            .animations
            .duration(AnimationKind::Workspace)
            .filter(|_| !self.is_locked())
        else {
            return;
        };
        let now = self.animation_time();
        let (windows, ghosts) = self.scene_windows(now);
        let windows: Vec<_> = windows
            .into_iter()
            .filter(|w| !carried.contains(&w.window))
            .collect();
        let outputs: Vec<_> = self
            .space
            .outputs()
            .filter_map(|output| {
                let rect = self.space.output_geometry(output)?;
                Some((rect, crate::scale::output_scale(output), output.name()))
            })
            .collect();
        let Pila { backend, blur, .. } = self;
        let Some(mut renderer) = backend.as_mut().and_then(|b| b.renderer()) else {
            return;
        };
        let renderer = &mut *renderer;
        let old: Vec<_> = outputs
            .into_iter()
            .filter_map(|(rect, scale, name)| {
                let buffer = render::snapshot_windows(
                    renderer,
                    &windows,
                    &ghosts,
                    running.as_ref(),
                    rect,
                    scale,
                    &mut |surface, origin, scale, alpha| {
                        blur.element(surface, &name, origin, scale, alpha)
                    },
                );
                if buffer.is_none() {
                    debug!(?rect, "no picture of an output's windows; no slide there");
                }
                Some((rect, buffer?))
            })
            .collect();
        if old.is_empty() {
            return;
        }
        let direction = if index > self.workspaces.active {
            1.0
        } else {
            -1.0
        };
        crate::animation::log_workspace_slide(index);
        self.workspace_slide = Some(WorkspaceSlide {
            progress: Animation::new(now, duration, Curve::EaseOut, 0.0, 1.0),
            direction,
            old,
            carried,
            textures: Rc::default(),
        });
        self.queue_redraw();
    }

    /// The slide to draw this frame, if one is running (and not over).
    pub fn scene_slide(&self, now: Duration) -> Option<SceneSlide> {
        let slide = self.workspace_slide.as_ref()?;
        Some(SceneSlide {
            progress: slide.progress.value(now),
            direction: slide.direction,
            old: slide.old.clone(),
            carried: slide.carried.clone(),
            textures: slide.textures.clone(),
        })
    }

    /// Whether a slide is still running at `now`.
    pub fn sliding(&self, now: Duration) -> bool {
        self.workspace_slide
            .as_ref()
            .is_some_and(|slide| !slide.progress.done(now))
    }

    /// Drop a finished slide (`step_animations`); whether there was one.
    pub fn end_finished_slide(&mut self, now: Duration) -> bool {
        let done = self
            .workspace_slide
            .as_ref()
            .is_some_and(|slide| slide.progress.done(now));
        if done {
            self.workspace_slide = None;
        }
        done
    }
}
