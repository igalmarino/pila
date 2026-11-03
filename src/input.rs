use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
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
    state::{fill_state, Hit, Pila, Under},
};

// Linux input event codes (linux/input-event-codes.h).
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

impl Pila {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        self.notify_activity();
        match event {
            InputEvent::Keyboard { event, .. } => self.on_keyboard::<I>(event),
            InputEvent::PointerMotion { event, .. } => self.on_pointer_motion::<I>(event),
            InputEvent::PointerMotionAbsolute { event, .. } => {
                self.on_pointer_motion_absolute::<I>(event)
            }
            InputEvent::PointerButton { event, .. } => self.on_pointer_button::<I>(event),
            InputEvent::PointerAxis { event, .. } => self.on_pointer_axis::<I>(event),
            _ => {}
        }
    }

    fn on_keyboard<I: InputBackend>(&mut self, event: I::KeyboardKeyEvent) {
        let serial = SERIAL_COUNTER.next_serial();
        let time = Event::time_msec(&event);
        let keycode = event.key_code();
        let key_state = event.state();
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let inhibited = self.shortcuts_inhibited();

        let action = keyboard.input::<Option<Action>, _>(
            self,
            keycode,
            key_state,
            serial,
            time,
            |state, mods, handle| {
                // Ctrl+Alt+F<N>: the keymap turns it into XF86_Switch_VT_<N>.
                if key_state == KeyState::Pressed {
                    let raw = handle.modified_sym().raw();
                    let first = Keysym::XF86_Switch_VT_1.raw();
                    if (first..=Keysym::XF86_Switch_VT_12.raw()).contains(&raw) {
                        state.suppressed_keys.insert(keycode);
                        return FilterResult::Intercept(Some(Action::ChangeVt(
                            (raw - first + 1) as i32,
                        )));
                    }
                }
                match key_state {
                    // Any key can be bound (media keys need no Mod), so
                    // every press is looked up.
                    KeyState::Pressed => {
                        let action = handle
                            .raw_latin_sym_or_raw_current_sym()
                            .and_then(|sym| state.config.action_for(sym, mods))
                            // While locked, everything goes to the locker;
                            // debug builds keep Quit to escape a nested session.
                            .filter(|action| {
                                !state.is_locked()
                                    || (cfg!(debug_assertions) && *action == Action::Quit)
                            })
                            // A client inhibiting shortcuts gets them, but
                            // never Quit.
                            .filter(|action| !inhibited || *action == Action::Quit);
                        if let Some(action) = action {
                            state.suppressed_keys.insert(keycode);
                            return FilterResult::Intercept(Some(action));
                        }
                    }
                    // Don't send the client a release for a press it never saw.
                    KeyState::Released if state.suppressed_keys.remove(&keycode) => {
                        return FilterResult::Intercept(None);
                    }
                    _ => {}
                }
                FilterResult::Forward
            },
        );

        if let Some(action) = action.flatten() {
            self.run_action(action, serial);
        }
        // Mod+Tab ends once Ctrl/Alt/Super are all released.
        if self.switch.is_some() {
            let mods = keyboard.modifier_state();
            if !(mods.ctrl || mods.alt || mods.logo) {
                self.end_switch();
            }
        }
    }

    /// Whether the surface with keyboard focus (or the window of a focused
    /// menu) has an active keyboard-shortcuts inhibitor.
    fn shortcuts_inhibited(&self) -> bool {
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
        let time = self.start_time.elapsed().as_millis() as u32;
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
                    if state.suppressed_keys.remove(&keycode) {
                        FilterResult::Intercept(())
                    } else {
                        FilterResult::Forward
                    }
                },
            );
        }
        self.suppressed_keys.clear();
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
            .host_pointer
            .map(|last| location - last)
            .unwrap_or_default();
        self.host_pointer = Some(location);
        self.pointer_motion(
            location,
            delta,
            delta,
            Event::time(&event),
            event.time_msec(),
        );
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
            event.time_msec(),
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
        utime: u64,
        time_msec: u32,
    ) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };

        // The constraint of the surface the pointer is on now (none during
        // pila's own move/resize grabs).
        let current = pointer.current_location();
        let under_current = self.surface_under(current);
        let constraint = match &under_current {
            Some((surface, origin)) if !pointer.is_grabbed() => {
                self.active_constraint(surface, *origin, current)
            }
            _ => Constraint::None,
        };
        pointer.relative_motion(
            self,
            under_current.clone(),
            &RelativeMotionEvent {
                delta,
                delta_unaccel,
                utime,
            },
        );
        match constraint {
            // Locked: only relative motion; pila's pointer stays put.
            Constraint::Locked => {
                pointer.frame(self);
                return;
            }
            // Confined: drop motion that would leave the surface or region.
            Constraint::Confined(region) => {
                let (surface, origin) = under_current.as_ref().expect("constraint without surface");
                let stays = self
                    .surface_under(location)
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
            under.clone(),
            &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time: time_msec,
            },
        );
        pointer.frame(self);

        if !pointer.is_grabbed() {
            // Over pila's own decorations no client sets the cursor, so pila does.
            self.update_decoration_cursor(location);
            if let Some((surface, origin)) = &under {
                self.maybe_activate_constraint(surface, *origin, location);
            }
        }
    }

    /// Resize arrows over a window's resize border, the default arrow over a
    /// title bar or the background. Client surfaces set their own.
    fn update_decoration_cursor(&mut self, pos: Point<f64, Logical>) {
        if self.is_locked() {
            return;
        }
        let icon = match self.under(pos) {
            Some(Under::Window(_, Hit::Surface(..)) | Under::Layer(..)) => return,
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
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let serial = SERIAL_COUNTER.next_serial();
        let button = event.button_code();
        let button_state = event.state();
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
                None => self.focus_window(None, serial),
            }
        }

        pointer.button(
            self,
            &ButtonEvent {
                button,
                state: button_state,
                serial,
                time: event.time_msec(),
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

        let mut frame = AxisFrame::new(event.time_msec()).source(source);
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
