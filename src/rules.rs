//! Window rules and layer rules: `[[window-rule]]` and `[[layer-rule]]` in
//! the config. A rule matches windows by app id (xdg `app_id`, or the class
//! of an X11 window's WM_CLASS) and title, or layer surfaces by namespace,
//! with `*` standing for any run of characters; every matcher a rule gives
//! must match, and a rule without matchers matches everything. Matching
//! rules apply in order, the last one winning field by field.
//!
//! What they set is resolved when a window or layer surface appears and
//! kept in its user data (`WindowRuleProps`, `LayerRuleProps`), so nothing
//! is matched per frame; a config reload resolves it again for everything
//! already there (`Pila::refresh_rule_props`). A window's title changing
//! later doesn't re-evaluate its rules.

use std::cell::RefCell;

use serde::Deserialize;
use smithay::desktop::{LayerSurface, Window};
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
    animations: Option<AnimationsFile>,
}

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
}

impl WindowProps {
    fn merge(&mut self, other: &WindowProps) {
        self.animations.merge(&other.animations);
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
    /// message).
    pub fn validate(self, _index: usize) -> Result<WindowRule, String> {
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
            props: WindowProps { animations },
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

/// What `rules` set for a window with this app id and title.
pub fn resolve_window(rules: &[WindowRule], app_id: &str, title: &str) -> WindowProps {
    let mut props = WindowProps::default();
    for rule in rules {
        if matches(&rule.app_id, app_id) && matches(&rule.title, title) {
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
    window
        .user_data()
        .get::<WindowRuleProps>()
        .map(|p| p.0.borrow().clone())
        .unwrap_or_default()
}

/// What the rules set for `layer`, as last resolved (nothing before).
pub fn layer_props(layer: &LayerSurface) -> LayerProps {
    layer
        .user_data()
        .get::<LayerRuleProps>()
        .map(|p| *p.0.borrow())
        .unwrap_or_default()
}

impl Pila {
    /// Match `window` against the window rules again and keep the result.
    pub fn resolve_window_rules(&self, window: &Window) -> WindowProps {
        let (title, app_id) = crate::foreign_toplevel::title_and_app_id(window);
        let props = resolve_window(&self.config.window_rules, &app_id, &title);
        debug!(target: "pila::rules", app_id, title, ?props, "window rules");
        window
            .user_data()
            .insert_if_missing(WindowRuleProps::default);
        if let Some(stored) = window.user_data().get::<WindowRuleProps>() {
            *stored.0.borrow_mut() = props.clone();
        }
        props
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

    /// The config changed: resolve the rules again for every window and
    /// layer surface there is (what applies only when they open stays).
    pub fn refresh_rule_props(&mut self) {
        let windows: Vec<Window> = self
            .space
            .elements()
            .chain(self.unmapped.iter())
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
            .map(|(i, r)| r.validate(i + 1).unwrap())
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
        assert!(!resolve_window(&rules, "firefox", "Picture-in-Picture")
            .animates(AnimationKind::WindowOpen));
        assert!(resolve_window(&rules, "firefox", "Mozilla Firefox")
            .animates(AnimationKind::WindowOpen));
        assert!(resolve_window(&rules, "chromium", "Picture-in-Picture")
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
        let other = resolve_window(&rules, "kitty", "");
        assert!(!other.animates(AnimationKind::WindowOpen));
        assert!(!other.animates(AnimationKind::WindowClose));
        let foot = resolve_window(&rules, "foot", "");
        assert!(!foot.animates(AnimationKind::WindowOpen));
        assert!(foot.animates(AnimationKind::WindowClose));
        assert!(!foot.animates(AnimationKind::Minimize));
        // No rules: everything plays.
        assert!(resolve_window(&[], "foot", "").animates(AnimationKind::Minimize));
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
