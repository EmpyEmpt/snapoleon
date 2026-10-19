//! Chip, kbd keycaps and badge.

use egui::{Response, Role, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo};

use super::util::{self, focus_ring, hover_anim, lerp_color};
use crate::{
    fonts, text,
    theme::{Palette, Tone, with_alpha},
};

/// Pill-shaped filter chip. Clicking toggles whatever the caller binds to `selected`.
pub fn chip(ui: &mut Ui, label: &str, selected: bool) -> Response {
    chip_impl(ui, None, label, selected)
}

/// [`chip`] with a leading icon.
pub fn chip_with_icon(ui: &mut Ui, icon: &str, label: &str, selected: bool) -> Response {
    chip_impl(ui, Some(icon), label, selected)
}

fn chip_impl(ui: &mut Ui, icon: Option<&str>, label: &str, selected: bool) -> Response {
    let p = Palette::of(ui);
    let style = text::body_style();
    let height = 26.0;
    let galley = util::layout(ui, label, &style, p.text_dim);
    let icon_w = if icon.is_some() { 14.0 + 6.0 } else { 0.0 };
    let size = Vec2::new(galley.size().x + icon_w + 24.0, height);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| WidgetInfo::selected(Role::Button, ui.is_enabled(), selected, label));

    if ui.is_rect_visible(rect) {
        let sel = util::anim(ui.ctx(), resp.id.with("sel"), selected, util::Motion::HOVER);
        let hover = hover_anim(ui, &resp);
        let round = height / 2.0;
        let fill = lerp_color(with_alpha(p.surface_2, hover), p.accent_soft, sel);
        let stroke = lerp_color(
            lerp_color(p.border, p.border_strong(), hover),
            with_alpha(p.accent, 0.45),
            sel,
        );
        let fg = lerp_color(lerp_color(p.text_dim, p.text, hover), p.accent, sel);
        let painter = ui.painter();
        painter.rect_filled(rect, round, fill);
        painter.rect_stroke(rect, round, Stroke::new(1.0, stroke), StrokeKind::Inside);
        let mut x = rect.left() + 12.0;
        if let Some(icon) = icon {
            util::paint_icon(ui, egui::pos2(x + 7.0, rect.center().y), icon, 14.0, fg);
            x += icon_w;
        }
        let galley = util::layout(ui, label, &style, fg);
        let pos = util::snap(ui, egui::pos2(x, rect.center().y - galley.size().y / 2.0));
        ui.painter().galley(pos, galley, fg);
        focus_ring(ui, &resp, rect, round);
    }
    resp.on_hover_cursor(egui::CursorIcon::Default)
}

/// Splits an accelerator like `Ctrl+Shift+4` into display labels, using symbols on macOS.
pub fn accel_parts(accel: &str) -> Vec<String> {
    accel
        .split('+')
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(key_label)
        .collect()
}

fn key_label(key: &str) -> String {
    let mac = cfg!(target_os = "macos");
    let pick = |mac_label: &str, other: &str| if mac { mac_label } else { other }.to_owned();
    match key.to_ascii_lowercase().as_str() {
        "cmdorctrl" | "commandorcontrol" => pick("⌘", "Ctrl"),
        "cmd" | "command" | "super" | "win" | "meta" => {
            if mac {
                "⌘".into()
            } else if cfg!(windows) {
                "Win".into()
            } else {
                "Super".into()
            }
        }
        "ctrl" | "control" => pick("⌃", "Ctrl"),
        "alt" | "option" => pick("⌥", "Alt"),
        "shift" => pick("⇧", "Shift"),
        "printscreen" | "prtsc" | "prtscn" => "PrtSc".into(),
        "escape" | "esc" => "Esc".into(),
        "backspace" => pick("⌫", "Backspace"),
        "delete" | "del" => pick("⌦", "Del"),
        "enter" | "return" => pick("↵", "Enter"),
        "pageup" => "PgUp".into(),
        "pagedown" => "PgDn".into(),
        "up" | "arrowup" => "↑".into(),
        "down" | "arrowdown" => "↓".into(),
        "left" | "arrowleft" => "←".into(),
        "right" | "arrowright" => "→".into(),
        _ if key.chars().count() == 1 => key.to_uppercase(),
        _ => key.to_owned(),
    }
}

/// Keyboard shortcut rendered as a row of keycaps.
pub fn kbd(ui: &mut Ui, accel: &str) -> Response {
    let parts = accel_parts(accel);
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(keycaps_width(ui, &parts), KEYCAP_H),
        Sense::hover(),
    );
    if ui.is_rect_visible(rect) {
        paint_keycaps(ui, rect.left_center(), &parts, false);
    }
    resp
}

const KEYCAP_H: f32 = 20.0;
const KEYCAP_GAP: f32 = 3.0;

fn keycap_galley(ui: &Ui, label: &str, color: egui::Color32) -> std::sync::Arc<egui::Galley> {
    let font = egui::FontId::new(11.0, egui::FontFamily::Monospace);
    ui.painter().layout_no_wrap(label.to_owned(), font, color)
}

fn keycap_width(ui: &Ui, label: &str) -> f32 {
    (keycap_galley(ui, label, egui::Color32::PLACEHOLDER)
        .size()
        .x
        + 10.0)
        .max(20.0)
}

pub(crate) fn keycaps_width(ui: &Ui, parts: &[String]) -> f32 {
    parts.iter().map(|k| keycap_width(ui, k)).sum::<f32>()
        + KEYCAP_GAP * parts.len().saturating_sub(1) as f32
}

/// Paints keycaps left to right starting at `left_center`; `active` tints them with the accent.
/// Returns the total width painted.
pub(crate) fn paint_keycaps(
    ui: &Ui,
    left_center: egui::Pos2,
    parts: &[String],
    active: bool,
) -> f32 {
    let p = Palette::of(ui);
    let (fg, edge, face) = if active {
        (p.accent, with_alpha(p.accent, 0.5), p.accent_soft)
    } else {
        (p.text_dim, p.border, p.surface_2)
    };
    let mut x = left_center.x;
    for part in parts {
        let galley = keycap_galley(ui, part, fg);
        let size = Vec2::new(keycap_width(ui, part), KEYCAP_H);
        let rect = egui::Rect::from_min_size(egui::pos2(x, left_center.y - KEYCAP_H / 2.0), size);
        let body = rect.with_max_y(rect.max.y - 1.0);
        let painter = ui.painter();
        // A 1 px darker underside gives the cap its key-like edge.
        painter.rect_filled(rect, 5.0, edge);
        painter.rect_filled(body, 5.0, if active { p.surface_2 } else { face });
        if active {
            painter.rect_filled(body, 5.0, face);
        }
        painter.rect_stroke(body, 5.0, Stroke::new(1.0, edge), StrokeKind::Inside);
        util::paint_centered(ui, body.center(), galley, fg);
        x += size.x + KEYCAP_GAP;
    }
    x - left_center.x - if parts.is_empty() { 0.0 } else { KEYCAP_GAP }
}

/// Small status pill, e.g. a count or "Beta".
pub fn badge(ui: &mut Ui, label: &str, tone: Tone) -> Response {
    let p = Palette::of(ui);
    let font = egui::FontId::new(11.0, fonts::semibold());
    let fg = if tone == Tone::Neutral {
        p.text_dim
    } else {
        p.tone(tone)
    };
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, fg);
    let size = Vec2::new((galley.size().x + 12.0).max(20.0), 20.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let fill = p.tone_soft(tone);
        painter.rect_filled(rect, rect.height() / 2.0, fill);
        if tone == Tone::Neutral {
            painter.rect_stroke(
                rect,
                rect.height() / 2.0,
                Stroke::new(1.0, p.border),
                StrokeKind::Inside,
            );
        }
        util::paint_centered(ui, rect.center(), galley, fg);
    }
    resp
}
