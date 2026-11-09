//! The headless backend's control socket, for automated tests:
//! `$XDG_RUNTIME_DIR/<WAYLAND_DISPLAY>.control`, one JSON request per line,
//! one JSON response per line (`{"ok": true, ...}` or `{"ok": false,
//! "error": "..."}`). Input sent here goes through
//! `Pila::process_input_event` as `TestInput` events, so it meets bindings,
//! grabs, constraints, idle and the lock like real input.
//!
//! Requests (`cmd` and its fields):
//! - `state`: `Pila::state_json` (`introspect.rs`).
//! - `key` `key` (a keysym name such as `Super_L`, `Return`, `q`, or
//!   `code:<xkb keycode>`), `state` (`press`, `release`, `tap`).
//! - `combo` `keys` (`Super+Shift+q`): press in order, release in reverse.
//! - `move` `x`, `y`: move the pointer to a global logical point (as relative
//!   motion, so constraints apply). `rel` `dx`, `dy`: relative motion.
//! - `button` `button` (`left`, `right`, `middle` or a code), `state`
//!   (`press`, `release`, `click`).
//! - `axis` `source` (`wheel`, `finger`, `continuous`), `h`, `v`.
//! - `gesture` `kind` (`swipe`, `pinch`, `hold`), `phase` (`begin`,
//!   `update`, `end`), `fingers`, `dx`, `dy`, `scale`, `rotation`,
//!   `cancelled`.
//! - `pixels` `x`, `y`, `w`, `h` (global logical): the pixels of the last
//!   frame, as `rgba` (a flat array) with its physical `width`/`height`.
//! - `screenshot` `output`, `path`: the output's last frame as a PPM file.
//! - `output_add` `spec` (`NAME:WxH[@HZ][=MAKE/MODEL[/SERIAL]]`),
//!   `output_remove` `name`.
//! - `output_fail_next`: the next time outputs are applied, the backend fails.
//! - `spawn` `command`: run a command as a binding would.
//! - `quit`.
//! - `panic`: crash right after answering (the crash report in the log).

use std::{
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
};

use serde_json::{json, Value};
use smithay::{
    backend::input::{AxisSource, ButtonState, InputEvent, KeyState, Keycode},
    input::keyboard::{xkb, Layout},
    reexports::calloop::{generic::Generic, Interest, Mode, PostAction},
    utils::{Physical, Rectangle},
};
use tracing::{info, warn};

use crate::{
    backend::Backend,
    headless::{
        self, event_time, AxisEvent, ButtonEvent, GestureEvent, KeyEvent, MotionEvent, TestInput,
    },
    Pila,
};

/// Linux input event codes of the mouse buttons.
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;

/// Open the control socket. Returns its path.
pub fn init(state: &mut Pila) -> std::io::Result<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| std::io::Error::other("XDG_RUNTIME_DIR is not set"))?;
    let mut name = state.socket_name.clone();
    name.push(".control");
    let path = dir.join(name);
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    listener.set_nonblocking(true)?;
    state
        .loop_handle
        .insert_source(
            Generic::new(listener, Interest::READ, Mode::Level),
            |_, listener, state| {
                loop {
                    match listener.accept() {
                        Ok((stream, _)) => add_client(state, stream),
                        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(err) => {
                            warn!(?err, "control socket: accept failed");
                            break;
                        }
                    }
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    info!(path = %path.display(), "control socket");
    Ok(path)
}

fn add_client(state: &mut Pila, stream: UnixStream) {
    if let Err(err) = stream.set_nonblocking(true) {
        warn!(?err, "control socket: client setup failed");
        return;
    }
    let mut pending: Vec<u8> = Vec::new();
    let result = state.loop_handle.insert_source(
        Generic::new(stream, Interest::READ, Mode::Level),
        move |_, stream, state| {
            let stream: &UnixStream = stream;
            let mut chunk = [0u8; 4096];
            loop {
                match (&*stream).read(&mut chunk) {
                    Ok(0) => return Ok(PostAction::Remove),
                    Ok(n) => pending.extend_from_slice(&chunk[..n]),
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(_) => return Ok(PostAction::Remove),
                }
            }
            while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = pending.drain(..=end).collect();
                let response = match serde_json::from_slice::<Value>(&line) {
                    Ok(request) => handle(state, &request),
                    Err(err) => Err(format!("bad request: {err}")),
                };
                let response = match response {
                    Ok(Value::Object(mut map)) => {
                        map.insert("ok".into(), true.into());
                        Value::Object(map)
                    }
                    Ok(other) => json!({ "ok": true, "value": other }),
                    Err(error) => json!({ "ok": false, "error": error }),
                };
                let mut text = response.to_string();
                text.push('\n');
                // Answers may be large (pixels): written whole, blocking.
                let written = stream
                    .set_nonblocking(false)
                    .and_then(|()| (&*stream).write_all(text.as_bytes()))
                    .and_then(|()| stream.set_nonblocking(true));
                if written.is_err() {
                    return Ok(PostAction::Remove);
                }
            }
            Ok(PostAction::Continue)
        },
    );
    if let Err(err) = result {
        warn!(?err, "control socket: client setup failed");
    }
}

fn field<'a>(request: &'a Value, name: &str) -> Result<&'a Value, String> {
    request.get(name).ok_or_else(|| format!("missing {name:?}"))
}

fn str_field<'a>(request: &'a Value, name: &str) -> Result<&'a str, String> {
    field(request, name)?
        .as_str()
        .ok_or_else(|| format!("{name:?} must be a string"))
}

fn f64_field(request: &Value, name: &str, default: Option<f64>) -> Result<f64, String> {
    match request.get(name) {
        Some(v) => v
            .as_f64()
            .ok_or_else(|| format!("{name:?} must be a number")),
        None => default.ok_or_else(|| format!("missing {name:?}")),
    }
}

fn handle(state: &mut Pila, request: &Value) -> Result<Value, String> {
    let cmd = str_field(request, "cmd")?;
    let result = match cmd {
        "state" => return Ok(state.state_json()),
        "key" => {
            let keycode = keycode(state, str_field(request, "key")?)?;
            match request
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or("tap")
            {
                "press" => key(state, keycode, KeyState::Pressed),
                "release" => key(state, keycode, KeyState::Released),
                "tap" => {
                    key(state, keycode, KeyState::Pressed);
                    key(state, keycode, KeyState::Released);
                }
                other => return Err(format!("bad key state {other:?}")),
            }
            json!({})
        }
        "combo" => {
            let keys = str_field(request, "keys")?
                .split('+')
                .map(|name| keycode(state, name))
                .collect::<Result<Vec<_>, _>>()?;
            for keycode in &keys {
                key(state, *keycode, KeyState::Pressed);
            }
            for keycode in keys.iter().rev() {
                key(state, *keycode, KeyState::Released);
            }
            json!({})
        }
        "move" => {
            let x = f64_field(request, "x", None)?;
            let y = f64_field(request, "y", None)?;
            let current = state.pointer_location();
            motion(state, (x - current.x, y - current.y));
            json!({})
        }
        "rel" => {
            let dx = f64_field(request, "dx", Some(0.0))?;
            let dy = f64_field(request, "dy", Some(0.0))?;
            motion(state, (dx, dy));
            json!({})
        }
        "button" => {
            let button = match field(request, "button")? {
                Value::String(name) => match name.as_str() {
                    "left" => BTN_LEFT,
                    "right" => BTN_RIGHT,
                    "middle" => BTN_MIDDLE,
                    other => return Err(format!("bad button {other:?}")),
                },
                Value::Number(n) => n.as_u64().ok_or("bad button")? as u32,
                _ => return Err("bad button".into()),
            };
            match request
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or("click")
            {
                "press" => mouse_button(state, button, ButtonState::Pressed),
                "release" => mouse_button(state, button, ButtonState::Released),
                "click" => {
                    mouse_button(state, button, ButtonState::Pressed);
                    mouse_button(state, button, ButtonState::Released);
                }
                other => return Err(format!("bad button state {other:?}")),
            }
            json!({})
        }
        "axis" => {
            let source = match request
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or("wheel")
            {
                "wheel" => AxisSource::Wheel,
                "finger" => AxisSource::Finger,
                "continuous" => AxisSource::Continuous,
                other => return Err(format!("bad axis source {other:?}")),
            };
            let event = AxisEvent {
                time: event_time(),
                source,
                amount: (
                    f64_field(request, "h", Some(0.0))?,
                    f64_field(request, "v", Some(0.0))?,
                ),
            };
            state.process_input_event(InputEvent::<TestInput>::PointerAxis { event });
            json!({})
        }
        "gesture" => {
            gesture(state, request)?;
            json!({})
        }
        "pixels" => return pixels(state, request),
        "screenshot" => {
            screenshot(
                state,
                str_field(request, "output")?,
                str_field(request, "path")?,
            )?;
            json!({})
        }
        "output_add" => {
            let spec = headless::parse_output_spec(str_field(request, "spec")?)?;
            if !headless::add_output(state, &spec) {
                return Err("could not add the output".into());
            }
            json!({})
        }
        "output_remove" => {
            if !headless::remove_output(state, str_field(request, "name")?) {
                return Err("no such output".into());
            }
            json!({})
        }
        "output_fail_next" => {
            let Some(Backend::Headless(data)) = &mut state.backend else {
                return Err("not headless".into());
            };
            data.fail_next_apply = true;
            json!({})
        }
        "spawn" => {
            state.spawn(str_field(request, "command")?);
            json!({})
        }
        "quit" => {
            state.loop_signal.stop();
            json!({})
        }
        // After the answer, so the test reads it before Pila is gone.
        "panic" => {
            state
                .loop_handle
                .insert_idle(|_| panic!("a test asked for it"));
            state.wake.ping();
            json!({})
        }
        other => return Err(format!("unknown command {other:?}")),
    };
    // As after any input event nested: something may have changed.
    state.queue_redraw();
    Ok(result)
}

/// The keycode of a keysym name in the current keymap (its first level), or
/// `code:<n>`. `Super`, `Alt`, `Ctrl` and `Shift` name the left keys.
fn keycode(state: &mut Pila, name: &str) -> Result<Keycode, String> {
    if let Some(code) = name.strip_prefix("code:") {
        return code
            .parse::<u32>()
            .map(Keycode::new)
            .map_err(|_| format!("bad keycode {code:?}"));
    }
    let name = match name {
        "Super" | "Logo" => "Super_L",
        "Alt" => "Alt_L",
        "Ctrl" | "Control" => "Control_L",
        "Shift" => "Shift_L",
        other => other,
    };
    let mut sym = xkb::keysym_from_name(name, xkb::KEYSYM_NO_FLAGS);
    if sym.raw() == xkb::keysyms::KEY_NoSymbol {
        sym = xkb::keysym_from_name(name, xkb::KEYSYM_CASE_INSENSITIVE);
    }
    if sym.raw() == xkb::keysyms::KEY_NoSymbol {
        return Err(format!("unknown key {name:?}"));
    }
    let keyboard = state.seat.get_keyboard().ok_or("no keyboard")?;
    keyboard
        .with_xkb_state(state, |context| {
            let xkb = context.xkb().lock().unwrap();
            // SAFETY: the keymap isn't kept beyond this closure.
            let keymap = unsafe { xkb.keymap() };
            let (min, max) = (keymap.min_keycode().raw(), keymap.max_keycode().raw());
            (min..=max).map(Keycode::new).find(|code| {
                xkb.raw_syms_for_key_in_layout(*code, Layout(0))
                    .contains(&sym)
            })
        })
        .ok_or_else(|| format!("no key for {name:?} in the keymap"))
}

fn key(state: &mut Pila, key: Keycode, key_state: KeyState) {
    let event = KeyEvent {
        time: event_time(),
        key,
        state: key_state,
    };
    state.process_input_event(InputEvent::<TestInput>::Keyboard { event });
}

fn motion(state: &mut Pila, delta: (f64, f64)) {
    let event = MotionEvent {
        time: event_time(),
        delta,
    };
    state.process_input_event(InputEvent::<TestInput>::PointerMotion { event });
}

fn mouse_button(state: &mut Pila, button: u32, button_state: ButtonState) {
    let event = ButtonEvent {
        time: event_time(),
        button,
        state: button_state,
    };
    state.process_input_event(InputEvent::<TestInput>::PointerButton { event });
}

fn gesture(state: &mut Pila, request: &Value) -> Result<(), String> {
    let event = GestureEvent {
        time: event_time(),
        fingers: f64_field(request, "fingers", Some(3.0))? as u32,
        delta: (
            f64_field(request, "dx", Some(0.0))?,
            f64_field(request, "dy", Some(0.0))?,
        ),
        scale: f64_field(request, "scale", Some(1.0))?,
        rotation: f64_field(request, "rotation", Some(0.0))?,
        cancelled: request
            .get("cancelled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    };
    let event = match (str_field(request, "kind")?, str_field(request, "phase")?) {
        ("swipe", "begin") => InputEvent::<TestInput>::GestureSwipeBegin { event },
        ("swipe", "update") => InputEvent::GestureSwipeUpdate { event },
        ("swipe", "end") => InputEvent::GestureSwipeEnd { event },
        ("pinch", "begin") => InputEvent::GesturePinchBegin { event },
        ("pinch", "update") => InputEvent::GesturePinchUpdate { event },
        ("pinch", "end") => InputEvent::GesturePinchEnd { event },
        ("hold", "begin") => InputEvent::GestureHoldBegin { event },
        ("hold", "end") => InputEvent::GestureHoldEnd { event },
        (kind, phase) => return Err(format!("bad gesture {kind:?} {phase:?}")),
    };
    state.process_input_event(event);
    Ok(())
}

/// The pixels of a global logical rectangle, from the output holding its
/// top-left corner.
fn pixels(state: &mut Pila, request: &Value) -> Result<Value, String> {
    let x = f64_field(request, "x", None)?;
    let y = f64_field(request, "y", None)?;
    let w = f64_field(request, "w", Some(1.0))?;
    let h = f64_field(request, "h", Some(1.0))?;
    let output = state.output_at((x, y).into()).ok_or("no output there")?;
    let geo = state
        .space
        .output_geometry(&output)
        .ok_or("output not mapped")?;
    let scale = output.current_scale().fractional_scale();
    let phys = |v: f64| (v * scale).round() as i32;
    let rect: Rectangle<i32, Physical> = Rectangle::new(
        (phys(x - geo.loc.x as f64), phys(y - geo.loc.y as f64)).into(),
        (phys(w).max(1), phys(h).max(1)).into(),
    );
    let Some(Backend::Headless(data)) = &mut state.backend else {
        return Err("not headless".into());
    };
    let rgba = data.read_pixels(&output, rect)?;
    Ok(json!({ "width": rect.size.w, "height": rect.size.h, "rgba": rgba }))
}

fn screenshot(state: &mut Pila, name: &str, path: &str) -> Result<(), String> {
    let output = state
        .space
        .outputs()
        .find(|o| o.name() == name)
        .cloned()
        .ok_or("no such output")?;
    let mode = output.current_mode().ok_or("no mode")?;
    let rect = Rectangle::from_size((mode.size.w, mode.size.h).into());
    let Some(Backend::Headless(data)) = &mut state.backend else {
        return Err("not headless".into());
    };
    let rgba = data.read_pixels(&output, rect)?;
    let mut ppm = format!("P6\n{} {}\n255\n", mode.size.w, mode.size.h).into_bytes();
    for px in rgba.as_chunks::<4>().0 {
        ppm.extend_from_slice(&px[..3]);
    }
    std::fs::write(path, ppm).map_err(|e| e.to_string())
}
