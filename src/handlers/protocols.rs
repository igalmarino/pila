// Protocols that need little or no compositor logic: a global plus a
// delegate, sometimes a trivial handler. Created in `init_globals`; the ones
// whose state a handler must return are stored in `Pila`.

use smithay::{
    delegate_content_type, delegate_ext_data_control, delegate_foreign_toplevel_list,
    delegate_fractional_scale, delegate_keyboard_shortcuts_inhibit, delegate_pointer_gestures,
    delegate_presentation, delegate_relative_pointer, delegate_single_pixel_buffer,
    delegate_viewporter, delegate_xdg_foreign, delegate_xdg_toplevel_icon,
    reexports::{
        calloop::LoopHandle,
        wayland_server::{protocol::wl_surface::WlSurface, DisplayHandle},
    },
    wayland::{
        content_type::ContentTypeState,
        foreign_toplevel_list::{ForeignToplevelListHandler, ForeignToplevelListState},
        fractional_scale::{FractionalScaleHandler, FractionalScaleManagerState},
        idle_inhibit::IdleInhibitManagerState,
        idle_notify::IdleNotifierState,
        keyboard_shortcuts_inhibit::{
            KeyboardShortcutsInhibitHandler, KeyboardShortcutsInhibitState,
            KeyboardShortcutsInhibitor,
        },
        pointer_constraints::PointerConstraintsState,
        pointer_gestures::PointerGesturesState,
        relative_pointer::RelativePointerManagerState,
        selection::{ext_data_control, primary_selection::PrimarySelectionState},
        single_pixel_buffer::SinglePixelBufferState,
        viewporter::ViewporterState,
        xdg_foreign::{XdgForeignHandler, XdgForeignState},
        xdg_toplevel_icon::{XdgToplevelIconHandler, XdgToplevelIconManager},
    },
};

use crate::Pila;

/// Protocol state that handlers need to hand back to Smithay.
pub struct Globals {
    pub xdg_foreign: XdgForeignState,
    pub ext_data_control: ext_data_control::DataControlState,
    pub foreign_toplevel_list: ForeignToplevelListState,
    pub keyboard_shortcuts_inhibit: KeyboardShortcutsInhibitState,
    pub idle_notifier: IdleNotifierState<Pila>,
}

/// Create the globals of this module.
pub fn init_globals(
    dh: &DisplayHandle,
    loop_handle: &LoopHandle<'static, Pila>,
    primary_selection: &PrimarySelectionState,
) -> Globals {
    // Stateless as far as pila is concerned: the global lives on after the
    // state is dropped.
    ViewporterState::new::<Pila>(dh);
    SinglePixelBufferState::new::<Pila>(dh);
    ContentTypeState::new::<Pila>(dh);
    FractionalScaleManagerState::new::<Pila>(dh);
    PointerGesturesState::new::<Pila>(dh);
    RelativePointerManagerState::new::<Pila>(dh);
    // Its handler is in `pointer_constraints.rs`.
    PointerConstraintsState::new::<Pila>(dh);
    XdgToplevelIconManager::new::<Pila>(dh);
    // Its handler is in `idle.rs`.
    IdleInhibitManagerState::new::<Pila>(dh);

    Globals {
        xdg_foreign: XdgForeignState::new::<Pila>(dh),
        foreign_toplevel_list: ForeignToplevelListState::new::<Pila>(dh),
        keyboard_shortcuts_inhibit: KeyboardShortcutsInhibitState::new::<Pila>(dh),
        idle_notifier: IdleNotifierState::new(dh, loop_handle.clone()),
        // Clipboard managers and wl-clipboard; any client may use it.
        ext_data_control: ext_data_control::DataControlState::new::<Pila, _>(
            dh,
            Some(primary_selection),
            |_| true,
        ),
    }
}

// wp_viewporter: clients (video players, browsers) crop and scale buffers;
// the renderer applies it.
delegate_viewporter!(Pila);

// wp_single_pixel_buffer_manager_v1: one-color buffers (backgrounds, borders).
delegate_single_pixel_buffer!(Pila);

// wp_presentation: when frames reached the screen. The global is created in
// `Pila::new` (it needs the clock); feedback is sent from `frame.rs`.
delegate_presentation!(Pila);

// wp_content_type_v1: a hint (video, game, photo); unused for now.
delegate_content_type!(Pila);

// wp_pointer_gestures_v1, zwp_relative_pointer_v1: the seat forwards these;
// the nested backend produces neither yet, libinput will.
delegate_pointer_gestures!(Pila);
delegate_relative_pointer!(Pila);

// xdg_toplevel_icon_v1: the icon is kept in the surface's cached state; pila
// doesn't draw it (yet).
impl XdgToplevelIconHandler for Pila {}
delegate_xdg_toplevel_icon!(Pila);

// wp_fractional_scale_v1: tell each surface the scale of the output it's on
// (`scale.rs`, which keeps it up to date as surfaces move between outputs).
impl FractionalScaleHandler for Pila {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        let output = self.output_for_surface(&surface);
        self.send_initial_scale(&surface, output);
    }
}
delegate_fractional_scale!(Pila);

// zxdg_exporter_v2/importer_v2: portals parent their dialogs to app windows.
impl XdgForeignHandler for Pila {
    fn xdg_foreign_state(&mut self) -> &mut XdgForeignState {
        &mut self.globals.xdg_foreign
    }
}
delegate_xdg_foreign!(Pila);

// ext_data_control_v1: clipboard access without focus. (The older
// zwlr_data_control_v1 is deliberately not offered; see CLAUDE.md.)
impl ext_data_control::DataControlHandler for Pila {
    fn data_control_state(&self) -> &ext_data_control::DataControlState {
        &self.globals.ext_data_control
    }
}
delegate_ext_data_control!(Pila);

// ext_foreign_toplevel_list_v1: taskbars list windows (`foreign_toplevel.rs`).
impl ForeignToplevelListHandler for Pila {
    fn foreign_toplevel_list_state(&mut self) -> &mut ForeignToplevelListState {
        &mut self.globals.foreign_toplevel_list
    }
}
delegate_foreign_toplevel_list!(Pila);

// zwp_keyboard_shortcuts_inhibit_manager_v1: VMs and remote desktops get the
// keys pila would otherwise bind. Every request is granted; it applies while
// the surface has keyboard focus (`Pila::shortcuts_inhibited`), so clicking
// another window always gets out, and Quit is never inhibited.
impl KeyboardShortcutsInhibitHandler for Pila {
    fn keyboard_shortcuts_inhibit_state(&mut self) -> &mut KeyboardShortcutsInhibitState {
        &mut self.globals.keyboard_shortcuts_inhibit
    }

    fn new_inhibitor(&mut self, inhibitor: KeyboardShortcutsInhibitor) {
        inhibitor.activate();
    }
}
delegate_keyboard_shortcuts_inhibit!(Pila);
