//! Animations: short and subtle, only where they help to follow what
//! happened. A window rises and fades in when it opens and shrinks, drops and
//! fades out when it closes. Each one can be turned off or retimed in
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
    desktop::{layer_map_for_output, LayerSurface},
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{user_data::UserDataMap, Logical, Point, Rectangle},
    wayland::{
        compositor::{
            add_destruction_hook, add_pre_commit_hook, with_states, BufferAssignment,
            SurfaceAttributes,
        },
        seat::WaylandFocus,
        shell::wlr_layer::{ExclusiveZone, Layer},
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

/// How a window comes and goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// Opening and closing: it rises in from a little lower and smaller.
    Open,
    /// Minimizing and restoring: smaller still, and a little lower, as if
    /// put away towards the bottom of the screen.
    Minimize,
    /// Launchers and notifications (layer surfaces) coming and going:
    /// barely scaled, mostly a fade.
    Layer,
}

/// How much smaller a window is when it starts to open or ends closing, and
/// how far below its place (logical pixels).
const OPEN_SCALE: f64 = 0.9;
const OPEN_DROP: f64 = 24.0;
/// How much smaller a minimized window ends up, and how far it drops
/// (logical pixels).
const MINIMIZE_SCALE: f64 = 0.9;
const MINIMIZE_DROP: f64 = 48.0;
/// How much smaller a layer surface is when it starts to show.
const LAYER_SCALE: f64 = 0.97;

impl Style {
    /// The look of a window that is `visibility` (0: gone, 1: fully there)
    /// of the way in.
    fn transform(self, visibility: f64) -> Transform {
        let (scale, drop) = match self {
            Style::Open => (OPEN_SCALE, OPEN_DROP),
            Style::Minimize => (MINIMIZE_SCALE, MINIMIZE_DROP),
            Style::Layer => (LAYER_SCALE, 0.0),
        };
        Transform {
            alpha: visibility.clamp(0.0, 1.0) as f32,
            scale: scale + (1.0 - scale) * visibility,
            offset: (0.0, drop * (1.0 - visibility)).into(),
        }
    }
}

/// A live window's animation: how visible it is.
struct Appearing {
    window: Window,
    style: Style,
    visibility: Animation,
}

/// A window that is gone, shown from its snapshot until it has faded out.
pub struct Ghost {
    /// The window it shows (gone, or unmapped by its client).
    window: Window,
    pub snapshot: Snapshot,
    /// The window just in front of it when it went (`None`: it was in front).
    above: Option<Window>,
    /// The workspace it went from: it's drawn only there (a switch takes it
    /// along in the leaving picture).
    workspace: usize,
    style: Style,
    visibility: Animation,
}

/// A window drawn into a texture: what it showed and where (`rect`, space
/// coordinates), with the center its animations scale about.
#[derive(Clone)]
pub struct Snapshot {
    pub buffer: TextureRenderBuffer<GlesTexture>,
    /// The texture's size in pixels.
    pub size: smithay::utils::Size<i32, smithay::utils::Physical>,
    pub rect: Rectangle<i32, Logical>,
    pub center: Point<f64, Logical>,
}

/// A ghost as it's drawn this frame: in front of `Scene::windows[before]`
/// (or behind every window if that's past the end).
#[derive(Clone)]
pub struct SceneGhost {
    pub snapshot: Snapshot,
    pub before: usize,
    pub transform: Transform,
    /// Instead of `transform`'s scaling: where its frame goes.
    pub frame: Option<FrameMap>,
}

/// Maps a window's frame (geometry and title bar) onto another rectangle,
/// stretching what it draws along: a window changing size and place.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameMap {
    pub from: Rectangle<f64, Logical>,
    pub to: Rectangle<f64, Logical>,
}

impl FrameMap {
    /// Where `rect` (drawn relative to `from`) goes.
    pub fn apply(&self, rect: Rectangle<f64, Logical>) -> Rectangle<f64, Logical> {
        let sx = self.to.size.w / self.from.size.w.max(1.0);
        let sy = self.to.size.h / self.from.size.h.max(1.0);
        let loc = self.to.loc
            + Point::from((
                (rect.loc.x - self.from.loc.x) * sx,
                (rect.loc.y - self.from.loc.y) * sy,
            ));
        Rectangle::new(loc, (rect.size.w * sx, rect.size.h * sy).into())
    }
}

/// `a` to `b`, `t` of the way (0 to 1).
fn lerp_rect(
    a: Rectangle<i32, Logical>,
    b: Rectangle<i32, Logical>,
    t: f64,
) -> Rectangle<f64, Logical> {
    let (a, b) = (a.to_f64(), b.to_f64());
    let mix = |x: f64, y: f64| x + (y - x) * t;
    Rectangle::new(
        (mix(a.loc.x, b.loc.x), mix(a.loc.y, b.loc.y)).into(),
        (mix(a.size.w, b.size.w), mix(a.size.h, b.size.h)).into(),
    )
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
    /// Instead of `transform`'s scaling: where its frame is drawn (a size
    /// or place change, `animate_geometry`).
    pub frame: Option<FrameMap>,
}

/// A window changing size or place (maximize, fullscreen, snap): shown from
/// its snapshot at the old rect until its client commits the new size, then
/// moving and stretching to the new rect while the snapshot fades out over
/// the live window.
struct Resizing {
    window: Window,
    snapshot: Snapshot,
    /// Its frame (geometry and title bar) and geometry size before.
    from: Rectangle<i32, Logical>,
    from_size: smithay::utils::Size<i32, Logical>,
    /// When it began, how long the move lasts, and the move once the
    /// client has answered (or `RESIZE_WAIT` went by).
    began: Duration,
    duration: Duration,
    progress: Option<Animation>,
}

/// How long a resize waits for its client's new size before moving anyway.
const RESIZE_WAIT: Duration = Duration::from_millis(300);

/// A layer surface showing up.
struct LayerAppearing {
    layer: LayerSurface,
    visibility: Animation,
}

/// A layer surface that went away (hidden or destroyed), fading out from
/// its snapshot in front of the upper layers.
struct LayerGhost {
    layer: LayerSurface,
    snapshot: Snapshot,
    visibility: Animation,
}

/// Every animation in progress.
#[derive(Default)]
pub struct Animations {
    appearing: Vec<Appearing>,
    ghosts: Vec<Ghost>,
    layers: Vec<LayerAppearing>,
    layer_ghosts: Vec<LayerGhost>,
    resizing: Vec<Resizing>,
    /// Size and place changes don't animate (`without_geometry_animation`).
    geometry_held: bool,
}

/// A window's or layer surface's texture while it animates (`render.rs`),
/// one per scale it's drawn at; kept in its user data and dropped when it
/// stops animating.
#[derive(Default)]
pub struct AnimTextures(pub RefCell<Vec<render::OffscreenWindow>>);

fn clear_textures(user_data: &UserDataMap) {
    if let Some(textures) = user_data.get::<AnimTextures>() {
        textures.0.borrow_mut().clear();
    }
}

/// Whether a layer surface animates (`what`: `layer-in` or `layer-out`, for
/// the log): launchers, notifications, menus of shells (Top or Overlay, no
/// exclusive zone), unless a layer rule says not to (a shell that animates
/// its own). Bars, docks with a zone, wallpapers and the Bottom layer just
/// appear.
fn animates_layer(layer: &LayerSurface, what: &str) -> bool {
    let transient = matches!(layer.layer(), Layer::Top | Layer::Overlay)
        && !matches!(
            layer.cached_state().exclusive_zone,
            ExclusiveZone::Exclusive(_)
        );
    if !transient {
        return false;
    }
    if crate::rules::layer_props(layer).animations == Some(false) {
        log_event(what, layer.namespace(), Outcome::OffByRule);
        return false;
    }
    true
}

/// What happened to an animation, for the log.
enum Outcome {
    Starts,
    OffByRule,
}

/// Log an animation starting or turned off by a rule (`what` it is, for the
/// window with app id or layer surface with namespace `id`), at debug level
/// on `pila::animation`: `scripts/check` reads these.
fn log_event(what: &str, id: &str, outcome: Outcome) {
    match outcome {
        Outcome::Starts => debug!(target: "pila::animation", what, id, "starts"),
        Outcome::OffByRule => debug!(target: "pila::animation", what, id, "off by rule"),
    }
}

/// Log a workspace slide to workspace `index` (from 0) starting.
pub(crate) fn log_workspace_slide(index: usize) {
    log_event("workspace", &(index + 1).to_string(), Outcome::Starts);
}

/// The name of a window animation in the log: `kind`, `coming` in or going.
fn window_event(kind: AnimationKind, coming: bool) -> &'static str {
    match (kind, coming) {
        (AnimationKind::WindowOpen, _) => "window-open",
        (AnimationKind::WindowClose, _) => "window-close",
        (AnimationKind::Minimize, true) => "restore",
        (AnimationKind::Minimize, false) => "minimize",
        (AnimationKind::Maximize, _) => "maximize",
        (AnimationKind::Layer, true) => "layer-in",
        (AnimationKind::Layer, false) => "layer-out",
        (AnimationKind::Workspace, _) => "workspace",
    }
}

/// The size of what `window`'s client last drew: its geometry, except for
/// an X11 window, whose geometry is what pila last configured (X applies it
/// at once), so it's the size of its surface's buffer.
fn committed_size(window: &Window) -> smithay::utils::Size<i32, Logical> {
    if window.x11_surface().is_some() {
        if let Some(surface) = window.wl_surface() {
            let size = with_states(&surface, |states| {
                states
                    .data_map
                    .get::<smithay::backend::renderer::utils::RendererSurfaceStateUserData>()
                    .and_then(|state| state.lock().ok()?.surface_size())
            });
            if let Some(size) = size {
                return size;
            }
        }
    }
    window.geometry().size
}

/// A window's app id, for the log.
fn app_id(window: &Window) -> String {
    crate::foreign_toplevel::title_and_app_id(window).1
}

/// Whether a layer surface had a buffer at its last commit, to tell when
/// it shows up (`Pila::layer_committed`).
#[derive(Default)]
struct LayerShown(std::cell::Cell<bool>);

/// Fade a layer surface out while its content is still there: when its
/// client takes the buffer away (pre-commit) or the surface is destroyed
/// (`layer_destroyed` comes first when the client destroys the role; a dead
/// client's surface goes without it). Called for each new layer surface,
/// before its first commit (see `watch_unmap`).
pub fn watch_layer(surface: &WlSurface) {
    add_destruction_hook::<Pila, _>(surface, |state, surface| {
        state.animate_layer_close(surface, Surfaces::MainOnly);
    });
    add_pre_commit_hook::<Pila, _>(surface, |state, _, surface| {
        let removed = with_states(surface, |states| {
            let mut attributes = states.cached_state.get::<SurfaceAttributes>();
            matches!(attributes.pending().buffer, Some(BufferAssignment::Removed))
        });
        if removed && crate::state::has_buffer(surface) {
            state.animate_layer_close(surface, Surfaces::Tree);
        }
    });
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

    /// How visible `window` is now, if it's appearing or fading out.
    fn visibility(&self, window: &Window, now: Duration) -> Option<f64> {
        let appearing = self
            .animations
            .appearing
            .iter()
            .find(|a| a.window == *window);
        let ghost = self.animations.ghosts.iter().find(|g| g.window == *window);
        appearing
            .map(|a| a.visibility.value(now))
            .or_else(|| ghost.map(|g| g.visibility.value(now)))
    }

    /// Whether `window` plays `kind` (coming in or going): not X11 menus and
    /// tooltips (they're like popups), nor windows whose rules turn it off.
    fn animates_window(&self, window: &Window, kind: AnimationKind, coming: bool) -> bool {
        if crate::workspaces::is_override_redirect(window) {
            return false;
        }
        if !crate::rules::window_props(window).animates(kind) {
            log_event(
                window_event(kind, coming),
                &app_id(window),
                Outcome::OffByRule,
            );
            return false;
        }
        true
    }

    /// `window` was just mapped: fade it in.
    pub fn animate_open(&mut self, window: &Window) {
        self.appear(window, AnimationKind::WindowOpen, Style::Open);
    }

    /// A minimized `window` was just put back: the reverse of minimizing.
    pub fn animate_restore(&mut self, window: &Window) {
        self.appear(window, AnimationKind::Minimize, Style::Minimize);
    }

    /// Fade the just mapped `window` in, from wherever an animation in
    /// progress (e.g. a minimize being undone) left it.
    fn appear(&mut self, window: &Window, kind: AnimationKind, style: Style) {
        let now = self.animation_time();
        let from = self.visibility(window, now).unwrap_or(0.0);
        self.forget_animations(window);
        self.animations.ghosts.retain(|g| g.window != *window);
        if !self.animates_window(window, kind, true) {
            return;
        }
        let Some(duration) = self.animation_duration(kind) else {
            return;
        };
        self.animations.appearing.push(Appearing {
            window: window.clone(),
            style,
            visibility: Animation::new(now, duration, Curve::EaseOut, from, 1.0),
        });
        log_event(window_event(kind, true), &app_id(window), Outcome::Starts);
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
        if self.is_ghost(window) {
            return;
        }
        self.disappear(window, surfaces, AnimationKind::WindowClose, Style::Open);
    }

    /// `window` is about to be minimized: it shrinks away. Call while it's
    /// still mapped.
    pub fn animate_minimize(&mut self, window: &Window) {
        self.disappear(
            window,
            Surfaces::Tree,
            AnimationKind::Minimize,
            Style::Minimize,
        );
    }

    /// Show a snapshot of the still mapped `window` fading out where it is,
    /// from wherever an animation in progress left it.
    fn disappear(
        &mut self,
        window: &Window,
        surfaces: Surfaces,
        kind: AnimationKind,
        style: Style,
    ) {
        let now = self.animation_time();
        let from = self.visibility(window, now).unwrap_or(1.0);
        self.forget_animations(window);
        self.animations.ghosts.retain(|g| g.window != *window);
        if !self.animates_window(window, kind, false) {
            return;
        }
        let Some(duration) = self.animation_duration(kind) else {
            return;
        };
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
            workspace: self.workspaces.active,
            style,
            visibility: Animation::new(now, duration, Curve::EaseOut, from, 0.0),
        });
        log_event(window_event(kind, false), &app_id(window), Outcome::Starts);
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
        self.animations.resizing.retain(|r| r.window != *window);
        clear_textures(window.user_data());
    }

    /// How `window` is drawn this frame, if it's animating.
    pub fn window_effect(&self, window: &Window, now: Duration) -> Option<WindowEffect> {
        if let Some(resizing) = self
            .animations
            .resizing
            .iter()
            .find(|r| r.window == *window)
        {
            return self.resizing_effect(resizing, now);
        }
        let appearing = self
            .animations
            .appearing
            .iter()
            .find(|a| a.window == *window)?;
        let geo = self.space.element_geometry(window)?;
        let bar = crate::decoration::title_bar_rect(window, geo.loc);
        let rect = bar.map_or(geo, |bar| bar.merge(geo));
        Some(WindowEffect {
            transform: appearing.style.transform(appearing.visibility.value(now)),
            center: center(rect),
            output: self.window_output(window),
            frame: None,
        })
    }

    /// The ghosts to draw this frame, placed among `windows` (front to back).
    pub fn scene_ghosts(&self, windows: &[Window], now: Duration) -> Vec<SceneGhost> {
        let resizing = self.animations.resizing.iter().filter_map(|r| {
            // Right in front of its window.
            let before = windows.iter().position(|w| *w == r.window)?;
            let (alpha, to) = match &r.progress {
                None => (1.0, r.from.to_f64()),
                Some(progress) => {
                    let t = progress.value(now);
                    (1.0 - t, lerp_rect(r.from, self.frame_rect(&r.window)?, t))
                }
            };
            Some(SceneGhost {
                snapshot: r.snapshot.clone(),
                before,
                transform: Transform {
                    alpha: alpha as f32,
                    scale: 1.0,
                    offset: Point::default(),
                },
                frame: Some(FrameMap {
                    from: r.from.to_f64(),
                    to,
                }),
            })
        });
        let resizing: Vec<SceneGhost> = resizing.collect();
        let active = self.workspaces.active;
        self.animations
            .ghosts
            .iter()
            .filter(|ghost| ghost.workspace == active)
            .map(|ghost| {
                let before = ghost
                    .above
                    .as_ref()
                    .and_then(|above| windows.iter().position(|w| w == above))
                    .map_or(0, |i| i + 1);
                SceneGhost {
                    snapshot: ghost.snapshot.clone(),
                    before,
                    transform: ghost.style.transform(ghost.visibility.value(now)),
                    frame: None,
                }
            })
            .chain(resizing)
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
        let before = self.animation_count();
        let animations = &mut self.animations;
        animations.appearing.retain(|a| {
            let done = a.visibility.done(now);
            if done {
                clear_textures(a.window.user_data());
            }
            !done
        });
        animations.ghosts.retain(|g| !g.visibility.done(now));
        animations.layers.retain(|a| {
            let done = a.visibility.done(now);
            if done {
                clear_textures(a.layer.user_data());
            }
            !done
        });
        animations.layer_ghosts.retain(|g| !g.visibility.done(now));
        for resizing in &mut animations.resizing {
            // The client didn't answer in time: move anyway.
            if resizing.progress.is_none() && now >= resizing.began + RESIZE_WAIT {
                resizing.progress = Some(Animation::new(
                    now,
                    resizing.duration,
                    Curve::EaseOut,
                    0.0,
                    1.0,
                ));
            }
        }
        animations.resizing.retain(|r| {
            let done = r.progress.as_ref().is_some_and(|p| p.done(now));
            if done {
                clear_textures(r.window.user_data());
            }
            !done
        });
        let slide_ended = self.end_finished_slide(now);
        let left = self.animation_count() + usize::from(self.workspace_slide.is_some());
        let before = before + usize::from(slide_ended);
        if left > 0 || left != before {
            // From an idle callback, with the loop woken so it runs right
            // away (calloop wouldn't): this runs after the event loop's
            // dispatch, too late for the winit source to see a redraw
            // request before the loop sleeps.
            self.loop_handle.insert_idle(|state| state.queue_redraw());
            self.wake.ping();
        }
    }

    /// How many animations are in progress.
    fn animation_count(&self) -> usize {
        let a = &self.animations;
        a.appearing.len()
            + a.ghosts.len()
            + a.layers.len()
            + a.layer_ghosts.len()
            + a.resizing.len()
    }

    /// Whether anything is still animating now, so frames are wanted back
    /// to back (one that ended during the last frame doesn't count).
    pub fn animating(&self) -> bool {
        let now = self.animation_time();
        let a = &self.animations;
        let running = |anim: &Animation| !anim.done(now);
        a.appearing.iter().any(|x| running(&x.visibility))
            || a.ghosts.iter().any(|x| running(&x.visibility))
            || a.layers.iter().any(|x| running(&x.visibility))
            || a.layer_ghosts.iter().any(|x| running(&x.visibility))
            || a.resizing
                .iter()
                .any(|x| x.progress.as_ref().is_none_or(running))
            || self.sliding(now)
    }

    /// Stop every animation (the session locked).
    pub fn drop_animations(&mut self) {
        for appearing in std::mem::take(&mut self.animations.appearing) {
            clear_textures(appearing.window.user_data());
        }
        for appearing in std::mem::take(&mut self.animations.layers) {
            clear_textures(appearing.layer.user_data());
        }
        for resizing in std::mem::take(&mut self.animations.resizing) {
            clear_textures(resizing.window.user_data());
        }
        self.animations.ghosts.clear();
        self.animations.layer_ghosts.clear();
        self.workspace_slide = None;
    }

    // --- Size and place changes ---------------------------------------------

    /// Run `f` without size and place animations: a change it makes goes
    /// straight to the new rect (e.g. a drag taking a maximized window out:
    /// it follows the pointer, and no snapshot is taken for nothing).
    pub fn without_geometry_animation(&mut self, f: impl FnOnce(&mut Self)) {
        let held = std::mem::replace(&mut self.animations.geometry_held, true);
        f(self);
        self.animations.geometry_held = held;
    }

    /// `window`'s frame: its geometry and title bar, in space coordinates.
    fn frame_rect(&self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        let geo = self.space.element_geometry(window)?;
        let bar = crate::decoration::title_bar_rect(window, geo.loc);
        Some(bar.map_or(geo, |bar| bar.merge(geo)))
    }

    /// `window` is about to change size and place (maximize, fullscreen,
    /// snap, or back): keep how it looks now, to move from. Call before the
    /// change; a second call before its client answers keeps the first.
    pub fn animate_geometry(&mut self, window: &Window) {
        let waiting = self
            .animations
            .resizing
            .iter()
            .any(|r| r.window == *window && r.progress.is_none());
        // Its open animation shows it at the new size already (e.g. an X11
        // window a rule maximizes as it maps): leave that one be.
        let opening = self
            .animations
            .appearing
            .iter()
            .any(|a| a.window == *window);
        if waiting || opening || self.animations.geometry_held {
            return;
        }
        if !self.animates_window(window, AnimationKind::Maximize, true) {
            return;
        }
        let Some(duration) = self.animation_duration(AnimationKind::Maximize) else {
            return;
        };
        let Some(from) = self.frame_rect(window) else {
            return;
        };
        let from_size = committed_size(window);
        self.forget_animations(window);
        let Some(snapshot) = self.snapshot(window, Surfaces::Tree) else {
            return;
        };
        log_event("maximize", &app_id(window), Outcome::Starts);
        self.animations.resizing.push(Resizing {
            window: window.clone(),
            snapshot,
            from,
            from_size,
            began: self.animation_time(),
            duration,
            progress: None,
        });
        self.queue_redraw();
    }

    /// `window` committed: if it was waiting for its new size and has it,
    /// start moving.
    pub fn geometry_committed(&mut self, window: &Window) {
        // Unmapped by its client (attach(null), now a ghost) and shown
        // again before the fade ended: drop the ghost and open it again.
        if self.is_ghost(window)
            && self.space.element_location(window).is_some()
            && window
                .wl_surface()
                .is_some_and(|surface| crate::state::has_buffer(&surface))
        {
            self.animations.ghosts.retain(|g| g.window != *window);
            self.animate_open(window);
            return;
        }
        let size = committed_size(window);
        let now = self.animation_time();
        let Some(resizing) = self
            .animations
            .resizing
            .iter_mut()
            .find(|r| r.window == *window && r.progress.is_none())
        else {
            return;
        };
        if size.is_empty() || size == resizing.from_size {
            return;
        }
        resizing.progress = Some(Animation::new(
            now,
            resizing.duration,
            Curve::EaseOut,
            0.0,
            1.0,
        ));
        self.queue_redraw();
    }

    /// How a resizing window is drawn: not at all while it waits (its
    /// snapshot stands in), then stretched from the old rect to the new.
    fn resizing_effect(&self, resizing: &Resizing, now: Duration) -> Option<WindowEffect> {
        let to = self.frame_rect(&resizing.window)?;
        let (alpha, frame) = match &resizing.progress {
            None => (0.0, None),
            Some(progress) => (
                1.0,
                Some(FrameMap {
                    from: to.to_f64(),
                    to: lerp_rect(resizing.from, to, progress.value(now)),
                }),
            ),
        };
        Some(WindowEffect {
            transform: Transform {
                alpha,
                scale: 1.0,
                offset: Point::default(),
            },
            center: center(to),
            output: self.window_output(&resizing.window),
            frame,
        })
    }

    // --- Layer surfaces ---------------------------------------------------

    /// A layer surface was committed: if it just got a buffer (it shows
    /// up), fade it in. Called from `handle_layer_commit`.
    pub fn layer_committed(&mut self, layer: &LayerSurface) {
        layer.user_data().insert_if_missing(LayerShown::default);
        let Some(shown) = layer.user_data().get::<LayerShown>() else {
            return;
        };
        let has_buffer = crate::state::has_buffer(layer.wl_surface());
        if shown.0.replace(has_buffer) || !has_buffer {
            return;
        }
        let now = self.animation_time();
        let from = self.layer_visibility(layer, now).unwrap_or(0.0);
        self.forget_layer_animations(layer);
        if !animates_layer(layer, "layer-in") {
            return;
        }
        let Some(duration) = self.animation_duration(AnimationKind::Layer) else {
            return;
        };
        self.animations.layers.push(LayerAppearing {
            layer: layer.clone(),
            visibility: Animation::new(now, duration, Curve::EaseOut, from, 1.0),
        });
        log_event("layer-in", layer.namespace(), Outcome::Starts);
        self.queue_redraw();
    }

    /// The layer surface `surface` is about to be hidden or destroyed: fade
    /// out a snapshot of it. Once per hiding (a ghost already there wins).
    pub fn animate_layer_close(&mut self, surface: &WlSurface, surfaces: Surfaces) {
        let Some((output, layer)) = self.layer_for_surface(surface) else {
            return;
        };
        if self
            .animations
            .layer_ghosts
            .iter()
            .any(|g| g.layer == layer)
        {
            return;
        }
        let now = self.animation_time();
        let from = self.layer_visibility(&layer, now).unwrap_or(1.0);
        self.forget_layer_animations(&layer);
        if !animates_layer(&layer, "layer-out") {
            return;
        }
        let Some(duration) = self.animation_duration(AnimationKind::Layer) else {
            return;
        };
        let Some(rect) = self.layer_rect(&output, &layer) else {
            return;
        };
        let scale = crate::scale::output_scale(&output);
        let Some(renderer) = self.backend.as_mut().map(|b| b.renderer()) else {
            return;
        };
        let Some(snapshot) = render::snapshot_layer(renderer, &layer, rect, surfaces, scale) else {
            debug!("no snapshot of a closing layer surface; it just goes");
            return;
        };
        log_event("layer-out", layer.namespace(), Outcome::Starts);
        self.animations.layer_ghosts.push(LayerGhost {
            layer,
            snapshot,
            visibility: Animation::new(now, duration, Curve::EaseOut, from, 0.0),
        });
        self.queue_redraw();
    }

    /// Where `layer` is on `output`, in space coordinates: its place in the
    /// layer map (kept even while its surface is being destroyed).
    fn layer_rect(&self, output: &Output, layer: &LayerSurface) -> Option<Rectangle<i32, Logical>> {
        let output_loc = self.space.output_geometry(output)?.loc;
        let geo = layer_map_for_output(output).layer_geometry(layer)?;
        Some(Rectangle::new(output_loc + geo.loc, geo.size))
    }

    /// How visible `layer` is now, if it's animating.
    fn layer_visibility(&self, layer: &LayerSurface, now: Duration) -> Option<f64> {
        let a = &self.animations;
        a.layers
            .iter()
            .find(|x| x.layer == *layer)
            .map(|x| x.visibility.value(now))
            .or_else(|| {
                a.layer_ghosts
                    .iter()
                    .find(|x| x.layer == *layer)
                    .map(|x| x.visibility.value(now))
            })
    }

    fn forget_layer_animations(&mut self, layer: &LayerSurface) {
        self.animations.layers.retain(|a| a.layer != *layer);
        self.animations.layer_ghosts.retain(|g| g.layer != *layer);
        clear_textures(layer.user_data());
    }

    /// How the layer surface at `location` is drawn this frame, if it's
    /// appearing.
    pub fn layer_effect(
        &self,
        layer: &LayerSurface,
        location: Point<i32, Logical>,
        now: Duration,
    ) -> Option<WindowEffect> {
        let appearing = self.animations.layers.iter().find(|a| a.layer == *layer)?;
        let bbox = layer.bbox();
        let rect = Rectangle::new(location + bbox.loc, bbox.size);
        Some(WindowEffect {
            transform: Style::Layer.transform(appearing.visibility.value(now)),
            center: center(rect),
            output: self.layer_for_surface(layer.wl_surface()).map(|(o, _)| o),
            frame: None,
        })
    }

    /// Layer surfaces fading out, drawn in front of the upper layers.
    pub fn scene_layer_ghosts(&self, now: Duration) -> Vec<SceneGhost> {
        self.animations
            .layer_ghosts
            .iter()
            .map(|ghost| SceneGhost {
                snapshot: ghost.snapshot.clone(),
                before: 0,
                transform: Style::Layer.transform(ghost.visibility.value(now)),
                frame: None,
            })
            .collect()
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
    fn frame_maps_stretch_what_they_carry() {
        let map = FrameMap {
            from: Rectangle::new((100.0, 100.0).into(), (200.0, 100.0).into()),
            to: Rectangle::new((0.0, 0.0).into(), (400.0, 300.0).into()),
        };
        assert_eq!(map.apply(map.from), map.to);
        // A popup sticking out 10 px to the left of the frame.
        let popup = Rectangle::new((90.0, 150.0).into(), (50.0, 20.0).into());
        assert_eq!(
            map.apply(popup),
            Rectangle::new((-20.0, 150.0).into(), (100.0, 60.0).into())
        );
        let a = Rectangle::new((0, 0).into(), (100, 100).into());
        let b = Rectangle::new((100, 50).into(), (300, 100).into());
        assert_eq!(
            lerp_rect(a, b, 0.5),
            Rectangle::new((50.0, 25.0).into(), (200.0, 100.0).into())
        );
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
    fn windows_rise_and_fade_in() {
        let gone = Style::Open.transform(0.0);
        assert_eq!(gone.alpha, 0.0);
        assert_eq!(gone.scale, OPEN_SCALE);
        assert_eq!(gone.offset, (0.0, OPEN_DROP).into());
        for style in [Style::Open, Style::Minimize] {
            let there = style.transform(1.0);
            assert_eq!(there.alpha, 1.0);
            assert_eq!(there.scale, 1.0);
            assert_eq!(there.offset, Point::default());
        }
    }

    #[test]
    fn minimized_windows_shrink_further_and_drop() {
        let gone = Style::Minimize.transform(0.0);
        assert_eq!(gone.scale, MINIMIZE_SCALE);
        assert!(gone.scale <= Style::Open.transform(0.0).scale);
        assert!(gone.offset.y > Style::Open.transform(0.0).offset.y);
        assert_eq!(gone.offset, (0.0, MINIMIZE_DROP).into());
        let half = Style::Minimize.transform(0.5);
        assert_eq!(half.offset, (0.0, MINIMIZE_DROP / 2.0).into());
    }
}
