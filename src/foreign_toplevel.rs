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
        compositor::with_states,
        foreign_toplevel_list::ForeignToplevelHandle,
        shell::xdg::{XdgToplevelSurfaceData, XdgToplevelSurfaceRoleAttributes},
    },
};

use crate::Pila;

/// The window's (title, app id), empty if unset.
pub(crate) fn title_and_app_id(window: &Window) -> (String, String) {
    // X11: the title, and WM_CLASS's class as the app id.
    if let Some(x11) = window.x11_surface() {
        return (x11.title(), x11.class());
    }
    xdg_attributes(window, |data| {
        (
            data.title.clone().unwrap_or_default(),
            data.app_id.clone().unwrap_or_default(),
        )
    })
    .unwrap_or_default()
}

/// The window's title, empty if unset (`title_and_app_id` without the app
/// id: the title bar asks for it each time it's drawn).
pub(crate) fn title(window: &Window) -> String {
    if let Some(x11) = window.x11_surface() {
        return x11.title();
    }
    xdg_attributes(window, |data| data.title.clone().unwrap_or_default()).unwrap_or_default()
}

/// The window's app id (an X11 window's: WM_CLASS's class), empty if unset
/// (`title_and_app_id` without the title).
pub(crate) fn app_id(window: &Window) -> String {
    if let Some(x11) = window.x11_surface() {
        return x11.class();
    }
    xdg_attributes(window, |data| data.app_id.clone().unwrap_or_default()).unwrap_or_default()
}

/// What `read` takes from an xdg toplevel's attributes; `None` for an X11
/// window, or one whose attributes can't be read.
fn xdg_attributes<T>(
    window: &Window,
    read: impl FnOnce(&XdgToplevelSurfaceRoleAttributes) -> T,
) -> Option<T> {
    let toplevel = window.toplevel()?;
    with_states(toplevel.wl_surface(), |states| {
        let data = states.data_map.get::<XdgToplevelSurfaceData>()?;
        Some(read(&*data.lock().ok()?))
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

    /// Send title and app id changes, of windows on the screen or off it
    /// (minimized, on another workspace). Called on every commit and as
    /// either is set.
    pub fn update_foreign_toplevel(&self, surface: &WlSurface) {
        if let Some(window) = self.window_for_surface(surface) {
            update(&window);
        }
    }

    /// Send an X11 window's title and class (after a property change).
    pub fn update_foreign_toplevel_x11(&self, window: &Window) {
        update(window);
    }

    /// Unlist a window whose toplevel was destroyed.
    pub fn remove_foreign_toplevel(&mut self, window: &Window) {
        // Captures name the window by its identifier, which dies with it.
        if let Some(identifier) = identifier(window) {
            self.stop_window_captures(&identifier);
        }
        if let Some(handle) = handle(window) {
            self.globals.foreign_toplevel_list.remove_toplevel(handle);
        }
        self.toplevel_management.closed(window);
    }
}
