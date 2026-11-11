use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        InputTime, KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
        PointerMotionEvent,
    },
    desktop::Window,
    input::{
        keyboard::{FilterResult, Keysym},
        pointer::{
            AxisFrame, ButtonEvent, CursorIcon, CursorImageStatus,
            GrabStartData as PointerGrabStartData, MotionEvent, RelativeMotionEvent,
        },
    },
    utils::{Logical, Point, Serial, SERIAL_COUNTER},
    wayland::{
        keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitorSeat, shell::wlr_layer::Layer,
    },
};

use crate::{
    config::Action,
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
        self.queue_redraw_pointer(from, to, drawn || self.dnd_icon.is_some());
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

        let action = keyboard.input_from_source::<Option<Action>, _>(
            source,
            self,
            keycode,
            key_state,
            serial,
            time,
            |state, mods, handle| {
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
                // The window menu takes every press while it's open.
                if key_state == KeyState::Pressed && state.window_menu.is_some() {
                    state.input.suppressed_keys.insert(keycode);
                    state.input.menu_key = Some(handle.modified_sym());
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
                        if let Some(action) = action {
                            state.input.suppressed_keys.insert(keycode);
                            return FilterResult::Intercept(Some(action));
                        }
                    }
                    // Don't send the client a release for a press it never saw.
                    KeyState::Released if state.input.suppressed_keys.remove(&keycode) => {
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
                        state.input.tap_action = state
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

        // Intercepted (`Some`), or forwarded to the client.
        let mut acted = action.is_some();
        if let Some(sym) = self.input.menu_key.take() {
            self.window_menu_key(sym);
        }
        if let Some(action) = action.flatten() {
            self.run_action(action, serial);
        }
        if let Some(action) = self.input.tap_action.take() {
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

    /// Whether the surface with keyboard focus (or the window of a focused
    /// menu) has an active keyboard-shortcuts inhibitor.
    pub(crate) fn shortcuts_inhibited(&self) -> bool {
        let Some(focus) = self.keyboard_focus() else {
            return false;
        };
        let surface = self.popup_root(&focus).unwrap_or(focus);
        self.seat
            .keyboard_shortcuts_inhibitor_for_surface(&surface)
            .is_some_and(|inhibitor| inhibitor.is_active())
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
        for keycode in keyboard.pressed_keys() {
            let serial = SERIAL_COUNTER.next_serial();
            keyboard.input::<(), _>(
                self,
                keycode,
                KeyState::Released,
                serial,
                time,
                |state, _, _| {
                    // Like a real release: a key whose press triggered a
                    // binding was never seen by the client.
                    if state.input.suppressed_keys.remove(&keycode) {
                        FilterResult::Intercept(())
                    } else {
                        FilterResult::Forward
                    }
                },
            );
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
        self.pointer_motion(location, delta, delta, Event::time(&event));
    }

    /// Relative motion (libinput): move by the delta, staying on the outputs.
    fn on_pointer_motion<I: InputBackend>(&mut self, event: I::PointerMotionEvent) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let current = pointer.current_location();
        let location = self.clamp_to_outputs(current, current + event.delta());
        self.pointer_motion(
            location,
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

    /// Move the pointer to `location` (after relative motion of `delta`),
    /// honoring the active pointer constraint. Shared by both input paths.
    fn pointer_motion(
        &mut self,
        location: Point<f64, Logical>,
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
        // A click picks the window itself; a Mod+Tab in progress is over.
        if button_state == ButtonState::Pressed {
            self.end_switch();
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
    }

    fn on_pointer_axis<I: InputBackend>(&mut self, event: I::PointerAxisEvent) {
        let source = event.source();

        let amount = |axis| {
            event
                .amount(axis)
                .unwrap_or_else(|| event.amount_v120(axis).unwrap_or(0.0) * 15.0 / 120.0)
        };
        let horizontal = amount(Axis::Horizontal);
        let vertical = amount(Axis::Vertical);

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
}
