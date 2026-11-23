//! pila's state as JSON, for automated tests: what the headless backend's
//! control socket answers to `state` (`test_control.rs`). Read-only; built
//! on demand, never per frame.

use serde_json::{json, Value};
use smithay::{
    desktop::{
        layer_map_for_output, utils::bbox_from_surface_tree, PopupKeyboardGrab, PopupManager,
        Window,
    },
    input::pointer::CursorImageStatus,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Rectangle},
    wayland::{
        input_method::{InputMethodKeyboardGrab, InputMethodSeat},
        seat::WaylandFocus,
        shell::wlr_layer::{ExclusiveZone, KeyboardInteractivity, Layer},
        text_input::TextInputSeat,
    },
};

use crate::{backend::Backend, decoration, foreign_toplevel, rules, snap, state::fill_state, Pila};

fn rect(r: Rectangle<i32, Logical>) -> Value {
    json!({ "x": r.loc.x, "y": r.loc.y, "w": r.size.w, "h": r.size.h })
}

fn layer_name(layer: Layer) -> &'static str {
    match layer {
        Layer::Background => "background",
        Layer::Bottom => "bottom",
        Layer::Top => "top",
        Layer::Overlay => "overlay",
    }
}

impl Pila {
    fn window_json(
        &self,
        window: &Window,
        stack: Option<usize>,
        now: std::time::Duration,
    ) -> Value {
        let (title, app_id) = foreign_toplevel::title_and_app_id(window);
        let in_space = self.space.elements().any(|w| w == window);
        let location = if in_space {
            self.space.element_location(window).unwrap_or_default()
        } else {
            crate::workspaces::space_location(window)
        };
        let geometry = Rectangle::new(location, window.geometry().size);
        let (maximized, fullscreen) = fill_state(window);
        let focused = window
            .wl_surface()
            .is_some_and(|s| self.keyboard_focus().as_ref() == Some(&*s));
        let props = rules::window_props(window);
        json!({
            "id": foreign_toplevel::identifier(window),
            "app_id": app_id,
            "title": title,
            "tag": rules::window_tag(window),
            "x11": window.x11_surface().is_some(),
            "override_redirect": crate::workspaces::is_override_redirect(window),
            "mapped": in_space,
            "stack": stack,
            "geometry": rect(geometry),
            "workspace": self.workspace_of(window) + 1,
            "minimized": self.is_minimized(window),
            "maximized": maximized,
            "fullscreen": fullscreen,
            "snapped": snap::snapped_side(window).map(snap::Side::name),
            "always_on_top": crate::stacking::is_always_on_top(window),
            "all_workspaces": crate::stacking::is_on_all_workspaces(window),
            "urgent": crate::urgency::is_urgent(window),
            "ssd": decoration::is_ssd(window),
            "title_bar": decoration::has_title_bar(window),
            "focused": focused,
            "output": self.window_output(window).map(|o| o.name()),
            "opacity": props.opacity,
            "corner_radius": props.corner_radius,
            "rule_title_bar": props.title_bar,
            "animating": self.window_effect(window, now).is_some(),
            // Its textures while it animates, one per scale it's drawn at.
            "animation_textures": window
                .user_data()
                .get::<crate::animation::AnimTextures>()
                .map_or(0, |textures| textures.0.borrow().len()),
            "ghost": self.is_ghost(window),
            "flashing": crate::bell::flash(window, location).is_some(),
        })
    }

    /// What has keyboard focus: a window's app id, `popup:<app id or
    /// namespace>`, `layer:<namespace>`, `lock`, or `None`.
    fn keyboard_focus_name(&self) -> Option<String> {
        let focus = self.keyboard_focus()?;
        let name_of_root = |root: &WlSurface| {
            if let Some(window) = self.window_for_surface(root) {
                Some(foreign_toplevel::app_id(&window))
            } else {
                self.layer_for_surface(root)
                    .map(|(_, layer)| layer.namespace().to_string())
            }
        };
        if let Some(root) = self.popup_root(&focus) {
            return Some(format!("popup:{}", name_of_root(&root)?));
        }
        if self.space.outputs().any(|o| {
            self.lock_surface(o)
                .is_some_and(|s| s.wl_surface() == &focus)
        }) {
            return Some("lock".into());
        }
        if let Some(window) = self.window_for_surface(&focus) {
            return Some(foreign_toplevel::app_id(&window));
        }
        self.layer_for_surface(&focus)
            .map(|(_, layer)| format!("layer:{}", layer.namespace()))
    }

    /// Popups open on mapped windows and layer surfaces: their owner's name
    /// and global geometry.
    fn popups_json(&self) -> Vec<Value> {
        let mut roots: Vec<(String, WlSurface, Point<i32, Logical>)> = Vec::new();
        for window in self.space.elements() {
            let (Some(surface), Some(loc)) =
                (window.wl_surface(), self.space.element_location(window))
            else {
                continue;
            };
            let name = foreign_toplevel::app_id(window);
            roots.push((name, surface.into_owned(), loc - window.geometry().loc));
        }
        for output in self.space.outputs() {
            let map = layer_map_for_output(output);
            let origin = self.space.output_geometry(output).unwrap_or_default().loc;
            for layer in map.layers() {
                if let Some(geo) = map.layer_geometry(layer) {
                    roots.push((
                        layer.namespace().to_string(),
                        layer.wl_surface().clone(),
                        origin + geo.loc,
                    ));
                }
            }
        }
        let mut popups = Vec::new();
        for (owner, root, origin) in roots {
            for (popup, offset) in PopupManager::popups_for_surface(&root) {
                // Without a window geometry, the surface tree's extent.
                let mut geo = popup.geometry();
                if geo.is_empty() {
                    geo = bbox_from_surface_tree(popup.wl_surface(), (0, 0));
                }
                popups.push(json!({
                    "owner": owner,
                    "geometry": rect(Rectangle::new(origin + offset, geo.size)),
                }));
            }
        }
        popups
    }

    /// Everything a test may want to check, as one JSON object.
    pub fn state_json(&self) -> Value {
        let now = self.animation_time();
        let mut windows: Vec<Value> = self
            .space
            .elements()
            .enumerate()
            .map(|(i, w)| self.window_json(w, Some(i), now))
            .collect();
        windows.extend(
            self.off_space_windows()
                .map(|w| self.window_json(w, None, now)),
        );
        let unmapped = self
            .windows
            .unmapped
            .iter()
            .map(foreign_toplevel::app_id)
            .collect::<Vec<_>>();

        // Whether each output's frame may tear (`tearing.rs`), as a backend
        // that can tear decides it.
        let scene = matches!(&self.backend, Some(Backend::Headless(_))).then(|| self.scene());
        let tears = |output: &smithay::output::Output| {
            let geometry = self.space.output_geometry(output);
            scene
                .as_ref()
                .zip(geometry)
                .is_some_and(|(scene, geometry)| scene.tears(geometry))
        };
        let outputs: Vec<Value> = match &self.backend {
            Some(Backend::Headless(data)) => data
                .outputs
                .iter()
                .map(|o| {
                    let output = &o.output;
                    let mode = output.current_mode();
                    json!({
                        "name": output.name(),
                        "make": output.physical_properties().make,
                        "model": output.physical_properties().model,
                        "enabled": o.enabled(),
                        "power": o.power,
                        "geometry": self.space.output_geometry(output).map(rect),
                        "mode": mode.map(|m| json!({ "w": m.size.w, "h": m.size.h, "refresh_mhz": m.refresh })),
                        "scale": output.current_scale().fractional_scale(),
                        "transform": format!("{:?}", output.current_transform()),
                        "usable": o.enabled().then(|| {
                            let map = layer_map_for_output(output);
                            let zone = map.non_exclusive_zone();
                            let loc = self.space.output_geometry(output).unwrap_or_default().loc;
                            rect(Rectangle::new(zone.loc + loc, zone.size))
                        }),
                        "gamma_set": o.gamma.is_some(),
                        "frames": o.frames,
                        "idle": o.idle,
                        "tears": tears(output),
                    })
                })
                .collect(),
            _ => self
                .space
                .outputs()
                .map(|o| json!({ "name": o.name(), "geometry": self.space.output_geometry(o).map(rect) }))
                .collect(),
        };

        let mut layers = Vec::new();
        for output in self.space.outputs() {
            let map = layer_map_for_output(output);
            let loc = self.space.output_geometry(output).unwrap_or_default().loc;
            for layer in map.layers() {
                let cached = layer.cached_state();
                let geometry = map
                    .layer_geometry(layer)
                    .map(|g| rect(Rectangle::new(g.loc + loc, g.size)));
                layers.push(json!({
                    "namespace": layer.namespace(),
                    "output": output.name(),
                    "layer": layer_name(layer.layer()),
                    "geometry": geometry,
                    "exclusive_zone": match cached.exclusive_zone {
                        ExclusiveZone::Exclusive(n) => n as i64,
                        ExclusiveZone::Neutral => 0,
                        ExclusiveZone::DontCare => -1,
                    },
                    "keyboard": match cached.keyboard_interactivity {
                        KeyboardInteractivity::None => "none",
                        KeyboardInteractivity::Exclusive => "exclusive",
                        KeyboardInteractivity::OnDemand => "on_demand",
                    },
                    "focused": self.keyboard_focus().as_ref() == Some(layer.wl_surface()),
                }));
            }
        }

        let pointer = self.seat.get_pointer();
        let pointer_loc = pointer
            .as_ref()
            .map(|p| p.current_location())
            .unwrap_or_default();
        let pointer_focus = self
            .surface_under(pointer_loc)
            .and_then(|(surface, _)| self.window_for_surface(&surface))
            .map(|w| foreign_toplevel::app_id(&w));
        let cursor = match &self.cursor_status {
            CursorImageStatus::Hidden => "hidden".to_string(),
            CursorImageStatus::Named(icon) => icon.name().to_string(),
            CursorImageStatus::Surface(_) => "surface".to_string(),
        };
        let focused = self.focused_window().map(|w| foreign_toplevel::app_id(&w));
        let focus_history = self
            .windows
            .focus_history
            .iter()
            .map(foreign_toplevel::app_id)
            .collect::<Vec<_>>();
        let switch = self.windows.switch.as_ref().map(|s| {
            let (order, index) = s.order();
            json!({
                "order": order.iter().map(foreign_toplevel::app_id).collect::<Vec<_>>(),
                "index": index,
            })
        });
        let [appearing, ghosts, layers_appearing, layer_ghosts, resizing] =
            self.animations.counts();
        let headless = match &self.backend {
            Some(Backend::Headless(data)) => Some(data),
            _ => None,
        };

        json!({
            "windows": windows,
            "unmapped": unmapped,
            "focused": focused,
            "keyboard_focus": self.keyboard_focus_name(),
            "popups": self.popups_json(),
            "keyboard_focus_layer": self
                .keyboard_focus()
                .and_then(|s| self.layer_for_surface(&s))
                .map(|(_, l)| l.namespace().to_string()),
            "focus_history": focus_history,
            "switch": switch,
            "switcher_view": self.switcher_view_json(),
            "shortcuts_view": self.shortcuts_view_json(),
            "share_picker": self.share_picker_json(),
            "color_pick": self.color_pick_json(),
            "app_shortcuts": self.app_shortcuts_json(),
            "screencast": {
                "casts": self.screencast.cast_count(),
                "pipewire": self.screencast.connected(),
            },
            "overview": self.overview_json(),
            "workspace": self.workspaces.active + 1,
            "workspaces": self.config.workspaces,
            "outputs": outputs,
            "layers": layers,
            "pointer": {
                "x": pointer_loc.x,
                "y": pointer_loc.y,
                "focus": pointer_focus,
                "magnification": self.cursor_magnification().unwrap_or(1.0),
                "grabbed": pointer.as_ref().is_some_and(|p| p.is_grabbed()),
                "grab": pointer.as_ref().and_then(|p| {
                    p.with_grab(|_, grab| {
                        if grab.downcast_ref::<crate::grabs::MoveSurfaceGrab>().is_some() {
                            "move"
                        } else if grab.downcast_ref::<crate::grabs::ResizeSurfaceGrab>().is_some() {
                            "resize"
                        } else if grab.downcast_ref::<crate::grabs::MenuPointerGrab>().is_some() {
                            "popup"
                        } else {
                            "click"
                        }
                    })
                }),
                "cursor": cursor,
                "cursor_theme": self.cursor_theme.name(),
                "cursor_size": self.cursor_theme.size(),
                "constrained": self.input.constrained.is_some(),
            },
            "keyboard_grabbed": self.seat.get_keyboard().is_some_and(|k| k.is_grabbed()),
            "keyboard_grab": self.seat.get_keyboard().and_then(|k| {
                k.with_grab(|_, grab| {
                    if grab.downcast_ref::<PopupKeyboardGrab<Pila>>().is_some() {
                        "popup"
                    } else if grab.downcast_ref::<InputMethodKeyboardGrab>().is_some() {
                        "input_method"
                    } else {
                        "other"
                    }
                })
            }),
            "input_method": {
                "present": self.seat.input_method().has_instance(),
                "grab": self.seat.input_method().keyboard_grabbed(),
                "text_input_focus": self.seat.text_input().focus().map(|surface| {
                    self.window_for_surface(&surface)
                        .map(|w| crate::foreign_toplevel::app_id(&w))
                        .or_else(|| {
                            self.layer_for_surface(&surface)
                                .map(|(_, l)| l.namespace().to_string())
                        })
                        .unwrap_or_else(|| "other".to_string())
                }),
                "popups": self.input_method_popups(),
            },
            "window_menu": self.window_menu.as_ref().map(|menu| json!({
                "window": crate::foreign_toplevel::app_id(&menu.window),
                "geometry": rect(menu.rect),
                "items": menu.labels(),
                "checked": menu.checked(),
                "hover": menu.hover,
            })),
            "locked": self.is_locked(),
            // Whose keys Pila's bindings leave alone: the focus's, if it
            // inhibits them (keyboard-shortcuts-inhibit, an X11 keyboard grab).
            "shortcuts_inhibited": self.shortcuts_inhibited(),
            "lock_pending": self.lock_pending(),
            "snap_preview": self.snap_preview.map(rect),
            "animations": {
                "running": self.animating() || self.animations_held(),
                "slide": self.workspace_slide.is_some(),
                "slide_textures": self.slide_pool.count(),
                "appearing": appearing,
                "ghosts": ghosts,
                "layers": layers_appearing,
                "layer_ghosts": layer_ghosts,
                "resizing": resizing,
            },
            // Commits whose shm buffers were uploaded at once (`upload.rs`).
            "early_uploads": self.early_uploads,
            // A redraw asked for from `after_frames` is queued by an idle
            // callback (`defer_redraws`): pending until it runs.
            "redraw_pending": self.deferred_redraw_idles > 0
                || headless.is_some_and(|d| crate::headless::redraw_pending(d)),
            "blur": {
                "allocated": self.blur.stats().0,
                "blurred": self.blur.stats().1,
                "entries": self.blur.stats().2,
            },
            "capture": {
                "textures": self.capture.texture_count(),
                "dmabuf": self.capture.dmabuf.as_ref().is_some_and(|t| t.offers()),
            },
            // When a window or layer surface last appeared or a window drew
            // at a new size, in monotonic microseconds (`Pila::start_settling`).
            "settling_us": self.settling_since.map(|at| at.as_micros() as u64),
            "event_loop": {
                "iterations": self.loop_stats.total.0,
                "housekeeping": self.loop_stats.total.1,
            },
            "xwayland": self.xwayland.display(),
            "xauthority": self.xwayland.authority.as_ref().map(|a| a.path()),
            "xwayland_running": self.xwayland.running(),
            "children": self.children.len(),
        })
    }
}
