//! Redraws on demand, per output (see "Redraws" in `docs/ARCHITECTURE.md`).
//!
//! A change asks for a redraw of the outputs it shows on: a client's commit
//! those of its surface (before and after the commit), the pointer those of
//! where it was and where it is, a window those its frame, title bar, shadow
//! and popups touch. `queue_redraw` asks every output, for what really is
//! global (the config, the lock, the workspace, focus and stacking, a
//! window's state from a binding). Each request also says whether it surely
//! brings something new to show (`content`): on bare metal a redraw that may
//! bring nothing waits after one that found nothing new (`udev/draw.rs`,
//! `idle_wait`), so that a client committing nothing new (no buffer, or one
//! without damage: `brings_content`) in a loop of frame callbacks or fifo
//! barriers can't make Pila redraw as fast as it loops. A change of Pila's
//! own never waits (`Content::Sure`); a client's new picture doesn't, once
//! per wait (`Content::Client`): a second redraw in a row that finds nothing
//! means it wasn't new (a covered surface's, say), and the next waits too.

use smithay::{
    desktop::Window,
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Rectangle},
    wayland::compositor::{get_parent, with_states, with_surface_tree_downward, TraversalAction},
};

use crate::{sync::Locked, Pila};

/// What a redraw asked for brings to show, which decides on bare metal
/// whether it waits after redraws that found nothing new
/// (`udev/draw.rs::idle_wait`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Content {
    /// Maybe nothing: a commit with no new picture, the pointer moving with
    /// no cursor drawn. Waits.
    Maybe,
    /// A client's new picture, which may still show nowhere (covered, say):
    /// doesn't wait, once per wait.
    Client,
    /// A change of Pila's own (a window moved, an animation, the cursor
    /// drawn): never waits.
    Sure,
}

impl Content {
    /// `Sure` or `Maybe`.
    pub fn sure(sure: bool) -> Self {
        if sure {
            Content::Sure
        } else {
            Content::Maybe
        }
    }
}

/// A redraw held back by `Pila::defer_redraws`.
#[derive(Clone)]
pub enum Deferred {
    All,
    Output(Output, Content),
}

/// Set on a surface by a commit that brings a buffer (or removes it), until
/// the commit handler asks for its redraw (`Pila::commit_brings_content`).
#[derive(Default)]
struct BringsContent(std::cell::Cell<bool>);

/// Called before a commit applies: whether it brings something new to show
/// (`brings_content`: a buffer with damage, a first one, or none any more).
/// A commit without may still move or resize something (a subsurface, the
/// window geometry), but most often it only asks for a frame callback.
pub fn note_pending_content(surface: &WlSurface) {
    use smithay::backend::renderer::utils::RendererSurfaceStateUserData;
    use smithay::wayland::compositor::{BufferAssignment, SurfaceAttributes};
    with_states(surface, |states| {
        let (attached, damaged) = {
            let mut attributes = states.cached_state.get::<SurfaceAttributes>();
            let pending = attributes.pending();
            let attached = match &pending.buffer {
                Some(BufferAssignment::NewBuffer(_)) => Some(Attached::New),
                Some(BufferAssignment::Removed) => Some(Attached::Removed),
                None => None,
            };
            (attached, !pending.damage.is_empty())
        };
        let had_buffer = states
            .data_map
            .get::<RendererSurfaceStateUserData>()
            .is_some_and(|state| state.locked().buffer().is_some());
        if brings_content(attached, damaged, had_buffer) {
            states.data_map.insert_if_missing(BringsContent::default);
            if let Some(mark) = states.data_map.get::<BringsContent>() {
                mark.0.set(true);
            }
        }
    });
}

/// What a commit's buffer brings to show (`note_pending_content`): `Some`
/// whether a buffer was attached or removed, `damaged` whether it came with
/// damage, `had_buffer` whether the surface showed one before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Attached {
    New,
    Removed,
}

/// A new buffer without damage is the same picture again (Wayland: only
/// damage says what changed), unless it's the surface's first; a client
/// pacing itself by frame callbacks or fifo barriers may send one each time
/// it's let, and with it counted new would redraw as fast as it loops.
fn brings_content(attached: Option<Attached>, damaged: bool, had_buffer: bool) -> bool {
    match attached {
        Some(Attached::New) => damaged || !had_buffer,
        Some(Attached::Removed) => true,
        None => false,
    }
}

/// Every output of the `Space`, collected (to ask while borrowing `state`).
fn outputs_of_all(state: &Pila) -> std::vec::IntoIter<Output> {
    state
        .space
        .outputs()
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
}

/// `rect` grown by `margin` on every side.
pub fn grown(rect: Rectangle<i32, Logical>, margin: i32) -> Rectangle<i32, Logical> {
    let margin = margin.max(0);
    Rectangle::new(
        rect.loc - Point::from((margin, margin)),
        rect.size + (2 * margin, 2 * margin).into(),
    )
}

/// The surface at the root of `surface`'s subsurface tree.
pub fn root_surface(surface: &WlSurface) -> WlSurface {
    let mut root = surface.clone();
    while let Some(parent) = get_parent(&root) {
        root = parent;
    }
    root
}

impl Pila {
    /// Ask for a redraw of every output: for changes that aren't one
    /// output's, or that can't tell which.
    pub fn queue_redraw(&mut self) {
        if let Some(deferred) = &mut self.deferred_redraws {
            deferred.push(Deferred::All);
            return;
        }
        if let Some(backend) = &mut self.backend {
            backend.queue_redraw();
        }
        // On bare metal the redraw is an idle callback, which calloop only
        // runs after the next event; waking the loop makes that now.
        self.wake.ping();
    }

    /// Ask for a redraw of `output`, for something new to show there.
    pub fn queue_redraw_output(&mut self, output: &Output) {
        self.request_redraw_as(output, Content::Sure);
    }

    /// Ask for a redraw of `output`; `content`: what changed surely shows
    /// (else it may be nothing, see the module docs).
    pub fn request_redraw(&mut self, output: &Output, content: bool) {
        self.request_redraw_as(output, Content::sure(content));
    }

    /// Ask for a redraw of `output`, bringing `content`.
    pub fn request_redraw_as(&mut self, output: &Output, content: Content) {
        if let Some(deferred) = &mut self.deferred_redraws {
            deferred.push(Deferred::Output(output.clone(), content));
            return;
        }
        if let Some(backend) = &mut self.backend {
            backend.queue_redraw_output(output, content);
        }
        self.wake.ping();
    }

    /// Ask for a redraw of each output `rect` (global) overlaps.
    pub fn queue_redraw_rect(&mut self, rect: Rectangle<i32, Logical>, content: bool) {
        for output in self.outputs_overlapping(rect) {
            self.request_redraw(&output, content);
        }
    }

    /// Ask for a redraw of the outputs `window` shows on (nothing if it's
    /// not on the `Space`: minimized, on another workspace, unmapped).
    pub fn queue_redraw_window(&mut self, window: &Window) {
        if let Some(rect) = self.window_visual_rect(window) {
            self.queue_redraw_rect(rect, true);
        }
    }

    /// Map `window` at `loc` (or move it there, or raise it with
    /// `activate`), redrawing where it was and where it is: every change to
    /// the `Space`'s windows goes through these, so none forgets its redraw.
    pub fn map_window(&mut self, window: Window, loc: Point<i32, Logical>, activate: bool) {
        self.queue_redraw_window(&window);
        self.space.map_element(window.clone(), loc, activate);
        self.queue_redraw_window(&window);
    }

    /// Take `window` off the `Space`, redrawing where it was.
    pub fn unmap_window(&mut self, window: &Window) {
        self.queue_redraw_window(window);
        self.space.unmap_elem(window);
    }

    /// Raise `window` in the `Space`, redrawing where it is.
    pub fn raise_window(&mut self, window: &Window, activate: bool) {
        self.space.raise_element(window, activate);
        self.queue_redraw_window(window);
    }

    /// The pointer moved from `from` to `to`: redraw the outputs of both,
    /// where its cursor (and a drag's icon, a title bar button's highlight)
    /// was and is. `content`: Pila draws something there that moved.
    pub fn queue_redraw_pointer(
        &mut self,
        from: Point<f64, Logical>,
        to: Point<f64, Logical>,
        content: bool,
    ) {
        let mut outputs: Vec<Output> = self.space.output_under(from).cloned().collect();
        for output in self.space.output_under(to) {
            if !outputs.contains(output) {
                outputs.push(output.clone());
            }
        }
        for output in outputs {
            self.request_redraw(&output, content);
        }
    }

    /// Hold back the redraws `f` asks for until an idle callback (with a
    /// wake-up): for code run from `after_frames`, where nested the winit
    /// source has already looked for redraw requests (see "Redraws are on
    /// demand" in `docs/ARCHITECTURE.md`).
    pub fn defer_redraws(&mut self, f: impl FnOnce(&mut Self)) {
        let outer = self.deferred_redraws.replace(Vec::new());
        f(self);
        let asked = std::mem::replace(&mut self.deferred_redraws, outer).unwrap_or_default();
        if asked.is_empty() {
            return;
        }
        if let Some(outer) = &mut self.deferred_redraws {
            outer.extend(asked);
            return;
        }
        self.loop_handle.insert_idle(move |state| {
            for request in asked {
                match request {
                    Deferred::All => state.queue_redraw(),
                    Deferred::Output(output, content) => state.request_redraw_as(&output, content),
                }
            }
        });
        self.wake.ping();
    }

    /// The outputs `rect` (global) overlaps.
    pub fn outputs_overlapping(&self, rect: Rectangle<i32, Logical>) -> Vec<Output> {
        self.space
            .outputs()
            .filter(|output| {
                self.space
                    .output_geometry(output)
                    .is_some_and(|geo| geo.overlaps(rect))
            })
            .cloned()
            .collect()
    }

    /// Everything Pila may draw of a mapped `window`, global: its surfaces
    /// and popups, title bar and shadow (whether it has one or not: a little
    /// more is redrawn than needed, never less). `None` off the `Space`.
    pub fn window_visual_rect(&self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        let mut rect = self.space.element_bbox(window)?;
        let loc = self.space.element_location(window)?;
        if let Some(bar) = crate::decoration::title_bar_rect(window, loc) {
            rect = rect.merge(bar);
        }
        Some(grown(rect, self.shadow_margin()))
    }

    /// How far a window's shadow may reach past its frame, any side.
    pub fn shadow_margin(&self) -> i32 {
        let shadow = &self.config.shadow;
        shadow.size.max(0) + shadow.offset.0.abs().max(shadow.offset.1.abs())
    }

    /// The outputs `surface` (any surface of a tree: a subsurface, a popup)
    /// shows on: `Some(vec![])` if it shows nowhere (a window off the
    /// `Space`), `None` if it isn't known (no role yet, say).
    pub fn surface_outputs(&self, surface: &WlSurface) -> Option<Vec<Output>> {
        let root = root_surface(surface);
        let pointer = || {
            self.space
                .output_under(self.pointer_location())
                .cloned()
                .collect::<Vec<_>>()
        };
        if matches!(&self.cursor_status, smithay::input::pointer::CursorImageStatus::Surface(s) if *s == root)
            || self
                .dnd_icon
                .as_ref()
                .is_some_and(|icon| icon.surface == root)
        {
            return Some(pointer());
        }
        // A popup belongs to its window or layer surface.
        let root = self.popup_root(&root).unwrap_or(root);
        if let Some(window) = self.window_for_surface(&root) {
            return Some(
                self.window_visual_rect(&window)
                    .map(|rect| self.outputs_overlapping(rect))
                    .unwrap_or_default(),
            );
        }
        if let Some((output, _)) = self.layer_for_surface(&root) {
            return Some(vec![output]);
        }
        let lock_output = self.space.outputs().find(|output| {
            self.lock_surface(output)
                .is_some_and(|lock| *lock.wl_surface() == root)
        });
        if let Some(output) = lock_output {
            return Some(vec![output.clone()]);
        }
        None
    }

    /// Whether a commit of `surface` (or of its subsurfaces, applied with it)
    /// brought a buffer since the last time this was asked; forgets it.
    pub fn commit_brings_content(&self, surface: &WlSurface) -> bool {
        let mut content = false;
        with_surface_tree_downward(
            surface,
            (),
            |_, _, _| TraversalAction::DoChildren(()),
            |_, states, _| {
                if let Some(mark) = states.data_map.get::<BringsContent>() {
                    content |= mark.0.replace(false);
                }
            },
            |_, _, _| true,
        );
        content
    }

    /// Redraw what a commit of `surface` changed: the outputs it showed on
    /// before (`before`, from `surface_outputs`) and shows on now.
    pub fn redraw_committed(&mut self, surface: &WlSurface, before: Option<Vec<Output>>) {
        // A client's picture: new, maybe shown nowhere.
        let content = if self.commit_brings_content(surface) {
            Content::Client
        } else {
            Content::Maybe
        };
        let (Some(mut outputs), Some(after)) = (before, self.surface_outputs(surface)) else {
            tracing::trace!(target: "pila::frames", "commit of an unknown surface: every output");
            outputs_of_all(self).for_each(|output| self.request_redraw_as(&output, content));
            return;
        };
        for output in after {
            if !outputs.contains(&output) {
                outputs.push(output);
            }
        }
        // Shown nowhere, it may still be captured (`after_frames`).
        self.capture_due |= outputs.is_empty();
        for output in outputs {
            self.request_redraw_as(&output, content);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_buffer_brings_something_new_only_with_damage_or_as_the_first() {
        use Attached::*;
        // A new buffer with damage, a surface's first, or none any more.
        assert!(brings_content(Some(New), true, true));
        assert!(brings_content(Some(New), false, false));
        assert!(brings_content(Some(Removed), false, true));
        // The same picture again (a new buffer, no damage: nothing changed),
        // as a client pacing itself by frame callbacks or fifo barriers
        // does: it may bring nothing, so it doesn't skip the wait after a
        // redraw with nothing new (`udev/draw.rs::idle_wait`).
        assert!(!brings_content(Some(New), false, true));
        // No buffer at all.
        assert!(!brings_content(None, true, true));
    }
}
