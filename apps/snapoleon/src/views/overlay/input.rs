//! One window's input for one frame, read once so the interaction code does not touch egui's input state.

use egui::{Context, Event, Key, MouseWheelUnit, Order, PointerButton, Pos2};

pub struct Input {
    /// Pointer in this window's points; `None` when it is elsewhere.
    pub pos: Option<Pos2>,
    pub pressed: bool,
    pub released: bool,
    pub down: bool,
    pub double_click: bool,
    pub secondary_pressed: bool,
    pub shift: bool,
    /// Ctrl, or Cmd on macOS.
    pub ctrl: bool,
    pub alt: bool,
    pub space: bool,
    /// This window has keyboard focus.
    pub focused: bool,
    /// The pointer is over a toolbar or other interactive egui area.
    pub over_ui: bool,
    /// Anything happened this frame that other windows of the session may need to redraw for.
    pub active: bool,
    /// Wheel notches this frame; positive = away from the user (up).
    pub wheel: f32,
    /// Arrow key presses this frame (repeats included) as (right - left, down - up).
    pub arrows: (i32, i32),
    keys: Vec<Key>,
}

impl Input {
    pub fn read(ctx: &Context) -> Self {
        let mut input = ctx.input(|i| {
            let mut keys = Vec::new();
            let mut wheel = 0.0;
            let mut arrows = (0, 0);
            for event in &i.events {
                match event {
                    Event::Key { key, pressed: true, .. } => {
                        keys.push(*key);
                        match key {
                            Key::ArrowLeft => arrows.0 -= 1,
                            Key::ArrowRight => arrows.0 += 1,
                            Key::ArrowUp => arrows.1 -= 1,
                            Key::ArrowDown => arrows.1 += 1,
                            _ => {}
                        }
                    }
                    Event::MouseWheel { unit, delta, .. } => {
                        wheel += delta.y
                            * match unit {
                                MouseWheelUnit::Line => 1.0,
                                MouseWheelUnit::Point => 1.0 / 40.0,
                                MouseWheelUnit::Page => 4.0,
                            };
                    }
                    _ => {}
                }
            }
            Input {
                pos: i.pointer.latest_pos(),
                pressed: i.pointer.primary_pressed(),
                released: i.pointer.primary_released(),
                down: i.pointer.primary_down(),
                double_click: i.pointer.button_double_clicked(PointerButton::Primary),
                secondary_pressed: i.pointer.button_pressed(PointerButton::Secondary),
                shift: i.modifiers.shift,
                ctrl: i.modifiers.command,
                alt: i.modifiers.alt,
                space: i.key_down(Key::Space),
                focused: i.focused,
                over_ui: false,
                active: !i.events.is_empty(),
                wheel,
                arrows,
                keys,
            }
        });
        // Outside the input closure: the layer lookup takes the context lock itself.
        input.over_ui = input.pos.is_some_and(|p| ctx.layer_id_at(p).is_some_and(|layer| layer.order != Order::Background));
        input
    }

    /// `key` went down this frame with exactly these modifiers.
    pub fn hotkey(&self, key: Key, ctrl: bool, shift: bool) -> bool {
        self.keys.contains(&key) && self.ctrl == ctrl && self.shift == shift && !self.alt
    }

    pub fn pressed_key(&self, key: Key) -> bool {
        self.keys.contains(&key)
    }
}
