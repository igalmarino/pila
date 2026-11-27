//! Screen capture permissions for direct Wayland protocols
//! (`wlr-screencopy` and `ext-image-copy-capture`).
//!
//! Off by default (`[permissions] enforce = false`): any unsandboxed client
//! may capture the screen (sandboxed apps never see these protocols,
//! `state::is_privileged`). With `enforce = true`, capture requests from
//! clients are checked against:
//!
//! 1. `[[permission]]` rules in `config.toml`, matched against the client's
//!    executable path (`/proc/<pid>/exe`, read off the event loop) with `*`
//!    wildcards (`rules::wildcard_match`), last match winning (`allow`, `ask`,
//!    `deny`). The config is the only place a permission outlives Pila.
//! 2. Grants given in the prompt, in memory until Pila exits (the user logs
//!    out), listed by `pila msg capture-permissions` and revoked by
//!    `pila msg forget-capture-permission`.
//! 3. Otherwise, a modal confirmation panel (`panel.rs`) showing the program
//!    name and full executable path, with Allow (`Enter`) and Deny (`Esc`).
//!
//! While the screen is locked, clients that would need a prompt are denied
//! outright; clients a rule or a grant allows (such as a locker capturing a
//! blurred background) continue to work.
//!
//! Portal screen sharing and portal screenshots always use Pila's own picker
//! (`share_picker.rs`) and grants (`screen_grants.rs`) and are unaffected.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet, VecDeque},
    path::Path,
    rc::Rc,
    time::{Duration, Instant},
};

use serde_json::{json, Value};
use smithay::{
    backend::renderer::{element::solid::SolidColorBuffer, gles::GlesRenderer},
    input::keyboard::Keysym,
    reexports::{
        wayland_protocols::ext::image_copy_capture::v1::server::ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1,
        wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
        wayland_server::{backend::ClientId, Client, Resource},
    },
    utils::{Logical, Point, Rectangle, Scale, Size},
};
use tracing::{debug, info};

use crate::{
    config::{Colors, PermissionAccess, PermissionRule},
    panel::{self, Align, ContentImage, FrameBuffers, Item, TextKey},
    render::PilaRenderElement,
    rules::wildcard_match,
    state::ClientState,
    state_file, Pila,
};

/// Panel geometry (logical pixels): wide enough for the question whole with a
/// long program name (`the_question_fits_for_a_long_program_name`), the
/// Allow/Deny buttons at the bottom right.
const WIDTH: i32 = 720;
const PATH_AT: i32 = 58;
const NOTE_AT: i32 = 82;
const LASTS_AT: i32 = 106;
const NOTE_HEIGHT: i32 = 20;
const BUTTON_HEIGHT: i32 = 34;
const BUTTON_WIDTH: i32 = 88;
const BUTTON_GAP: i32 = 10;

/// How long Allow does nothing once the prompt shows: it may pop up while
/// the user is typing, and an Enter meant for the app mustn't allow a capture.
/// Deny works at once.
pub const ALLOW_DELAY: Duration = Duration::from_millis(500);

/// Resolved executable identity for a Wayland client, cached on `ClientState`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientIdentity {
    /// Canonical path from `/proc/<pid>/exe`, if readable.
    pub executable: Option<String>,
}

/// A capture request waiting on permission resolution or user confirmation.
#[derive(Clone)]
pub enum PendingCapture {
    Session(ExtImageCopyCaptureSessionV1),
    Screencopy(ZwlrScreencopyFrameV1),
}

impl PendingCapture {
    fn is_alive(&self) -> bool {
        match self {
            PendingCapture::Session(resource) => resource.is_alive(),
            PendingCapture::Screencopy(frame) => frame.is_alive(),
        }
    }

    /// The client that asked.
    fn client_id(&self) -> Option<ClientId> {
        match self {
            PendingCapture::Session(resource) => resource.client(),
            PendingCapture::Screencopy(frame) => frame.client(),
        }
        .map(|client| client.id())
    }

    fn matches_session(&self, target: &ExtImageCopyCaptureSessionV1) -> bool {
        matches!(self, PendingCapture::Session(resource) if resource == target)
    }

    fn matches_screencopy(&self, target: &ZwlrScreencopyFrameV1) -> bool {
        matches!(self, PendingCapture::Screencopy(frame) if frame == target)
    }
}

/// What groups concurrent requests into one prompt so a tool capturing
/// multiple outputs (e.g. `grim`) prompts once rather than per output.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PromptKey {
    Executable(String),
    Client(ClientId),
}

/// A capture permission given in the prompt, until Pila exits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub id: String,
    pub executable: String,
}

/// A queued prompt waiting for its turn behind another prompt or modal panel.
struct Queued {
    key: PromptKey,
    executable: Option<String>,
    requests: Vec<PendingCapture>,
}

/// The prompt currently shown on screen.
struct Shown {
    key: PromptKey,
    executable: Option<String>,
    output: String,
    requests: Vec<PendingCapture>,
    /// Since when it can be seen (its text drawn): Allow waits `ALLOW_DELAY`
    /// from then.
    visible_since: Option<Instant>,
}

impl Shown {
    fn armed(&self) -> bool {
        self.visible_since
            .is_some_and(|since| since.elapsed() >= ALLOW_DELAY)
    }
}

#[derive(Default)]
struct Buffers {
    frame: FrameBuffers,
    solids: HashMap<&'static str, SolidColorBuffer>,
    dims: HashMap<String, SolidColorBuffer>,
    text: Option<ContentImage>,
}

/// Where the prompt panel and its interactive controls sit, global logical
/// coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub panel: Rectangle<i32, Logical>,
    pub allow: Rectangle<i32, Logical>,
    pub deny: Rectangle<i32, Logical>,
}

/// Lay out the prompt centered in `area`, narrower than `WIDTH` on an output
/// that is.
pub fn layout(area: Rectangle<i32, Logical>) -> Layout {
    let width = WIDTH.min(area.size.w - 2 * panel::PADDING);
    let body_bottom = LASTS_AT + NOTE_HEIGHT;
    let height = body_bottom + panel::FOOTER_GAP + BUTTON_HEIGHT + panel::PADDING;
    let panel = Rectangle::new(
        area.loc + Point::from(((area.size.w - width) / 2, (area.size.h - height) / 2)),
        Size::from((width, height)),
    );
    let buttons_y = height - panel::PADDING - BUTTON_HEIGHT;
    let allow = Rectangle::new(
        panel.loc + Point::from((width - panel::PADDING - BUTTON_WIDTH, buttons_y)),
        Size::from((BUTTON_WIDTH, BUTTON_HEIGHT)),
    );
    let deny = Rectangle::new(
        allow.loc - Point::from((BUTTON_GAP + BUTTON_WIDTH, 0)),
        Size::from((BUTTON_WIDTH, BUTTON_HEIGHT)),
    );
    Layout { panel, allow, deny }
}

/// What `Pila::scene` passes to the renderer while the prompt is open.
#[derive(Clone)]
pub struct ScenePrompt {
    pub output: String,
    pub layout: Layout,
    pub rounding: panel::Shape,
    buffers: Rc<RefCell<Buffers>>,
}

/// Capture permission state (`Pila::capture_permissions`).
#[derive(Default)]
pub struct CapturePermissions {
    /// Grants given in the prompt, by executable path.
    grants: Vec<Grant>,
    /// Session-only grants for clients whose executable couldn't be read.
    client_grants: HashSet<ClientId>,
    /// Connections whose request the user denied: the next ones that would
    /// ask fail without asking, until the program connects again (one
    /// retrying would bring the modal prompt back forever).
    client_denials: HashSet<ClientId>,
    /// Connections whose captures the user stopped (the tray's Stop): their
    /// next requests fail, enforced or not and whatever the rules say, until
    /// the program connects again, or one recording would start over at once.
    stopped: HashSet<ClientId>,
    /// Requests waiting for `/proc/<pid>/exe` to be read on a thread.
    resolving: HashMap<ClientId, Vec<PendingCapture>>,
    /// Prompts queued behind the current prompt or another modal panel.
    waiting: VecDeque<Queued>,
    /// The modal prompt currently shown.
    shown: Option<Shown>,
    /// Text key currently being rendered on a thread.
    pending_text: Option<TextKey>,
    buffers: Rc<RefCell<Buffers>>,
}

/// The decision of policy evaluation for an executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
    Ask,
}

/// Evaluate `[[permission]]` rules (last match wins), falling back to the
/// prompt's grants, then `Ask`.
pub fn evaluate(rules: &[PermissionRule], grants: &[Grant], executable: Option<&str>) -> Decision {
    let Some(exe) = executable else {
        return Decision::Ask;
    };
    let rule = rules
        .iter()
        .rev()
        .find(|rule| wildcard_match(&rule.executable, exe))
        .map(|rule| rule.access);
    match rule {
        Some(PermissionAccess::Allow) => Decision::Allow,
        Some(PermissionAccess::Deny) => Decision::Deny,
        Some(PermissionAccess::Ask) | None => {
            if grants.iter().any(|g| g.executable == exe) {
                Decision::Allow
            } else {
                Decision::Ask
            }
        }
    }
}

/// The prompt's question, short enough to fit whole for a long program
/// name. A permission is a program's, for every capture (the screen or any
/// window), whatever it asked for first: the note under it says so.
fn title(executable: Option<&str>) -> String {
    match executable {
        Some(exe) => format!("Allow {} to capture your screen?", program_name(exe)),
        None => "Allow an unknown program to capture your screen?".to_string(),
    }
}

/// Short program name for the prompt title (e.g. `"/usr/bin/grim"` -> `"grim"`).
pub(crate) fn program_name(executable: &str) -> &str {
    Path::new(executable)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(executable)
}

/// The executable a `/proc/<pid>/exe` link names: without the ` (deleted)`
/// the kernel adds once the file is gone, as when a package update replaced
/// a program still running, so its rules and grants still match.
fn executable_path(target: &Path) -> String {
    let path = target.to_string_lossy();
    path.strip_suffix(" (deleted)").unwrap_or(&path).to_string()
}

/// Read `/proc/<pid>/exe` off the event loop.
pub(crate) fn read_executable(pid: i32) -> ClientIdentity {
    let path = format!("/proc/{pid}/exe");
    let executable = match std::fs::read_link(&path) {
        Ok(target) => Some(executable_path(&target)),
        Err(err) => {
            debug!(pid, ?err, "permissions: couldn't read client executable");
            None
        }
    };
    ClientIdentity { executable }
}

impl Pila {
    /// Check whether `client` is allowed to carry out `request`.
    ///
    /// When `[permissions] enforce` is off, allows immediately with no I/O.
    /// Otherwise resolves `/proc/<pid>/exe` off the event loop (cached on
    /// `ClientState`), evaluates `[[permission]]` rules and grants, and either
    /// executes `request`, denies it, or opens the modal prompt.
    pub(crate) fn authorize_capture(&mut self, client: &Client, request: PendingCapture) {
        // Stopped by the user (the tray's Stop, `stop_client_captures`),
        // enforced or not.
        if self.capture_permissions.stopped.contains(&client.id()) {
            // Logged once, when stopped (`tray.rs`): a recorder retries a
            // few times before it gives up.
            debug!("capture refused: stopped on this connection");
            self.deny_capture(request);
            return;
        }
        if !self.config.permissions.enforce {
            self.approve_capture(request);
            return;
        }
        let client_id = client.id();
        let cached = client
            .get_data::<ClientState>()
            .and_then(|s| s.executable.get().cloned());
        if let Some(identity) = cached {
            self.decide_capture(client_id, identity, request);
            return;
        }
        let first = !self.capture_permissions.resolving.contains_key(&client_id);
        self.capture_permissions
            .resolving
            .entry(client_id)
            .or_default()
            .push(request);
        if first {
            self.resolve_executable(client);
        }
    }

    /// Read `client`'s executable on a thread, then decide the requests
    /// waiting for it in `resolving` and stop its captures if it isn't
    /// allowed (`stop_denied_captures`).
    fn resolve_executable(&mut self, client: &Client) {
        let pid = client.get_data::<ClientState>().and_then(|s| s.pid);
        let Some(pid) = pid else {
            let identity = ClientIdentity { executable: None };
            self.client_executable_resolved(client.clone(), identity);
            return;
        };
        let waiting = client.clone();
        let started = self.off_loop(
            "pila-client-exe",
            move || read_executable(pid),
            move |state, identity| state.client_executable_resolved(waiting, identity),
        );
        if !started {
            let identity = ClientIdentity { executable: None };
            self.client_executable_resolved(client.clone(), identity);
        }
    }

    fn client_executable_resolved(&mut self, client: Client, identity: ClientIdentity) {
        if let Some(data) = client.get_data::<ClientState>() {
            let _ = data.executable.set(identity.clone());
        }
        let client_id = client.id();
        let requests = self
            .capture_permissions
            .resolving
            .remove(&client_id)
            .unwrap_or_default();
        for request in requests {
            if !request.is_alive() {
                continue;
            }
            if !self.config.permissions.enforce {
                self.approve_capture(request);
            } else {
                self.decide_capture(client_id.clone(), identity.clone(), request);
            }
        }
        // Its captures started while `enforce` was off, unchecked.
        self.stop_denied_captures();
    }

    fn decide_capture(
        &mut self,
        client_id: ClientId,
        identity: ClientIdentity,
        request: PendingCapture,
    ) {
        let exe = identity.executable.as_deref();
        if exe.is_none() && self.capture_permissions.client_grants.contains(&client_id) {
            self.approve_capture(request);
            return;
        }
        match evaluate(
            &self.config.permission_rules,
            &self.capture_permissions.grants,
            exe,
        ) {
            Decision::Allow => {
                debug!(executable = exe.unwrap_or("?"), "capture allowed");
                self.approve_capture(request);
            }
            Decision::Deny => {
                info!(executable = exe.unwrap_or("?"), "capture denied by rule");
                self.deny_capture(request);
            }
            Decision::Ask => {
                if self.capture_permissions.client_denials.contains(&client_id) {
                    info!(
                        executable = exe.unwrap_or("?"),
                        "capture denied: denied before on this connection"
                    );
                    self.deny_capture(request);
                    return;
                }
                if self.is_locked() {
                    info!(
                        executable = exe.unwrap_or("?"),
                        "capture denied while locked"
                    );
                    self.deny_capture(request);
                    return;
                }
                self.queue_capture_prompt(client_id, identity.executable, request);
            }
        }
    }

    fn queue_capture_prompt(
        &mut self,
        client_id: ClientId,
        executable: Option<String>,
        request: PendingCapture,
    ) {
        let key = match &executable {
            Some(exe) => PromptKey::Executable(exe.clone()),
            None => PromptKey::Client(client_id),
        };
        // Coalesce into an open prompt for the same program/client.
        if let Some(shown) = self.capture_permissions.shown.as_mut() {
            if shown.key == key {
                shown.requests.push(request);
                return;
            }
        }
        // Or into an already-queued prompt for the same program/client.
        if let Some(queued) = self
            .capture_permissions
            .waiting
            .iter_mut()
            .find(|q| q.key == key)
        {
            queued.requests.push(request);
            return;
        }
        self.capture_permissions.waiting.push_back(Queued {
            key,
            executable,
            requests: vec![request],
        });
        self.show_capture_permission();
    }

    /// Show the next queued permission prompt if no modal panel is open.
    pub(crate) fn show_capture_permission(&mut self) {
        if self.capture_permissions.shown.is_some() || self.capture_permissions.waiting.is_empty() {
            return;
        }
        if self.is_locked() {
            let waiting = std::mem::take(&mut self.capture_permissions.waiting);
            for queued in waiting {
                for request in queued.requests {
                    self.deny_capture(request);
                }
            }
            return;
        }
        if self.share_picker_open()
            || self.color_pick_open()
            || self.app_shortcuts_open()
            || self.quit_dialog_open()
        {
            return;
        }
        while let Some(mut queued) = self.capture_permissions.waiting.pop_front() {
            queued.requests.retain(PendingCapture::is_alive);
            if queued.requests.is_empty() {
                continue;
            }
            let Some(output) = self.output_at(self.pointer_location()) else {
                for request in queued.requests {
                    self.deny_capture(request);
                }
                continue;
            };
            self.end_switch();
            self.hide_shortcuts();
            self.close_overview();
            self.close_window_menu();
            info!(
                executable = queued.executable.as_deref().unwrap_or("?"),
                "asking for screen capture permission"
            );
            self.capture_permissions.shown = Some(Shown {
                key: queued.key,
                executable: queued.executable,
                output: output.name(),
                requests: queued.requests,
                visible_since: None,
            });
            self.capture_permissions.pending_text = None;
            *self.capture_permissions.buffers.borrow_mut() = Buffers::default();
            self.queue_redraw();
            self.draw_capture_permission_text();
            return;
        }
    }

    pub fn capture_permission_open(&self) -> bool {
        self.capture_permissions.shown.is_some()
    }

    /// The prompt has been seen long enough to take an Allow (`ALLOW_DELAY`).
    fn capture_permission_armed(&self) -> bool {
        self.capture_permissions
            .shown
            .as_ref()
            .is_some_and(Shown::armed)
    }

    fn take_capture_permission(&mut self) -> Option<Shown> {
        let shown = self.capture_permissions.shown.take()?;
        self.capture_permissions.pending_text = None;
        *self.capture_permissions.buffers.borrow_mut() = Buffers::default();
        self.queue_redraw();
        Some(shown)
    }

    fn after_capture_permission_closed(&mut self) {
        self.show_capture_permission();
        if !self.capture_permission_open() {
            self.show_share_picker();
        }
    }

    /// User clicked Allow or pressed Enter: record the grant, until Pila
    /// exits, and proceed with the pending captures.
    fn allow_capture_permission(&mut self) {
        let Some(shown) = self.take_capture_permission() else {
            return;
        };
        match &shown.executable {
            Some(exe) => {
                info!(executable = exe.as_str(), "capture permission allowed");
                self.record_capture_grant(exe.clone());
            }
            None => {
                if let PromptKey::Client(id) = &shown.key {
                    self.capture_permissions.client_grants.insert(id.clone());
                }
                info!("capture permission allowed for unknown client (this session)");
            }
        }
        for request in shown.requests {
            if request.is_alive() {
                self.approve_capture(request);
            }
        }
        self.after_capture_permission_closed();
    }

    /// User clicked Deny or pressed Escape: fail/stop the pending captures.
    fn deny_capture_permission(&mut self) {
        let Some(shown) = self.take_capture_permission() else {
            return;
        };
        info!(
            executable = shown.executable.as_deref().unwrap_or("?"),
            "capture permission denied"
        );
        // Not asked again on these connections; those gone are forgotten.
        let dh = self.display_handle.backend_handle();
        let denials = &mut self.capture_permissions.client_denials;
        denials.retain(|id| dh.get_client_data(id.clone()).is_ok());
        denials.extend(shown.requests.iter().filter_map(PendingCapture::client_id));
        for request in shown.requests {
            if request.is_alive() {
                self.deny_capture(request);
            }
        }
        self.after_capture_permission_closed();
    }

    /// Cancel the open and queued capture prompts (when the session locks).
    /// Requests still waiting for an executable aren't decided yet: they're
    /// decided once it's read, and those that would ask are refused then,
    /// while locked (`decide_capture`).
    pub fn cancel_capture_permissions(&mut self) {
        let waiting = std::mem::take(&mut self.capture_permissions.waiting);
        for queued in waiting {
            for request in queued.requests {
                if request.is_alive() {
                    self.deny_capture(request);
                }
            }
        }
        if let Some(shown) = self.take_capture_permission() {
            info!(
                executable = shown.executable.as_deref().unwrap_or("?"),
                "capture permission cancelled"
            );
            for request in shown.requests {
                if request.is_alive() {
                    self.deny_capture(request);
                }
            }
        }
    }

    /// A client destroyed an ext-image-copy-capture session while it may have
    /// been awaiting permission.
    pub(crate) fn cancel_pending_capture_session(
        &mut self,
        session: &ExtImageCopyCaptureSessionV1,
    ) {
        self.remove_pending_capture(|r| r.matches_session(session));
    }

    /// A client destroyed a wlr-screencopy frame while it may have been
    /// awaiting permission.
    pub(crate) fn cancel_pending_screencopy(&mut self, frame: &ZwlrScreencopyFrameV1) {
        self.remove_pending_capture(|r| r.matches_screencopy(frame));
    }

    fn remove_pending_capture(&mut self, matches: impl Fn(&PendingCapture) -> bool) {
        for requests in self.capture_permissions.resolving.values_mut() {
            requests.retain(|r| !matches(r));
        }
        self.capture_permissions
            .resolving
            .retain(|_, requests| !requests.is_empty());
        for queued in &mut self.capture_permissions.waiting {
            queued.requests.retain(|r| !matches(r));
        }
        self.capture_permissions
            .waiting
            .retain(|q| !q.requests.is_empty());
        let empty = self
            .capture_permissions
            .shown
            .as_mut()
            .is_some_and(|shown| {
                shown.requests.retain(|r| !matches(r));
                shown.requests.is_empty()
            });
        if empty {
            self.take_capture_permission();
            self.after_capture_permission_closed();
        }
    }

    fn approve_capture(&mut self, request: PendingCapture) {
        match request {
            PendingCapture::Session(resource) => self.authorize_capture_session(&resource),
            PendingCapture::Screencopy(frame) => self.enqueue_screencopy(&frame),
        }
    }

    fn deny_capture(&mut self, request: PendingCapture) {
        match request {
            PendingCapture::Session(resource) => self.deny_capture_session(&resource),
            PendingCapture::Screencopy(frame) => {
                if frame.is_alive() {
                    frame.failed();
                }
            }
        }
    }

    fn record_capture_grant(&mut self, executable: String) {
        let grants = &mut self.capture_permissions.grants;
        if grants.iter().any(|g| g.executable == executable) {
            return;
        }
        let id = state_file::random_id().unwrap_or_else(|| format!("grant-{}", grants.len() + 1));
        grants.push(Grant { id, executable });
    }

    /// Re-evaluate open/queued prompts and active captures when `[permissions]`
    /// or `[[permission]]` changed on config reload.
    pub(crate) fn permissions_config_changed(&mut self) {
        if !self.config.permissions.enforce {
            let waiting = std::mem::take(&mut self.capture_permissions.waiting);
            for queued in waiting {
                for request in queued.requests {
                    if request.is_alive() {
                        self.approve_capture(request);
                    }
                }
            }
            if let Some(shown) = self.take_capture_permission() {
                for request in shown.requests {
                    if request.is_alive() {
                        self.approve_capture(request);
                    }
                }
                self.after_capture_permission_closed();
            }
            return;
        }

        // Re-evaluate currently open prompt against the updated rules.
        if let Some(decision) = self.capture_permissions.shown.as_ref().map(|shown| {
            evaluate(
                &self.config.permission_rules,
                &self.capture_permissions.grants,
                shown.executable.as_deref(),
            )
        }) {
            match decision {
                Decision::Allow => {
                    if let Some(shown) = self.take_capture_permission() {
                        for request in shown.requests {
                            if request.is_alive() {
                                self.approve_capture(request);
                            }
                        }
                        self.after_capture_permission_closed();
                    }
                }
                Decision::Deny => self.deny_capture_permission(),
                Decision::Ask => {}
            }
        }

        // Re-evaluate queued prompts.
        let waiting = std::mem::take(&mut self.capture_permissions.waiting);
        for queued in waiting {
            match evaluate(
                &self.config.permission_rules,
                &self.capture_permissions.grants,
                queued.executable.as_deref(),
            ) {
                Decision::Allow => {
                    for request in queued.requests {
                        if request.is_alive() {
                            self.approve_capture(request);
                        }
                    }
                }
                Decision::Deny => {
                    for request in queued.requests {
                        if request.is_alive() {
                            self.deny_capture(request);
                        }
                    }
                }
                Decision::Ask => self.capture_permissions.waiting.push_back(queued),
            }
        }
        self.show_capture_permission();

        // Stop any running captures whose executable is now explicitly denied.
        self.stop_denied_captures();
    }

    /// Stop active capture sessions / pending screencopies whose client is no
    /// longer allowed (after forgetting a grant or adding a `deny` rule).
    fn stop_denied_captures(&mut self) {
        if !self.config.permissions.enforce {
            return;
        }
        let rules = &self.config.permission_rules;
        let grants = &self.capture_permissions.grants;
        let client_grants = &self.capture_permissions.client_grants;
        // Whether the client of `id` may capture: `None` while its
        // executable isn't known yet (it captured while `enforce` was off),
        // to be read and checked then (`resolve_executable`).
        let mut unresolved = Vec::new();
        let mut allowed = |dh: &smithay::reexports::wayland_server::DisplayHandle,
                           id: smithay::reexports::wayland_server::backend::ObjectId|
         -> Option<bool> {
            let client = dh.get_client(id).ok()?;
            let Some(identity) = client
                .get_data::<ClientState>()
                .and_then(|s| s.executable.get())
            else {
                unresolved.push(client);
                return None;
            };
            let exe = identity.executable.as_deref();
            Some(
                (exe.is_none() && client_grants.contains(&client.id()))
                    || evaluate(rules, grants, exe) == Decision::Allow,
            )
        };

        let mut stopped = Vec::new();
        for session in &self.capture.sessions {
            if session.is_authorized()
                && allowed(&self.display_handle, session.resource_id()) == Some(false)
            {
                stopped.push(session.resource());
            }
        }
        let dh = &self.display_handle;
        self.capture.screencopies.retain(|frame| {
            let keep = allowed(dh, frame.id()) != Some(false);
            if !keep && frame.is_alive() {
                frame.failed();
            }
            keep
        });
        for resource in stopped {
            self.deny_capture_session(&resource);
        }
        for client in unresolved {
            let entry = self.capture_permissions.resolving.entry(client.id());
            if let std::collections::hash_map::Entry::Vacant(entry) = entry {
                entry.insert(Vec::new());
                self.resolve_executable(&client);
            }
        }
    }

    /// The user stopped `client`'s captures (the tray's Stop,
    /// `Pila::stop_capture`): its sessions stop, its screencopies fail, and
    /// so do its next requests until it connects again (`stopped`).
    pub(crate) fn stop_client_captures(&mut self, client: &ClientId) {
        let dh = self.display_handle.backend_handle();
        let stopped = &mut self.capture_permissions.stopped;
        stopped.retain(|id| dh.get_client_data(id.clone()).is_ok());
        stopped.insert(client.clone());
        let mine = |owner: Option<Client>| owner.is_some_and(|c| c.id() == *client);
        let sessions: Vec<_> = self
            .capture
            .sessions
            .iter()
            .map(|session| session.resource())
            .filter(|resource| mine(resource.client()))
            .collect();
        for resource in sessions {
            self.deny_capture_session(&resource);
        }
        self.capture.screencopies.retain(|frame| {
            let stop = mine(frame.client());
            if stop && frame.is_alive() {
                frame.failed();
            }
            !stop
        });
    }

    // --- IPC (`ipc.rs`) -----------------------------------------------------

    /// The prompt's grants for `pila msg capture-permissions` (`persistent`,
    /// always `false` since only the config keeps one, stays for scripts).
    pub(crate) fn ipc_capture_permissions(&self) -> Vec<Value> {
        let mut grants = self.capture_permissions.grants.clone();
        grants.sort_by(|a, b| a.executable.cmp(&b.executable).then(a.id.cmp(&b.id)));
        grants
            .into_iter()
            .map(|g| {
                json!({
                    "id": g.id,
                    "executable": g.executable,
                    "persistent": false,
                })
            })
            .collect()
    }

    /// Forget one capture permission by `id`. Returns
    /// `true` if one was found and removed.
    pub(crate) fn forget_capture_permission(&mut self, id: &str) -> bool {
        let Some(pos) = self
            .capture_permissions
            .grants
            .iter()
            .position(|g| g.id == id)
        else {
            return false;
        };
        let removed = self.capture_permissions.grants.remove(pos);
        info!(
            id = removed.id,
            executable = removed.executable,
            "forgot capture permission"
        );
        self.stop_denied_captures();
        true
    }

    /// Forget all capture permissions. Returns how many were removed.
    pub(crate) fn forget_capture_permissions(&mut self) -> usize {
        // What `capture-permissions` lists (grants to clients of no known
        // executable go too, unlisted).
        let count = self.capture_permissions.grants.len();
        self.capture_permissions.grants.clear();
        self.capture_permissions.client_grants.clear();
        info!(count, "forgot all capture permissions");
        self.stop_denied_captures();
        count
    }

    // --- Input (`input.rs`) -------------------------------------------------

    /// Key press while the prompt is open (modal: takes every key).
    pub fn capture_permission_key(&mut self, sym: Keysym) {
        if self.capture_permissions.shown.is_none() {
            return;
        }
        match sym {
            Keysym::Return | Keysym::KP_Enter if self.capture_permission_armed() => {
                self.allow_capture_permission()
            }
            Keysym::Escape => self.deny_capture_permission(),
            _ => {}
        }
    }

    /// Pointer press at `location` while the prompt is open (modal: takes every
    /// click).
    pub fn capture_permission_press(&mut self, location: Point<f64, Logical>) {
        let Some(view) = self.scene_capture_permission() else {
            return;
        };
        let layout = &view.layout;
        if layout.allow.to_f64().contains(location) {
            if self.capture_permission_armed() {
                self.allow_capture_permission();
            }
        } else if layout.deny.to_f64().contains(location) {
            self.deny_capture_permission();
        }
    }

    // --- Scene & Rendering (`render.rs`, `panel.rs`) ------------------------

    /// The prompt for `Pila::scene`, while it's open.
    pub(crate) fn scene_capture_permission(&self) -> Option<ScenePrompt> {
        let shown = self.capture_permissions.shown.as_ref()?;
        let output = self
            .space
            .outputs()
            .find(|o| o.name() == shown.output)
            .or_else(|| self.space.outputs().next())?;
        let area = self.space.output_geometry(output)?;
        Some(ScenePrompt {
            output: output.name(),
            layout: layout(area),
            rounding: panel::rounding(&self.config),
            buffers: self.capture_permissions.buffers.clone(),
        })
    }

    /// Render the prompt's text on a background thread when needed.
    pub(crate) fn draw_capture_permission_text(&mut self) {
        let (Some(view), Some(shown)) = (
            self.scene_capture_permission(),
            self.capture_permissions.shown.as_ref(),
        ) else {
            return;
        };
        let scale = self.panel_text_scale(&view.output);
        let key = TextKey {
            items: items(shown, &view.layout, &self.config.colors),
            caps: panel::key_caps(&self.config.colors),
            size: (view.layout.panel.size.w, view.layout.panel.size.h),
            scale,
        };
        let drawn = view.buffers.borrow().text.as_ref().map(|t| &t.key) == Some(&key);
        self.draw_panel_text(
            key,
            drawn,
            "pila-permission-text",
            |state| &mut state.capture_permissions.pending_text,
            Self::draw_capture_permission_text,
            |state, text| {
                state.capture_permissions.buffers.borrow_mut().text = Some(text);
                // Seen from its next frame: Allow counts from now.
                if let Some(shown) = state.capture_permissions.shown.as_mut() {
                    shown.visible_since.get_or_insert_with(Instant::now);
                }
                state.queue_redraw();
            },
        );
    }

    /// State dump for automated tests (`introspect.rs`).
    pub fn capture_permission_json(&self) -> Value {
        let rect = |r: Rectangle<i32, Logical>| json!({ "x": r.loc.x, "y": r.loc.y, "w": r.size.w, "h": r.size.h });
        let shown = self
            .capture_permissions
            .shown
            .as_ref()
            .zip(self.scene_capture_permission())
            .map(|(shown, view)| {
                json!({
                    "executable": shown.executable,
                    "title": title(shown.executable.as_deref()),
                    "output": view.output,
                    "requests": shown.requests.len(),
                    "panel": rect(view.layout.panel),
                    "allow": rect(view.layout.allow),
                    "deny": rect(view.layout.deny),
                    "text": view.buffers.borrow().text.is_some(),
                    "armed": shown.armed(),
                })
            });
        json!({
            "enforce": self.config.permissions.enforce,
            "shown": shown,
            "waiting": self.capture_permissions.waiting.len(),
            "grants": self.ipc_capture_permissions(),
        })
    }
}

fn items(shown: &Shown, layout: &Layout, colors: &Colors) -> Vec<Item> {
    let (text, dimmed, _) = panel::text_colors(colors);
    let width = layout.panel.size.w;
    let title = title(shown.executable.as_deref());
    let path_line = shown
        .executable
        .clone()
        .unwrap_or_else(|| "Executable path unavailable".to_string());
    // A program allowed may capture anything, whatever it asked for first.
    let detail = "It will be able to see everything on your screen, and any window.";
    // Only the config keeps a permission past the session.
    let lasts = "Allowed until you log out. To always allow it, add a [[permission]] rule.";
    let mut items = vec![
        panel::title(&title, width, colors),
        Item::Text {
            text: path_line,
            at: (panel::PADDING, PATH_AT),
            w: width - 2 * panel::PADDING,
            h: NOTE_HEIGHT,
            font: panel::NOTE,
            align: Align::Left,
            color: dimmed,
        },
        Item::Text {
            text: detail.into(),
            at: (panel::PADDING, NOTE_AT),
            w: width - 2 * panel::PADDING,
            h: NOTE_HEIGHT,
            font: panel::NOTE,
            align: Align::Left,
            color: dimmed,
        },
        Item::Text {
            text: lasts.into(),
            at: (panel::PADDING, LASTS_AT),
            w: width - 2 * panel::PADDING,
            h: NOTE_HEIGHT,
            font: panel::NOTE,
            align: Align::Left,
            color: dimmed,
        },
    ];

    let local =
        |r: Rectangle<i32, Logical>| (r.loc.x - layout.panel.loc.x, r.loc.y - layout.panel.loc.y);

    for (rect, label, color) in [
        (layout.allow, "Allow", panel::bits(colors.hover_icon)),
        (layout.deny, "Deny", text),
    ] {
        let (x, y) = local(rect);
        items.push(Item::Text {
            text: label.into(),
            at: (x, y + (BUTTON_HEIGHT - 20) / 2),
            w: rect.size.w,
            h: 20,
            font: panel::BODY,
            align: Align::Center,
            color,
        });
    }
    items
}

/// The prompt's render elements for `output_name`, front to back: the panel on
/// its output once its text is drawn, and the modal dim on every output.
pub fn elements(
    renderer: &mut GlesRenderer,
    view: &ScenePrompt,
    colors: &Colors,
    output_name: &str,
    output_geo: Rectangle<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement> {
    let origin = output_geo.loc;
    let mut buffers = view.buffers.borrow_mut();
    let buffers = &mut *buffers;
    let dim = panel::dim(
        &mut buffers.dims,
        output_name,
        output_geo,
        1.0,
        origin,
        scale,
    );
    let Some(text) = buffers.text.as_mut().filter(|_| output_name == view.output) else {
        return vec![dim];
    };
    let layout = &view.layout;
    let mut elements: Vec<PilaRenderElement> = text
        .element(renderer, layout.panel, origin, scale)
        .into_iter()
        .collect();
    elements.extend(panel::solids(
        renderer,
        &mut buffers.solids,
        panel::buttons(layout.allow, layout.deny, colors),
        origin,
        scale,
    ));
    elements.extend(panel::frame(
        renderer,
        &mut buffers.frame,
        layout.panel,
        view.rounding,
        colors,
        origin,
        scale,
    ));
    elements.push(dim);
    elements
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(executable: &str, access: PermissionAccess) -> PermissionRule {
        PermissionRule {
            executable: executable.to_string(),
            access,
        }
    }

    fn grant(id: &str, executable: &str) -> Grant {
        Grant {
            id: id.to_string(),
            executable: executable.to_string(),
        }
    }

    #[test]
    fn rules_apply_in_order_and_override_grants() {
        let rules = [
            rule("*", PermissionAccess::Deny),
            rule("/usr/bin/grim", PermissionAccess::Allow),
            rule("/usr/bin/wf-*", PermissionAccess::Ask),
        ];
        let grants = [
            grant("1", "/usr/bin/wf-recorder"),
            grant("2", "/usr/bin/secret"),
        ];
        // Explicitly allowed by rule.
        assert_eq!(
            evaluate(&rules, &grants, Some("/usr/bin/grim")),
            Decision::Allow
        );
        // `ask` rule honors a grant from the prompt.
        assert_eq!(
            evaluate(&rules, &grants, Some("/usr/bin/wf-recorder")),
            Decision::Allow
        );
        // `ask` rule without grant prompts.
        assert_eq!(
            evaluate(&rules, &grants, Some("/usr/bin/wf-other")),
            Decision::Ask
        );
        // `deny` rule overrides even a grant from the prompt (a rule
        // reloaded after it was given).
        assert_eq!(
            evaluate(&rules, &grants, Some("/usr/bin/secret")),
            Decision::Deny
        );
        // Unknown executable always asks (never matches wildcards or grants).
        assert_eq!(evaluate(&rules, &grants, None), Decision::Ask);
    }

    #[test]
    fn prompt_layout_fits_inside_panel_without_overlapping_buttons() {
        let narrow = Rectangle::new((0, 0).into(), (480, 800).into());
        let l = layout(narrow);
        assert!(narrow.contains_rect(l.panel));
        assert!(l.panel.contains_rect(l.deny) && !l.allow.overlaps(l.deny));
        let area = Rectangle::new((0, 0).into(), (1280, 800).into());
        let l = layout(area);
        assert!(area.contains_rect(l.panel));
        assert!(l.panel.contains_rect(l.allow));
        assert!(l.panel.contains_rect(l.deny));
        assert!(!l.allow.overlaps(l.deny));
        // The buttons sit below the notes.
        assert!(l.allow.loc.y >= l.panel.loc.y + LASTS_AT + NOTE_HEIGHT);
    }

    /// The question fits the prompt whole for a long program name (it used
    /// to end in "…" for one as long as wf-recorder).
    #[test]
    fn the_question_fits_for_a_long_program_name() {
        let style = crate::text::Style {
            size: f32::from(panel::TITLE.size),
            bold: panel::TITLE.bold,
        };
        for question in [title(Some("/usr/bin/gpu-screen-recorder")), title(None)] {
            let Some(fits) = crate::text::fits(&question, style, WIDTH - 2 * panel::PADDING) else {
                eprintln!("no fonts: skipped");
                return;
            };
            assert!(fits, "{question:?} is cut");
        }
    }

    #[test]
    fn a_replaced_executable_keeps_its_path() {
        assert_eq!(
            executable_path(Path::new("/usr/bin/wf-recorder (deleted)")),
            "/usr/bin/wf-recorder"
        );
        assert_eq!(executable_path(Path::new("/usr/bin/grim")), "/usr/bin/grim");
    }

    #[test]
    fn program_name_extracts_basename() {
        assert_eq!(program_name("/usr/bin/grim"), "grim");
        assert_eq!(program_name("grim"), "grim");
        assert_eq!(program_name("/"), "/");
    }
}
