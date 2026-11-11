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
//!
//! The textures are as large as their outputs, and a new one costs the GPU
//! milliseconds (allocating it, clearing it): made as a slide starts, they
//! made its first frame miss a refresh. So they're kept (`SlidePool`): made
//! ahead for each output, before any slide (`prepare_slide_textures`), and
//! taken back when a slide ends.

use std::{cell::RefCell, rc::Rc, time::Duration};

use smithay::{
    backend::renderer::{
        element::texture::TextureRenderBuffer,
        gles::{GlesRenderer, GlesTexture},
    },
    desktop::Window,
    utils::{Logical, Rectangle, Scale},
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

/// How many textures are kept per output: the leaving picture and the new
/// workspace's, and a third for a switch during a slide, whose leaving
/// picture is drawn from the two in use.
const TEXTURES_PER_OUTPUT: usize = 3;
/// How many are made ahead: what a slide from rest takes.
const TEXTURES_AHEAD: usize = 2;

/// The textures slides draw into, kept between slides (`Pila::slide_pool`):
/// those no slide is using.
#[derive(Default)]
pub struct SlidePool {
    free: Vec<OffscreenWindow>,
}

impl SlidePool {
    /// A texture for the output at `rect`, to draw all of: one kept, else a
    /// new one (a cost the first frame pays; `prepare_slide_textures` is
    /// there to make it rare).
    fn take(
        &mut self,
        renderer: &mut GlesRenderer,
        rect: Rectangle<i32, Logical>,
        scale: Scale<f64>,
    ) -> Option<OffscreenWindow> {
        match self.free.iter().position(|t| t.fits(rect, scale)) {
            Some(index) => {
                let mut texture = self.free.swap_remove(index);
                texture.reset();
                Some(texture)
            }
            None => {
                debug!(?rect, "slide: a new texture, none was kept for this output");
                OffscreenWindow::for_output(renderer, rect, scale)
            }
        }
    }

    /// Take a slide's textures back.
    fn give_back(&mut self, slide: WorkspaceSlide) {
        self.free.extend(slide.old.into_iter().map(|(_, t)| t));
        self.free
            .extend(slide.textures.borrow_mut().drain(..).map(|(_, t)| t));
    }

    /// How many are kept for an output at `rect` and `scale`.
    fn kept(&self, rect: Rectangle<i32, Logical>, scale: Scale<f64>) -> usize {
        self.free.iter().filter(|t| t.fits(rect, scale)).count()
    }

    /// Drop those that fit none of `outputs`, and those past
    /// `TEXTURES_PER_OUTPUT` for one.
    fn trim(&mut self, outputs: &[(Rectangle<i32, Logical>, Scale<f64>)]) {
        let mut kept = vec![0usize; outputs.len()];
        self.free.retain(|texture| {
            let Some(index) = outputs
                .iter()
                .position(|(rect, scale)| texture.fits(*rect, *scale))
            else {
                return false;
            };
            // Outputs of one size share theirs.
            let same = outputs
                .iter()
                .filter(|(rect, scale)| texture.fits(*rect, *scale))
                .count();
            kept[index] += 1;
            kept[index] <= TEXTURES_PER_OUTPUT * same
        });
    }

    /// How many textures are kept (for tests, `Pila::state_json`).
    pub fn count(&self) -> usize {
        self.free.len()
    }
}

/// A workspace switch being animated.
pub struct WorkspaceSlide {
    progress: Animation,
    /// 1: the new workspace comes in from the right; -1: from the left.
    direction: f64,
    /// Per output, where it is and its windows as they were.
    old: Vec<(Rectangle<i32, Logical>, OffscreenWindow)>,
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
        // A slide still running: the new one starts from what it shows, so
        // its textures are in use until the leaving picture is drawn.
        let running = self.scene_slide(self.animation_time());
        let previous = self.workspace_slide.take();
        let Some(duration) = self
            .config
            .animations
            .duration(AnimationKind::Workspace)
            .filter(|_| !self.is_locked())
        else {
            self.retire_slide(previous);
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
        let Pila {
            backend,
            blur,
            slide_pool,
            ..
        } = self;
        let Some(mut renderer) = backend.as_mut().and_then(|b| b.renderer()) else {
            drop(running);
            self.retire_slide(previous);
            return;
        };
        let renderer = &mut *renderer;
        let mut old = Vec::new();
        // The new workspace's, drawn each frame (`render::slide_elements`).
        let mut textures = Vec::new();
        for (rect, scale, name) in outputs {
            let Some(mut leaving) = slide_pool.take(renderer, rect, scale) else {
                debug!(?rect, "no texture for an output's windows; no slide there");
                continue;
            };
            let drawn = render::snapshot_windows(
                renderer,
                &windows,
                &ghosts,
                running.as_ref(),
                rect,
                scale,
                &mut |surface, origin, scale, alpha| {
                    blur.element(surface, &name, origin, scale, alpha)
                },
                &mut leaving,
            );
            if !drawn {
                debug!(?rect, "no picture of an output's windows; no slide there");
                slide_pool.free.push(leaving);
                continue;
            }
            old.push((rect, leaving, scale));
        }
        // The running slide's are free now, for the new workspace's.
        drop(running);
        if let Some(previous) = previous {
            slide_pool.give_back(previous);
        }
        let old: Vec<_> = old
            .into_iter()
            .map(|(rect, leaving, scale)| {
                if let Some(coming) = slide_pool.take(renderer, rect, scale) {
                    textures.push((rect, coming));
                }
                (rect, leaving)
            })
            .collect();
        if old.is_empty() {
            slide_pool.free.extend(textures.into_iter().map(|(_, t)| t));
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
            textures: Rc::new(RefCell::new(textures)),
        });
        self.queue_redraw();
    }

    /// A slide that's over, or dropped: its textures are kept for the next.
    pub(crate) fn retire_slide(&mut self, slide: Option<WorkspaceSlide>) {
        if let Some(slide) = slide {
            self.slide_pool.give_back(slide);
        }
    }

    /// Have the textures a slide takes ready for every output, and drop
    /// those of outputs that are gone or changed: off a slide's first frame,
    /// which would otherwise pay for them (`after_frames`). Nothing while
    /// workspace switches aren't animated.
    pub fn prepare_slide_textures(&mut self) {
        let animated = self.config.workspaces > 1
            && self
                .config
                .animations
                .duration(AnimationKind::Workspace)
                .is_some();
        let outputs: Vec<_> = self
            .space
            .outputs()
            .filter(|_| animated)
            .filter_map(|output| {
                let rect = self.space.output_geometry(output)?;
                Some((rect, crate::scale::output_scale(output)))
            })
            .collect();
        // Those in use come back later, and are trimmed then.
        if self.workspace_slide.is_some() {
            return;
        }
        self.slide_pool.trim(&outputs);
        // How many each output is short of; outputs of one size share
        // theirs, so those after the first count against what it gets.
        let mut missing: Vec<(Rectangle<i32, Logical>, Scale<f64>)> = Vec::new();
        for (rect, scale) in &outputs {
            let have = self.slide_pool.kept(*rect, *scale)
                + missing
                    .iter()
                    .filter(|(r, s)| r.size == rect.size && s == scale)
                    .count();
            let same = outputs
                .iter()
                .filter(|(r, s)| r.size == rect.size && s == scale)
                .count();
            let short = (TEXTURES_AHEAD * same)
                .saturating_sub(have)
                .min(TEXTURES_AHEAD);
            missing.extend(std::iter::repeat_n((*rect, *scale), short));
        }
        if missing.is_empty() {
            return;
        }
        let Pila {
            backend,
            slide_pool,
            ..
        } = self;
        let Some(mut renderer) = backend.as_mut().and_then(|b| b.renderer()) else {
            return;
        };
        for (rect, scale) in missing {
            if let Some(texture) = OffscreenWindow::for_output(&mut renderer, rect, scale) {
                debug!(?rect, "slide: a texture made ahead");
                slide_pool.free.push(texture);
            }
        }
    }

    /// The slide to draw this frame, if one is running (and not over).
    pub fn scene_slide(&self, now: Duration) -> Option<SceneSlide> {
        let slide = self.workspace_slide.as_ref()?;
        Some(SceneSlide {
            progress: slide.progress.value(now),
            direction: slide.direction,
            old: slide
                .old
                .iter()
                .map(|(rect, texture)| (*rect, texture.buffer().clone()))
                .collect(),
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
            let slide = self.workspace_slide.take();
            self.retire_slide(slide);
        }
        done
    }
}
