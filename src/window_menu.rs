//! The window menu: what a right click on a title bar opens, pila's own or an
//! app's (`xdg_toplevel.show_window_menu`, sent by GTK and other apps that
//! draw their own title bar).
//!
//! Its items act on the window: minimize, maximize or restore, keep it above
//! the others, keep it on all workspaces (those two show a mark while on),
//! close. While it's open a pointer grab (`WindowMenuGrab`) has the pointer:
//! releasing a button over an item runs it (so press, drag to an item,
//! release works too), a press outside closes the menu and goes nowhere else,
//! except a right press, which still counts (another title bar's menu, an
//! app's own).
//! The keyboard path (`input.rs`) gives it Up, Down, Return and Escape. It
//! closes with its window (`forget_window`), a lock or a workspace switch.
//!
//! It's drawn by pila, above everything but the cursor (`render::output_elements`),
//! in the title bar's colors; its textures and solid buffers are kept between
//! frames (`MenuBuffers`), so an open menu costs a few elements and no damage
//! unless the hover moves.

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use smithay::{
    backend::{
        allocator::Fourcc,
        input::ButtonState,
        renderer::{
            element::{
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                solid::{SolidColorBuffer, SolidColorRenderElement},
                Kind,
            },
            gles::GlesRenderer,
        },
    },
    desktop::Window,
    input::keyboard::Keysym,
    input::pointer::{
        AxisFrame, ButtonEvent, CursorImageStatus, Focus, GestureHoldBeginEvent,
        GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
        GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
        GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab, PointerInnerHandle,
        RelativeMotionEvent,
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Physical, Point, Rectangle, Scale, Serial, Size, Transform, SERIAL_COUNTER},
};
use tracing::debug;

use crate::{config::Colors, render::PilaRenderElement, state::fill_state, Pila};

/// A row's height and the menu's width, in logical pixels.
pub const ITEM_HEIGHT: i32 = 28;
pub const WIDTH: i32 = 220;
/// Space above the first row and below the last.
const PADDING: i32 = 4;
/// Left of a label.
const TEXT_INSET: i32 = 12;
/// The mark of an item that's on, at the row's right.
const MARK_SIZE: i32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Minimize,
    Maximize,
    AlwaysOnTop,
    AllWorkspaces,
    Close,
}

const ITEMS: [Item; 5] = [
    Item::Minimize,
    Item::Maximize,
    Item::AlwaysOnTop,
    Item::AllWorkspaces,
    Item::Close,
];

impl Item {
    fn label(self, window: &Window) -> &'static str {
        match self {
            Item::Minimize => "Minimize",
            Item::Maximize if fill_state(window).0 => "Restore",
            Item::Maximize => "Maximize",
            Item::AlwaysOnTop => "Always on top",
            Item::AllWorkspaces => "On all workspaces",
            Item::Close => "Close",
        }
    }

    /// Whether the item shows the mark of a setting that's on.
    fn checked(self, window: &Window) -> bool {
        match self {
            Item::AlwaysOnTop => crate::stacking::is_always_on_top(window),
            Item::AllWorkspaces => crate::stacking::is_on_all_workspaces(window),
            _ => false,
        }
    }
}

/// The last pointer button press (`InputState::last_press`).
pub struct LastPress {
    pub serial: Serial,
    pub button: u32,
    pub location: Point<f64, Logical>,
    pub client: Option<smithay::reexports::wayland_server::backend::ClientId>,
    pub at: std::time::Instant,
}

/// How long after its press a client may still ask for the menu with it.
const PRESS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

/// An open window menu.
pub struct WindowMenu {
    pub window: Window,
    /// Where it is, global logical coordinates.
    pub rect: Rectangle<i32, Logical>,
    /// The row under the pointer or picked with the keyboard.
    pub hover: Option<usize>,
    buffers: Rc<RefCell<MenuBuffers>>,
}

/// Kept between frames so an unchanged menu adds no damage.
#[derive(Default)]
struct MenuBuffers {
    background: Option<SolidColorBuffer>,
    hover: Option<SolidColorBuffer>,
    mark: Option<SolidColorBuffer>,
    /// Label textures by text and buffer scale.
    labels: HashMap<(&'static str, i32), (MemoryRenderBuffer, i32)>,
}

/// What `Pila::scene` gives the renderer of an open menu.
pub struct SceneMenu {
    rect: Rectangle<i32, Logical>,
    rows: Vec<(&'static str, bool)>,
    hover: Option<usize>,
    buffers: Rc<RefCell<MenuBuffers>>,
}

impl WindowMenu {
    fn row_rect(&self, index: usize) -> Rectangle<i32, Logical> {
        row_rect(self.rect, index)
    }

    /// The row at `location` (global), if any.
    fn row_at(&self, location: Point<f64, Logical>) -> Option<usize> {
        (0..ITEMS.len()).find(|&i| self.row_rect(i).to_f64().contains(location))
    }

    pub fn scene(&self) -> SceneMenu {
        SceneMenu {
            rect: self.rect,
            rows: ITEMS
                .iter()
                .map(|item| (item.label(&self.window), item.checked(&self.window)))
                .collect(),
            hover: self.hover,
            buffers: self.buffers.clone(),
        }
    }

    /// For the test control socket and `pila msg`-style introspection.
    pub fn labels(&self) -> Vec<&'static str> {
        ITEMS.iter().map(|item| item.label(&self.window)).collect()
    }

    pub fn checked(&self) -> Vec<bool> {
        ITEMS
            .iter()
            .map(|item| item.checked(&self.window))
            .collect()
    }
}

fn row_rect(menu: Rectangle<i32, Logical>, index: usize) -> Rectangle<i32, Logical> {
    Rectangle::new(
        (
            menu.loc.x,
            menu.loc.y + PADDING + index as i32 * ITEM_HEIGHT,
        )
            .into(),
        (menu.size.w, ITEM_HEIGHT).into(),
    )
}

impl Pila {
    /// Open the menu of `window` with its top-left at `at` (moved to stay on
    /// the output), taking the pointer from `start_data`'s press.
    pub fn open_window_menu(
        &mut self,
        window: &Window,
        at: Point<i32, Logical>,
        start_data: PointerGrabStartData<Pila>,
        serial: Serial,
    ) {
        if self.is_locked() {
            return;
        }
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        self.dismiss_popup_grab();
        let size = Size::from((WIDTH, PADDING * 2 + ITEMS.len() as i32 * ITEM_HEIGHT));
        let mut rect = Rectangle::new(at, size);
        if let Some(output) = self.output_geometry_at(at.to_f64()) {
            let max = output.loc + output.size.to_point() - size.to_point();
            rect.loc.x = rect.loc.x.clamp(output.loc.x, max.x.max(output.loc.x));
            rect.loc.y = rect.loc.y.clamp(output.loc.y, max.y.max(output.loc.y));
        }
        debug!(
            app_id = crate::foreign_toplevel::title_and_app_id(window).1,
            ?rect,
            "window menu"
        );
        self.window_menu = Some(WindowMenu {
            window: window.clone(),
            rect,
            hover: None,
            buffers: Default::default(),
        });
        let grab = WindowMenuGrab {
            start_data,
            opened: std::time::Instant::now(),
            opening: serial,
        };
        pointer.set_grab(self, grab, serial, Focus::Clear);
        self.set_cursor(CursorImageStatus::default_named());
        self.queue_redraw();
    }

    /// The grab start of the last press, if it's `serial`, the client of
    /// `surface` got it and it's recent: GTK asks for the menu when it
    /// handles the press, often after the release (a quick click), when the
    /// press no longer holds a grab.
    pub fn recent_press(
        &self,
        surface: &WlSurface,
        serial: Serial,
    ) -> Option<PointerGrabStartData<Pila>> {
        use smithay::reexports::wayland_server::Resource;
        let press = self.input.last_press.as_ref()?;
        let client = self.display_handle.get_client(surface.id()).ok()?.id();
        (press.serial == serial
            && press.client.as_ref() == Some(&client)
            && press.at.elapsed() < PRESS_TIMEOUT)
            .then(|| PointerGrabStartData {
                focus: None,
                button: press.button,
                location: press.location,
            })
    }

    /// Close the menu, if one is open, without running anything.
    pub fn close_window_menu(&mut self) {
        if self.window_menu.take().is_none() {
            return;
        }
        self.queue_redraw();
        // Its grab ends with it (`WindowMenuGrab::unset` finds no menu).
        if let Some(pointer) = self.seat.get_pointer() {
            if pointer.is_grabbed() {
                let time = smithay::backend::input::InputTime::now();
                pointer.unset_grab(self, SERIAL_COUNTER.next_serial(), time);
            }
        }
    }

    /// The menu of `window` is open: close it (it went away).
    pub fn close_window_menu_of(&mut self, window: &Window) {
        if self
            .window_menu
            .as_ref()
            .is_some_and(|m| m.window == *window)
        {
            self.close_window_menu();
        }
    }

    fn set_menu_hover(&mut self, hover: Option<usize>) {
        if let Some(menu) = &mut self.window_menu {
            if menu.hover != hover {
                menu.hover = hover;
                self.queue_redraw();
            }
        }
    }

    /// The window and item of row `index` of the open menu.
    fn menu_row(&self, index: usize) -> Option<(Window, Item)> {
        let menu = self.window_menu.as_ref()?;
        Some((menu.window.clone(), *ITEMS.get(index)?))
    }

    /// Run row `index` of the open menu, closing it. Not from inside the
    /// grab: it ends the grab through the pointer (`WindowMenuGrab::button`
    /// ends it through its handle and runs the item from an idle callback).
    fn activate_menu_row(&mut self, index: usize) {
        let Some((window, item)) = self.menu_row(index) else {
            return;
        };
        self.close_window_menu();
        self.run_menu_item(&window, item);
    }

    fn run_menu_item(&mut self, window: &Window, item: Item) {
        let window = window.clone();
        debug!(?item, "window menu item");
        match item {
            Item::Minimize => self.minimize_window(&window),
            Item::Maximize => {
                let (maximized, fullscreen) = fill_state(&window);
                self.set_fill_state(&window, !maximized, fullscreen);
            }
            Item::AlwaysOnTop => self.toggle_always_on_top(&window),
            Item::AllWorkspaces => self.toggle_all_workspaces(&window),
            Item::Close => crate::state::close_window(&window),
        }
        self.queue_redraw();
    }

    /// A key pressed while the menu is open (the keyboard path takes every
    /// press then).
    pub fn window_menu_key(&mut self, sym: Keysym) {
        let Some(menu) = self.window_menu.as_ref() else {
            return;
        };
        let count = ITEMS.len();
        match sym {
            Keysym::Escape => self.close_window_menu(),
            Keysym::Down | Keysym::Tab => {
                let next = menu.hover.map_or(0, |i| (i + 1) % count);
                self.set_menu_hover(Some(next));
            }
            Keysym::Up | Keysym::ISO_Left_Tab => {
                let next = menu.hover.map_or(count - 1, |i| (i + count - 1) % count);
                self.set_menu_hover(Some(next));
            }
            Keysym::Return | Keysym::KP_Enter | Keysym::space => {
                if let Some(index) = menu.hover {
                    self.activate_menu_row(index);
                }
            }
            _ => {}
        }
    }
}

/// The pointer while the menu is open: no client sees it.
pub struct WindowMenuGrab {
    start_data: PointerGrabStartData<Pila>,
    opened: std::time::Instant,
    /// The press that opened it, which reaches the grab right after: not a
    /// click on the menu, nor one outside it.
    opening: Serial,
}

/// The release of the click that opened the menu is no choice before this:
/// the menu opens under the pointer, and a hand moves a few pixels during a
/// click, onto its first item. Held longer, it's a press, drag, release.
const OPENING_CLICK: std::time::Duration = std::time::Duration::from_millis(300);

impl PointerGrab<Pila> for WindowMenuGrab {
    fn motion(
        &mut self,
        data: &mut Pila,
        handle: &mut PointerInnerHandle<'_, Pila>,
        _focus: Option<(crate::focus::PointerFocus, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        let hover = data
            .window_menu
            .as_ref()
            .and_then(|menu| menu.row_at(event.location));
        data.set_menu_hover(hover);
    }

    fn relative_motion(
        &mut self,
        data: &mut Pila,
        handle: &mut PointerInnerHandle<'_, Pila>,
        _focus: Option<(crate::focus::PointerFocus, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, None, event);
    }

    fn button(
        &mut self,
        data: &mut Pila,
        handle: &mut PointerInnerHandle<'_, Pila>,
        event: &ButtonEvent,
    ) {
        if event.serial == self.opening {
            return;
        }
        let location = handle.current_location();
        let Some(menu) = data.window_menu.as_ref() else {
            handle.unset_grab(self, data, event.serial, event.time, true);
            return;
        };
        let row = menu.row_at(location);
        let inside = menu.rect.to_f64().contains(location);
        // Inside a grab the pointer is locked: end it through `handle`
        // (`unset` drops the menu), never `Pila::close_window_menu`.
        match event.state {
            ButtonState::Released
                if event.button == self.start_data.button
                    && self.opened.elapsed() < OPENING_CLICK => {}
            ButtonState::Released => {
                if let Some((window, item)) = row.and_then(|row| data.menu_row(row)) {
                    handle.unset_grab(self, data, event.serial, event.time, true);
                    // Items move focus and the pointer's: after this
                    // callback, which holds the pointer.
                    data.loop_handle
                        .insert_idle(move |state| state.run_menu_item(&window, item));
                    data.wake.ping();
                }
            }
            // Elsewhere: closed. A right press still counts (another title
            // bar's menu, an app's own): replayed once the grab is gone and
            // the pointer's focus is what's under it.
            ButtonState::Pressed if !inside => {
                handle.unset_grab(self, data, event.serial, event.time, true);
                if event.button == crate::input::BTN_RIGHT {
                    data.loop_handle.insert_idle(|state| {
                        state.refresh_pointer_focus();
                        let time = smithay::backend::input::InputTime::now();
                        state.pointer_button(crate::input::BTN_RIGHT, ButtonState::Pressed, time);
                    });
                    data.wake.ping();
                }
            }
            ButtonState::Pressed => {}
        }
    }

    fn axis(&mut self, _: &mut Pila, _: &mut PointerInnerHandle<'_, Pila>, _: AxisFrame) {}

    fn frame(&mut self, data: &mut Pila, handle: &mut PointerInnerHandle<'_, Pila>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        _: &mut Pila,
        _: &mut PointerInnerHandle<'_, Pila>,
        _: &GestureSwipeBeginEvent,
    ) {
    }

    fn gesture_swipe_update(
        &mut self,
        _: &mut Pila,
        _: &mut PointerInnerHandle<'_, Pila>,
        _: &GestureSwipeUpdateEvent,
    ) {
    }

    fn gesture_swipe_end(
        &mut self,
        _: &mut Pila,
        _: &mut PointerInnerHandle<'_, Pila>,
        _: &GestureSwipeEndEvent,
    ) {
    }

    fn gesture_pinch_begin(
        &mut self,
        _: &mut Pila,
        _: &mut PointerInnerHandle<'_, Pila>,
        _: &GesturePinchBeginEvent,
    ) {
    }

    fn gesture_pinch_update(
        &mut self,
        _: &mut Pila,
        _: &mut PointerInnerHandle<'_, Pila>,
        _: &GesturePinchUpdateEvent,
    ) {
    }

    fn gesture_pinch_end(
        &mut self,
        _: &mut Pila,
        _: &mut PointerInnerHandle<'_, Pila>,
        _: &GesturePinchEndEvent,
    ) {
    }

    fn gesture_hold_begin(
        &mut self,
        _: &mut Pila,
        _: &mut PointerInnerHandle<'_, Pila>,
        _: &GestureHoldBeginEvent,
    ) {
    }

    fn gesture_hold_end(
        &mut self,
        _: &mut Pila,
        _: &mut PointerInnerHandle<'_, Pila>,
        _: &GestureHoldEndEvent,
    ) {
    }

    fn start_data(&self) -> &PointerGrabStartData<Pila> {
        &self.start_data
    }

    fn unset(&mut self, data: &mut Pila) {
        if data.window_menu.take().is_some() {
            data.queue_redraw();
        }
        data.set_cursor(CursorImageStatus::default_named());
    }
}

/// The open menu's elements, front to back, relative to `origin`.
pub fn elements(
    renderer: &mut GlesRenderer,
    menu: &SceneMenu,
    colors: &Colors,
    origin: Point<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<PilaRenderElement> {
    let mut buffers = menu.buffers.borrow_mut();
    let buffer_scale = crate::scale::buffer_scale(scale);
    let physical = |loc: Point<i32, Logical>| -> Point<i32, Physical> {
        (loc - origin).to_physical_precise_round(scale)
    };
    let solid = |slot: &mut Option<SolidColorBuffer>, size: Size<i32, Logical>, color: [f32; 4]| {
        let buffer = slot.get_or_insert_with(|| SolidColorBuffer::new(size, color));
        buffer.update(size, color);
        buffer.clone()
    };
    let opaque = |[r, g, b]: [f32; 3]| [r, g, b, 1.0];
    let mut elements = Vec::new();

    for (index, (label, checked)) in menu.rows.iter().enumerate() {
        let row = row_rect(menu.rect, index);
        let key = (*label, buffer_scale);
        if let std::collections::hash_map::Entry::Vacant(slot) = buffers.labels.entry(key) {
            let width = WIDTH - TEXT_INSET * 2 - MARK_SIZE;
            if let Some((pixels, w)) =
                crate::text::render_title(label, colors.icon, width, ITEM_HEIGHT, buffer_scale)
            {
                let buffer = MemoryRenderBuffer::from_slice(
                    &pixels,
                    Fourcc::Argb8888,
                    (w, ITEM_HEIGHT * buffer_scale),
                    buffer_scale,
                    Transform::Normal,
                    None,
                );
                slot.insert((buffer, w / buffer_scale));
            }
        }
        if let Some((buffer, _)) = buffers.labels.get(&key) {
            let loc = (row.loc + Point::from((TEXT_INSET, 0))).to_f64();
            let loc = (loc - origin.to_f64()).to_physical(scale);
            if let Ok(element) = MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                loc,
                buffer,
                None,
                None,
                None,
                Kind::Unspecified,
            ) {
                elements.push(PilaRenderElement::Memory(element));
            }
        }
        if *checked {
            let size = Size::from((MARK_SIZE, MARK_SIZE));
            let buffer = solid(&mut buffers.mark, size, opaque(colors.icon));
            let at = row.loc
                + Point::from((
                    row.size.w - TEXT_INSET - MARK_SIZE,
                    (ITEM_HEIGHT - MARK_SIZE) / 2,
                ));
            elements.push(PilaRenderElement::Solid(
                SolidColorRenderElement::from_buffer(
                    &buffer,
                    physical(at),
                    scale,
                    1.0,
                    Kind::Unspecified,
                ),
            ));
        }
    }
    if let Some(index) = menu.hover {
        let row = row_rect(menu.rect, index);
        let buffer = solid(&mut buffers.hover, row.size, opaque(colors.hover));
        elements.push(PilaRenderElement::Solid(
            SolidColorRenderElement::from_buffer(
                &buffer,
                physical(row.loc),
                scale,
                1.0,
                Kind::Unspecified,
            ),
        ));
    }
    let buffer = solid(&mut buffers.background, menu.rect.size, colors.bar);
    elements.push(PilaRenderElement::Solid(
        SolidColorRenderElement::from_buffer(
            &buffer,
            physical(menu.rect.loc),
            scale,
            1.0,
            Kind::Unspecified,
        ),
    ));
    elements
}
