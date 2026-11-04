//! Animations: short and subtle, only where they help to follow what
//! happened. A window fades and grows in when it opens and fades and shrinks
//! out when it closes. Each one can be turned off or retimed in
//! `[animations]` (`config::AnimationsConfig`).
//!
//! A window that is animating isn't drawn as its surfaces: `render.rs` draws
//! it (title bar, popups and rounded corners included) into a texture and
//! shows that with the animation's opacity and scale (`WindowEffect`). A
//! window that is gone (closed) is shown from a snapshot taken while it still
//! existed (`Ghost`), at its place in the stacking order.
//!
//! Time comes from `Pila::clock`, read once per scene so every output of a
//! frame sees the same moment. Redraws are on demand, so while anything
//! animates `after_frames` queues the next one (`Pila::step_animations`).
//! Animations are visual only: input always sees the final geometry. Locking
//! the session drops them all.

use std::{cell::RefCell, time::Duration};

use smithay::{
    backend::renderer::{element::texture::TextureRenderBuffer, gles::GlesTexture},
    desktop::Window,
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Rectangle},
    wayland::{
        compositor::{
            add_destruction_hook, add_pre_commit_hook, with_states, BufferAssignment,
            SurfaceAttributes,
        },
        seat::WaylandFocus,
    },
};
use tracing::debug;

use crate::{
    config::AnimationKind,
    render::{self, Surfaces},
    Pila,
};

/// The shape of an animation's progress over time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Curve {
    /// Fast, then settling: things appearing or leaving.
    EaseOut,
}

impl Curve {
    /// The eased value of `t` (0..=1).
    pub fn apply(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Curve::EaseOut => 1.0 - (1.0 - t).powi(3),
        }
    }
}

/// A value going from `from` to `to` over `duration`, starting at `start`
/// (times on `Pila::clock`).
#[derive(Debug, Clone, Copy)]
pub struct Animation {
    start: Duration,
    duration: Duration,
    curve: Curve,
    from: f64,
    to: f64,
}

impl Animation {
    pub fn new(start: Duration, duration: Duration, curve: Curve, from: f64, to: f64) -> Self {
        Self {
            start,
            duration,
            curve,
            from,
            to,
        }
    }

    /// Its value at `now`.
    pub fn value(&self, now: Duration) -> f64 {
        let t = if self.duration.is_zero() {
            1.0
        } else {
            now.saturating_sub(self.start).as_secs_f64() / self.duration.as_secs_f64()
        };
        self.from + (self.to - self.from) * self.curve.apply(t)
    }

    pub fn done(&self, now: Duration) -> bool {
        now >= self.start + self.duration
    }
}

/// How a window is drawn while it animates, relative to where it really is:
/// scaled by `scale` around the center of its geometry (title bar included),
/// moved by `offset`, at `alpha`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub alpha: f32,
    pub scale: f64,
    pub offset: Point<f64, Logical>,
}

impl Transform {
    /// Where `rect` is drawn, scaled about `center` and moved.
    pub fn apply(
        &self,
        rect: Rectangle<f64, Logical>,
        center: Point<f64, Logical>,
    ) -> Rectangle<f64, Logical> {
        let loc = center + (rect.loc - center).upscale(self.scale) + self.offset;
        Rectangle::new(loc, rect.size.upscale(self.scale))
    }
}

/// The middle of `rect`.
pub fn center(rect: Rectangle<i32, Logical>) -> Point<f64, Logical> {
    let rect = rect.to_f64();
    rect.loc + rect.size.downscale(2.0).to_point()
}

/// How much smaller a window is when it starts to open or ends closing.
const OPEN_SCALE: f64 = 0.95;

/// The look of a window that is `visibility` (0: gone, 1: fully there) of
/// the way in.
fn appear_transform(visibility: f64) -> Transform {
    Transform {
        alpha: visibility.clamp(0.0, 1.0) as f32,
        scale: OPEN_SCALE + (1.0 - OPEN_SCALE) * visibility,
        offset: Point::default(),
    }
}

/// A live window's animation: how visible it is.
struct Appearing {
    window: Window,
    visibility: Animation,
}

/// A window that is gone, shown from its snapshot until it has faded out.
pub struct Ghost {
    /// The window it shows (gone, or unmapped by its client).
    window: Window,
    pub snapshot: Snapshot,
    /// The window just in front of it when it went (`None`: it was in front).
    above: Option<Window>,
    visibility: Animation,
}

/// A window drawn into a texture: what it showed and where (`rect`, space
/// coordinates), with the center its animations scale about.
#[derive(Clone)]
pub struct Snapshot {
    pub buffer: TextureRenderBuffer<GlesTexture>,
    pub rect: Rectangle<i32, Logical>,
    pub center: Point<f64, Logical>,
}

/// A ghost as it's drawn this frame: in front of `Scene::windows[before]`
/// (or behind every window if that's past the end).
pub struct SceneGhost {
    pub snapshot: Snapshot,
    pub before: usize,
    pub transform: Transform,
}

/// How an animating window is drawn this frame (`SceneWindow::effect`).
#[derive(Clone)]
pub struct WindowEffect {
    pub transform: Transform,
    /// The center it's scaled about.
    pub center: Point<f64, Logical>,
    /// The output it's paced by: frame callbacks and presentation feedback
    /// go by it, since a window drawn through a texture isn't in the
    /// output's render states.
    pub output: Option<Output>,
}

/// Every animation in progress.
#[derive(Default)]
pub struct Animations {
    appearing: Vec<Appearing>,
    ghosts: Vec<Ghost>,
}

/// A window's texture while it animates (`render.rs`), one per scale it's
/// drawn at; kept in its user data and dropped when it stops animating.
#[derive(Default)]
pub struct WindowTextures(pub RefCell<Vec<render::OffscreenWindow>>);

fn clear_textures(window: &Window) {
    if let Some(textures) = window.user_data().get::<WindowTextures>() {
        textures.0.borrow_mut().clear();
    }
}

/// Start a window's close animation while its content is still there:
/// when its client takes the buffer away (`attach(null)`, which most do
/// before destroying the toplevel; once that commit is applied there's
/// nothing left to snapshot), or when its surface is destroyed without that
/// (the client died or just exited). Called for each new toplevel, before
/// its first commit: Smithay drops a surface's textures in a destruction
/// hook it adds then, and hooks run in the order they were added.
pub fn watch_unmap(surface: &WlSurface) {
    add_destruction_hook::<Pila, _>(surface, |state, surface| {
        if let Some(window) = state.mapped_window(surface) {
            state.animate_close_from(&window, Surfaces::MainOnly);
        }
    });
    add_pre_commit_hook::<Pila, _>(surface, |state, _, surface| {
        let removed = with_states(surface, |states| {
            let mut attributes = states.cached_state.get::<SurfaceAttributes>();
            matches!(attributes.pending().buffer, Some(BufferAssignment::Removed))
        });
        if !removed {
            return;
        }
        if let Some(window) = state.mapped_window(surface) {
            state.animate_close(&window);
        }
    });
}

impl Pila {
    /// Now, on the clock animations use.
    pub fn animation_time(&self) -> Duration {
        self.clock.now().into()
    }

    /// How long `kind` lasts, if it plays.
    fn animation_duration(&self, kind: AnimationKind) -> Option<Duration> {
        if self.is_locked() {
            return None;
        }
        self.config.animations.duration(kind)
    }

    /// How visible `window` is now, if it's appearing.
    fn visibility(&self, window: &Window, now: Duration) -> Option<f64> {
        self.animations
            .appearing
            .iter()
            .find(|a| a.window == *window)
            .map(|a| a.visibility.value(now))
    }

    /// `window` was just mapped: fade it in.
    pub fn animate_open(&mut self, window: &Window) {
        if crate::workspaces::is_override_redirect(window) {
            return;
        }
        let Some(duration) = self.animation_duration(AnimationKind::WindowOpen) else {
            return;
        };
        let now = self.animation_time();
        let from = self.visibility(window, now).unwrap_or(0.0);
        self.animations.appearing.retain(|a| a.window != *window);
        self.animations.appearing.push(Appearing {
            window: window.clone(),
            visibility: Animation::new(now, duration, Curve::EaseOut, from, 1.0),
        });
        self.queue_redraw();
    }

    /// `window` is about to leave the space for good: keep a snapshot of it
    /// fading out where it was. Call while it's still mapped and has its
    /// content: clients usually take their buffer away before they destroy
    /// the toplevel, so that's caught first (`watch_unmap`).
    pub fn animate_close(&mut self, window: &Window) {
        self.animate_close_from(window, Surfaces::Tree);
    }

    /// `animate_close`, with the snapshot's surfaces taken from `surfaces`.
    fn animate_close_from(&mut self, window: &Window, surfaces: Surfaces) {
        if crate::workspaces::is_override_redirect(window) || self.is_ghost(window) {
            return;
        }
        let Some(duration) = self.animation_duration(AnimationKind::WindowClose) else {
            return;
        };
        let now = self.animation_time();
        let from = self.visibility(window, now).unwrap_or(1.0);
        self.forget_animations(window);
        let Some(snapshot) = self.snapshot(window, surfaces) else {
            return;
        };
        let above = {
            let mut behind_first = self.space.elements().skip_while(|w| *w != window);
            behind_first.next();
            behind_first.next().cloned()
        };
        self.animations.ghosts.push(Ghost {
            window: window.clone(),
            snapshot,
            above,
            visibility: Animation::new(now, duration, Curve::EaseOut, from, 0.0),
        });
        self.queue_redraw();
    }

    /// Draw a mapped `window` into a texture as it looks now, at the scale
    /// of its output.
    fn snapshot(&mut self, window: &Window, surfaces: Surfaces) -> Option<Snapshot> {
        // Its title bar as it was drawn: focused or not, nothing hovered.
        let focused = self.focused_window();
        let entry = self.scene_window(window, focused.as_ref(), None, None)?;
        let scale = self
            .window_output(window)
            .map(|o| crate::scale::output_scale(&o))
            .unwrap_or_else(|| 1.0.into());
        let renderer = self.backend.as_mut()?.renderer();
        let snapshot = render::snapshot_window(renderer, &entry, surfaces, scale);
        if snapshot.is_none() {
            debug!("no snapshot of a closing window; it just goes");
        }
        snapshot
    }

    /// The mapped window whose toplevel surface is `surface`.
    fn mapped_window(&self, surface: &WlSurface) -> Option<Window> {
        self.space
            .elements()
            .find(|w| w.wl_surface().is_some_and(|s| *s == *surface))
            .cloned()
    }

    /// Whether `window` is shown by its close animation instead of itself.
    pub fn is_ghost(&self, window: &Window) -> bool {
        self.animations.ghosts.iter().any(|g| g.window == *window)
    }

    /// Stop animating `window` (it's gone or hidden).
    pub fn forget_animations(&mut self, window: &Window) {
        self.animations.appearing.retain(|a| a.window != *window);
        clear_textures(window);
    }

    /// How `window` is drawn this frame, if it's animating.
    pub fn window_effect(&self, window: &Window, now: Duration) -> Option<WindowEffect> {
        let visibility = self.visibility(window, now)?;
        let geo = self.space.element_geometry(window)?;
        let bar = crate::decoration::title_bar_rect(window, geo.loc);
        let rect = bar.map_or(geo, |bar| bar.merge(geo));
        Some(WindowEffect {
            transform: appear_transform(visibility),
            center: center(rect),
            output: self.window_output(window),
        })
    }

    /// The ghosts to draw this frame, placed among `windows` (front to back).
    pub fn scene_ghosts(&self, windows: &[Window], now: Duration) -> Vec<SceneGhost> {
        self.animations
            .ghosts
            .iter()
            .map(|ghost| {
                let before = ghost
                    .above
                    .as_ref()
                    .and_then(|above| windows.iter().position(|w| w == above))
                    .map_or(0, |i| i + 1);
                SceneGhost {
                    snapshot: ghost.snapshot.clone(),
                    before,
                    transform: appear_transform(ghost.visibility.value(now)),
                }
            })
            .collect()
    }

    /// After a frame: drop finished animations and keep redrawing while any
    /// remain. Called from `after_frames`.
    pub fn step_animations(&mut self) {
        if self.is_locked() {
            self.drop_animations();
            return;
        }
        let now = self.animation_time();
        let mut finished = Vec::new();
        self.animations.appearing.retain(|a| {
            let done = a.visibility.done(now);
            if done {
                finished.push(a.window.clone());
            }
            !done
        });
        let ghosts = self.animations.ghosts.len();
        self.animations.ghosts.retain(|g| !g.visibility.done(now));
        let changed = !finished.is_empty() || ghosts != self.animations.ghosts.len();
        for window in &finished {
            clear_textures(window);
        }
        if changed || !self.animations.appearing.is_empty() || !self.animations.ghosts.is_empty() {
            // From an idle callback, with the loop woken so it runs right
            // away (calloop wouldn't): this runs after the event loop's
            // dispatch, too late for the winit source to see a redraw
            // request before the loop sleeps.
            self.loop_handle.insert_idle(|state| state.queue_redraw());
            self.wake.ping();
        }
    }

    /// Whether anything is still animating now, so frames are wanted back
    /// to back (one that ended during the last frame doesn't count).
    pub fn animating(&self) -> bool {
        let now = self.animation_time();
        self.animations
            .appearing
            .iter()
            .any(|a| !a.visibility.done(now))
            || self
                .animations
                .ghosts
                .iter()
                .any(|g| !g.visibility.done(now))
    }

    /// Stop every animation (the session locked).
    pub fn drop_animations(&mut self) {
        for appearing in std::mem::take(&mut self.animations.appearing) {
            clear_textures(&appearing.window);
        }
        self.animations.ghosts.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_out_starts_fast_and_ends_at_one() {
        assert_eq!(Curve::EaseOut.apply(0.0), 0.0);
        assert_eq!(Curve::EaseOut.apply(1.0), 1.0);
        assert!(Curve::EaseOut.apply(0.5) > 0.8);
        assert_eq!(Curve::EaseOut.apply(2.0), 1.0);
        assert_eq!(Curve::EaseOut.apply(-1.0), 0.0);
    }

    #[test]
    fn animation_runs_from_start_to_end() {
        let ms = Duration::from_millis;
        let anim = Animation::new(ms(100), ms(200), Curve::EaseOut, 0.2, 1.0);
        assert_eq!(anim.value(ms(50)), 0.2);
        assert_eq!(anim.value(ms(300)), 1.0);
        assert_eq!(anim.value(ms(1000)), 1.0);
        let mid = anim.value(ms(200));
        assert!(0.2 < mid && mid < 1.0);
        assert!(!anim.done(ms(299)));
        assert!(anim.done(ms(300)));
        let instant = Animation::new(ms(100), Duration::ZERO, Curve::EaseOut, 0.0, 1.0);
        assert_eq!(instant.value(ms(100)), 1.0);
    }

    #[test]
    fn transform_scales_about_the_center() {
        let t = Transform {
            alpha: 1.0,
            scale: 0.5,
            offset: (10.0, 0.0).into(),
        };
        let rect = Rectangle::new((0.0, 0.0).into(), (100.0, 50.0).into());
        let out = t.apply(rect, center(rect.to_i32_round()));
        assert_eq!(
            out,
            Rectangle::new((35.0, 12.5).into(), (50.0, 25.0).into())
        );
    }

    #[test]
    fn appearing_windows_grow_and_fade_in() {
        let gone = appear_transform(0.0);
        assert_eq!(gone.alpha, 0.0);
        assert_eq!(gone.scale, OPEN_SCALE);
        let there = appear_transform(1.0);
        assert_eq!(there.alpha, 1.0);
        assert_eq!(there.scale, 1.0);
    }
}
