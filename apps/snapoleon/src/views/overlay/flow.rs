//! What input does: selecting, snapping, picking colours, measuring. Runs once per window frame, before painting.

use egui::{Context, Key, Pos2, vec2};
use snap_core::{CaptureKind, Rect as PxRect, tf};

use super::{
    OverlayAction, OverlayMode,
    bars::BarEvent,
    input::Input,
    loupe::{self, ColorFormat},
    selection::{Limits, Release},
    session::{BarKind, Finish, Session},
    space::{Space, pixel_at, rect_f},
};
use crate::views::ViewCx;
use crate::views::colors::ColorSample;

/// How long "Copied #3D5AFE" stays on screen.
const FLASH: std::time::Duration = std::time::Duration::from_millis(1400);

impl Session {
    pub fn update(&mut self, cx: &mut ViewCx, ctx: &Context, input: &Input, space: Space, win: snap_shell::WindowKey) -> Option<Finish> {
        self.track_pointer(input, space, win);
        if input.focused {
            self.ctrl = input.ctrl;
        }
        if input.pressed_key(Key::Escape) {
            return Some(Finish::Cancel);
        }
        if input.pressed_key(Key::Enter) || input.arrows != (0, 0) {
            self.engaged = true;
        }
        if self.flash.as_ref().is_some_and(|(_, since)| since.elapsed() >= FLASH) {
            self.flash = None;
        } else if let Some((_, since)) = &self.flash {
            ctx.request_repaint_after(FLASH.saturating_sub(since.elapsed()));
        }
        self.look_up_elements(cx, ctx, input, win);
        let cursor = self.cursor.pos();

        match self.mode() {
            OverlayMode::ColorPicker => self.picker_flow(input, cursor),
            OverlayMode::Measure => self.measure_flow(input, cursor),
            OverlayMode::Window | OverlayMode::Element => self.target_flow(input, cursor),
            _ => self.region_flow(input, space, cursor),
        }
    }

    /// Turn a bar's button into the way the session ends. `None` for events that only change an option.
    pub fn bar_event(&mut self, cx: &mut ViewCx, event: BarEvent) -> Option<Finish> {
        let (rect, kind) = (self.sel.rect?, self.sel.kind);
        match event {
            BarEvent::Action(action) => Some(Finish::Action(action, rect, kind)),
            BarEvent::Confirm => Some(Finish::Selected(rect, kind)),
            BarEvent::Close => Some(Finish::Cancel),
            BarEvent::Audio => {
                let (microphone, system_audio) = (self.record.microphone, self.record.system_audio);
                cx.update_settings(|s| {
                    s.recording.microphone = microphone;
                    s.recording.system_audio = system_audio;
                });
                None
            }
        }
    }

    /// A colour picked from the recent-colors strip.
    pub fn pick_recent(&mut self, sample: ColorSample) -> Finish {
        self.finish_pick(sample)
    }

    fn track_pointer(&mut self, input: &Input, space: Space, win: snap_shell::WindowKey) {
        match input.pos {
            Some(local) => {
                let global = space.global(local);
                if self.cursor.raw != Some(global) {
                    self.cursor.nudge = vec2(0.0, 0.0);
                }
                self.cursor.raw = Some(global);
                self.cursor.owner = Some(win);
            }
            None if self.cursor.owner == Some(win) && !input.down && !self.sel.is_dragging() => {
                self.cursor.raw = None;
                self.cursor.owner = None;
            }
            None => {}
        }
    }

    /// Ask the accessibility API about the pointer's spot, and let the wheel walk up and down the chain.
    fn look_up_elements(&mut self, cx: &mut ViewCx, ctx: &Context, input: &Input, win: snap_shell::WindowKey) {
        if !self.wants_elements() || self.cursor.owner != Some(win) {
            return;
        }
        self.targets.scroll(input.wheel);
        if let Some(at) = self.point().filter(|_| !input.over_ui && !self.sel.is_dragging()) {
            self.targets.request(cx, ctx, at);
        }
    }

    /// Without a selection, arrow keys move the pointer by whole pixels.
    fn nudge_pointer(&mut self, input: &Input) {
        if input.arrows != (0, 0) && self.cursor.raw.is_some() {
            let step = if input.shift { 10.0 } else { 1.0 };
            self.cursor.nudge += vec2(input.arrows.0 as f32, input.arrows.1 as f32) * step;
        }
    }

    // --- region, OCR, pin, record, scroll ---

    fn region_flow(&mut self, input: &Input, space: Space, cursor: Option<Pos2>) -> Option<Finish> {
        if input.secondary_pressed {
            if self.sel.rect.is_none() && !self.sel.is_dragging() {
                return Some(Finish::Cancel);
            }
            self.sel.clear();
            self.pressing = false;
            return None;
        }
        if input.hotkey(Key::C, false, false) {
            self.copy_color_under_pointer();
        }
        if self.sel.rect.is_none() {
            self.nudge_pointer(input);
        }
        if !self.sel.is_dragging()
            && let Some(finish) = self.selection_keys(input)
        {
            return Some(finish);
        }

        let cursor = cursor?;
        if input.pressed && !input.over_ui {
            self.engaged = true;
            self.pressing = true;
            let hit = self.sel.hit(cursor, space.ppp);
            self.sel.press(cursor, hit);
        }
        if self.sel.is_dragging() {
            let limits = Limits {
                bounds: self.bounds,
                lines: &self.lines,
                square: input.shift,
                snap: self.opts.snap_to_windows && !input.alt,
                shift_all: input.space,
            };
            self.sel.drag(cursor, &limits);
            if input.released || !input.down {
                if self.sel.release() == Some(Release::Click)
                    && let Some(target) = self.hover_target()
                {
                    let rect = target.rect.intersect(self.bounds)?;
                    self.sel.set(rect, target.kind);
                }
                self.pressing = false;
                if self.finishes_on_release()
                    && let Some(rect) = self.sel.rect.filter(is_usable)
                {
                    return Some(Finish::Selected(rect, self.sel.kind));
                }
            }
        }
        if input.double_click
            && !input.over_ui
            && let Some(rect) = self.sel.rect.filter(is_usable)
            && rect_f(rect).contains(cursor)
        {
            return Some(Finish::Selected(rect, self.sel.kind));
        }
        None
    }

    /// Keys that act on a finished selection: nudge, accept, and the toolbar's hotkeys.
    fn selection_keys(&mut self, input: &Input) -> Option<Finish> {
        let Some(rect) = self.sel.rect else {
            return input.pressed_key(Key::Enter).then(|| self.enter_target());
        };
        if input.arrows != (0, 0) {
            let step = if input.shift { 10 } else { 1 };
            let (dx, dy) = (input.arrows.0 * step, input.arrows.1 * step);
            if input.ctrl {
                self.sel.grow(dx, dy, self.bounds);
            } else {
                self.sel.nudge(dx, dy, self.bounds);
            }
        }
        let rect = self.sel.rect.filter(is_usable).unwrap_or(rect);
        let kind = self.sel.kind;
        if input.pressed_key(Key::Enter) {
            return Some(Finish::Selected(rect, kind));
        }
        match self.bar()? {
            BarKind::Instant => {
                let action = instant_hotkey(input)?;
                Some(Finish::Action(action, rect, kind))
            }
            BarKind::Record => input.hotkey(Key::R, false, false).then_some(Finish::Selected(rect, kind)),
            BarKind::Scroll => input.hotkey(Key::S, false, false).then_some(Finish::Selected(rect, kind)),
        }
    }

    /// Enter with nothing selected: the window or element under the pointer, else the whole monitor.
    fn enter_target(&self) -> Finish {
        match self.hover_target().and_then(|t| Some((t.rect.intersect(self.bounds)?, t.kind))) {
            Some((rect, kind)) => Finish::Selected(rect, kind),
            None => Finish::Selected(self.monitor_rect(), CaptureKind::Monitor),
        }
    }

    // --- window and element ---

    fn target_flow(&mut self, input: &Input, cursor: Option<Pos2>) -> Option<Finish> {
        if input.secondary_pressed {
            return Some(Finish::Cancel);
        }
        self.nudge_pointer(input);
        if input.pressed && !input.over_ui {
            self.engaged = true;
            self.pressing = true;
        }
        if input.released && std::mem::take(&mut self.pressing) && cursor.is_some() {
            let target = self.hover_target()?;
            return Some(Finish::Selected(target.rect.intersect(self.bounds)?, target.kind));
        }
        input.pressed_key(Key::Enter).then(|| self.enter_target())
    }

    // --- colour picker ---

    fn picker_flow(&mut self, input: &Input, cursor: Option<Pos2>) -> Option<Finish> {
        if input.secondary_pressed {
            return Some(Finish::Cancel);
        }
        // Shift cycles HEX, RGB, HSL, OKLCH; only the focused window sees the key.
        if input.focused {
            if input.shift && !self.shift_was_down {
                self.format = self.format.next();
                self.engaged = true;
            }
            self.shift_was_down = input.shift;
        }
        self.nudge_pointer(input);
        if input.pressed && !input.over_ui {
            self.engaged = true;
            self.pressing = true;
        }
        let at = pixel_at(cursor?);
        let clicked = input.released && std::mem::take(&mut self.pressing);
        if input.hotkey(Key::C, false, false) {
            // C always copies HEX, whatever the readout shows.
            return self.frozen.pixel(at).map(|rgba| {
                copy_text(&ColorFormat::Hex.write(rgba));
                self.finish_pick(ColorSample { rgba, at })
            });
        }
        if clicked || input.pressed_key(Key::Enter) {
            let rgba = self.frozen.pixel(at)?;
            copy_text(&self.format.write(rgba));
            return Some(self.finish_pick(ColorSample { rgba, at }));
        }
        None
    }

    fn finish_pick(&mut self, sample: ColorSample) -> Finish {
        loupe::remember(sample);
        Finish::Color(sample)
    }

    /// The recent-colors strip: copy that colour in the current format.
    pub fn copy_recent(&self, sample: ColorSample) {
        copy_text(&self.format.write(sample.rgba));
    }

    /// `C` outside the picker: copy the HEX of the pixel under the pointer and say so.
    fn copy_color_under_pointer(&mut self) {
        let Some(rgba) = self.point().and_then(|p| self.frozen.pixel(p)) else { return };
        let hex = ColorFormat::Hex.write(rgba);
        copy_text(&hex);
        self.flash(tf("overlay.copied", &[("value", &hex)]));
    }

    // --- measure ---

    fn measure_flow(&mut self, input: &Input, cursor: Option<Pos2>) -> Option<Finish> {
        if input.secondary_pressed {
            if self.measure.is_empty() {
                return Some(Finish::Cancel);
            }
            self.measure.clear();
            return None;
        }
        self.nudge_pointer(input);
        let cursor = cursor?;
        if input.pressed && !input.over_ui {
            self.engaged = true;
            self.measure.press(cursor);
        }
        if input.down {
            self.measure.drag(cursor, input.shift);
        }
        if input.released {
            let target = self.hover_target();
            self.measure.release(target.as_ref());
        }
        if input.pressed_key(Key::Enter) {
            return self.measure.result().map(Finish::Measured);
        }
        None
    }
}

/// A selection worth acting on.
fn is_usable(r: &PxRect) -> bool {
    r.w >= 2 && r.h >= 2
}

/// The instant toolbar's keyboard shortcuts, as their tooltips advertise them.
fn instant_hotkey(input: &Input) -> Option<OverlayAction> {
    let plain = |key| input.hotkey(key, false, false);
    if input.hotkey(Key::C, true, false) {
        Some(OverlayAction::Copy)
    } else if input.hotkey(Key::S, true, false) {
        Some(OverlayAction::Save)
    } else if input.hotkey(Key::R, false, true) {
        Some(OverlayAction::Record)
    } else if plain(Key::E) {
        Some(OverlayAction::Edit)
    } else if plain(Key::P) {
        Some(OverlayAction::Pin)
    } else if plain(Key::U) {
        Some(OverlayAction::Upload)
    } else if plain(Key::T) {
        Some(OverlayAction::Text)
    } else if plain(Key::Q) {
        Some(OverlayAction::Qr)
    } else if plain(Key::R) {
        Some(OverlayAction::Redact)
    } else if plain(Key::S) {
        Some(OverlayAction::Scroll)
    } else {
        None
    }
}

fn copy_text(text: &str) {
    if let Err(e) = snap_platform::clipboard::set_text(text) {
        tracing::warn!("cannot copy {text} to the clipboard: {e}");
    }
}
