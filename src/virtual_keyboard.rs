//! zwp_virtual_keyboard_v1: tools like wtype, on-screen keyboards, and shells
//! pasting from a clipboard history type through it.
//!
//! Smithay hands its events in like a device's (`InputEvent<VirtualKeyboardBackend>`),
//! and pila treats it as a keyboard: its keys go through `process_input_event`
//! (bindings, `suppressed_keys`, idle activity), with a key source of their own
//! (`on_keyboard`), so a key the user holds isn't released by a virtual
//! keyboard releasing the same key. As in niri, which this follows:
//!
//! - A device's keycodes are read with its own keymap, which replaces the
//!   seat's (and so every client's) while it types; the next physical key, or
//!   the device going away, brings the configured one back. Swapping resets
//!   xkb's state, so the layout and Caps/Num Lock from before are restored.
//! - The configured keymap is kept as text (`InputState::configured_keymap`):
//!   restoring it doesn't compile the xkb config again on the event loop.
//! - A device with no keymap, or the configured one, types on the seat's state.
//! - Modifiers set with `modifiers` apply to the shared state and aren't undone
//!   when the device goes: the masks are absolute, so which bits were the
//!   device's can't be told apart from other keyboards' (niri's reasoning).

use crate::sync::Locked;
use std::any::Any;

use smithay::{
    backend::input::{Event, InputEvent},
    input::keyboard::{KeymapFile, Layout},
    wayland::virtual_keyboard::{
        VirtualKeyboardBackend, VirtualKeyboardDevice, VirtualKeyboardHandler,
        VirtualKeyboardSpecialEvent,
    },
};
use tracing::{debug, warn};

use crate::Pila;

/// A virtual keyboard's keymap in place of the configured one.
pub struct ActiveKeymap {
    pub device: VirtualKeyboardDevice,
    keymap: KeymapFile,
    saved: SavedKeyboardState,
}

/// What a keymap swap resets, from before the first virtual keymap.
#[derive(Clone, Copy)]
struct SavedKeyboardState {
    layout: Layout,
    caps_lock: bool,
    num_lock: bool,
}

/// The virtual keyboard a key event came from, if it did.
pub fn virtual_device<D: Any>(device: &D) -> Option<&VirtualKeyboardDevice> {
    (device as &dyn Any).downcast_ref::<VirtualKeyboardDevice>()
}

impl VirtualKeyboardHandler for Pila {
    fn process_virtual_keyboard_event(&mut self, event: InputEvent<VirtualKeyboardBackend>) {
        match event {
            InputEvent::Keyboard { event } => {
                self.activate_virtual_keymap(&event.device());
                self.process_input_event(InputEvent::<VirtualKeyboardBackend>::Keyboard { event });
            }
            InputEvent::Special(VirtualKeyboardSpecialEvent::Modifiers {
                device,
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
            }) => {
                // The masks are relative to the device's keymap.
                self.activate_virtual_keymap(&device);
                if let Some(keyboard) = self.seat.get_keyboard() {
                    keyboard.with_xkb_state(self, |mut context| {
                        context.set_modifier_mask(mods_depressed, mods_latched, mods_locked, group)
                    });
                }
            }
            // A new keymap is activated by the device's next key or modifiers.
            InputEvent::Special(VirtualKeyboardSpecialEvent::KeymapChanged { .. }) => {}
            // Smithay has released its keys already. Not activity, so not
            // through `process_input_event`.
            InputEvent::DeviceRemoved { device } => {
                let active = self
                    .input
                    .virtual_keymap
                    .as_ref()
                    .is_some_and(|active| active.device == device);
                if active {
                    self.restore_configured_keymap();
                }
            }
            _ => {}
        }
    }
}

impl Pila {
    /// Give the seat `device`'s keymap, unless it has it already.
    fn activate_virtual_keymap(&mut self, device: &VirtualKeyboardDevice) {
        let Some(keymap) = device.keymap() else {
            // Its keycodes are the configured keymap's.
            return self.restore_configured_keymap();
        };
        if let Some(active) = &mut self.input.virtual_keymap {
            if active.keymap.id() == keymap.id() {
                // Another device's identical keymap: take it over, so that
                // device going away doesn't take it from under this one.
                active.device = device.clone();
                return;
            }
        }
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        // An input method passing keys through sends the keymap it got:
        // nothing to swap.
        let configured = self
            .input
            .configured_keymap
            .get_or_insert_with(|| keyboard.keymap_file())
            .clone();
        if configured.id() == keymap.id() {
            return self.restore_configured_keymap();
        }
        let saved = match &self.input.virtual_keymap {
            Some(active) => active.saved,
            None => self.keyboard_state(),
        };
        if let Err(err) = keyboard.set_keymap_from_keymap_file(self, keymap.clone()) {
            warn!(?err, "failed to apply a virtual keyboard's keymap");
            return;
        }
        debug!("keymap: a virtual keyboard's");
        self.input.virtual_keymap = Some(ActiveKeymap {
            device: device.clone(),
            keymap,
            saved,
        });
    }

    /// Back to the configured keymap and the state from before, if a virtual
    /// keyboard's is in place.
    pub fn restore_configured_keymap(&mut self) {
        let Some(active) = self.input.virtual_keymap.take() else {
            return;
        };
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        debug!("keymap: the configured one again");
        let restored = self.input.configured_keymap.clone().is_some_and(|keymap| {
            keyboard
                .set_keymap_from_keymap_file(self, keymap)
                .inspect_err(|err| warn!(?err, "failed to restore the configured keymap"))
                .is_ok()
        });
        if !restored {
            // Compile it from the config instead.
            self.apply_keyboard_config();
        }
        self.restore_keyboard_state(active.saved);
    }

    fn keyboard_state(&mut self) -> SavedKeyboardState {
        // `Pila::new` gives the seat its keyboard, and it's never removed.
        let keyboard = self.seat.get_keyboard().expect("a keyboard");
        let mods = keyboard.modifier_state();
        let (layout, names) = keyboard.with_xkb_state(self, |context| {
            let xkb = context.xkb().locked();
            let names: Vec<String> = xkb
                .layouts()
                .map(|layout| xkb.layout_name(layout).to_string())
                .collect();
            (xkb.active_layout(), names)
        });
        // Told while the virtual keymap is in place (`keyboard_layout.rs`).
        self.input.layout_names = names;
        SavedKeyboardState {
            layout,
            caps_lock: mods.caps_lock,
            num_lock: mods.num_lock,
        }
    }

    fn restore_keyboard_state(&mut self, saved: SavedKeyboardState) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let mut mods = keyboard.modifier_state();
        if (mods.caps_lock, mods.num_lock) != (saved.caps_lock, saved.num_lock) {
            mods.caps_lock = saved.caps_lock;
            mods.num_lock = saved.num_lock;
            keyboard.set_modifier_state(mods);
            // `set_modifier_state` tells neither the client nor the LEDs.
            keyboard.advertise_modifier_state(self);
            if let Some(backend) = &mut self.backend {
                backend.update_leds(keyboard.led_state());
            }
        }
        // `set_modifier_state` can reset the layout: after it.
        keyboard.with_xkb_state(self, |mut context| {
            let (exists, current) = {
                let xkb = context.xkb().locked();
                (
                    xkb.layouts().any(|l| l == saved.layout),
                    xkb.active_layout(),
                )
            };
            if exists && current != saved.layout {
                context.set_layout(saved.layout);
            }
        });
    }
}

impl Pila {
    /// The physical keyboard's layout while a virtual keyboard's keymap is in
    /// place: the one it comes back to.
    pub fn saved_layout(&self) -> Option<Layout> {
        self.input.virtual_keymap.as_ref().map(|a| a.saved.layout)
    }

    /// Change the layout the physical keyboard comes back to; `false` when no
    /// virtual keymap is in place (the seat's layout is the physical one).
    pub fn set_saved_layout(&mut self, layout: Layout) -> bool {
        match &mut self.input.virtual_keymap {
            Some(active) => {
                active.saved.layout = layout;
                true
            }
            None => false,
        }
    }
}
