//! Helpers shared by the widgets: motion, text measuring, focus ring.

use std::sync::Arc;

use egui::{Align2, Color32, Context, FontId, Galley, Id, Pos2, Rect, Response, Stroke, StrokeKind, TextStyle, Ui, emath::easing};

use crate::theme::Palette;

/// Motion durations in seconds (docs/DESIGN.md). All collapse to zero under "reduce motion",
/// which [`crate::install`] expresses as `animation_time == 0`.
pub struct Motion;

impl Motion {
    pub const HOVER: f32 = 0.1;
    pub const SLIDE: f32 = 0.16;
    pub const ENTER: f32 = 0.2;
}

fn reduced(ctx: &Context) -> bool {
    ctx.global_style().animation_time <= 0.0
}

/// Seconds for an animation of nominal length `secs`, honouring reduce-motion.
pub fn dur(ctx: &Context, secs: f32) -> f32 {
    if reduced(ctx) { 0.0 } else { secs }
}

/// 0..1 eased toward `on`, with the given nominal duration.
pub fn anim(ctx: &Context, id: Id, on: bool, secs: f32) -> f32 {
    ctx.animate_bool_with_time_and_easing(id, on, dur(ctx, secs), easing::cubic_out)
}

/// 0..1 hover fade (100 ms).
pub fn hover_anim(ui: &Ui, resp: &Response) -> f32 {
    anim(ui.ctx(), resp.id.with("hover"), resp.hovered() && ui.is_enabled(), Motion::HOVER)
}

/// Animates a value toward `target` linearly over `secs`.
pub fn anim_value(ctx: &Context, id: Id, target: f32, secs: f32) -> f32 {
    ctx.animate_value_with_time(id, target, dur(ctx, secs))
}

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    a.lerp_to_gamma(b, t.clamp(0.0, 1.0))
}

/// Draws the 2 px accent focus ring outside `rect` when the widget has keyboard focus.
pub fn focus_ring(ui: &Ui, resp: &Response, rect: Rect, rounding: f32) {
    if resp.has_focus() && ui.is_enabled() {
        let p = Palette::of(ui);
        ui.painter().rect_stroke(rect, rounding, Stroke::new(2.0, p.accent), StrokeKind::Outside);
    }
}

/// Current control height: 32 by default, 28 inside a compact scope.
pub fn control_height(ui: &Ui) -> f32 {
    ui.spacing().interact_size.y
}

pub fn font_id(ui: &Ui, style: &TextStyle) -> FontId {
    style.resolve(ui.style())
}

pub fn layout(ui: &Ui, text: &str, style: &TextStyle, color: Color32) -> Arc<Galley> {
    ui.painter().layout_no_wrap(text.to_owned(), font_id(ui, style), color)
}

pub fn layout_wrapped(ui: &Ui, text: &str, style: &TextStyle, color: Color32, width: f32) -> Arc<Galley> {
    ui.painter().layout(text.to_owned(), font_id(ui, style), color, width)
}

/// Paints `galley` centered on `center`, snapped to whole pixels so text stays crisp.
pub fn paint_centered(ui: &Ui, center: Pos2, galley: Arc<Galley>, color: Color32) {
    let pos = center - galley.size() / 2.0;
    ui.painter().galley(snap(ui, pos), galley, color);
}

pub fn snap(ui: &Ui, p: Pos2) -> Pos2 {
    let ppp = ui.pixels_per_point();
    Pos2::new((p.x * ppp).round() / ppp, (p.y * ppp).round() / ppp)
}

/// Draws an icon glyph centered on `center`.
pub fn paint_icon(ui: &Ui, center: Pos2, icon: &str, size: f32, color: Color32) {
    ui.painter().text(snap(ui, center), Align2::CENTER_CENTER, icon, FontId::proportional(size), color);
}
