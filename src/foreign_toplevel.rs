//! ext-foreign-toplevel-list: lets taskbars and window switchers list the
//! open windows (title and app id). Read-only: activating, minimizing and
//! closing go through wlr-foreign-toplevel-management (`toplevel_management.rs`),
//! which identifies windows by the identifier of their handle here.
//!
//! A window gets its handle when it's mapped (`place_new_window`), stored in
//! its user data. Commits update the title and app id, and destroying the
//! toplevel closes the handle.

use smithay::{
    desktop::Window,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    wayland::{
        compositor::with_states, foreign_toplevel_list::ForeignToplevelHandle,
        shell::xdg::XdgToplevelSurfaceData,
    },
};

use crate::Pila;

/// The window's (title, app id), empty if unset.
pub(crate) fn title_and_app_id(window: &Window) -> (String, String) {
    // X11: the title, and WM_CLASS's class as the app id.
    if let Some(x11) = window.x11_surface() {
        return (x11.title(), x11.class());
    }
    let Some(toplevel) = window.toplevel() else {
        return Default::default();
    };
    with_states(toplevel.wl_surface(), |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .and_then(|data| {
                let data = data.lock().ok()?;
                Some((
                    data.title.clone().unwrap_or_default(),
                    data.app_id.clone().unwrap_or_default(),
                ))
            })
            .unwrap_or_default()
    })
}

/// Send the window's title and app id to listeners if they changed.
fn update(window: &Window) {
    let Some(handle) = handle(window) else {
        return;
    };
    let (title, app_id) = title_and_app_id(window);
    if handle.title() == title && handle.app_id() == app_id {
        return;
    }
    handle.send_title(&title);
    handle.send_app_id(&app_id);
    handle.send_done();
}

fn handle(window: &Window) -> Option<&ForeignToplevelHandle> {
    window.user_data().get::<ForeignToplevelHandle>()
}

/// The window's foreign-toplevel identifier (screen capture names windows
/// by it), once it has been mapped.
pub fn identifier(window: &Window) -> Option<String> {
    handle(window).map(ForeignToplevelHandle::identifier)
}

impl Pila {
    /// The window a foreign-toplevel identifier names, mapped, minimized or
    /// on another workspace.
    pub(crate) fn window_for_identifier(&self, identifier: &str) -> Option<Window> {
        self.space
            .elements()
            .chain(self.off_space_windows())
            .find(|w| crate::foreign_toplevel::identifier(w).as_deref() == Some(identifier))
            .cloned()
    }

    /// List a newly mapped window.
    pub fn announce_toplevel(&mut self, window: &Window) {
        if handle(window).is_some() {
            return;
        }
        let (title, app_id) = title_and_app_id(window);
        let handle = self
            .globals
            .foreign_toplevel_list
            .new_toplevel::<Pila>(title, app_id);
        window.user_data().insert_if_missing_threadsafe(|| handle);
    }

    /// Send title and app id changes. Called on every commit.
    pub fn update_foreign_toplevel(&self, surface: &WlSurface) {
        let window = self
            .space
            .elements()
            .find(|w| w.toplevel().is_some_and(|t| t.wl_surface() == surface));
        if let Some(window) = window {
            update(window);
        }
    }

    /// Send an X11 window's title and class (after a property change).
    pub fn update_foreign_toplevel_x11(&self, window: &Window) {
        update(window);
    }

    /// Unlist a window whose toplevel was destroyed.
    pub fn remove_foreign_toplevel(&mut self, window: &Window) {
        if let Some(handle) = handle(window) {
            self.globals.foreign_toplevel_list.remove_toplevel(handle);
        }
        self.toplevel_management.closed(window);
    }
}
