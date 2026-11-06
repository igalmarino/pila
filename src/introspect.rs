//! pila's state as JSON, for automated tests: what the headless backend's
//! control socket answers to `state` (`test_control.rs`). Read-only; built
//! on demand, never per frame.

use serde_json::{json, Value};
use smithay::{
    desktop::{
        layer_map_for_output, utils::bbox_from_surface_tree, PopupManager, PopupPointerGrab, Window,
    },
    input::pointer::CursorImageStatus,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Rectangle},
    wayland::{
        seat::WaylandFocus,
        shell::wlr_layer::{ExclusiveZone, KeyboardInteractivity, Layer},
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
            "snapped": snap::snapped_side(window).map(|s| match s {
                snap::Side::Left => "left",
                snap::Side::Right => "right",
            }),
            "ssd": decoration::is_ssd(window),
            "title_bar": decoration::has_title_bar(window),
            "focused": focused,
            "output": self.window_output(window).map(|o| o.name()),
            "opacity": props.opacity,
            "corner_radius": props.corner_radius,
            "rule_title_bar": props.title_bar,
            "animating": self.window_effect(window, now).is_some(),
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
                Some(foreign_toplevel::title_and_app_id(&window).1)
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
            return Some(foreign_toplevel::title_and_app_id(&window).1);
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
            let name = foreign_toplevel::title_and_app_id(window).1;
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
            .map(|w| foreign_toplevel::title_and_app_id(w).1)
            .collect::<Vec<_>>();

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
                        "enabled": o.enabled,
                        "power": o.power,
                        "geometry": self.space.output_geometry(output).map(rect),
                        "mode": mode.map(|m| json!({ "w": m.size.w, "h": m.size.h, "refresh_mhz": m.refresh })),
                        "scale": output.current_scale().fractional_scale(),
                        "transform": format!("{:?}", output.current_transform()),
                        "usable": o.enabled.then(|| {
                            let map = layer_map_for_output(output);
                            let zone = map.non_exclusive_zone();
                            let loc = self.space.output_geometry(output).unwrap_or_default().loc;
                            rect(Rectangle::new(zone.loc + loc, zone.size))
                        }),
                        "gamma_set": o.gamma.is_some(),
                        "frames": o.frames,
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
            .map(|w| foreign_toplevel::title_and_app_id(&w).1);
        let cursor = match &self.cursor_status {
            CursorImageStatus::Hidden => "hidden".to_string(),
            CursorImageStatus::Named(icon) => icon.name().to_string(),
            CursorImageStatus::Surface(_) => "surface".to_string(),
        };
        let focused = self
            .focused_window()
            .map(|w| foreign_toplevel::title_and_app_id(&w).1);
        let focus_history = self
            .windows
            .focus_history
            .iter()
            .map(|w| foreign_toplevel::title_and_app_id(w).1)
            .collect::<Vec<_>>();
        let switch = self.windows.switch.as_ref().map(|s| {
            let (order, index) = s.order();
            json!({
                "order": order.iter().map(|w| foreign_toplevel::title_and_app_id(w).1).collect::<Vec<_>>(),
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
            "workspace": self.workspaces.active + 1,
            "workspaces": self.config.workspaces,
            "outputs": outputs,
            "layers": layers,
            "pointer": {
                "x": pointer_loc.x,
                "y": pointer_loc.y,
                "focus": pointer_focus,
                "grabbed": pointer.as_ref().is_some_and(|p| p.is_grabbed()),
                "grab": pointer.as_ref().and_then(|p| {
                    p.with_grab(|_, grab| {
                        if grab.downcast_ref::<crate::grabs::MoveSurfaceGrab>().is_some() {
                            "move"
                        } else if grab.downcast_ref::<crate::grabs::ResizeSurfaceGrab>().is_some() {
                            "resize"
                        } else if grab.downcast_ref::<PopupPointerGrab<Pila>>().is_some() {
                            "popup"
                        } else {
                            "click"
                        }
                    })
                }),
                "cursor": cursor,
                "constrained": self.input.constrained.is_some(),
            },
            "keyboard_grabbed": self.seat.get_keyboard().is_some_and(|k| k.is_grabbed()),
            "locked": self.is_locked(),
            "lock_pending": self.lock_pending(),
            "snap_preview": self.snap_preview.map(rect),
            "animations": {
                "running": self.animating(),
                "slide": self.workspace_slide.is_some(),
                "appearing": appearing,
                "ghosts": ghosts,
                "layers": layers_appearing,
                "layer_ghosts": layer_ghosts,
                "resizing": resizing,
            },
            "redraw_pending": headless.is_some_and(|d| crate::headless::redraw_pending(d)),
            "blur": {
                "allocated": self.blur.stats().0,
                "blurred": self.blur.stats().1,
                "entries": self.blur.stats().2,
            },
            "xwayland": self.xwayland.display,
            "children": self.children.len(),
        })
    }
}
