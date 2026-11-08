//! Window rules and layer rules: `[[window-rule]]` and `[[layer-rule]]` in
//! the config. A rule matches windows by app id (xdg `app_id`, or the class
//! of an X11 window's WM_CLASS), title and tag (xdg-toplevel-tag; an X11
//! window has none, like a Wayland one that sets none: its tag is empty), or
//! layer surfaces by namespace,
//! with `*` standing for any run of characters; every matcher a rule gives
//! must match, and a rule without matchers matches everything. Matching
//! rules apply in order, the last one winning field by field.
//!
//! What they set is resolved when a window or layer surface appears and
//! kept in its user data (`WindowRuleProps`, `LayerRuleProps`), so nothing
//! is matched per frame; a config reload resolves it again for everything
//! already there (`Pila::refresh_rule_props`), and a window's title or app
//! id changing does for that window (`rules_maybe_changed`). What applies only
//! when a window opens (size, position, workspace, fill, on top) stays.

use std::cell::RefCell;

use serde::Deserialize;
use smithay::{
    desktop::{LayerSurface, Window},
    wayland::{compositor::with_states, xdg_toplevel_tag::XdgToplevelTagSurfaceData},
};
use tracing::debug;

use crate::{config::AnimationKind, Pila};

/// Whether `text` matches `pattern`, where `*` stands for any run of
/// characters (possibly none) and everything else for itself.
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = text.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else {
        // No `*`: the whole text.
        return rest.is_empty();
    };
    for part in middle {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

/// A rule's matcher: `None` matches anything.
fn matches(pattern: &Option<String>, text: &str) -> bool {
    pattern.as_ref().is_none_or(|p| wildcard_match(p, text))
}

/// `animations = false`, or per animation: `{ open = false, ... }`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
enum AnimationsFile {
    All(bool),
    Each(EachAnimationFile),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct EachAnimationFile {
    open: Option<bool>,
    close: Option<bool>,
    minimize: Option<bool>,
    maximize: Option<bool>,
}

/// `[[window-rule]]` as written.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct WindowRuleFile {
    app_id: Option<String>,
    title: Option<String>,
    tag: Option<String>,
    animations: Option<AnimationsFile>,
    open_on_workspace: Option<usize>,
    open_maximized: Option<bool>,
    open_fullscreen: Option<bool>,
    size: Option<[i32; 2]>,
    position: Option<[i32; 2]>,
    title_bar: Option<bool>,
    corner_radius: Option<u32>,
    opacity: Option<f64>,
    blur: Option<bool>,
    shadow: Option<bool>,
    always_on_top: Option<bool>,
    all_workspaces: Option<bool>,
}

/// The range of `opacity`: never fully transparent.
const OPACITIES: std::ops::RangeInclusive<f64> = 0.1..=1.0;

/// The largest `size` a rule may give (logical pixels).
const MAX_RULE_SIZE: i32 = 16384;

/// `[[layer-rule]]` as written.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LayerRuleFile {
    namespace: Option<String>,
    animations: Option<bool>,
    blur: Option<bool>,
}

/// A window's animations as rules set them (`None`: not set, they play).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowAnimations {
    pub open: Option<bool>,
    pub close: Option<bool>,
    pub minimize: Option<bool>,
    pub maximize: Option<bool>,
}

impl WindowAnimations {
    fn merge(&mut self, other: &WindowAnimations) {
        self.open = other.open.or(self.open);
        self.close = other.close.or(self.close);
        self.minimize = other.minimize.or(self.minimize);
        self.maximize = other.maximize.or(self.maximize);
    }
}

/// What the matching window rules set, merged (`None`: not set).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WindowProps {
    pub animations: WindowAnimations,
    /// The workspace it opens on (from 0).
    pub open_on_workspace: Option<usize>,
    pub open_maximized: Option<bool>,
    pub open_fullscreen: Option<bool>,
    /// Its size when it opens (logical; the client may adjust it).
    pub size: Option<(i32, i32)>,
    /// Where its top-left (title bar included) goes when it opens,
    /// relative to the usable area of the output it opens on.
    pub position: Option<(i32, i32)>,
    /// `false`: pila draws no title bar (the client doesn't either).
    pub title_bar: Option<bool>,
    /// Instead of `corner-radius`.
    pub corner_radius: Option<u32>,
    /// Of everything it draws, title bar included.
    pub opacity: Option<f64>,
    /// `true`: blur behind all of it; `false`: never; unset: where the
    /// client asks (ext-background-effect).
    pub blur: Option<bool>,
    /// `true`: pila draws one, even if the client draws its own; `false`:
    /// never; unset: `[shadow]`, for a window that doesn't draw its own.
    pub shadow: Option<bool>,
    /// When it opens: above the others, or on all workspaces (`stacking.rs`).
    pub always_on_top: Option<bool>,
    pub all_workspaces: Option<bool>,
}

impl WindowProps {
    fn merge(&mut self, other: &WindowProps) {
        self.animations.merge(&other.animations);
        self.open_on_workspace = other.open_on_workspace.or(self.open_on_workspace);
        self.open_maximized = other.open_maximized.or(self.open_maximized);
        self.open_fullscreen = other.open_fullscreen.or(self.open_fullscreen);
        self.size = other.size.or(self.size);
        self.position = other.position.or(self.position);
        self.title_bar = other.title_bar.or(self.title_bar);
        self.corner_radius = other.corner_radius.or(self.corner_radius);
        self.opacity = other.opacity.or(self.opacity);
        self.blur = other.blur.or(self.blur);
        self.shadow = other.shadow.or(self.shadow);
        self.always_on_top = other.always_on_top.or(self.always_on_top);
        self.all_workspaces = other.all_workspaces.or(self.all_workspaces);
    }

    /// Whether `kind` plays for this window.
    pub fn animates(&self, kind: AnimationKind) -> bool {
        let a = &self.animations;
        match kind {
            AnimationKind::WindowOpen => a.open,
            AnimationKind::WindowClose => a.close,
            AnimationKind::Minimize => a.minimize,
            AnimationKind::Maximize => a.maximize,
            AnimationKind::Layer | AnimationKind::Workspace => None,
        }
        .unwrap_or(true)
    }
}

/// What the matching layer rules set, merged (`None`: not set).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LayerProps {
    /// `false`: it neither fades in nor out.
    pub animations: Option<bool>,
    /// `true`: blur behind all of it; `false`: never; unset: where the
    /// client asks (ext-background-effect).
    pub blur: Option<bool>,
}

impl LayerProps {
    fn merge(&mut self, other: &LayerProps) {
        self.animations = other.animations.or(self.animations);
        self.blur = other.blur.or(self.blur);
    }
}

/// A validated `[[window-rule]]`.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowRule {
    app_id: Option<String>,
    title: Option<String>,
    tag: Option<String>,
    props: WindowProps,
}

/// A validated `[[layer-rule]]`.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerRule {
    namespace: Option<String>,
    props: LayerProps,
}

impl WindowRuleFile {
    /// The rule, or what's wrong with it (`index` counts from 1, for the
    /// message; `workspaces`: how many there are).
    pub fn validate(self, index: usize, workspaces: usize) -> Result<WindowRule, String> {
        let open_on_workspace = match self.open_on_workspace {
            None => None,
            Some(n @ 1..) if n <= workspaces => Some(n - 1),
            Some(n) => {
                return Err(format!(
                    "window-rule {index}: open-on-workspace = {n}: workspaces are 1 to {workspaces}"
                ))
            }
        };
        if let Some(opacity) = self.opacity {
            if !OPACITIES.contains(&opacity) {
                return Err(format!(
                    "window-rule {index}: opacity = {opacity}: must be {} to {}",
                    OPACITIES.start(),
                    OPACITIES.end()
                ));
            }
        }
        if let Some(radius) = self.corner_radius {
            if radius > crate::config::MAX_CORNER_RADIUS {
                return Err(format!(
                    "window-rule {index}: corner-radius = {radius}: must be 0 to {}",
                    crate::config::MAX_CORNER_RADIUS
                ));
            }
        }
        if let Some([w, h]) = self.size {
            if !(1..=MAX_RULE_SIZE).contains(&w) || !(1..=MAX_RULE_SIZE).contains(&h) {
                return Err(format!(
                    "window-rule {index}: size = [{w}, {h}]: each must be 1 to {MAX_RULE_SIZE}"
                ));
            }
        }
        let animations = match self.animations {
            None => WindowAnimations::default(),
            Some(AnimationsFile::All(on)) => WindowAnimations {
                open: Some(on),
                close: Some(on),
                minimize: Some(on),
                maximize: Some(on),
            },
            Some(AnimationsFile::Each(each)) => WindowAnimations {
                open: each.open,
                close: each.close,
                minimize: each.minimize,
                maximize: each.maximize,
            },
        };
        Ok(WindowRule {
            app_id: self.app_id,
            title: self.title,
            tag: self.tag,
            props: WindowProps {
                animations,
                open_on_workspace,
                open_maximized: self.open_maximized,
                open_fullscreen: self.open_fullscreen,
                size: self.size.map(|[w, h]| (w, h)),
                position: self.position.map(|[x, y]| (x, y)),
                title_bar: self.title_bar,
                corner_radius: self.corner_radius,
                opacity: self.opacity,
                blur: self.blur,
                shadow: self.shadow,
                always_on_top: self.always_on_top,
                all_workspaces: self.all_workspaces,
            },
        })
    }
}

impl LayerRuleFile {
    pub fn validate(self) -> LayerRule {
        LayerRule {
            namespace: self.namespace,
            props: LayerProps {
                animations: self.animations,
                blur: self.blur,
            },
        }
    }
}

/// What `rules` set for a window with this app id, title and tag.
pub fn resolve_window(rules: &[WindowRule], app_id: &str, title: &str, tag: &str) -> WindowProps {
    let mut props = WindowProps::default();
    for rule in rules {
        if matches(&rule.app_id, app_id) && matches(&rule.title, title) && matches(&rule.tag, tag) {
            props.merge(&rule.props);
        }
    }
    props
}

/// What `rules` set for a layer surface with this namespace.
pub fn resolve_layer(rules: &[LayerRule], namespace: &str) -> LayerProps {
    let mut props = LayerProps::default();
    for rule in rules {
        if matches(&rule.namespace, namespace) {
            props.merge(&rule.props);
        }
    }
    props
}

/// A window's resolved rules, in its user data.
#[derive(Default)]
struct WindowRuleProps(RefCell<WindowProps>);

/// A layer surface's resolved rules, in its user data.
#[derive(Default)]
struct LayerRuleProps(RefCell<LayerProps>);

/// What the rules set for `window`, as last resolved (nothing before).
pub fn window_props(window: &Window) -> WindowProps {
    with_window_props(window, WindowProps::clone)
}

/// `f` of what the rules set for `window`, without copying it (for what's
/// asked often, e.g. per frame or pointer motion).
pub fn with_window_props<R>(window: &Window, f: impl FnOnce(&WindowProps) -> R) -> R {
    match window.user_data().get::<WindowRuleProps>() {
        Some(props) => f(&props.0.borrow()),
        None => f(&WindowProps::default()),
    }
}

/// What the rules set for `layer`, as last resolved (nothing before).
pub fn layer_props(layer: &LayerSurface) -> LayerProps {
    layer
        .user_data()
        .get::<LayerRuleProps>()
        .map(|p| *p.0.borrow())
        .unwrap_or_default()
}

/// The tag `window` set with xdg-toplevel-tag, or an empty one.
pub fn window_tag(window: &Window) -> String {
    window
        .toplevel()
        .and_then(|toplevel| {
            with_states(toplevel.wl_surface(), |states| {
                states
                    .data_map
                    .get::<XdgToplevelTagSurfaceData>()
                    .and_then(|data| data.tag())
            })
        })
        .map(|tag| tag.to_string())
        .unwrap_or_default()
}

impl Pila {
    /// Match `window` against the window rules again and keep the result.
    pub fn resolve_window_rules(&self, window: &Window) -> WindowProps {
        let (title, app_id) = crate::foreign_toplevel::title_and_app_id(window);
        let tag = window_tag(window);
        let props = resolve_window(&self.config.window_rules, &app_id, &title, &tag);
        debug!(target: "pila::rules", app_id, title, tag, ?props, "window rules");
        window
            .user_data()
            .insert_if_missing(WindowRuleProps::default);
        if let Some(stored) = window.user_data().get::<WindowRuleProps>() {
            *stored.0.borrow_mut() = props.clone();
        }
        props
    }

    /// A new xdg toplevel is about to get its first configure: give it the
    /// size and fill state its rules ask for, so it draws that way from its
    /// first buffer. (A window placed on another workspace or at a position
    /// is handled when it's mapped, `place_new_window`.)
    pub fn apply_initial_rules(&mut self, window: &Window) {
        let props = self.resolve_window_rules(window);
        let app_id = crate::foreign_toplevel::title_and_app_id(window).1;
        if let (Some((w, h)), Some(toplevel)) = (props.size, window.toplevel()) {
            debug!(target: "pila::rules", app_id, w, h, "initial size");
            toplevel.with_pending_state(|state| state.size = Some((w, h).into()));
        }
        self.apply_open_fill(window, &props);
    }

    /// Maximize or fullscreen a window opening, if its rules say so (on top
    /// of what the client asked for).
    pub fn apply_open_fill(&mut self, window: &Window, props: &WindowProps) {
        let (maximized, fullscreen) = crate::state::fill_state(window);
        let wanted = (
            maximized || props.open_maximized == Some(true),
            fullscreen || props.open_fullscreen == Some(true),
        );
        if wanted != (maximized, fullscreen) {
            let app_id = crate::foreign_toplevel::title_and_app_id(window).1;
            debug!(
                target: "pila::rules",
                app_id,
                maximized = wanted.0,
                fullscreen = wanted.1,
                "opens filled"
            );
            self.set_fill_state(window, wanted.0, wanted.1);
        } else if maximized || fullscreen {
            // Asked for by the client before its initial commit: the size
            // was worked out then, maybe before it asked for server-side
            // decorations (a title bar). Again, now that it's all known.
            self.set_fill_state(window, maximized, fullscreen);
        }
    }

    /// Match `layer` against the layer rules again and keep the result.
    pub fn resolve_layer_rules(&self, layer: &LayerSurface) -> LayerProps {
        let props = resolve_layer(&self.config.layer_rules, layer.namespace());
        debug!(target: "pila::rules", namespace = layer.namespace(), ?props, "layer rules");
        layer.user_data().insert_if_missing(LayerRuleProps::default);
        if let Some(stored) = layer.user_data().get::<LayerRuleProps>() {
            *stored.0.borrow_mut() = props;
        }
        props
    }

    /// A window's title or app id changed: resolve its rules again (a rule
    /// may match its new title), and lay it out again if they changed.
    pub fn rules_maybe_changed(&mut self, window: &Window) {
        let before = window
            .user_data()
            .get::<WindowRuleProps>()
            .map(|stored| stored.0.borrow().clone());
        let after = self.resolve_window_rules(window);
        if before.as_ref() == Some(&after) {
            return;
        }
        if before.is_some_and(|before| before.title_bar != after.title_bar) {
            // A title bar shown or hidden changes where filled windows go.
            self.refit_maximized();
        }
        self.queue_redraw();
    }

    /// The config changed: resolve the rules again for every window and
    /// layer surface there is (what applies only when they open stays).
    pub fn refresh_rule_props(&mut self) {
        let windows: Vec<Window> = self
            .space
            .elements()
            .chain(self.windows.unmapped.iter())
            .chain(self.off_space_windows())
            .cloned()
            .collect();
        for window in &windows {
            self.resolve_window_rules(window);
        }
        let layers: Vec<LayerSurface> = self
            .space
            .outputs()
            .flat_map(|output| {
                smithay::desktop::layer_map_for_output(output)
                    .layers()
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .collect();
        for layer in &layers {
            self.resolve_layer_rules(layer);
        }
        // A title bar shown or hidden changes where filled windows go.
        self.refit_maximized();
        self.queue_redraw();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window_rules(toml_text: &str) -> Vec<WindowRule> {
        #[derive(Deserialize)]
        struct File {
            #[serde(rename = "window-rule")]
            rules: Vec<WindowRuleFile>,
        }
        let file: File = toml::from_str(toml_text).unwrap();
        file.rules
            .into_iter()
            .enumerate()
            .map(|(i, r)| r.validate(i + 1, 4).unwrap())
            .collect()
    }

    #[test]
    fn wildcards_match_runs_of_anything() {
        assert!(wildcard_match("noctalia-*", "noctalia-panel"));
        assert!(wildcard_match("noctalia-*", "noctalia-"));
        assert!(!wildcard_match("noctalia-*", "noctalia"));
        assert!(wildcard_match("*", ""));
        assert!(wildcard_match("launcher", "launcher"));
        assert!(!wildcard_match("launcher", "launcher2"));
        assert!(wildcard_match("*-panel", "noctalia-panel"));
        assert!(wildcard_match("a*b*c", "a-x-b-y-c"));
        assert!(!wildcard_match("a*b*c", "a-x-c"));
        assert!(!wildcard_match("ab*ba", "aba"));
    }

    #[test]
    fn every_matcher_must_match() {
        let rules = window_rules(
            r#"
            [[window-rule]]
            app-id = "firefox"
            title = "*Picture-in-Picture*"
            animations = false
            "#,
        );
        assert!(!resolve_window(&rules, "firefox", "Picture-in-Picture", "")
            .animates(AnimationKind::WindowOpen));
        assert!(resolve_window(&rules, "firefox", "Mozilla Firefox", "")
            .animates(AnimationKind::WindowOpen));
        assert!(resolve_window(&rules, "chromium", "Picture-in-Picture", "")
            .animates(AnimationKind::WindowOpen));
    }

    #[test]
    fn later_rules_win_field_by_field() {
        let rules = window_rules(
            r#"
            [[window-rule]]
            animations = false

            [[window-rule]]
            app-id = "foot"
            animations = { close = true }
            "#,
        );
        // A rule without matchers applies to every window.
        let other = resolve_window(&rules, "kitty", "", "");
        assert!(!other.animates(AnimationKind::WindowOpen));
        assert!(!other.animates(AnimationKind::WindowClose));
        let foot = resolve_window(&rules, "foot", "", "");
        assert!(!foot.animates(AnimationKind::WindowOpen));
        assert!(foot.animates(AnimationKind::WindowClose));
        assert!(!foot.animates(AnimationKind::Minimize));
        // No rules: everything plays.
        assert!(resolve_window(&[], "foot", "", "").animates(AnimationKind::Minimize));
    }

    #[test]
    fn open_properties_merge_and_are_checked() {
        let rules = window_rules(
            r#"
            [[window-rule]]
            app-id = "mpv"
            open-on-workspace = 3
            size = [800, 450]
            position = [20, 30]

            [[window-rule]]
            app-id = "mpv"
            title = "*fullscreen*"
            open-fullscreen = true
            size = [1280, 720]
            "#,
        );
        let mpv = resolve_window(&rules, "mpv", "film", "");
        assert_eq!(mpv.open_on_workspace, Some(2));
        assert_eq!(mpv.size, Some((800, 450)));
        assert_eq!(mpv.position, Some((20, 30)));
        assert_eq!(mpv.open_fullscreen, None);
        let full = resolve_window(&rules, "mpv", "fullscreen film", "");
        assert_eq!(full.size, Some((1280, 720)));
        assert_eq!(full.open_fullscreen, Some(true));
        assert_eq!(full.open_on_workspace, Some(2));

        let invalid = |text: &str| {
            #[derive(Deserialize)]
            struct File {
                #[serde(rename = "window-rule")]
                rules: Vec<WindowRuleFile>,
            }
            let file: File = toml::from_str(text).unwrap();
            file.rules
                .into_iter()
                .next()
                .unwrap()
                .validate(1, 4)
                .unwrap_err()
        };
        assert!(invalid("[[window-rule]]\nopen-on-workspace = 5").contains("1 to 4"));
        assert!(invalid("[[window-rule]]\nopen-on-workspace = 0").contains("1 to 4"));
        assert!(invalid("[[window-rule]]\nsize = [0, 300]").contains("size"));
        assert!(invalid("[[window-rule]]\nopacity = 0").contains("opacity"));
        assert!(invalid("[[window-rule]]\nopacity = 1.5").contains("opacity"));
        assert!(invalid("[[window-rule]]\ncorner-radius = 100").contains("corner-radius"));
    }

    #[test]
    fn layer_rules_match_namespaces() {
        let rules: Vec<LayerRule> = [
            LayerRuleFile {
                namespace: Some("noctalia-*".into()),
                animations: Some(false),
                blur: None,
            },
            LayerRuleFile {
                namespace: Some("noctalia-bar-*".into()),
                animations: None,
                blur: Some(true),
            },
        ]
        .into_iter()
        .map(LayerRuleFile::validate)
        .collect();
        let bar = resolve_layer(&rules, "noctalia-bar-main");
        assert_eq!(bar.animations, Some(false));
        assert_eq!(bar.blur, Some(true));
        let panel = resolve_layer(&rules, "noctalia-panel");
        assert_eq!(panel.blur, None);
        assert_eq!(resolve_layer(&rules, "launcher"), LayerProps::default());
    }

    #[test]
    fn bad_animation_keys_are_refused() {
        #[derive(Deserialize)]
        struct File {
            #[serde(rename = "window-rule")]
            _rules: Vec<WindowRuleFile>,
        }
        assert!(toml::from_str::<File>("[[window-rule]]\nanimations = { opne = false }").is_err());
        assert!(toml::from_str::<File>("[[window-rule]]\napp_id = \"x\"").is_err());
    }
}
