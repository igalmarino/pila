//! Transient windows: a dialog set on its parent with
//! `xdg_toplevel.set_parent` (or xdg-foreign's `set_parent_of`) or, on X11,
//! `WM_TRANSIENT_FOR`. A window and its descendants are a family: children
//! stay above their parent in the `Space`'s stacking order, and the family
//! minimizes and changes workspace together.
//!
//! Smithay's `Space::map_element` puts the window on top, so whatever moves
//! a mapped window then calls `keep_transients_above`.
//!
//! X11 can make cycles of `WM_TRANSIENT_FOR`, so walks remember where they
//! have been instead of trusting the relation to be a tree.

use smithay::desktop::Window;

use crate::{workspaces::is_override_redirect, Pila};

/// Whether `parent` is the window `child` is set on.
fn is_parent_of(parent: &Window, child: &Window) -> bool {
    if parent == child {
        return false;
    }
    if let Some(x11) = child.x11_surface() {
        let parent_id = parent.x11_surface().map(|p| p.window_id());
        return !x11.is_override_redirect()
            && x11.is_transient_for().is_some()
            && x11.is_transient_for() == parent_id;
    }
    match (child.toplevel().and_then(|t| t.parent()), parent.toplevel()) {
        (Some(surface), Some(toplevel)) => surface == *toplevel.wl_surface(),
        _ => false,
    }
}

impl Pila {
    /// Managed windows, on the `Space` (back to front) or off it.
    fn managed_windows(&self) -> impl Iterator<Item = &Window> {
        self.space
            .elements()
            .chain(self.off_space_windows())
            .filter(|w| !is_override_redirect(w))
    }

    /// The window `window` is set on, if that one is managed.
    pub fn transient_parent(&self, window: &Window) -> Option<Window> {
        self.managed_windows()
            .find(|w| is_parent_of(w, window))
            .cloned()
    }

    /// The windows set on `window`, back to front.
    pub fn transient_children(&self, window: &Window) -> Vec<Window> {
        self.managed_windows()
            .filter(|w| is_parent_of(window, w))
            .cloned()
            .collect()
    }

    /// `window`'s ancestors, nearest first.
    fn transient_ancestors(&self, window: &Window) -> Vec<Window> {
        let mut ancestors: Vec<Window> = Vec::new();
        let mut current = window.clone();
        while let Some(parent) = self.transient_parent(&current) {
            if parent == *window || ancestors.contains(&parent) {
                break;
            }
            ancestors.push(parent.clone());
            current = parent;
        }
        ancestors
    }

    /// `root` and its descendants, each before its children. Children keep
    /// their order, except that the one leading to `path` goes last.
    fn descendants_in_order(&self, root: &Window, path: &[Window], out: &mut Vec<Window>) {
        if out.contains(root) {
            return;
        }
        out.push(root.clone());
        let children = self.transient_children(root);
        let (leading, others): (Vec<_>, Vec<_>) =
            children.into_iter().partition(|w| path.contains(w));
        for child in others.iter().chain(&leading) {
            self.descendants_in_order(child, path, out);
        }
    }

    /// `window`'s family, each window before its children, ending as high
    /// as it can with `window`'s own line.
    pub fn transient_family(&self, window: &Window) -> Vec<Window> {
        let mut path = self.transient_ancestors(window);
        let root = path.last().cloned().unwrap_or_else(|| window.clone());
        path.push(window.clone());
        let mut family = Vec::new();
        self.descendants_in_order(&root, &path, &mut family);
        family
    }

    /// Put `windows` on top, in order, in X's stacking order too.
    fn raise_in_order(&mut self, windows: &[Window]) {
        for window in windows {
            self.space.raise_element(window, false);
            self.x11_window_raised(window);
        }
    }

    /// Raise `window` with its family, and make it the active window. Its
    /// children stay above it even though it has the focus.
    pub fn raise_family(&mut self, window: &Window) {
        let family = self.transient_family(window);
        self.raise_in_order(&family);
        for w in self.space.elements() {
            w.set_activated(w == window);
        }
    }

    /// Put `window`'s descendants back above it, after it was raised alone.
    /// Nothing for a window without children, the usual case.
    pub fn keep_transients_above(&mut self, window: &Window) {
        let mut descendants = Vec::new();
        self.descendants_in_order(window, &[], &mut descendants);
        if descendants.len() > 1 {
            self.raise_in_order(&descendants[1..]);
        }
    }

    /// `window` was set on another window, or on none: if that left it
    /// below its new parent, it comes above, without taking the focus.
    pub fn transient_parent_changed(&mut self, window: &Window) {
        let Some(parent) = self.transient_parent(window) else {
            return;
        };
        let position = |w: &Window, space: &smithay::desktop::Space<Window>| {
            space.elements().position(|e| e == w)
        };
        let (Some(child_at), Some(parent_at)) = (
            position(window, &self.space),
            position(&parent, &self.space),
        ) else {
            return;
        };
        if child_at < parent_at {
            let mut descendants = Vec::new();
            self.descendants_in_order(window, &[], &mut descendants);
            self.raise_in_order(&descendants);
            self.refresh_pointer_focus();
            self.queue_redraw();
        }
    }
}
