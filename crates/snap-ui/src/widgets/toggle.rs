//! Pill switch and sliding segmented control.

use egui::{Pos2, Rect, Response, Role, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo};

use super::util::{self, Motion, anim, anim_value, control_height, focus_ring, hover_anim, lerp_color};
use crate::{
    text::{body_strong_style, body_style},
    theme::Palette,
};

/// Animated pill switch. `changed()` is set on the returned response when the value flips.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let p = Palette::of(ui);
    let row = control_height(ui);
    let compact = row < 30.0;
    let (w, h) = if compact { (34.0, 20.0) } else { (40.0, 22.0) };
    let (outer, mut resp) = ui.allocate_exact_size(Vec2::new(w, row), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp.widget_info(|| WidgetInfo::selected(Role::Switch, ui.is_enabled(), *on, ""));

    if ui.is_rect_visible(outer) {
        let rect = Rect::from_center_size(outer.center(), Vec2::new(w, h));
        let t = anim(ui.ctx(), resp.id.with("on"), *on, Motion::SLIDE);
        let hover = hover_anim(ui, &resp);
        let round = h / 2.0;

        let off_fill = lerp_color(p.surface_2, p.border, hover * 0.6);
        let fill = lerp_color(off_fill, lerp_color(p.accent, p.on_accent, hover * 0.1), t);
        let stroke = lerp_color(lerp_color(p.border, p.border_strong(), hover), p.accent, t);
        let painter = ui.painter();
        painter.rect_filled(rect, round, fill);
        painter.rect_stroke(rect, round, Stroke::new(1.0, stroke), StrokeKind::Inside);

        let pressed = resp.is_pointer_button_down_on();
        let knob_r = (h - 6.0) / 2.0;
        let stretch = if pressed { 2.0 } else { 0.0 };
        let left = rect.left() + 3.0 + knob_r;
        let right = rect.right() - 3.0 - knob_r;
        let cx = left + (right - left) * t;
        let knob =
            Rect::from_center_size(Pos2::new(cx + stretch * (t - 0.5), rect.center().y), Vec2::new(knob_r * 2.0 + stretch, knob_r * 2.0));
        let knob_color = lerp_color(lerp_color(p.text_dim, p.text, hover * 0.6), p.on_accent, t);
        painter.rect_filled(knob.translate(Vec2::new(0.0, 1.0)), knob_r, egui::Color32::from_black_alpha(if p.dark { 70 } else { 36 }));
        painter.rect_filled(knob, knob_r, knob_color);
        focus_ring(ui, &resp, rect, round);
    }
    resp
}

/// Checkbox with a label. `changed()` is set when the value flips.
pub fn checkbox(ui: &mut Ui, checked: &mut bool, label: &str) -> Response {
    let p = Palette::of(ui);
    let style = body_style();
    let galley = util::layout(ui, label, &style, p.text);
    let size = Vec2::new(16.0 + 8.0 + galley.size().x, control_height(ui).min(28.0));
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *checked = !*checked;
        resp.mark_changed();
    }
    resp.widget_info(|| WidgetInfo::selected(Role::CheckBox, ui.is_enabled(), *checked, label));

    if ui.is_rect_visible(rect) {
        let t = anim(ui.ctx(), resp.id.with("on"), *checked, Motion::HOVER);
        let hover = hover_anim(ui, &resp);
        let boxr = Rect::from_min_size(Pos2::new(rect.left(), rect.center().y - 8.0), Vec2::splat(16.0));
        let round = 5.0;
        let fill = lerp_color(lerp_color(p.surface_2, p.border, hover * 0.6), p.accent, t);
        let stroke = lerp_color(lerp_color(p.border_strong(), p.text_faint, hover), p.accent, t);
        let painter = ui.painter();
        painter.rect_filled(boxr, round, fill);
        painter.rect_stroke(boxr, round, Stroke::new(1.0, stroke), StrokeKind::Inside);
        if t > 0.0 {
            let c = boxr.min;
            let tick = [Pos2::new(c.x + 3.6, c.y + 8.4), Pos2::new(c.x + 6.6, c.y + 11.2), Pos2::new(c.x + 12.2, c.y + 4.9)];
            painter.line(tick.to_vec(), Stroke::new(1.8, p.on_accent.gamma_multiply(t)));
        }
        let pos = Pos2::new(rect.left() + 24.0, rect.center().y - galley.size().y / 2.0);
        ui.painter().galley(util::snap(ui, pos), galley, p.text);
        focus_ring(ui, &resp, boxr, round);
    }
    resp
}

/// Segmented control with a sliding selection. `changed()` is set when the selection moves.
pub fn segmented<T: PartialEq + Clone>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> Response {
    let p = Palette::of(ui);
    let height = control_height(ui);
    let style = body_strong_style();
    let pad = 3.0;
    let seg_h = height - pad * 2.0;
    let seg_w = options.iter().map(|(_, l)| util::layout(ui, l, &style, p.text).size().x).fold(0.0_f32, f32::max) + 24.0;
    let total = Vec2::new(seg_w * options.len() as f32 + pad * 2.0, height);
    let (rect, mut resp) = ui.allocate_exact_size(total, Sense::hover());
    let round = 8.0;

    let selected = options.iter().position(|(v, _)| v == value);
    let seg_rect =
        |i: usize| Rect::from_min_size(Pos2::new(rect.left() + pad + seg_w * i as f32, rect.top() + pad), Vec2::new(seg_w, seg_h));

    let mut clicked = None;
    let mut seg_resps = Vec::with_capacity(options.len());
    for (i, (_, label)) in options.iter().enumerate() {
        let r = ui.interact(seg_rect(i), resp.id.with(i), Sense::click());
        r.widget_info(|| WidgetInfo::selected(Role::RadioButton, ui.is_enabled(), selected == Some(i), *label));
        if r.clicked() {
            clicked = Some(i);
        }
        seg_resps.push(r);
    }
    if let Some(i) = clicked
        && selected != Some(i)
    {
        *value = options[i].0.clone();
        resp.mark_changed();
    }
    let selected = options.iter().position(|(v, _)| v == value);

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, round, p.surface_2);
        painter.rect_stroke(rect, round, Stroke::new(1.0, p.border), StrokeKind::Inside);

        if let Some(sel) = selected {
            let target = seg_rect(sel);
            let x = anim_value(ui.ctx(), resp.id.with("thumb"), target.left(), Motion::SLIDE);
            let thumb = Rect::from_min_size(Pos2::new(x, target.top()), target.size());
            painter.add(p.shadow_small().as_shape(thumb, 6.0));
            painter.rect_filled(thumb, 6.0, p.elevated);
            painter.rect_stroke(
                thumb,
                6.0,
                Stroke::new(1.0, p.border_strong().gamma_multiply(if p.dark { 0.7 } else { 0.55 })),
                StrokeKind::Inside,
            );
        }
        for (i, (r, (_, label))) in seg_resps.iter().zip(options).enumerate() {
            let sel = anim(ui.ctx(), resp.id.with(("sel", i)), selected == Some(i), Motion::SLIDE);
            let hover = hover_anim(ui, r);
            let fg = lerp_color(lerp_color(p.text_dim, p.text, hover), p.text, sel);
            let galley = util::layout(ui, label, &style, fg);
            util::paint_centered(ui, seg_rect(i).center(), galley, fg);
            if r.has_focus() {
                focus_ring(ui, r, seg_rect(i), 6.0);
            }
        }
    }
    resp
}
