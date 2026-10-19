//! Stroke-width picker with visual line weights.

use egui::{Pos2, Rect, Response, Role, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo};
use snap_core::tf;

use super::util::{Motion, anim, anim_value, control_height, focus_ring, hover_anim, lerp_color};
use crate::theme::Palette;

/// Widths offered by [`stroke_picker`], in pixels.
pub const DEFAULT_WIDTHS: [f32; 6] = [2.0, 3.0, 4.0, 6.0, 8.0, 12.0];

pub fn stroke_picker(ui: &mut Ui, width: &mut f32) -> Response {
    stroke_picker_with(ui, width, &DEFAULT_WIDTHS)
}

/// Segmented row where each cell previews its line weight; a sliding thumb marks the current one.
pub fn stroke_picker_with(ui: &mut Ui, width: &mut f32, presets: &[f32]) -> Response {
    let p = Palette::of(ui);
    let height = control_height(ui);
    let pad = 3.0;
    let cell_w = 34.0;
    let cell_h = height - pad * 2.0;
    let total = Vec2::new(cell_w * presets.len() as f32 + pad * 2.0, height);
    let (rect, mut resp) = ui.allocate_exact_size(total, Sense::hover());
    let cell =
        |i: usize| Rect::from_min_size(Pos2::new(rect.left() + pad + cell_w * i as f32, rect.top() + pad), Vec2::new(cell_w, cell_h));

    let mut cells = Vec::with_capacity(presets.len());
    let mut clicked = None;
    for (i, &w) in presets.iter().enumerate() {
        let tip = tf("ui.stroke.px", &[("n", &w.to_string())]);
        let r = ui.interact(cell(i), resp.id.with(i), Sense::click()).on_hover_text(&tip);
        r.widget_info(|| WidgetInfo::selected(Role::RadioButton, ui.is_enabled(), (*width - w).abs() < 0.01, &tip));
        if r.clicked() {
            clicked = Some(w);
        }
        cells.push(r);
    }
    if let Some(w) = clicked
        && (*width - w).abs() >= 0.01
    {
        *width = w;
        resp.mark_changed();
    }
    let selected = presets.iter().position(|w| (*width - w).abs() < 0.01);

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, 8.0, p.surface_2);
        painter.rect_stroke(rect, 8.0, Stroke::new(1.0, p.border), StrokeKind::Inside);

        let present = anim(ui.ctx(), resp.id.with("present"), selected.is_some(), Motion::HOVER);
        let last = selected.unwrap_or(0);
        let x = anim_value(ui.ctx(), resp.id.with("thumb"), cell(last).left(), Motion::SLIDE);
        if present > 0.0 {
            let thumb = Rect::from_min_size(Pos2::new(x, cell(last).top()), Vec2::new(cell_w, cell_h));
            let mut shadow = p.shadow_small();
            shadow.color = shadow.color.gamma_multiply(present);
            painter.add(shadow.as_shape(thumb, 6.0));
            painter.rect_filled(thumb, 6.0, p.elevated.gamma_multiply(present));
            painter.rect_stroke(thumb, 6.0, Stroke::new(1.0, p.border_strong().gamma_multiply(0.6 * present)), StrokeKind::Inside);
        }
        for (i, (r, &w)) in cells.iter().zip(presets).enumerate() {
            let sel = anim(ui.ctx(), resp.id.with(("sel", i)), selected == Some(i), Motion::SLIDE);
            let hover = hover_anim(ui, r);
            let color = lerp_color(lerp_color(p.text_faint, p.text_dim, hover), p.text, sel);
            let c = cell(i).center();
            let line = Rect::from_center_size(c, Vec2::new(16.0, w.clamp(1.0, cell_h - 6.0)));
            ui.painter().rect_filled(line, line.height() / 2.0, color);
            if r.has_focus() {
                focus_ring(ui, r, cell(i), 6.0);
            }
        }
    }
    resp
}
