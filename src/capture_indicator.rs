//! The capture indicator: what is capturing the screen now, whichever way
//! (the portal's casts, ext-image-copy-capture sessions, wlr-screencopy
//! frames), for `pila msg captures` and its event, and a light on each
//! monitor being captured: a small square in the accent in the middle of
//! its top edge, where the eye passes often, drawn on the screen alone (`render::screen_elements`), never in a
//! capture. `[permissions] indicator` turns the light off; the list stays.
//!
//! A capture is listed while it runs and for `HOLD` after it ends, so a
//! one-shot screenshot (grim: one frame, then gone) shows too; the one that
//! ended last is kept, with when (`CaptureIndicator::last`), so a capture
//! over before anyone looked can still be named. A window's
//! light is on the monitor it's on (`Pila::window_output`), or the one a new
//! window would open on if it's on none (minimized, another workspace).
//!
//! Recomputed at the end of a loop iteration (`Pila::update_capture_indicator`,
//! from `after_frames`) when something asked (`dirty`: a capture starting, a
//! program's name read) or while anything is listed, so an ending capture,
//! a window moving to another monitor or `HOLD` running out is seen. A change
//! redraws the outputs whose light comes or goes and tells the event's
//! subscribers.

use std::{
    collections::HashSet,
    time::{Duration, Instant, SystemTime},
};

use serde_json::{json, Value};
use smithay::{
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            RegistrationToken,
        },
        wayland_server::{backend::ClientId, Client, Resource},
    },
    utils::{Logical, Rectangle},
};
use tracing::{info, warn};

use crate::{capture::Target, state::ClientState, Pila};

/// How long a capture stays listed after it ends.
pub const HOLD: Duration = Duration::from_secs(1);
/// The light's side, in logical pixels.
pub const LIGHT: i32 = 8;

/// How a program captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Portal,
    ExtImageCopyCapture,
    WlrScreencopy,
}

impl Protocol {
    pub fn name(self) -> &'static str {
        match self {
            Protocol::Portal => "portal",
            Protocol::ExtImageCopyCapture => "ext-image-copy-capture",
            Protocol::WlrScreencopy => "wlr-screencopy",
        }
    }
}

/// What it captures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Captured {
    Output(String),
    /// A part of an output, from its top-left (logical).
    Region(String, Rectangle<i32, Logical>),
    /// A window, by its foreign-toplevel identifier (`pila msg windows`'s
    /// `id`), its app id, and the output its light is on.
    Window {
        id: String,
        app_id: String,
        output: Option<String>,
    },
}

impl Captured {
    /// The output its light is on.
    fn output(&self) -> Option<&str> {
        match self {
            Captured::Output(name) | Captured::Region(name, _) => Some(name),
            Captured::Window { output, .. } => output.as_deref(),
        }
    }
}

/// A program capturing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capturer {
    /// The app as the portal named it, or the program's executable name.
    pub app: String,
    pub protocol: Protocol,
    pub target: Captured,
    /// The Wayland client capturing; `None` for the portal's casts.
    pub client: Option<ClientId>,
}

impl Capturer {
    /// The same capture, whatever it's named: a client's name comes once
    /// its executable is read, and a capture renamed hasn't ended.
    pub(crate) fn same(&self, other: &Capturer) -> bool {
        self.protocol == other.protocol
            && self.target == other.target
            && self.client == other.client
            && (self.client.is_some() || self.app == other.app)
    }

    /// For people, as `pila msg captures` writes it: the app, what, how.
    pub fn describe(&self) -> String {
        let what = match &self.target {
            Captured::Output(output) => format!("screen {output}"),
            Captured::Region(output, rect) => format!(
                "region of {output} {}x{}+{}+{}",
                rect.size.w, rect.size.h, rect.loc.x, rect.loc.y
            ),
            Captured::Window { app_id, .. } => format!("window {app_id}"),
        };
        format!("{} ({what}, {})", self.app, self.protocol.name())
    }

    pub fn json(&self) -> Value {
        let target = match &self.target {
            Captured::Output(output) => json!({ "kind": "output", "output": output }),
            Captured::Region(output, rect) => json!({
                "kind": "region",
                "output": output,
                "x": rect.loc.x,
                "y": rect.loc.y,
                "width": rect.size.w,
                "height": rect.size.h,
            }),
            Captured::Window { id, app_id, output } => json!({
                "kind": "window",
                "id": id,
                "app_id": app_id,
                "output": output,
            }),
        };
        json!({ "app": self.app, "protocol": self.protocol.name(), "target": target })
    }
}

/// The indicator's state (`Pila::capture_indicator`).
#[derive(Default)]
pub struct CaptureIndicator {
    /// What's listed: capturing now, then ended less than `HOLD` ago.
    pub shown: Vec<Capturer>,
    /// What was capturing at the last update.
    current: Vec<Capturer>,
    /// What ended, and until when it's still listed.
    held: Vec<(Capturer, Instant)>,
    /// The capture that ended last, and when.
    last: Option<(Capturer, SystemTime)>,
    /// The outputs with a light, by name, and where it is (global).
    pub lights: Vec<(String, Rectangle<i32, Logical>)>,
    /// Recompute at the end of this iteration.
    pub dirty: bool,
    /// Wakes the loop when the first of `held` runs out.
    timer: Option<(RegistrationToken, Instant)>,
}

impl CaptureIndicator {
    /// Name `name` the captures `of` picks, wherever they're kept: those
    /// capturing at the last update too, since one that ends before the
    /// next is held and kept as `last` under the name it had then.
    fn rename(&mut self, of: impl Fn(&Capturer) -> bool, name: &str) {
        let last = self.last.as_mut().map(|(last, _)| last);
        let held = self.held.iter_mut().map(|(held, _)| held);
        for capturer in self.current.iter_mut().chain(held).chain(last) {
            if of(capturer) {
                capturer.app = name.to_string();
            }
        }
    }

    /// Whether the next iteration must recompute.
    fn due(&self) -> bool {
        self.dirty || !self.shown.is_empty()
    }
}

impl Pila {
    /// Something may have started capturing: recompute at the end of this
    /// iteration.
    pub fn capture_indicator_changed(&mut self) {
        self.capture_indicator.dirty = true;
    }

    /// Recompute what's capturing, if something asked or anything is
    /// listed: redraw the outputs whose light changed and tell subscribers.
    pub fn update_capture_indicator(&mut self) {
        if !self.capture_indicator.due() {
            return;
        }
        self.capture_indicator.dirty = false;
        let now = Instant::now();
        let current = self.capturers();
        let indicator = &mut self.capture_indicator;
        let reading = &self.capture_permissions.reading;
        let ended: Vec<Capturer> = indicator
            .current
            .iter()
            .filter(|c| !current.iter().any(|now| now.same(c)))
            .cloned()
            .collect();
        for capturer in ended {
            indicator.held.retain(|(held, _)| !held.same(&capturer));
            indicator.held.push((capturer.clone(), now + HOLD));
            indicator.last = Some((capturer, SystemTime::now()));
        }
        indicator
            .held
            .retain(|(held, until)| *until > now && !current.iter().any(|now| now.same(held)));
        let mut shown = current.clone();
        for (held, _) in &indicator.held {
            if !shown.iter().any(|s| s.same(held)) {
                shown.push(held.clone());
            }
        }
        indicator.current = current;
        let listed_changed = shown != indicator.shown;
        if listed_changed {
            // Logged once, by name: when it's read, if it's being read.
            let new = shown.iter().filter(|c| {
                !c.client.as_ref().is_some_and(|id| reading.contains(id))
                    && !indicator.shown.contains(c)
            });
            for capturer in new {
                info!(
                    app = capturer.app,
                    protocol = capturer.protocol.name(),
                    target = %capturer.json()["target"],
                    "capture indicator: capturing"
                );
            }
            indicator.shown = shown;
        }
        let next_end = indicator.held.iter().map(|(_, until)| *until).min();
        self.schedule_capture_indicator(next_end);
        let lights = self.capture_lights();
        if lights != self.capture_indicator.lights {
            let before = std::mem::replace(&mut self.capture_indicator.lights, lights);
            let names: HashSet<String> = before
                .iter()
                .chain(&self.capture_indicator.lights)
                .map(|(name, _)| name.clone())
                .collect();
            let outputs: Vec<_> = self
                .space
                .outputs()
                .filter(|o| names.contains(&o.name()))
                .cloned()
                .collect();
            for output in outputs {
                self.queue_redraw_output(&output);
            }
        }
        if listed_changed {
            self.send_captures_event();
            self.update_tray();
        }
    }

    /// End `capturer`'s capture, as the user asked (the tray's Stop): a
    /// portal share ends, its app told; a program capturing directly has its
    /// captures stopped and can't capture again until it connects again
    /// (`Pila::stop_client_captures`), or it would at once.
    pub(crate) fn stop_capture(&mut self, capturer: &Capturer) {
        match &capturer.client {
            Some(client) => self.stop_client_captures(client),
            None => self.stop_casts(&capturer.app, &capturer.target),
        }
        self.capture_indicator_changed();
    }

    /// What `pila msg captures` lists.
    pub(crate) fn ipc_captures(&self) -> Vec<Value> {
        self.capture_indicator
            .shown
            .iter()
            .map(Capturer::json)
            .collect()
    }

    /// The capture that ended last for `pila msg captures`, with `ended` in
    /// Unix seconds; `null` before one has.
    pub(crate) fn ipc_last_capture(&self) -> Value {
        let Some((capturer, ended)) = &self.capture_indicator.last else {
            return Value::Null;
        };
        let mut value = capturer.json();
        let ended = ended
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        value["ended"] = ended.into();
        value
    }

    /// Wake the loop at `at`, when a held capture runs out (none: no wake).
    fn schedule_capture_indicator(&mut self, at: Option<Instant>) {
        let current = self.capture_indicator.timer.map(|(_, when)| when);
        if current == at {
            return;
        }
        if let Some((token, _)) = self.capture_indicator.timer.take() {
            self.loop_handle.remove(token);
        }
        let Some(at) = at else {
            return;
        };
        let timer = Timer::from_deadline(at);
        let inserted = self.loop_handle.insert_source(timer, |_, _, state| {
            state.capture_indicator.timer = None;
            state.capture_indicator.dirty = true;
            TimeoutAction::Drop
        });
        match inserted {
            Ok(token) => self.capture_indicator.timer = Some((token, at)),
            Err(err) => warn!(?err, "capture indicator: can't schedule its update"),
        }
    }

    /// The lights: one per output something listed captures, in the middle
    /// of its top edge. None when `[permissions] indicator` is off.
    fn capture_lights(&self) -> Vec<(String, Rectangle<i32, Logical>)> {
        if !self.config.permissions.indicator {
            return Vec::new();
        }
        let mut names: Vec<&str> = self
            .capture_indicator
            .shown
            .iter()
            .filter_map(|c| c.target.output())
            .collect();
        names.sort_unstable();
        names.dedup();
        names
            .into_iter()
            .filter_map(|name| {
                let output = self.space.outputs().find(|o| o.name() == name)?;
                let geo = self.space.output_geometry(output)?;
                let light = Rectangle::new(
                    (geo.loc.x + (geo.size.w - LIGHT) / 2, geo.loc.y).into(),
                    (LIGHT, LIGHT).into(),
                );
                Some((name.to_string(), light))
            })
            .collect()
    }

    /// Everything capturing now.
    fn capturers(&mut self) -> Vec<Capturer> {
        let mut found = Vec::new();
        let casts: Vec<(String, Target)> = self
            .screencast
            .shares()
            .map(|(app, target)| (app.to_string(), target.clone()))
            .collect();
        for (app, target) in casts {
            if let Some(target) = self.captured(&target) {
                found.push(Capturer {
                    app,
                    protocol: Protocol::Portal,
                    target,
                    client: None,
                });
            }
        }
        let sessions: Vec<(Option<Client>, Target)> = self
            .capture
            .capturing()
            .map(|(client, target)| (client, target.clone()))
            .collect();
        for (client, target) in sessions {
            if let Some(target) = self.captured(&target) {
                let app = self.capturing_program(client.as_ref());
                found.push(Capturer {
                    app,
                    protocol: Protocol::ExtImageCopyCapture,
                    target,
                    client: client.map(|c| c.id()),
                });
            }
        }
        let screencopies: Vec<(Option<Client>, Target)> = self
            .capture
            .screencopies
            .iter()
            .filter_map(|frame| Some((frame.client(), crate::screencopy::frame_target(frame)?)))
            .collect();
        for (client, target) in screencopies {
            if let Some(target) = self.captured(&target) {
                let app = self.capturing_program(client.as_ref());
                found.push(Capturer {
                    app,
                    protocol: Protocol::WlrScreencopy,
                    target,
                    client: client.map(|c| c.id()),
                });
            }
        }
        // Several sessions of one program on one target are one capture.
        let mut unique: Vec<Capturer> = Vec::with_capacity(found.len());
        for capturer in found {
            if !unique.contains(&capturer) {
                unique.push(capturer);
            }
        }
        unique
    }

    /// What `target` names now; `None` if it's gone.
    pub(crate) fn captured(&self, target: &Target) -> Option<Captured> {
        match target {
            Target::Output(output) => {
                let output = output.upgrade()?;
                self.space.output_geometry(&output)?;
                Some(Captured::Output(output.name()))
            }
            Target::Region(output, rect) => {
                let output = output.upgrade()?;
                let geo = self.space.output_geometry(&output)?;
                // A screencopy of the whole output.
                if *rect == Rectangle::from_size(geo.size) {
                    return Some(Captured::Output(output.name()));
                }
                Some(Captured::Region(output.name(), *rect))
            }
            Target::Toplevel(id) => {
                let window = self.window_for_identifier(id)?;
                let output = self
                    .window_output(&window)
                    .or_else(|| self.output_for_new_window())
                    .map(|o| o.name());
                Some(Captured::Window {
                    id: id.clone(),
                    app_id: crate::foreign_toplevel::app_id(&window),
                    output,
                })
            }
            Target::Gone => None,
        }
    }

    /// A capturing client's name: its executable's, read off the event loop
    /// the first time (until then, its process id).
    fn capturing_program(&mut self, client: Option<&Client>) -> String {
        let Some(client) = client else {
            return "a program".to_string();
        };
        let Some(data) = client.get_data::<ClientState>() else {
            return "a program".to_string();
        };
        if let Some(identity) = data.executable.get() {
            return display_name(identity, data.pid);
        }
        let pid = data.pid;
        self.read_client_executable(client);
        unknown(pid)
    }

    /// `client`'s executable was read (`Pila::read_client_executable`):
    /// its captures are named by it, recomputed.
    pub(crate) fn capturing_program_read(
        &mut self,
        client: &Client,
        identity: &crate::permissions::ClientIdentity,
    ) {
        let pid = client.get_data::<ClientState>().and_then(|data| data.pid);
        let name = display_name(identity, pid);
        let id = client.id();
        // A capture that ended (or ends before the next update) before its
        // name was read is named too.
        self.capture_indicator
            .rename(|c| c.client.as_ref() == Some(&id), &name);
        self.capture_indicator.dirty = true;
    }
}

/// A program by its executable's name, or its process id without one.
fn display_name(identity: &crate::permissions::ClientIdentity, pid: Option<i32>) -> String {
    match &identity.executable {
        Some(executable) => crate::permissions::program_name(executable).to_string(),
        None => unknown(pid),
    }
}

/// A program by its process id, before (or without) its name.
fn unknown(pid: Option<i32>) -> String {
    match pid {
        Some(pid) => format!("process {pid}"),
        None => "a program".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name read while the capture still runs reaches the capture as it
    /// was at the last update: if it ends before the next one, it's held
    /// and kept as `last` by name, not by process id (it was, on CI).
    #[test]
    fn a_name_read_while_capturing_names_what_ends() {
        let capturer = |app: &str| Capturer {
            app: app.into(),
            protocol: Protocol::WlrScreencopy,
            target: Captured::Output("DP-1".into()),
            client: None,
        };
        let mut indicator = CaptureIndicator {
            current: vec![capturer("process 7")],
            held: vec![(capturer("process 7"), Instant::now())],
            last: Some((capturer("process 7"), SystemTime::now())),
            ..Default::default()
        };
        indicator.rename(|c| c.app == "process 7", "grim");
        assert_eq!(indicator.current[0].app, "grim");
        assert_eq!(indicator.held[0].0.app, "grim");
        assert_eq!(
            indicator.last.as_ref().map(|(c, _)| c.app.as_str()),
            Some("grim")
        );
    }

    #[test]
    fn a_capturer_is_listed_as_the_ipc_documents() {
        let capturer = Capturer {
            app: "grim".into(),
            protocol: Protocol::WlrScreencopy,
            target: Captured::Region(
                "DP-1".into(),
                Rectangle::new((10, 20).into(), (300, 200).into()),
            ),
            client: None,
        };
        assert_eq!(
            capturer.json(),
            json!({
                "app": "grim",
                "protocol": "wlr-screencopy",
                "target": { "kind": "region", "output": "DP-1", "x": 10, "y": 20, "width": 300, "height": 200 },
            })
        );
        let window = Capturer {
            app: "Browser".into(),
            protocol: Protocol::Portal,
            target: Captured::Window {
                id: "w1".into(),
                app_id: "kitty".into(),
                output: None,
            },
            client: None,
        };
        assert_eq!(window.json()["target"]["output"], Value::Null);
        assert_eq!(window.target.output(), None);
    }
}
