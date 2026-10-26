//! Hotkey recorder: click, press a combination, get an accelerator string like `Ctrl+Shift+4`.

use std::time::Duration;

use egui::{Event, EventFilter, Key, Modifiers, Pos2, Rect, Response, Role, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo};
use snap_core::t;

use super::{
    chips::{accel_parts, paint_keycaps},
    util::{self, Motion, anim, control_height, focus_ring, hover_anim, lerp_color},
};
use crate::{
    icon, platform,
    text::{body_style, caption_style},
    theme::{Palette, radius, with_alpha},
};

/// How long each recording hint ("Esc to cancel", "Backspace to clear") stays before the other one takes its place.
const HINT_SECS: f64 = 2.5;

/// State kept by the caller across frames.
#[derive(Debug, Default)]
pub struct RecorderState {
    /// Shows the field in the danger color, e.g. after `snap_platform::hotkeys::validate` failed.
    pub invalid: bool,
    /// OS-polled key seen on the previous frame; `None` until recording has started.
    prev_extra_key: Option<Option<&'static str>>,
}

enum Outcome {
    Commit(String),
    Clear,
    Cancel,
}

/// Records a global-hotkey accelerator into `accel`.
///
/// While focused it shows "Press keys…" plus the modifiers currently held, with "Esc to cancel" and "Backspace to
/// clear" taking turns beside it and both in its tooltip. `Esc` cancels, `Backspace` or `Delete` clears. Bare letters
/// are rejected (a global hotkey needs a modifier), but F-keys, PrintScreen and similar standalone keys are accepted.
/// `changed()` is set when `accel` is rewritten.
///
/// PrintScreen, Pause, ScrollLock and the Windows key cannot be seen through egui, so on Windows they
/// are sampled from the OS; on macOS/Linux they cannot be recorded.
pub fn hotkey_recorder(ui: &mut Ui, accel: &mut String, state: &mut RecorderState) -> Response {
    let p = Palette::of(ui);
    let width = ui.available_width().clamp(210.0, 270.0);
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(width, control_height(ui)), Sense::click());
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, ui.is_enabled(), accel.as_str()));
    if resp.clicked() {
        resp.request_focus();
    }
    let recording = resp.has_focus() && ui.is_enabled();
    let mut held = Vec::new();

    if recording {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(resp.id, EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true });
        });
        let extra = platform::extra_keys_down();
        let outcome = ui.input_mut(|i| {
            let mods = i.modifiers;
            held = held_modifiers(mods, extra.super_key);
            let mut outcome = None;
            for event in &i.events {
                if let Event::Key { key, physical_key, pressed: true, repeat: false, modifiers, .. } = event {
                    outcome = outcome.or(classify(physical_key.unwrap_or(*key), *modifiers, extra.super_key));
                }
            }
            // A key already held when recording began must be released and pressed again.
            if outcome.is_none()
                && let Some(name) = extra.key
                && state.prev_extra_key.is_some_and(|prev| prev != Some(name))
            {
                outcome = Some(Outcome::Commit(build(mods, extra.super_key, name)));
            }
            state.prev_extra_key = Some(extra.key);
            // Keep the recorded keys away from the rest of the UI (Esc must not close the dialog, etc.).
            i.events.retain(|e| !matches!(e, Event::Key { .. } | Event::Text(_)));
            outcome
        });
        match outcome {
            Some(Outcome::Commit(s)) => {
                *accel = s;
                state.invalid = false;
                resp.mark_changed();
                resp.surrender_focus();
            }
            Some(Outcome::Clear) => {
                accel.clear();
                state.invalid = false;
                resp.mark_changed();
                resp.surrender_focus();
            }
            Some(Outcome::Cancel) => resp.surrender_focus(),
            None => {}
        }
        ui.ctx().request_repaint_after(Duration::from_millis(33));
    } else {
        state.prev_extra_key = None;
    }
    let recording = recording && resp.has_focus();
    if recording {
        resp = resp.on_hover_text(t("ui.hotkey.recording_tip"));
    }

    // Clear button, only on a hovered, filled, idle field.
    let clear_rect = Rect::from_center_size(Pos2::new(rect.right() - 18.0, rect.center().y), Vec2::splat(22.0));
    let hovered = ui.rect_contains_pointer(rect);
    let show_clear = !recording && !accel.is_empty() && hovered && ui.is_enabled();
    let clear = show_clear.then(|| ui.interact(clear_rect, resp.id.with("clear"), Sense::click()).on_hover_text(t("ui.hotkey.clear")));
    if clear.as_ref().is_some_and(Response::clicked) {
        accel.clear();
        state.invalid = false;
        resp.mark_changed();
    }

    if ui.is_rect_visible(rect) {
        let hover = hover_anim(ui, &resp);
        let active = anim(ui.ctx(), resp.id.with("rec"), recording, Motion::HOVER);
        let round = radius::CARD;
        let painter = ui.painter();
        painter.rect_filled(rect, round, p.surface_2);
        painter.rect_filled(rect, round, p.hover().gamma_multiply(hover * (1.0 - active)));
        let idle_border = if state.invalid { p.danger } else { lerp_color(p.border, p.border_strong(), hover) };
        painter.rect_stroke(rect, round, Stroke::new(1.0, lerp_color(idle_border, p.accent, active)), StrokeKind::Inside);
        if active > 0.0 {
            let pulse = 0.22 + 0.12 * ((ui.input(|i| i.time) * 4.0).sin() as f32 * 0.5 + 0.5);
            painter.rect_stroke(rect, round, Stroke::new(2.0, with_alpha(p.accent, pulse * active)), StrokeKind::Outside);
        }

        let origin = Pos2::new(rect.left() + 10.0, rect.center().y);
        if recording {
            if held.is_empty() {
                let galley = util::layout(ui, &t("ui.hotkey.press_keys"), &body_style(), p.text_dim);
                let end = origin.x + 2.0 + galley.size().x;
                ui.painter().galley(util::snap(ui, Pos2::new(origin.x + 2.0, rect.center().y - galley.size().y / 2.0)), galley, p.text_dim);
                // The two keys take turns in the one spot there is room for; the one whose turn it is falls back to
                // the other where it does not fit (long words in a narrow field).
                let clear_first = (ui.input(|i| i.time) / HINT_SECS) as i64 % 2 == 1;
                let keys = if clear_first {
                    ["ui.hotkey.clear_hint", "ui.hotkey.esc_hint"]
                } else {
                    ["ui.hotkey.esc_hint", "ui.hotkey.clear_hint"]
                };
                let fits = keys
                    .into_iter()
                    .map(|key| util::layout(ui, &t(key), &caption_style(), p.text_faint))
                    .map(|hint| (rect.right() - 10.0 - hint.size().x, hint))
                    .find(|(hint_x, _)| *hint_x > end + 12.0);
                if let Some((hint_x, hint)) = fits {
                    ui.painter().galley(util::snap(ui, Pos2::new(hint_x, rect.center().y - hint.size().y / 2.0)), hint, p.text_faint);
                }
            } else {
                let x = origin.x + paint_keycaps(ui, origin, &held, true) + 8.0;
                let galley = util::layout(ui, "…", &body_style(), p.text_dim);
                ui.painter().galley(util::snap(ui, Pos2::new(x, rect.center().y - galley.size().y / 2.0)), galley, p.text_dim);
            }
        } else if accel.is_empty() {
            let galley = util::layout(ui, &t("ui.hotkey.not_set"), &body_style(), p.text_faint);
            let pos = Pos2::new(origin.x + 2.0, rect.center().y - galley.size().y / 2.0);
            ui.painter().galley(util::snap(ui, pos), galley, p.text_faint);
        } else {
            paint_keycaps(ui, origin, &accel_parts(accel), false);
        }

        if let Some(clear) = &clear {
            let h = hover_anim(ui, clear);
            ui.painter().circle_filled(clear_rect.center(), 10.0, p.hover().gamma_multiply(h));
            util::paint_icon(ui, clear_rect.center(), icon::X, 12.0, lerp_color(p.text_faint, p.text, h));
        }
        focus_ring(ui, &resp, rect, round);
    }
    resp
}

fn held_modifiers(m: Modifiers, super_key: bool) -> Vec<String> {
    let mut parts = Vec::new();
    for (on, name) in [(m.ctrl, "Ctrl"), (m.alt, "Alt"), (m.shift, "Shift"), (m.mac_cmd, "Cmd"), (super_key, "Win")] {
        if on {
            parts.push(name);
        }
    }
    accel_parts(&parts.join("+"))
}

fn classify(key: Key, mods: Modifiers, super_key: bool) -> Option<Outcome> {
    let any_mod = mods.ctrl || mods.alt || mods.shift || mods.mac_cmd || super_key;
    match key {
        Key::Escape if !any_mod => Some(Outcome::Cancel),
        Key::Backspace | Key::Delete if !any_mod => Some(Outcome::Clear),
        _ => {
            let name = key_name(key)?;
            (any_mod || standalone_ok(key)).then(|| Outcome::Commit(build(mods, super_key, name)))
        }
    }
}

/// Modifiers in the canonical order, then the key.
fn build(mods: Modifiers, super_key: bool, key: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if mods.ctrl {
        parts.push("Ctrl");
    }
    if mods.alt {
        parts.push("Alt");
    }
    if mods.shift {
        parts.push("Shift");
    }
    if mods.mac_cmd {
        parts.push("Cmd");
    }
    if super_key {
        parts.push(if cfg!(windows) { "Win" } else { "Super" });
    }
    parts.push(key);
    parts.join("+")
}

/// Keys that make sense as a global hotkey without a modifier.
fn standalone_ok(key: Key) -> bool {
    use Key::*;
    matches!(
        key,
        F1 | F2
            | F3
            | F4
            | F5
            | F6
            | F7
            | F8
            | F9
            | F10
            | F11
            | F12
            | F13
            | F14
            | F15
            | F16
            | F17
            | F18
            | F19
            | F20
            | F21
            | F22
            | F23
            | F24
    )
}

fn key_name(key: Key) -> Option<&'static str> {
    use Key::*;
    Some(match key {
        ArrowDown => "Down",
        ArrowLeft => "Left",
        ArrowRight => "Right",
        ArrowUp => "Up",
        Escape => "Escape",
        Tab => "Tab",
        Backspace => "Backspace",
        Enter => "Enter",
        Space => "Space",
        Insert => "Insert",
        Delete => "Delete",
        Home => "Home",
        End => "End",
        PageUp => "PageUp",
        PageDown => "PageDown",
        Comma => ",",
        Period => ".",
        Slash => "/",
        Backslash => "\\",
        Semicolon => ";",
        Quote => "'",
        Backtick => "`",
        Minus => "-",
        Equals => "=",
        OpenBracket => "[",
        CloseBracket => "]",
        Num0 => "0",
        Num1 => "1",
        Num2 => "2",
        Num3 => "3",
        Num4 => "4",
        Num5 => "5",
        Num6 => "6",
        Num7 => "7",
        Num8 => "8",
        Num9 => "9",
        A => "A",
        B => "B",
        C => "C",
        D => "D",
        E => "E",
        F => "F",
        G => "G",
        H => "H",
        I => "I",
        J => "J",
        K => "K",
        L => "L",
        M => "M",
        N => "N",
        O => "O",
        P => "P",
        Q => "Q",
        R => "R",
        S => "S",
        T => "T",
        U => "U",
        V => "V",
        W => "W",
        X => "X",
        Y => "Y",
        Z => "Z",
        F1 => "F1",
        F2 => "F2",
        F3 => "F3",
        F4 => "F4",
        F5 => "F5",
        F6 => "F6",
        F7 => "F7",
        F8 => "F8",
        F9 => "F9",
        F10 => "F10",
        F11 => "F11",
        F12 => "F12",
        F13 => "F13",
        F14 => "F14",
        F15 => "F15",
        F16 => "F16",
        F17 => "F17",
        F18 => "F18",
        F19 => "F19",
        F20 => "F20",
        F21 => "F21",
        F22 => "F22",
        F23 => "F23",
        F24 => "F24",
        _ => return None,
    })
}
