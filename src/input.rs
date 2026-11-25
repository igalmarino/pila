use std::time::Duration;

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        InputTime, KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
        PointerMotionEvent,
    },
    desktop::Window,
    input::{
        keyboard::{FilterResult, Keycode, Keysym},
        pointer::{
            AxisFrame, ButtonEvent, CursorIcon, CursorImageStatus,
            GrabStartData as PointerGrabStartData, MotionEvent, RelativeMotionEvent,
        },
    },
    reexports::calloop::timer::{TimeoutAction, Timer},
    utils::{Logical, Point, Serial, SERIAL_COUNTER},
    wayland::{
        keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitorSeat, shell::wlr_layer::Layer,
    },
};

use crate::{
    config::{Action, WheelDirection},
    decoration::{Decoration, TitleBarPart},
    grabs::resize_grab::ResizeEdge,
    pointer_constraints::Constraint,
    state::{fill_state, Hit, ModTap, Pila, Under},
};

// Linux input event codes (linux/input-event-codes.h).
const BTN_LEFT: u32 = 0x110;
pub(crate) const BTN_RIGHT: u32 = 0x111;

/// See `Pila::motion_watch`.
#[derive(PartialEq)]
struct MotionWatch {
    keyboard_focus: Option<smithay::reexports::wayland_server::protocol::wl_surface::WlSurface>,
    output: Option<smithay::output::Output>,
}

impl Pila {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>)
    where
        I::Device: 'static,
    {
        // Activity for the idle notifier, told after the events
        // (`after_frames`).
        self.input_activity = true;
        // Whatever the input changed beyond the screen is `after_frames`'.
        // The pointer moving changes little of it, and comes up to a thousand
        // times a second: only if it changes what that looks at
        // (`MotionWatch`). Moving a window draws a frame, which does it.
        let motion = matches!(
            event,
            InputEvent::PointerMotion { .. } | InputEvent::PointerMotionAbsolute { .. }
        );
        let watched = motion.then(|| self.motion_watch());
        if !motion {
            self.housekeeping = true;
        }
        let pointer_from = self.pointer_location();
        // After `power-off-monitors` any input brings them back; a key or a
        // click that does is kept from apps (`on_keyboard`, `pointer_button`).
        if matches!(
            event,
            InputEvent::PointerMotion { .. }
                | InputEvent::PointerMotionAbsolute { .. }
                | InputEvent::PointerAxis { .. }
                | InputEvent::GestureSwipeBegin { .. }
                | InputEvent::GesturePinchBegin { .. }
                | InputEvent::GestureHoldBegin { .. }
        ) {
            self.wake_monitors();
        }
        // A tap of Mod is Mod alone: a click (Mod+drag), a scroll or a
        // gesture while it's held means it wasn't one. Moving doesn't count.
        if matches!(
            event,
            InputEvent::PointerButton { .. }
                | InputEvent::PointerAxis { .. }
                | InputEvent::GestureSwipeBegin { .. }
                | InputEvent::GesturePinchBegin { .. }
                | InputEvent::GestureHoldBegin { .. }
                | InputEvent::TouchDown { .. }
        ) {
            self.input.mod_tap = None;
        }
        // A gesture's begin and end; its updates come at the touchpad's rate.
        let gesture = matches!(
            event,
            InputEvent::GestureSwipeBegin { .. }
                | InputEvent::GestureSwipeEnd { .. }
                | InputEvent::GesturePinchBegin { .. }
                | InputEvent::GesturePinchEnd { .. }
                | InputEvent::GestureHoldBegin { .. }
                | InputEvent::GestureHoldEnd { .. }
        );
        // What it redraws (`redraw.rs`): a key or a scroll a client gets is
        // the client's to show (it commits), as is a gesture's update; a key
        // Pila acts on, a click, a swipe step Pila acts on or a gesture's
        // begin or end may change anything; the pointer moving, the outputs it
        // was and is on.
        match event {
            InputEvent::Keyboard { event, .. } => {
                if self.on_keyboard::<I>(event) {
                    self.queue_redraw();
                }
            }
            InputEvent::PointerMotion { event, .. } => {
                self.on_pointer_motion::<I>(event);
                self.redraw_pointer_moved(pointer_from);
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                self.on_pointer_motion_absolute::<I>(event);
                self.redraw_pointer_moved(pointer_from);
            }
            InputEvent::PointerButton { event, .. } => {
                self.on_pointer_button::<I>(event);
                self.queue_redraw();
            }
            InputEvent::PointerAxis { event, .. } => self.on_pointer_axis::<I>(event),
            // Touchpad gestures (`gestures.rs`).
            InputEvent::GestureSwipeBegin { event, .. } => self.on_gesture_swipe_begin::<I>(event),
            InputEvent::GestureSwipeUpdate { event, .. } => {
                if self.on_gesture_swipe_update::<I>(event) {
                    self.queue_redraw();
                }
            }
            InputEvent::GestureSwipeEnd { event, .. } => self.on_gesture_swipe_end::<I>(event),
            InputEvent::GesturePinchBegin { event, .. } => self.on_gesture_pinch_begin::<I>(event),
            InputEvent::GesturePinchUpdate { event, .. } => {
                self.on_gesture_pinch_update::<I>(event)
            }
            InputEvent::GesturePinchEnd { event, .. } => self.on_gesture_pinch_end::<I>(event),
            InputEvent::GestureHoldBegin { event, .. } => self.on_gesture_hold_begin::<I>(event),
            InputEvent::GestureHoldEnd { event, .. } => self.on_gesture_hold_end::<I>(event),
            _ => {}
        }
        if gesture {
            self.queue_redraw();
        }
        if let Some(watched) = watched {
            if self.motion_watch() != watched {
                self.housekeeping = true;
            }
        }
    }

    /// What the pointer moving may change of what `after_frames`' housekeeping
    /// looks at: the keyboard focus (taskbars, IPC) and the output under the
    /// pointer (IPC's focused output).
    fn motion_watch(&self) -> MotionWatch {
        MotionWatch {
            keyboard_focus: self.keyboard_focus(),
            output: self
                .space
                .output_under(self.pointer_location())
                .next()
                .cloned(),
        }
    }

    /// The pointer moved from `from` (or didn't): redraw where it was and
    /// is. Something there surely moved if Pila draws the cursor or a drag's
    /// icon; else only a title bar button's highlight may change.
    fn redraw_pointer_moved(&mut self, from: Point<f64, Logical>) {
        let to = self.pointer_location();
        if to == from {
            return;
        }
        let drawn = self.backend.as_ref().is_some_and(|backend| {
            backend.draws_cursor(&self.cursor_status, self.shake_cursor.magnified())
        });
        let content = if drawn || self.dnd_icon.is_some() {
            crate::redraw::Content::Sure
        } else {
            crate::redraw::Content::Maybe
        };
        self.queue_redraw_pointer(from, to, content);
    }

    /// A key: Pila's (a binding, the window menu, ...) or the client's.
    /// Whether Pila acted on it (else the client with focus got it).
    fn on_keyboard<I: InputBackend>(&mut self, event: I::KeyboardKeyEvent) -> bool
    where
        I::Device: 'static,
    {
        let serial = SERIAL_COUNTER.next_serial();
        let time = Event::time(&event);
        let keycode = event.key_code();
        let key_state = event.state();
        let Some(keyboard) = self.seat.get_keyboard() else {
            return false;
        };
        // Keys of a virtual keyboard (`virtual_keyboard.rs`) are tracked apart
        // from the physical ones; a physical key brings the configured keymap
        // back.
        let device = event.device();
        let source = match crate::virtual_keyboard::virtual_device(&device) {
            Some(device) => device.source(),
            None => {
                self.restore_configured_keymap();
                smithay::input::keyboard::KeyboardSource::MAIN
            }
        };
        let inhibited = self.shortcuts_inhibited();

        // What the filter leaves for after the key was handled: a press for
        // the window menu, a binding's repeat to start, a tap's action (run
        // once the client got the release).
        let mut menu_key = None;
        // A key that hides the shortcuts cheatsheet (`shortcuts_view.rs`).
        let mut hide_shortcuts = false;
        let mut picker_key = None;
        let mut pick_key = None;
        let mut shortcuts_key = None;
        let mut perm_key = None;
        let mut overview_key = None;
        let mut repeat_next = None;
        let mut tap_action = None;
        // `input_from_source`, but while locked a key that isn't Pila's goes
        // to the lock surface even if an input method holds the keyboard: it
        // mustn't see what's typed there (`input_method.rs`).
        let intercepted = keyboard.input_intercept::<FilterResult<Option<Action>>, _>(
            source,
            self,
            keycode,
            key_state,
            |state, mods, handle| {
                // A repeating binding stops when its key is released, or
                // another is pressed (xkb repeats the last key pressed).
                if let Some(repeating) = state.input.binding_repeat.as_ref().map(|r| r.keycode) {
                    let pressed = key_state == KeyState::Pressed;
                    if (pressed && repeating != keycode) || (!pressed && repeating == keycode) {
                        state.stop_binding_repeat();
                    }
                }
                // Any other key pressed means a held modifier isn't tapped
                // alone (a repeat of its own press leaves it).
                if key_state == KeyState::Pressed
                    && state
                        .input
                        .mod_tap
                        .is_some_and(|tap| tap.keycode != keycode)
                {
                    state.input.mod_tap = None;
                }
                // A key that wakes the monitors does only that.
                if key_state == KeyState::Pressed && state.wake_monitors() {
                    state.input.suppressed_keys.insert(keycode);
                    return FilterResult::Intercept(None);
                }
                // Ctrl+Alt+F<N>: the keymap turns it into XF86_Switch_VT_<N>.
                if key_state == KeyState::Pressed {
                    let raw = handle.modified_sym().raw();
                    let first = Keysym::XF86_Switch_VT_1.raw();
                    if (first..=Keysym::XF86_Switch_VT_12.raw()).contains(&raw) {
                        state.input.suppressed_keys.insert(keycode);
                        return FilterResult::Intercept(Some(Action::ChangeVt(
                            (raw - first + 1) as i32,
                        )));
                    }
                }
                // Locking closes Pila's panels; were one left open, the
                // lock screen's keys still wouldn't reach it.
                // The panel confirming an app's shortcuts takes every press
                // but a modifier's (they make up the keys it's given): it's
                // modal (`app_shortcuts.rs`).
                if key_state == KeyState::Pressed
                    && !state.is_locked()
                    && state.app_shortcuts_open()
                {
                    state.input.suppressed_keys.insert(keycode);
                    let sym = handle.raw_latin_sym_or_raw_current_sym();
                    if let Some(sym) = sym.filter(|sym| !sym.is_modifier_key()) {
                        shortcuts_key = Some((sym, *mods));
                    }
                    return FilterResult::Intercept(None);
                }
                // The color picker takes every press while it's on: it's
                // modal (`color_pick.rs`).
                if key_state == KeyState::Pressed && !state.is_locked() && state.color_pick_open() {
                    state.input.suppressed_keys.insert(keycode);
                    pick_key = Some(handle.modified_sym());
                    return FilterResult::Intercept(None);
                }
                // The screen capture permission prompt takes every press while
                // it's shown: it's modal (`permissions.rs`).
                if key_state == KeyState::Pressed
                    && !state.is_locked()
                    && state.capture_permission_open()
                {
                    state.input.suppressed_keys.insert(keycode);
                    perm_key = Some(handle.modified_sym());
                    return FilterResult::Intercept(None);
                }
                // The share picker takes every press while it's shown: it's
                // modal (`share_picker.rs`).
                if key_state == KeyState::Pressed && !state.is_locked() && state.share_picker_open()
                {
                    state.input.suppressed_keys.insert(keycode);
                    picker_key = Some(handle.modified_sym());
                    return FilterResult::Intercept(None);
                }
                // The window menu takes every press while it's open.
                if key_state == KeyState::Pressed
                    && !state.is_locked()
                    && state.window_menu.is_some()
                {
                    state.input.suppressed_keys.insert(keycode);
                    menu_key = Some(handle.modified_sym());
                    return FilterResult::Intercept(None);
                }
                // While locked, everything goes to the locker; debug builds
                // keep Quit to escape a nested session. A client inhibiting
                // shortcuts gets them, but never Quit.
                let allowed = |state: &Pila, action: &Action| {
                    (!state.is_locked() || (cfg!(debug_assertions) && *action == Action::Quit))
                        && (!inhibited || *action == Action::Quit)
                };
                let sym = handle.raw_latin_sym_or_raw_current_sym();
                match key_state {
                    // A modifier alone may be a tap, run on its release; the
                    // client sees it pressed meanwhile, as any modifier.
                    KeyState::Pressed
                        if sym.is_some_and(|sym| crate::config::tap_sym(sym).is_some()) =>
                    {
                        if state.input.mod_tap.is_none() {
                            state.input.mod_tap = sym.map(|sym| ModTap {
                                keycode,
                                sym,
                                mods: *mods,
                            });
                        }
                    }
                    // Any key can be bound (media keys need no Mod), so
                    // every press is looked up.
                    KeyState::Pressed => {
                        let action = sym
                            .and_then(|sym| state.config.action_for(sym, mods))
                            .filter(|action| allowed(state, action));
                        // The cheatsheet goes with any key but a modifier;
                        // a binding still runs (`show-shortcuts` hides it
                        // itself), any other key goes nowhere else.
                        if state.shortcuts_shown() && !handle.modified_sym().is_modifier_key() {
                            hide_shortcuts = action != Some(Action::ShowShortcuts);
                            if action.is_none() {
                                state.input.suppressed_keys.insert(keycode);
                                return FilterResult::Intercept(None);
                            }
                        }
                        // The overview takes every key that's no binding
                        // (bindings close it or work in it: `run_action`).
                        if action.is_none()
                            && state.overview_open()
                            && !handle.modified_sym().is_modifier_key()
                        {
                            state.input.suppressed_keys.insert(keycode);
                            overview_key = Some(handle.modified_sym());
                            return FilterResult::Intercept(None);
                        }
                        if let Some(action) = action {
                            state.input.suppressed_keys.insert(keycode);
                            if sym.is_some_and(|sym| state.config.repeats(sym, mods)) {
                                repeat_next = Some((keycode, action.clone()));
                            }
                            return FilterResult::Intercept(Some(action));
                        }
                        // An app's global shortcut goes to its session, not
                        // to the window with the keyboard; Pila's bindings
                        // came first, and an app inhibiting shortcuts keeps
                        // them (`app_shortcuts.rs`).
                        let shortcut = sym
                            .filter(|_| !inhibited)
                            .and_then(|sym| state.app_shortcut_for(sym, mods));
                        if let Some(shortcut) = shortcut {
                            state.input.suppressed_keys.insert(keycode);
                            state.app_shortcut_pressed(keycode.raw(), shortcut);
                            return FilterResult::Intercept(None);
                        }
                    }
                    // Don't send the client a release for a press it never saw
                    // (an app's shortcut's ends with it).
                    KeyState::Released if state.input.suppressed_keys.remove(&keycode) => {
                        state.app_shortcut_released(keycode.raw());
                        return FilterResult::Intercept(None);
                    }
                    // A modifier released with nothing since its press: its
                    // tap, after the client gets the release.
                    KeyState::Released
                        if state
                            .input
                            .mod_tap
                            .is_some_and(|tap| tap.keycode == keycode) =>
                    {
                        tap_action = state
                            .input
                            .mod_tap
                            .take()
                            .and_then(|tap| state.config.tap_action(tap.sym, &tap.mods))
                            .filter(|action| allowed(state, action));
                    }
                    _ => {}
                }
                FilterResult::Forward
            },
        );
        let action = match intercepted {
            // Absorbed: another source holds the key.
            None => None,
            Some((FilterResult::Intercept(action), _)) => Some(action),
            Some((FilterResult::Forward, mods_changed)) => {
                self.forward_key(&keyboard, keycode, key_state, serial, time, mods_changed);
                None
            }
        };

        // Intercepted (`Some`), or forwarded to the client.
        let mut acted = action.is_some();
        if let Some(sym) = menu_key {
            self.window_menu_key(sym);
        }
        if let Some(sym) = picker_key {
            self.share_picker_key(sym);
        }
        if let Some(sym) = pick_key {
            self.color_pick_key(sym);
        }
        if let Some((sym, mods)) = shortcuts_key {
            self.app_shortcuts_key(sym, &mods);
        }
        if let Some(sym) = perm_key {
            self.capture_permission_key(sym);
        }
        if hide_shortcuts {
            self.hide_shortcuts();
        }
        if let Some(sym) = overview_key {
            self.overview_key(sym);
        }
        if let Some(action) = action.flatten() {
            self.run_action(action, serial);
        }
        if let Some((keycode, action)) = repeat_next {
            self.start_binding_repeat(keycode, action);
        }
        if let Some(action) = tap_action {
            if action != Action::ShowShortcuts {
                self.hide_shortcuts();
            }
            self.run_action(action, serial);
            acted = true;
        }
        // Mod+Tab ends once Ctrl/Alt/Super are all released.
        if self.windows.switch.is_some() {
            let mods = keyboard.modifier_state();
            if !(mods.ctrl || mods.alt || mods.logo) {
                self.end_switch();
                acted = true;
            }
        }
        acted
    }

    /// A binding with `repeat = true` was pressed and has run: run it again
    /// while its key is held, after the keyboard's repeat delay and at its
    /// rate, as xkb repeats a key for a client. Its release, another key's
    /// press, `release_all_keys` or the lock stop it.
    fn start_binding_repeat(&mut self, keycode: Keycode, action: Action) {
        self.stop_binding_repeat();
        let kb = &self.config.keyboard;
        // A rate of 0 turns repeat off, as for clients.
        if kb.repeat_rate <= 0 {
            return;
        }
        let delay = Duration::from_millis(kb.repeat_delay.max(0) as u64);
        let every = Duration::from_secs_f64(1.0 / f64::from(kb.repeat_rate));
        let timer = Timer::from_duration(delay);
        let inserted = self.loop_handle.insert_source(timer, move |_, _, state| {
            // The checks a press goes through (`on_keyboard`), again.
            let allowed = !state.is_locked()
                && state.window_menu.is_none()
                && (!state.shortcuts_inhibited() || action == Action::Quit);
            if !allowed {
                state.input.binding_repeat = None;
                return TimeoutAction::Drop;
            }
            // A held key is activity, though it sends nothing more.
            state.input_activity = true;
            state.input.repeat_firing = true;
            state.run_action(action.clone(), SERIAL_COUNTER.next_serial());
            state.input.repeat_firing = false;
            if std::mem::take(&mut state.input.repeat_stopped) {
                return TimeoutAction::Drop;
            }
            TimeoutAction::ToDuration(every)
        });
        match inserted {
            Ok(timer) => {
                self.input.binding_repeat = Some(crate::state::BindingRepeat { keycode, timer });
            }
            Err(err) => tracing::warn!(?err, "can't repeat a binding"),
        }
    }

    /// End a binding's repeat, if one runs.
    pub(crate) fn stop_binding_repeat(&mut self) {
        let Some(repeat) = self.input.binding_repeat.take() else {
            return;
        };
        if self.input.repeat_firing {
            // Inside its own timer: it ends itself on return.
            self.input.repeat_stopped = true;
        } else {
            self.loop_handle.remove(repeat.timer);
        }
    }

    /// Whether the surface with keyboard focus (or the window of a focused
    /// menu) has an active keyboard-shortcuts inhibitor, or its X11 app
    /// grabbed the keyboard (`xwayland::has_keyboard_grab`).
    pub(crate) fn shortcuts_inhibited(&self) -> bool {
        let Some(focus) = self.keyboard_focus() else {
            return false;
        };
        let surface = self.popup_root(&focus).unwrap_or(focus);
        self.seat
            .keyboard_shortcuts_inhibitor_for_surface(&surface)
            .is_some_and(|inhibitor| inhibitor.is_active())
            || crate::xwayland::has_keyboard_grab(&self.seat, &surface)
    }

    /// Send a key past Pila's filter on to the keyboard's grab, or to the
    /// focus. While locked, straight to the focus, the lock surface: an input
    /// method holding the keyboard mustn't see what's typed there
    /// (`input_method.rs`).
    fn forward_key(
        &mut self,
        keyboard: &smithay::input::keyboard::KeyboardHandle<Self>,
        keycode: smithay::input::keyboard::Keycode,
        key_state: KeyState,
        serial: Serial,
        time: InputTime,
        mods_changed: bool,
    ) {
        if self.is_locked() {
            keyboard.input_forward_bypassing_grab(
                self,
                keycode,
                key_state,
                serial,
                time,
                mods_changed,
            );
        } else {
            keyboard.input_forward(self, keycode, key_state, serial, time, mods_changed);
        }
    }

    /// Release every key pila thinks is held. Nested: when pila's host window
    /// loses focus, the host delivers the releases of keys held at that moment
    /// to whatever got focus instead (and Smithay's winit backend drops winit's
    /// synthetic releases), so e.g. the Alt of a host Alt+Tab would stay
    /// pressed, turning Enter into Alt+Return. Called on `WinitEvent::Focus(false)`.
    pub fn release_all_keys(&mut self) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let time = InputTime::now();
        // A modifier held as focus went (the host's Alt+Tab) wasn't tapped.
        self.input.mod_tap = None;
        self.stop_binding_repeat();
        for keycode in keyboard.pressed_keys() {
            let serial = SERIAL_COUNTER.next_serial();
            let released = keyboard.input_intercept::<bool, _>(
                smithay::input::keyboard::KeyboardSource::MAIN,
                self,
                keycode,
                KeyState::Released,
                // Like a real release: a key whose press triggered a
                // binding was never seen by the client.
                |state, _, _| !state.input.suppressed_keys.remove(&keycode),
            );
            if let Some((true, mods_changed)) = released {
                self.forward_key(
                    &keyboard,
                    keycode,
                    KeyState::Released,
                    serial,
                    time,
                    mods_changed,
                );
            }
        }
        self.input.suppressed_keys.clear();
        self.end_switch();
    }

    fn on_pointer_motion_absolute<I: InputBackend>(
        &mut self,
        event: I::PointerMotionAbsoluteEvent,
    ) {
        let Some(output) = self.space.outputs().next() else {
            return;
        };
        let Some(output_geo) = self.space.output_geometry(output) else {
            return;
        };

        let location = event.position_transformed(output_geo.size) + output_geo.loc.to_f64();

        // Nested: relative motion is how far the host pointer moved.
        let delta = self
            .input
            .host_pointer
            .map(|last| location - last)
            .unwrap_or_default();
        self.input.host_pointer = Some(location);
        self.pointer_motion(
            location,
            Point::default(),
            delta,
            delta,
            Event::time(&event),
        );
    }

    /// Relative motion (libinput): move by the delta, staying on the outputs.
    fn on_pointer_motion<I: InputBackend>(&mut self, event: I::PointerMotionEvent) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let current = pointer.current_location();
        let to = current + event.delta();
        let location = self.clamp_to_outputs(current, to);
        self.pointer_motion(
            location,
            to - location,
            event.delta(),
            event.delta_unaccel(),
            Event::time(&event),
        );
    }

    /// `to` if it's on an output, else `to` clamped to the output `from` is on.
    fn clamp_to_outputs(
        &self,
        from: Point<f64, Logical>,
        to: Point<f64, Logical>,
    ) -> Point<f64, Logical> {
        if self.space.output_under(to).next().is_some() {
            return to;
        }
        let Some(geo) = self.output_geometry_at(from) else {
            return from;
        };
        let max = (geo.loc + geo.size.to_point()).to_f64() - Point::from((1.0, 1.0));
        Point::from((
            to.x.clamp(geo.loc.x as f64, max.x),
            to.y.clamp(geo.loc.y as f64, max.y),
        ))
    }

    /// Move the pointer to `location` (after relative motion of `delta`,
    /// `pushed` past it where the outputs end), honoring the active pointer
    /// constraint. Shared by both input paths.
    fn pointer_motion(
        &mut self,
        location: Point<f64, Logical>,
        pushed: Point<f64, Logical>,
        delta: Point<f64, Logical>,
        delta_unaccel: Point<f64, Logical>,
        time: InputTime,
    ) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };

        // The constraint of the surface the pointer is on now (none during
        // pila's own move/resize grabs).
        let current = pointer.current_location();
        let under_current = self.surface_under(current);
        let constrained = self.constraint_surface_under(current);
        let constraint = match &constrained {
            Some((surface, origin)) if !pointer.is_grabbed() => {
                self.active_constraint(surface, *origin, current)
            }
            _ => Constraint::None,
        };
        pointer.relative_motion(
            self,
            under_current.clone().map(|(s, at)| (s.into(), at)),
            &RelativeMotionEvent {
                delta,
                delta_unaccel,
                time,
            },
        );
        match constraint {
            // Locked: only relative motion; pila's pointer stays put.
            Constraint::Locked => {
                pointer.frame(self);
                return;
            }
            // Confined: drop motion that would leave the surface or region.
            // (A constraint comes from a surface, `constrained`.)
            Constraint::Confined(region) => {
                let Some((surface, origin)) = constrained.as_ref() else {
                    pointer.frame(self);
                    return;
                };
                let stays = self
                    .constraint_surface_under(location)
                    .is_some_and(|(under, _)| under == *surface)
                    && region.is_none_or(|r| r.contains((location - *origin).to_i32_round()));
                if !stays {
                    pointer.frame(self);
                    return;
                }
            }
            Constraint::None => {}
        }

        let under = self.surface_under(location);
        pointer.motion(
            self,
            under.clone().map(|(s, at)| (s.into(), at)),
            &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        pointer.frame(self);
        self.shake_cursor_motion(location);
        self.overview_pointer_moved(location);
        self.color_pick_moved();
        self.share_picker_moved(location);
        self.hot_corner(location, pushed, pointer.is_grabbed());

        if !pointer.is_grabbed() {
            // Over pila's own decorations no client sets the cursor, so pila does.
            self.update_decoration_cursor(location);
            if let Some((surface, origin)) = &under {
                self.maybe_activate_constraint(surface, *origin, location);
            }
        }
    }

    /// Put the pointer at `location` (a binding, not the device), unless a client
    /// holds it where it is. Clients get it as any other motion.
    pub(crate) fn warp_pointer(&mut self, location: Point<f64, Logical>) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let current = pointer.current_location();
        if let Some((surface, origin)) = self.constraint_surface_under(current) {
            if !matches!(
                self.active_constraint(&surface, origin, current),
                Constraint::None
            ) {
                return;
            }
        }
        let under = self.surface_under(location);
        pointer.motion(
            self,
            under.clone().map(|(s, at)| (s.into(), at)),
            &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time: InputTime::now(),
            },
        );
        pointer.frame(self);
        self.update_decoration_cursor(location);
        if let Some((surface, origin)) = &under {
            self.maybe_activate_constraint(surface, *origin, location);
        }
    }

    /// Resize arrows over a window's resize border, the default arrow over a
    /// title bar or the background. Client surfaces set their own.
    fn update_decoration_cursor(&mut self, pos: Point<f64, Logical>) {
        if self.is_locked() {
            return;
        }
        let icon = match self.under(pos) {
            _ if self.color_pick_open() || self.region_selecting() => CursorIcon::Crosshair,
            Some(Under::Window(_, Hit::Surface(..)) | Under::Layer(..) | Under::Lock(..)) => return,
            Some(Under::Window(_, Hit::Decoration(Decoration::Border(edges)))) => {
                edges.cursor_icon()
            }
            _ => CursorIcon::Default,
        };
        let status = CursorImageStatus::Named(icon);
        if self.cursor_status != status {
            self.set_cursor(status);
        }
    }

    /// A left click on a title bar or resize border. The window has already
    /// been focused and raised; the client gets no pointer events here.
    fn on_decoration_press(
        &mut self,
        window: Window,
        decoration: Decoration,
        start_data: PointerGrabStartData<Pila>,
        serial: Serial,
    ) {
        // A right click on a title bar opens the window menu where it is.
        if start_data.button == BTN_RIGHT && matches!(decoration, Decoration::TitleBar(_)) {
            // Down, so the pointer (between pixels) is inside it.
            let at = start_data.location.to_i32_floor();
            self.open_window_menu(&window, at, start_data, serial);
            return;
        }
        if start_data.button != BTN_LEFT {
            return;
        }
        match decoration {
            Decoration::TitleBar(TitleBarPart::Drag) => self.begin_move(window, start_data, serial),
            Decoration::TitleBar(TitleBarPart::Maximize) => {
                let (maximized, fullscreen) = fill_state(&window);
                self.set_fill_state(&window, !maximized, fullscreen);
            }
            Decoration::TitleBar(TitleBarPart::Close) => crate::state::close_window(&window),
            Decoration::TitleBar(TitleBarPart::Minimize) => self.minimize_window(&window),
            Decoration::Border(edges) => self.begin_resize(window, edges, start_data, serial),
        }
    }

    fn on_pointer_button<I: InputBackend>(&mut self, event: I::PointerButtonEvent) {
        self.pointer_button(event.button_code(), event.state(), event.time());
    }

    /// A pointer button pressed or released: focus, pila's decorations and
    /// bindings, then the client. Also replays the right press that closed
    /// the window menu (`window_menu.rs`).
    pub(crate) fn pointer_button(
        &mut self,
        button: u32,
        button_state: ButtonState,
        time: InputTime,
    ) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        // The release of a press the overview took on a window picks or
        // drops it, and goes nowhere else.
        if button_state == ButtonState::Released
            && self.overview_release(pointer.current_location(), button)
        {
            self.input.suppressed_buttons.remove(&button);
            return;
        }
        // The release of a press the share picker took: a region dragged
        // is let go.
        if button_state == ButtonState::Released
            && self.share_picker_open()
            && self.input.suppressed_buttons.remove(&button)
        {
            self.share_picker_release();
            return;
        }
        // A click that wakes the monitors does only that, release included.
        match button_state {
            ButtonState::Pressed if self.wake_monitors() => {
                self.input.suppressed_buttons.insert(button);
                return;
            }
            ButtonState::Released if self.input.suppressed_buttons.remove(&button) => return,
            _ => {}
        }
        let serial = SERIAL_COUNTER.next_serial();
        if button_state == ButtonState::Pressed {
            let client = pointer
                .current_focus()
                .and_then(|focus| {
                    use smithay::reexports::wayland_server::Resource;
                    self.display_handle.get_client(focus.surface()?.id()).ok()
                })
                .map(|client| client.id());
            self.input.last_press = Some(crate::window_menu::LastPress {
                serial,
                button,
                location: pointer.current_location(),
                client,
                at: std::time::Instant::now(),
            });
        }
        // Locking closes Pila's panels; were one left open, clicks on the
        // lock screen still wouldn't reach it.
        // The panel confirming an app's shortcuts takes every press, and
        // its release: it's modal.
        if button_state == ButtonState::Pressed && !self.is_locked() && self.app_shortcuts_open() {
            self.input.suppressed_buttons.insert(button);
            self.app_shortcuts_press(pointer.current_location());
            return;
        }
        // The color picker takes every press, and its release: the first
        // picks the color under the pointer.
        if button_state == ButtonState::Pressed && !self.is_locked() && self.color_pick_open() {
            self.input.suppressed_buttons.insert(button);
            self.color_pick_press(pointer.current_location());
            return;
        }
        // The screen capture permission prompt takes every press, and its
        // release: it's modal.
        if button_state == ButtonState::Pressed
            && !self.is_locked()
            && self.capture_permission_open()
        {
            self.input.suppressed_buttons.insert(button);
            self.capture_permission_press(pointer.current_location());
            return;
        }
        // The share picker takes every press, and its release: it's modal.
        if button_state == ButtonState::Pressed && !self.is_locked() && self.share_picker_open() {
            self.input.suppressed_buttons.insert(button);
            self.share_picker_press(pointer.current_location());
            return;
        }
        // A click hides the shortcuts cheatsheet: on it, it does nothing else
        // (its release neither).
        if button_state == ButtonState::Pressed && self.shortcuts_shown() {
            let on_it = self.shortcuts_view_covers(pointer.current_location());
            self.hide_shortcuts();
            if on_it {
                self.input.suppressed_buttons.insert(button);
                return;
            }
        }
        // A click on a tile of the switcher ends the switch on that window,
        // and does nothing else (its release neither).
        if button_state == ButtonState::Pressed {
            if let Some(window) = self.switcher_view_window_at(pointer.current_location()) {
                self.input.suppressed_buttons.insert(button);
                self.activate_toplevel(&window);
                return;
            }
        }
        // A click picks the window itself; a Mod+Tab in progress is over.
        if button_state == ButtonState::Pressed {
            self.end_switch();
        }
        // The overview takes a click on a window or beside them, and its
        // release.
        if button_state == ButtonState::Pressed
            && !self.is_locked()
            && self.overview_press(pointer.current_location(), button)
        {
            self.input.suppressed_buttons.insert(button);
            return;
        }

        if button_state == ButtonState::Pressed && self.is_locked() {
            // Clicking an output's lock surface gives it the keyboard.
            self.focus_lock_surface(serial);
        } else if button_state == ButtonState::Pressed && !pointer.is_grabbed() {
            let location = pointer.current_location();

            match self.under(location) {
                // Panels and launchers: focus the ones that take keyboard
                // input. Clicking the wallpaper is like clicking the empty
                // background.
                Some(Under::Layer(layer, ..)) if layer.can_receive_keyboard_focus() => {
                    self.focus_layer(&layer, serial)
                }
                Some(under @ Under::Layer(..)) => {
                    if under.layer() == Some(Layer::Background) {
                        self.focus_window(None, serial);
                    }
                }
                Some(Under::Window(window, hit)) => {
                    // Click to focus and raise.
                    self.focus_window(Some(window.clone()), serial);

                    // Mod+Left drag moves, Mod+Right drag resizes. The grab is
                    // installed before the press is delivered, so the client
                    // never sees this click.
                    let mods = keyboard.modifier_state();
                    let start_data = PointerGrabStartData {
                        focus: None,
                        button,
                        location,
                    };
                    if let (false, Hit::Decoration(decoration)) =
                        (self.config.mod_key.is_pressed(&mods), &hit)
                    {
                        self.on_decoration_press(window, *decoration, start_data, serial);
                    } else if self.config.mod_key.is_pressed(&mods) {
                        match button {
                            BTN_LEFT => self.begin_move(window, start_data, serial),
                            BTN_RIGHT => {
                                let edges = self
                                    .space
                                    .element_geometry(&window)
                                    .map(|geo| {
                                        let rel = location - geo.loc.to_f64();
                                        let horizontal = if rel.x < geo.size.w as f64 / 2.0 {
                                            ResizeEdge::LEFT
                                        } else {
                                            ResizeEdge::RIGHT
                                        };
                                        let vertical = if rel.y < geo.size.h as f64 / 2.0 {
                                            ResizeEdge::TOP
                                        } else {
                                            ResizeEdge::BOTTOM
                                        };
                                        horizontal | vertical
                                    })
                                    .unwrap_or(ResizeEdge::BOTTOM_RIGHT);
                                self.begin_resize(window, edges, start_data, serial);
                            }
                            _ => {}
                        }
                    }
                }
                // Locked (handled above, as `under` gives nothing else).
                Some(Under::Lock(..)) => self.focus_lock_surface(serial),
                None => self.focus_window(None, serial),
            }
        }

        let menu = self.pointer_popup_grab();
        pointer.button(
            self,
            &ButtonEvent {
                button,
                state: button_state,
                serial,
                time,
            },
        );
        pointer.frame(self);
        // A click outside a menu ended its pointer grab: end the keyboard's
        // now too, and give the keyboard back if the menu had it
        // (`end_finished_popup_grab`).
        if !pointer.is_grabbed() && (menu || self.keyboard_popup_grab()) {
            self.end_finished_popup_grab();
        }
    }

    fn on_pointer_axis<I: InputBackend>(&mut self, event: I::PointerAxisEvent) {
        let source = event.source();

        let amount = |axis| {
            event
                .amount(axis)
                .unwrap_or_else(|| event.amount_v120(axis).unwrap_or(0.0) * 15.0 / 120.0)
        };
        let [bound_horizontal, bound_vertical] = self.wheel_bindings::<I>(&event);
        let horizontal = if bound_horizontal {
            0.0
        } else {
            amount(Axis::Horizontal)
        };
        let vertical = if bound_vertical {
            0.0
        } else {
            amount(Axis::Vertical)
        };
        if (bound_horizontal || bound_vertical) && horizontal == 0.0 && vertical == 0.0 {
            return;
        }

        let mut frame = AxisFrame::new(event.time()).source(source);
        if horizontal != 0.0 {
            frame = frame.value(Axis::Horizontal, horizontal);
            if let Some(discrete) = event.amount_v120(Axis::Horizontal) {
                frame = frame.v120(Axis::Horizontal, discrete as i32);
            }
        }
        if vertical != 0.0 {
            frame = frame.value(Axis::Vertical, vertical);
            if let Some(discrete) = event.amount_v120(Axis::Vertical) {
                frame = frame.v120(Axis::Vertical, discrete as i32);
            }
        }
        if source == AxisSource::Finger {
            if event.amount(Axis::Horizontal) == Some(0.0) {
                frame = frame.stop(Axis::Horizontal);
            }
            if event.amount(Axis::Vertical) == Some(0.0) {
                frame = frame.stop(Axis::Vertical);
            }
        }

        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        pointer.axis(self, frame);
        pointer.frame(self);
    }

    /// A mouse wheel's steps bound with the held modifiers
    /// (`Config::wheel_action`): each notch (`value120` 120, summed for
    /// wheels that send parts of one) runs the action once, unless it comes
    /// within `wheel-binding-cooldown` of the last run on that axis: a quick
    /// turn of the wheel is many notches, and would run through all the
    /// workspaces. Returns which axes (horizontal, vertical) were bound,
    /// which the client doesn't get, dropped notches included.
    /// The same filters as the keyboard's bindings: not while locked, while
    /// the window menu is open or shortcuts are inhibited.
    fn wheel_bindings<I: InputBackend>(&mut self, event: &I::PointerAxisEvent) -> [bool; 2] {
        let mut bound = [false; 2];
        if !matches!(event.source(), AxisSource::Wheel | AxisSource::WheelTilt)
            || self.is_locked()
            || self.window_menu.is_some()
            || self.shortcuts_inhibited()
        {
            self.input.wheel_v120 = [0.0; 2];
            return bound;
        }
        let Some(keyboard) = self.seat.get_keyboard() else {
            return bound;
        };
        let mods = keyboard.modifier_state();
        let axes = [
            (
                Axis::Horizontal,
                WheelDirection::Left,
                WheelDirection::Right,
            ),
            (Axis::Vertical, WheelDirection::Up, WheelDirection::Down),
        ];
        let now = std::time::Instant::now();
        let cooldown =
            std::time::Duration::from_millis(self.config.input.wheel_binding_cooldown.into());
        let mut actions = Vec::new();
        for (i, (axis, back, forward)) in axes.into_iter().enumerate() {
            let Some(v120) = event.amount_v120(axis).filter(|v| *v != 0.0) else {
                continue;
            };
            let direction = if v120 > 0.0 { forward } else { back };
            let Some(action) = self.config.wheel_action(direction, &mods) else {
                self.input.wheel_v120[i] = 0.0;
                continue;
            };
            bound[i] = true;
            let sum = &mut self.input.wheel_v120[i];
            // Turning the other way starts over.
            if *sum * v120 < 0.0 {
                *sum = 0.0;
            }
            *sum += v120;
            while sum.abs() >= 120.0 {
                *sum -= 120.0f64.copysign(*sum);
                let ran = &mut self.input.wheel_ran[i];
                if ran.is_some_and(|ran| now.duration_since(ran) < cooldown) {
                    continue;
                }
                *ran = Some(now);
                actions.push(action.clone());
            }
        }
        for action in actions {
            self.run_action(action, SERIAL_COUNTER.next_serial());
        }
        bound
    }
}
