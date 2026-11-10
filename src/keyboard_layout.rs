//! Keyboard layouts (`layout = "us,es"` in `[keyboard]`, or what an xkb
//! option switches between): which there are and which is in use, for
//! `pila msg keyboard` and its events, and `switch-layout`. While a virtual
//! keyboard's keymap stands in (`virtual_keyboard.rs`), it's the physical
//! keyboard's that count: their names as last read, and the layout saved to
//! come back to, which a switch changes instead.

use crate::sync::Locked;
use smithay::input::keyboard::Layout;

use crate::{config::LayoutTarget, Pila};

impl Pila {
    /// The physical keyboard's layout names, and the one in use (from 0).
    pub fn keyboard_layouts(&mut self) -> (Vec<String>, usize) {
        if let Some(saved) = self.saved_layout() {
            return (self.input.layout_names.clone(), saved.0 as usize);
        }
        let Some(keyboard) = self.seat.get_keyboard() else {
            return (Vec::new(), 0);
        };
        let (names, active) = keyboard.with_xkb_state(self, |context| {
            let xkb = context.xkb().locked();
            let names: Vec<String> = xkb
                .layouts()
                .map(|layout| xkb.layout_name(layout).to_string())
                .collect();
            (names, xkb.active_layout().0 as usize)
        });
        if self.input.layout_names != names {
            self.input.layout_names.clone_from(&names);
        }
        (names, active)
    }

    /// Switch the physical keyboard's layout: `next` and `prev` wrap around,
    /// a number past the last does nothing.
    pub fn switch_layout(&mut self, target: LayoutTarget) {
        let (names, active) = self.keyboard_layouts();
        let count = names.len();
        let index = match target {
            _ if count == 0 => return,
            LayoutTarget::Next => (active + 1) % count,
            LayoutTarget::Prev => (active + count - 1) % count,
            LayoutTarget::Index(index) if index < count => index,
            LayoutTarget::Index(_) => return,
        };
        let layout = Layout(index as u32);
        if self.set_saved_layout(layout) {
            return;
        }
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.with_xkb_state(self, |mut context| context.set_layout(layout));
        }
    }
}
