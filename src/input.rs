use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
    },
    desktop::Window,
    input::{
        keyboard::FilterResult,
        pointer::{
            AxisFrame, ButtonEvent, CursorIcon, CursorImageStatus,
            GrabStartData as PointerGrabStartData, MotionEvent,
        },
    },
    utils::{Logical, Point, Serial, SERIAL_COUNTER},
};

use crate::{
    decoration::{Decoration, TitleBarPart},
    grabs::resize_grab::ResizeEdge,
    state::{fill_state, Hit, Pila},
};

// Linux input event codes (linux/input-event-codes.h).
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

impl Pila {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event, .. } => self.on_keyboard::<I>(event),
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

        let action = keyboard.input::<Option<crate::config::Action>, _>(
            self,
            keycode,
            key_state,
            serial,
            time,
            |state, mods, handle| {
                match key_state {
                    KeyState::Pressed if state.config.mod_key.is_pressed(mods) => {
                        let action = handle
                            .raw_latin_sym_or_raw_current_sym()
                            .and_then(|sym| state.config.action_for(sym, mods));
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
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };

        let location = event.position_transformed(output_geo.size) + output_geo.loc.to_f64();
        let under = self.surface_under(location);
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time_msec(),
            },
        );
        pointer.frame(self);

        // Over pila's own decorations no client sets the cursor, so pila does.
        if !pointer.is_grabbed() {
            self.update_decoration_cursor(location);
        }
    }

    /// Resize arrows over a window's resize border, the default arrow over a
    /// title bar or the background. Client surfaces set their own.
    fn update_decoration_cursor(&mut self, pos: Point<f64, Logical>) {
        let icon = match self.element_under(pos) {
            Some((_, Hit::Surface(..))) => return,
            Some((_, Hit::Decoration(Decoration::Border(edges)))) => edges.cursor_icon(),
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
            Decoration::TitleBar(TitleBarPart::Close) => {
                if let Some(toplevel) = window.toplevel() {
                    toplevel.send_close();
                }
            }
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

        if button_state == ButtonState::Pressed && !pointer.is_grabbed() {
            let location = pointer.current_location();

            match self.element_under(location) {
                Some((window, hit)) => {
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
