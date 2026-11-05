//! The synthetic client's protocols beyond a toplevel: any global can be
//! bound (`Client::bind`) and its objects' events are logged (`Logged`, by
//! the tag given when they were created); layer, popup and lock surfaces
//! ack and draw like windows (`Surf`); data sources answer with their text.

use std::os::fd::{AsFd, FromRawFd, OwnedFd};

use wayland_client::{
    event_created_child,
    protocol::{
        wl_callback::WlCallback,
        wl_data_device::{self, WlDataDevice},
        wl_data_device_manager::WlDataDeviceManager,
        wl_data_offer::WlDataOffer,
        wl_data_source::{self, WlDataSource},
        wl_output::WlOutput,
        wl_region::WlRegion,
        wl_subcompositor::WlSubcompositor,
        wl_subsurface::WlSubsurface,
        wl_surface::WlSurface,
    },
    Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_protocols::{
    ext::{
        background_effect::v1::client::{
            ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1,
            ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1,
        },
        data_control::v1::client::{
            ext_data_control_device_v1::{self, ExtDataControlDeviceV1},
            ext_data_control_manager_v1::ExtDataControlManagerV1,
            ext_data_control_offer_v1::ExtDataControlOfferV1,
            ext_data_control_source_v1::{self, ExtDataControlSourceV1},
        },
        foreign_toplevel_list::v1::client::{
            ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
            ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1},
        },
        idle_notify::v1::client::{
            ext_idle_notification_v1::ExtIdleNotificationV1,
            ext_idle_notifier_v1::ExtIdleNotifierV1,
        },
        image_capture_source::v1::client::{
            ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1,
            ext_image_capture_source_v1::ExtImageCaptureSourceV1,
            ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
        },
        image_copy_capture::v1::client::{
            ext_image_copy_capture_cursor_session_v1::ExtImageCopyCaptureCursorSessionV1,
            ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1,
            ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1,
            ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1,
        },
        session_lock::v1::client::{
            ext_session_lock_manager_v1::ExtSessionLockManagerV1,
            ext_session_lock_surface_v1::{self, ExtSessionLockSurfaceV1},
            ext_session_lock_v1::ExtSessionLockV1,
        },
        workspace::v1::client::{
            ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1,
            ext_workspace_handle_v1::ExtWorkspaceHandleV1,
            ext_workspace_manager_v1::{self, ExtWorkspaceManagerV1},
        },
    },
    wp::{
        content_type::v1::client::{
            wp_content_type_manager_v1::WpContentTypeManagerV1, wp_content_type_v1::WpContentTypeV1,
        },
        cursor_shape::v1::client::{
            wp_cursor_shape_device_v1::WpCursorShapeDeviceV1,
            wp_cursor_shape_manager_v1::WpCursorShapeManagerV1,
        },
        fractional_scale::v1::client::{
            wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
            wp_fractional_scale_v1::WpFractionalScaleV1,
        },
        idle_inhibit::zv1::client::{
            zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1,
            zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1,
        },
        keyboard_shortcuts_inhibit::zv1::client::{
            zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1,
            zwp_keyboard_shortcuts_inhibitor_v1::ZwpKeyboardShortcutsInhibitorV1,
        },
        pointer_constraints::zv1::client::{
            zwp_confined_pointer_v1::ZwpConfinedPointerV1,
            zwp_locked_pointer_v1::ZwpLockedPointerV1,
            zwp_pointer_constraints_v1::ZwpPointerConstraintsV1,
        },
        pointer_gestures::zv1::client::{
            zwp_pointer_gesture_hold_v1::ZwpPointerGestureHoldV1,
            zwp_pointer_gesture_pinch_v1::ZwpPointerGesturePinchV1,
            zwp_pointer_gesture_swipe_v1::ZwpPointerGestureSwipeV1,
            zwp_pointer_gestures_v1::ZwpPointerGesturesV1,
        },
        presentation_time::client::{
            wp_presentation::WpPresentation, wp_presentation_feedback::WpPresentationFeedback,
        },
        primary_selection::zv1::client::{
            zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1,
            zwp_primary_selection_device_v1::{self, ZwpPrimarySelectionDeviceV1},
            zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1,
            zwp_primary_selection_source_v1::{self, ZwpPrimarySelectionSourceV1},
        },
        relative_pointer::zv1::client::{
            zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1,
            zwp_relative_pointer_v1::ZwpRelativePointerV1,
        },
        security_context::v1::client::{
            wp_security_context_manager_v1::WpSecurityContextManagerV1,
            wp_security_context_v1::WpSecurityContextV1,
        },
        single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1,
        viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
    },
    xdg::{
        activation::v1::client::{
            xdg_activation_token_v1::XdgActivationTokenV1, xdg_activation_v1::XdgActivationV1,
        },
        foreign::zv2::client::{
            zxdg_exported_v2::ZxdgExportedV2, zxdg_exporter_v2::ZxdgExporterV2,
            zxdg_imported_v2::ZxdgImportedV2, zxdg_importer_v2::ZxdgImporterV2,
        },
        shell::client::{
            xdg_popup::{self, XdgPopup},
            xdg_positioner::{self, XdgPositioner},
            xdg_surface::{self, XdgSurface},
        },
        system_bell::v1::client::xdg_system_bell_v1::XdgSystemBellV1,
        xdg_output::zv1::client::{
            zxdg_output_manager_v1::ZxdgOutputManagerV1, zxdg_output_v1::ZxdgOutputV1,
        },
    },
};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};
use wayland_protocols_wlr::{
    foreign_toplevel::v1::client::{
        zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
        zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
    },
    gamma_control::v1::client::{
        zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1,
        zwlr_gamma_control_v1::ZwlrGammaControlV1,
    },
    layer_shell::v1::client::{
        zwlr_layer_shell_v1::{self, ZwlrLayerShellV1},
        zwlr_layer_surface_v1::{self, ZwlrLayerSurfaceV1},
    },
    output_management::v1::client::{
        zwlr_output_configuration_head_v1::ZwlrOutputConfigurationHeadV1,
        zwlr_output_configuration_v1::ZwlrOutputConfigurationV1,
        zwlr_output_head_v1::{self, ZwlrOutputHeadV1},
        zwlr_output_manager_v1::{self, ZwlrOutputManagerV1},
        zwlr_output_mode_v1::ZwlrOutputModeV1,
    },
    output_power_management::v1::client::{
        zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1,
        zwlr_output_power_v1::ZwlrOutputPowerV1,
    },
    screencopy::v1::client::{
        zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
        zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
    },
};

use super::{
    client::{paint, Client, Handler, Logged, SurfaceData, Window},
    session_protocol::{
        xdg_session_manager_v1::{Reason, XdgSessionManagerV1},
        xdg_session_v1::XdgSessionV1,
        xdg_toplevel_session_v1::XdgToplevelSessionV1,
    },
};

/// Implement `Dispatch<$iface, String>` by logging every event with the
/// object's tag; children created by events get the tags given.
macro_rules! logged {
    ($($iface:ty),* $(,)?) => {
        $(
            impl Dispatch<$iface, String> for Handler {
                fn event(
                    state: &mut Self,
                    proxy: &$iface,
                    event: <$iface as Proxy>::Event,
                    tag: &String,
                    _: &Connection,
                    _: &QueueHandle<Self>,
                ) {
                    log(state, tag, proxy.id(), format!("{event:?}"));
                }
            }
        )*
    };
}

macro_rules! logged_with_children {
    ($iface:ty, [$($opcode:path => ($child:ty, $tag:expr)),* $(,)?]) => {
        impl Dispatch<$iface, String> for Handler {
            fn event(
                state: &mut Self,
                proxy: &$iface,
                event: <$iface as Proxy>::Event,
                tag: &String,
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                log(state, tag, proxy.id(), format!("{event:?}"));
            }
            event_created_child!(Handler, $iface, [$($opcode => ($child, String::from($tag))),*]);
        }
    };
}

fn log(state: &mut Handler, tag: &str, object: wayland_client::backend::ObjectId, event: String) {
    state.lock().log.push(Logged {
        tag: tag.to_string(),
        id: object.protocol_id(),
        object,
        event,
    });
}

logged!(
    WlCallback,
    WlSubcompositor,
    WlSubsurface,
    WlRegion,
    WlDataDeviceManager,
    WlDataOffer,
    ZwlrLayerShellV1,
    ExtSessionLockManagerV1,
    ExtSessionLockV1,
    ExtIdleNotifierV1,
    ExtIdleNotificationV1,
    ZwpIdleInhibitManagerV1,
    ZwpIdleInhibitorV1,
    ZwpPointerConstraintsV1,
    ZwpLockedPointerV1,
    ZwpConfinedPointerV1,
    ZwpRelativePointerManagerV1,
    ZwpRelativePointerV1,
    ZwpPointerGesturesV1,
    ZwpPointerGestureSwipeV1,
    ZwpPointerGesturePinchV1,
    ZwpPointerGestureHoldV1,
    ZwpKeyboardShortcutsInhibitManagerV1,
    ZwpKeyboardShortcutsInhibitorV1,
    XdgActivationV1,
    XdgActivationTokenV1,
    XdgSystemBellV1,
    ZwpPrimarySelectionDeviceManagerV1,
    ZwpPrimarySelectionOfferV1,
    ExtDataControlManagerV1,
    ExtDataControlOfferV1,
    WpFractionalScaleManagerV1,
    WpFractionalScaleV1,
    WpViewporter,
    WpViewport,
    WpCursorShapeManagerV1,
    WpCursorShapeDeviceV1,
    WpSecurityContextManagerV1,
    WpSecurityContextV1,
    ZwpVirtualKeyboardManagerV1,
    ZwpVirtualKeyboardV1,
    WpPresentation,
    WpPresentationFeedback,
    WpContentTypeManagerV1,
    WpContentTypeV1,
    WpSinglePixelBufferManagerV1,
    ZxdgExporterV2,
    ZxdgExportedV2,
    ZxdgImporterV2,
    ZxdgImportedV2,
    ZxdgOutputManagerV1,
    ZxdgOutputV1,
    ExtForeignToplevelHandleV1,
    ZwlrForeignToplevelHandleV1,
    ExtWorkspaceGroupHandleV1,
    ExtWorkspaceHandleV1,
    ExtOutputImageCaptureSourceManagerV1,
    ExtForeignToplevelImageCaptureSourceManagerV1,
    ExtImageCaptureSourceV1,
    ExtImageCopyCaptureManagerV1,
    ExtImageCopyCaptureSessionV1,
    ExtImageCopyCaptureFrameV1,
    ExtImageCopyCaptureCursorSessionV1,
    ZwlrScreencopyManagerV1,
    ZwlrScreencopyFrameV1,
    ZwlrOutputModeV1,
    ZwlrOutputConfigurationV1,
    ZwlrOutputConfigurationHeadV1,
    ZwlrOutputPowerManagerV1,
    ZwlrOutputPowerV1,
    ZwlrGammaControlManagerV1,
    ZwlrGammaControlV1,
    ExtBackgroundEffectManagerV1,
    ExtBackgroundEffectSurfaceV1,
);

// Data devices: logged, and the current selection offer kept.
impl Dispatch<WlDataDevice, String> for Handler {
    fn event(
        state: &mut Self,
        proxy: &WlDataDevice,
        event: wl_data_device::Event,
        tag: &String,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        log(state, tag, proxy.id(), format!("{event:?}"));
        if let wl_data_device::Event::Selection { id } = event {
            state.lock().data_offer = id;
        }
    }
    event_created_child!(Handler, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, String::from("wl_data_offer")),
    ]);
}

impl Dispatch<ZwpPrimarySelectionDeviceV1, String> for Handler {
    fn event(
        state: &mut Self,
        proxy: &ZwpPrimarySelectionDeviceV1,
        event: zwp_primary_selection_device_v1::Event,
        tag: &String,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        log(state, tag, proxy.id(), format!("{event:?}"));
        if let zwp_primary_selection_device_v1::Event::Selection { id } = event {
            state.lock().primary_offer = id;
        }
    }
    event_created_child!(Handler, ZwpPrimarySelectionDeviceV1, [
        zwp_primary_selection_device_v1::EVT_DATA_OFFER_OPCODE =>
            (ZwpPrimarySelectionOfferV1, String::from("primary_offer")),
    ]);
}

impl Dispatch<ExtDataControlDeviceV1, String> for Handler {
    fn event(
        state: &mut Self,
        proxy: &ExtDataControlDeviceV1,
        event: ext_data_control_device_v1::Event,
        tag: &String,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        log(state, tag, proxy.id(), format!("{event:?}"));
        match event {
            ext_data_control_device_v1::Event::Selection { id } => {
                state.lock().control_offer = id;
            }
            ext_data_control_device_v1::Event::PrimarySelection { id } => {
                state.lock().control_primary_offer = id;
            }
            _ => {}
        }
    }
    event_created_child!(Handler, ExtDataControlDeviceV1, [
        ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE =>
            (ExtDataControlOfferV1, String::from("data_control_offer")),
    ]);
}
logged_with_children!(ExtForeignToplevelListV1, [
    ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE =>
        (ExtForeignToplevelHandleV1, "ext_toplevel"),
]);
logged_with_children!(ZwlrForeignToplevelManagerV1, [
    zwlr_foreign_toplevel_manager_v1::EVT_TOPLEVEL_OPCODE =>
        (ZwlrForeignToplevelHandleV1, "wlr_toplevel"),
]);
logged_with_children!(ExtWorkspaceManagerV1, [
    ext_workspace_manager_v1::EVT_WORKSPACE_GROUP_OPCODE =>
        (ExtWorkspaceGroupHandleV1, "workspace_group"),
    ext_workspace_manager_v1::EVT_WORKSPACE_OPCODE => (ExtWorkspaceHandleV1, "workspace"),
]);
logged_with_children!(ZwlrOutputManagerV1, [
    zwlr_output_manager_v1::EVT_HEAD_OPCODE => (ZwlrOutputHeadV1, "head"),
]);
logged_with_children!(ZwlrOutputHeadV1, [
    zwlr_output_head_v1::EVT_MODE_OPCODE => (ZwlrOutputModeV1, "mode"),
]);

// --- Data sources: they answer `send` with their text ------------------------

/// A selection source's tag and the text it offers.
pub struct SourceText {
    pub tag: String,
    pub text: String,
}

fn send_text(fd: OwnedFd, text: &str) {
    use std::io::Write;
    let mut file = std::fs::File::from(fd);
    let _ = file.write_all(text.as_bytes());
}

macro_rules! source {
    ($iface:ty, $module:ident) => {
        impl Dispatch<$iface, SourceText> for Handler {
            fn event(
                state: &mut Self,
                proxy: &$iface,
                event: $module::Event,
                data: &SourceText,
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                log(state, &data.tag, proxy.id(), format!("{event:?}"));
                if let $module::Event::Send { fd, .. } = event {
                    send_text(fd, &data.text);
                }
            }
        }
    };
}
source!(WlDataSource, wl_data_source);
source!(ZwpPrimarySelectionSourceV1, zwp_primary_selection_source_v1);
source!(ExtDataControlSourceV1, ext_data_control_source_v1);

// --- Surfaces that ack and draw ------------------------------------------------

/// A surface of `Shared::surfaces`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Surf(pub usize);

/// An xdg_surface/xdg_popup of a popup, by its index in `Shared::surfaces`.
pub struct PopupRef(pub usize);

fn configured(s: &SurfaceData, w: i32, h: i32) -> (i32, i32) {
    (
        if w > 0 { w } else { s.default_size.0 },
        if h > 0 { h } else { s.default_size.1 },
    )
}

impl Dispatch<ZwlrLayerSurfaceV1, usize> for Handler {
    fn event(
        state: &mut Self,
        proxy: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        index: &usize,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let s = &mut *state.lock();
        let shm = s.shm.clone().expect("wl_shm");
        let surface = &mut s.surfaces[*index];
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                proxy.ack_configure(serial);
                surface.configures.push((0, 0, width as i32, height as i32));
                if surface.auto_draw {
                    let size = configured(surface, width as i32, height as i32);
                    paint(&shm, qh, &surface.surface, surface.color, size);
                    surface.size = size;
                }
            }
            zwlr_layer_surface_v1::Event::Closed => surface.closed = true,
            _ => {}
        }
    }
}

impl Dispatch<XdgSurface, PopupRef> for Handler {
    fn event(
        state: &mut Self,
        proxy: &XdgSurface,
        event: xdg_surface::Event,
        index: &PopupRef,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            let s = &mut *state.lock();
            let shm = s.shm.clone().expect("wl_shm");
            let surface = &mut s.surfaces[index.0];
            proxy.ack_configure(serial);
            let pending = surface.pending;
            surface.configures.push(pending);
            if surface.auto_draw {
                let size = configured(surface, pending.2, pending.3);
                paint(&shm, qh, &surface.surface, surface.color, size);
                surface.size = size;
            }
        }
    }
}

impl Dispatch<XdgPopup, PopupRef> for Handler {
    fn event(
        state: &mut Self,
        proxy: &XdgPopup,
        event: xdg_popup::Event,
        index: &PopupRef,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let s = &mut *state.lock();
        s.log.push(Logged {
            tag: format!("popup:{}", s.surfaces[index.0].name),
            id: proxy.id().protocol_id(),
            object: proxy.id(),
            event: format!("{event:?}"),
        });
        let surface = &mut s.surfaces[index.0];
        match event {
            xdg_popup::Event::Configure {
                x,
                y,
                width,
                height,
            } => surface.pending = (x, y, width, height),
            xdg_popup::Event::PopupDone => surface.closed = true,
            _ => {}
        }
    }
}

impl Dispatch<XdgPositioner, ()> for Handler {
    fn event(
        _: &mut Self,
        _: &XdgPositioner,
        _: xdg_positioner::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtSessionLockSurfaceV1, usize> for Handler {
    fn event(
        state: &mut Self,
        proxy: &ExtSessionLockSurfaceV1,
        event: ext_session_lock_surface_v1::Event,
        index: &usize,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let ext_session_lock_surface_v1::Event::Configure {
            serial,
            width,
            height,
        } = event
        {
            let s = &mut *state.lock();
            let shm = s.shm.clone().expect("wl_shm");
            let surface = &mut s.surfaces[*index];
            proxy.ack_configure(serial);
            surface.configures.push((0, 0, width as i32, height as i32));
            if surface.auto_draw {
                let size = (width as i32, height as i32);
                paint(&shm, qh, &surface.surface, surface.color, size);
                surface.size = size;
            }
        }
    }
}

/// Where a popup goes, relative to its parent's geometry.
#[derive(Debug, Clone, Copy)]
pub struct Placement {
    pub size: (i32, i32),
    pub anchor_rect: (i32, i32, i32, i32),
    pub anchor: xdg_positioner::Anchor,
    pub gravity: xdg_positioner::Gravity,
    pub constraints: xdg_positioner::ConstraintAdjustment,
}

impl Default for Placement {
    fn default() -> Self {
        Placement {
            size: (100, 80),
            anchor_rect: (10, 10, 1, 1),
            anchor: xdg_positioner::Anchor::TopLeft,
            gravity: xdg_positioner::Gravity::BottomRight,
            constraints: xdg_positioner::ConstraintAdjustment::empty(),
        }
    }
}

/// A layer surface's settings.
#[derive(Debug, Clone)]
pub struct LayerSpec {
    pub layer: zwlr_layer_shell_v1::Layer,
    pub anchor: zwlr_layer_surface_v1::Anchor,
    pub size: (u32, u32),
    pub exclusive_zone: i32,
    pub keyboard: zwlr_layer_surface_v1::KeyboardInteractivity,
    /// The output's name (else pila picks).
    pub output: Option<String>,
    pub margin: (i32, i32, i32, i32),
}

impl Default for LayerSpec {
    fn default() -> Self {
        LayerSpec {
            layer: zwlr_layer_shell_v1::Layer::Top,
            anchor: zwlr_layer_surface_v1::Anchor::empty(),
            size: (200, 100),
            exclusive_zone: 0,
            keyboard: zwlr_layer_surface_v1::KeyboardInteractivity::None,
            output: None,
            margin: (0, 0, 0, 0),
        }
    }
}

impl Client {
    /// Bind a global by its interface name (at most `version`); its events
    /// are logged with the interface name as tag.
    pub fn bind<I>(&self, interface: &str, version: u32) -> I
    where
        I: Proxy + 'static,
        Handler: Dispatch<I, String>,
    {
        self.try_bind(interface, version)
            .unwrap_or_else(|| panic!("pila offers no {interface}"))
    }

    pub fn try_bind<I>(&self, interface: &str, version: u32) -> Option<I>
    where
        I: Proxy + 'static,
        Handler: Dispatch<I, String>,
    {
        let s = self.lock();
        let (name, _, offered) = s.globals.iter().find(|(_, i, _)| i == interface)?.clone();
        let registry = s.registry.clone().expect("a registry");
        drop(s);
        let proxy = registry.bind::<I, String, Handler>(
            name,
            version.min(offered),
            &self.qh,
            interface.to_string(),
        );
        self.flush();
        Some(proxy)
    }

    /// The events logged with `tag`.
    pub fn logged(&self, tag: &str) -> Vec<String> {
        self.lock().logged(tag)
    }

    /// Wait for an event with `tag` containing `text`.
    #[track_caller]
    pub fn wait_logged(&self, tag: &str, text: &str) {
        self.wait(&format!("a {tag} event with {text:?}"), |s| {
            s.has_logged(tag, text)
        });
    }

    pub fn has_logged(&self, tag: &str, text: &str) -> bool {
        self.lock().has_logged(tag, text)
    }

    /// The objects with `tag` that got an event containing `text`, in the
    /// order of their first such event.
    pub fn objects<I: Proxy>(&self, tag: &str, text: &str) -> Vec<I> {
        let mut ids: Vec<wayland_client::backend::ObjectId> = Vec::new();
        for l in self.lock().log.iter() {
            if l.tag == tag && l.event.contains(text) && !ids.contains(&l.object) {
                ids.push(l.object.clone());
            }
        }
        ids.into_iter()
            .filter_map(|id| I::from_id(&self.conn, id).ok())
            .collect()
    }

    /// The one object with `tag` that got an event containing `text`.
    #[track_caller]
    pub fn object<I: Proxy>(&self, tag: &str, text: &str) -> I {
        let mut found = self.objects::<I>(tag, text);
        assert_eq!(
            found.len(),
            1,
            "objects {tag} with {text:?}: {:?}",
            self.logged(tag)
        );
        found.remove(0)
    }

    /// The events of one object, in order.
    pub fn events_of(&self, object: &impl Proxy) -> Vec<String> {
        let id = object.id();
        self.lock()
            .log
            .iter()
            .filter(|l| l.object == id)
            .map(|l| l.event.clone())
            .collect()
    }

    /// An shm buffer of `size` (XRGB8888 unless `format` says otherwise)
    /// whose pixels can be read back (`read_pixels`): for captures.
    pub fn capture_buffer(
        &self,
        size: (i32, i32),
        format: wayland_client::protocol::wl_shm::Format,
    ) -> (wayland_client::protocol::wl_buffer::WlBuffer, std::fs::File) {
        let (w, h) = size;
        let len = (w * h * 4) as usize;
        // SAFETY: a new fd, owned by the File.
        let fd = unsafe { libc::memfd_create(c"capture".as_ptr(), libc::MFD_CLOEXEC) };
        assert!(fd >= 0);
        let file = unsafe { std::fs::File::from_raw_fd(fd) };
        file.set_len(len as u64).unwrap();
        let shm = self.lock().shm.clone().expect("wl_shm");
        let pool = shm.create_pool(file.as_fd(), len as i32, &self.qh, ());
        let buffer = pool.create_buffer(0, w, h, w * 4, format, &self.qh, ());
        pool.destroy();
        self.flush();
        (buffer, file)
    }

    /// The `wl_output` named `name`.
    pub fn output(&self, name: &str) -> WlOutput {
        self.lock()
            .outputs
            .iter()
            .find(|(_, n)| n == name)
            .map(|(o, _)| o.clone())
            .unwrap_or_else(|| panic!("no output {name}"))
    }

    fn new_surface(&self, name: &str, color: [u8; 4], size: (i32, i32)) -> (Surf, WlSurface) {
        let mut s = self.lock();
        let surface = s
            .compositor
            .as_ref()
            .expect("wl_compositor")
            .create_surface(&self.qh, ());
        s.surfaces.push(SurfaceData {
            name: name.to_string(),
            surface: surface.clone(),
            color,
            default_size: size,
            size: (0, 0),
            configures: Vec::new(),
            pending: (0, 0, 0, 0),
            auto_draw: true,
            closed: false,
        });
        (Surf(s.surfaces.len() - 1), surface)
    }

    pub fn surface_data<R>(&self, surf: Surf, f: impl FnOnce(&mut SurfaceData) -> R) -> R {
        f(&mut self.lock().surfaces[surf.0])
    }

    pub fn wl_surface_of(&self, surf: Surf) -> WlSurface {
        self.surface_data(surf, |s| s.surface.clone())
    }

    /// Wait for the surface's first configure; its size.
    #[track_caller]
    pub fn wait_surface_configured(&self, surf: Surf) -> (i32, i32, i32, i32) {
        self.wait("a configure", |s| !s.surfaces[surf.0].configures.is_empty());
        self.surface_data(surf, |s| *s.configures.last().unwrap())
    }

    #[track_caller]
    pub fn wait_surface_closed(&self, surf: Surf) {
        self.wait("the surface to close", |s| s.surfaces[surf.0].closed);
    }

    /// A layer surface named `namespace`, drawn in `color` once configured.
    pub fn layer_surface(
        &self,
        namespace: &str,
        spec: &LayerSpec,
        color: [u8; 4],
    ) -> (Surf, ZwlrLayerSurfaceV1) {
        let shell: ZwlrLayerShellV1 = self.bind("zwlr_layer_shell_v1", 5);
        let (surf, surface) =
            self.new_surface(namespace, color, (spec.size.0 as i32, spec.size.1 as i32));
        let output = spec.output.as_deref().map(|name| self.output(name));
        let layer = shell.get_layer_surface(
            &surface,
            output.as_ref(),
            spec.layer,
            namespace.to_string(),
            &self.qh,
            surf.0,
        );
        layer.set_size(spec.size.0, spec.size.1);
        layer.set_anchor(spec.anchor);
        layer.set_exclusive_zone(spec.exclusive_zone);
        layer.set_keyboard_interactivity(spec.keyboard);
        let (top, right, bottom, left) = spec.margin;
        layer.set_margin(top, right, bottom, left);
        surface.commit();
        self.flush();
        self.lock().layer_surfaces.push((surf.0, layer.clone()));
        (surf, layer)
    }

    fn positioner(&self, placement: &Placement) -> XdgPositioner {
        let wm_base = self.lock().wm_base.clone().expect("xdg_wm_base");
        let positioner = wm_base.create_positioner(&self.qh, ());
        positioner.set_size(placement.size.0, placement.size.1);
        let (x, y, w, h) = placement.anchor_rect;
        positioner.set_anchor_rect(x, y, w, h);
        positioner.set_anchor(placement.anchor);
        positioner.set_gravity(placement.gravity);
        positioner.set_constraint_adjustment(placement.constraints);
        positioner
    }

    /// A popup of `parent`'s window (`Ok`) or of a popup or layer surface
    /// (`Err`), optionally grabbing input (with the last input serial).
    pub fn popup(
        &self,
        name: &str,
        parent: Result<Window, Surf>,
        placement: &Placement,
        grab: bool,
        color: [u8; 4],
    ) -> (Surf, XdgPopup) {
        let positioner = self.positioner(placement);
        let (surf, surface) = self.new_surface(name, color, placement.size);
        let wm_base = self.lock().wm_base.clone().expect("xdg_wm_base");
        let xdg_surface = wm_base.get_xdg_surface(&surface, &self.qh, PopupRef(surf.0));
        let popup = match parent {
            Ok(window) => {
                let parent = self.with_window(window, |w| w.xdg_surface.clone());
                xdg_surface.get_popup(Some(&parent), &positioner, &self.qh, PopupRef(surf.0))
            }
            Err(parent) => {
                let parent_popup = self
                    .lock()
                    .popup_xdg_surfaces
                    .iter()
                    .find(|(i, _)| *i == parent.0)
                    .map(|(_, x)| x.clone());
                match parent_popup {
                    Some(parent_xdg) => xdg_surface.get_popup(
                        Some(&parent_xdg),
                        &positioner,
                        &self.qh,
                        PopupRef(surf.0),
                    ),
                    None => {
                        let layer = self
                            .lock()
                            .layer_surfaces
                            .iter()
                            .find(|(i, _)| *i == parent.0)
                            .map(|(_, l)| l.clone())
                            .expect("a popup or layer surface parent");
                        let popup =
                            xdg_surface.get_popup(None, &positioner, &self.qh, PopupRef(surf.0));
                        layer.get_popup(&popup);
                        popup
                    }
                }
            }
        };
        self.lock().popup_xdg_surfaces.push((surf.0, xdg_surface));
        if grab {
            let serial = self.last_serial();
            popup.grab(&self.seat(), serial);
        }
        surface.commit();
        positioner.destroy();
        self.flush();
        (surf, popup)
    }

    /// Lock the session; the lock object (its events are tagged "lock").
    pub fn lock_session(&self) -> ExtSessionLockV1 {
        let manager: ExtSessionLockManagerV1 = self.bind("ext_session_lock_manager_v1", 1);
        let lock = manager.lock(&self.qh, "lock".to_string());
        self.flush();
        lock
    }

    /// A lock surface on the output named `output`.
    pub fn lock_surface(&self, lock: &ExtSessionLockV1, output: &str, color: [u8; 4]) -> Surf {
        let output = self.output(output);
        let (surf, surface) = self.new_surface("lock", color, (0, 0));
        let _lock_surface = lock.get_lock_surface(&surface, &output, &self.qh, surf.0);
        self.flush();
        surf
    }

    /// A data source offering `text` as text/plain, tagged `tag`.
    pub fn data_source(&self, tag: &str, text: &str) -> WlDataSource {
        let manager: WlDataDeviceManager = self.bind("wl_data_device_manager", 3);
        let source = manager.create_data_source(
            &self.qh,
            SourceText {
                tag: tag.to_string(),
                text: text.to_string(),
            },
        );
        source.offer("text/plain".into());
        self.flush();
        source
    }

    /// Read what an offer gives for `mime` (`receive` takes the write end
    /// of a pipe).
    pub fn receive(&self, request: impl FnOnce(std::os::fd::BorrowedFd<'_>)) -> String {
        let mut fds = [0; 2];
        // SAFETY: two new fds, owned below.
        assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
        let read = unsafe { OwnedFd::from_raw_fd(fds[0]) };
        let write = unsafe { OwnedFd::from_raw_fd(fds[1]) };
        request(write.as_fd());
        self.flush();
        drop(write);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut text = String::new();
            let _ = std::fs::File::from(read).read_to_string(&mut text);
            let _ = tx.send(text);
        });
        rx.recv_timeout(super::TIMEOUT)
            .expect("the selection's data")
    }

    /// A dmabuf-backed buffer of `size` on the first render node, or `None`
    /// where pila has none to import it with (no GPU, as in CI).
    pub fn dmabuf_buffer(
        &self,
        size: (i32, i32),
    ) -> Option<wayland_client::protocol::wl_buffer::WlBuffer> {
        use smithay::reexports::gbm::{BufferObjectFlags, Device, Format};
        use wayland_protocols::wp::linux_dmabuf::zv1::client::{
            zwp_linux_buffer_params_v1::{Flags, ZwpLinuxBufferParamsV1},
            zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
        };
        let node = render_node()?;
        let device = Device::new(
            std::fs::File::options()
                .read(true)
                .write(true)
                .open(node)
                .ok()?,
        )
        .ok()?;
        let bo = device
            .create_buffer_object::<()>(
                size.0 as u32,
                size.1 as u32,
                Format::Argb8888,
                BufferObjectFlags::RENDERING | BufferObjectFlags::LINEAR,
            )
            .ok()?;
        let manager: ZwpLinuxDmabufV1 = self.bind("zwp_linux_dmabuf_v1", 3);
        let params: ZwpLinuxBufferParamsV1 =
            manager.create_params(&self.qh, "dmabuf_params".to_string());
        let modifier: u64 = bo.modifier().into();
        params.add(
            bo.fd_for_plane(0).ok()?.as_fd(),
            0,
            bo.offset(0),
            bo.stride(),
            (modifier >> 32) as u32,
            modifier as u32,
        );
        let buffer = params.create_immed(
            size.0,
            size.1,
            Format::Argb8888 as u32,
            Flags::empty(),
            &self.qh,
            (),
        );
        self.flush();
        Some(buffer)
    }

    /// A subsurface of `parent` at (x, y), drawn in `color`.
    pub fn subsurface(
        &self,
        parent: &WlSurface,
        name: &str,
        at: (i32, i32),
        size: (i32, i32),
        color: [u8; 4],
    ) -> (Surf, WlSubsurface) {
        let sub: WlSubcompositor = self.bind("wl_subcompositor", 1);
        let (surf, surface) = self.new_surface(name, color, size);
        let subsurface = sub.get_subsurface(&surface, parent, &self.qh, "wl_subsurface".into());
        subsurface.set_position(at.0, at.1);
        subsurface.set_desync();
        let shm = self.lock().shm.clone().unwrap();
        paint(&shm, &self.qh, &surface, color, size);
        self.flush();
        (surf, subsurface)
    }
}

/// The render node a test pila renders on, unless it was told to use none.
pub fn render_node() -> Option<std::path::PathBuf> {
    if std::env::var("PILA_HEADLESS_DEVICE").as_deref() == Ok("software") {
        return None;
    }
    let mut nodes: Vec<_> = std::fs::read_dir("/dev/dri")
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("renderD"))
        })
        .collect();
    nodes.sort();
    nodes.into_iter().next()
}

/// The pixel at (x, y) of a capture buffer `width` wide, as [r, g, b] (the
/// buffer is XRGB/ARGB8888: B, G, R, A in memory).
pub fn read_pixel(file: &std::fs::File, width: i32, x: i32, y: i32) -> [u8; 3] {
    use std::os::unix::fs::FileExt;
    let mut px = [0u8; 4];
    file.read_exact_at(&mut px, ((y * width + x) * 4) as u64)
        .unwrap();
    [px[2], px[1], px[0]]
}

// --- xdg-session-management -----------------------------------------------------

logged!(XdgSessionManagerV1, XdgSessionV1);
logged!(
    wayland_protocols::wp::commit_timing::v1::client::wp_commit_timing_manager_v1::WpCommitTimingManagerV1,
    wayland_protocols::wp::commit_timing::v1::client::wp_commit_timer_v1::WpCommitTimerV1,
);
logged!(
    wayland_protocols::wp::linux_drm_syncobj::v1::client::wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1,
    wayland_protocols::wp::linux_drm_syncobj::v1::client::wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1,
    wayland_protocols::wp::linux_drm_syncobj::v1::client::wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1,
);
logged!(
    wayland_protocols::wp::fifo::v1::client::wp_fifo_manager_v1::WpFifoManagerV1,
    wayland_protocols::wp::fifo::v1::client::wp_fifo_v1::WpFifoV1,
);
logged!(
    wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
    wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
);
logged!(
    wayland_protocols::xdg::dialog::v1::client::xdg_wm_dialog_v1::XdgWmDialogV1,
    wayland_protocols::xdg::dialog::v1::client::xdg_dialog_v1::XdgDialogV1,
    wayland_protocols::xdg::toplevel_tag::v1::client::xdg_toplevel_tag_manager_v1::XdgToplevelTagManagerV1,
);

/// Logged with how many configures the client's windows got by then.
impl Dispatch<XdgToplevelSessionV1, String> for Handler {
    fn event(
        state: &mut Self,
        proxy: &XdgToplevelSessionV1,
        event: <XdgToplevelSessionV1 as Proxy>::Event,
        tag: &String,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let configures: usize = state
            .lock()
            .windows
            .iter()
            .map(|w| w.configures.len())
            .sum();
        log(
            state,
            tag,
            proxy.id(),
            format!("{event:?} configures={configures}"),
        );
    }
}

impl Client {
    /// A session (`id`: an existing one), its events logged with `tag`.
    pub fn session(&self, id: Option<&str>, tag: &str) -> XdgSessionV1 {
        let manager: XdgSessionManagerV1 = self.bind("xdg_session_manager_v1", 1);
        let session = manager.get_session(
            Reason::Launch,
            id.map(str::to_string),
            &self.qh,
            tag.to_string(),
        );
        self.flush();
        session
    }

    /// The id of the session logged with `tag`, once created.
    pub fn session_id(&self, tag: &str) -> String {
        self.wait_logged(tag, "Created");
        let event = self
            .logged(tag)
            .into_iter()
            .find(|e| e.contains("Created"))
            .unwrap();
        event.split('"').nth(1).expect("a quoted id").to_string()
    }
}
