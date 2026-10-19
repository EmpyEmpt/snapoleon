//! Floating pill toolbar used by the capture overlay and the editor.

use egui::{Area, Context, Frame, Id, InnerResponse, Margin, Order, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};

use crate::theme::{Palette, radius};

/// The pill itself, drawn inline: elevated fill, 1 px border, shadow, fully rounded.
pub fn toolbar_pill<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
    let p = Palette::of(ui);
    Frame::new()
        .fill(p.elevated)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(radius::PILL as u8)
        .inner_margin(Margin::symmetric(6, 4))
        .shadow(p.shadow())
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(2.0, 0.0);
            ui.horizontal(add).inner
        })
}

/// A pill toolbar floating above everything at `pos` (top-left, in screen points).
pub fn floating_toolbar<R>(ctx: &Context, id: Id, pos: Pos2, add: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
    let out = Area::new(id)
        .order(Order::Foreground)
        .fixed_pos(pos)
        .constrain(true)
        .sense(Sense::hover())
        .show(ctx, |ui| toolbar_pill(ui, add).inner);
    ctx.data_mut(|d| d.insert_temp(id.with("size"), out.response.rect.size()));
    out
}

/// A pill toolbar placed relative to `anchor` (e.g. a capture selection): centered under it with a gap,
/// flipped above when it would leave `bounds`, and clamped horizontally into `bounds`.
pub fn floating_toolbar_anchored<R>(ctx: &Context, id: Id, anchor: Rect, bounds: Rect, add: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
    const GAP: f32 = 10.0;
    // The size is only known after the first frame; guess until then.
    let size = ctx.data(|d| d.get_temp::<Vec2>(id.with("size"))).unwrap_or(Vec2::new(420.0, 46.0));
    let below = anchor.bottom() + GAP;
    let y = if below + size.y <= bounds.bottom() - 4.0 { below } else { (anchor.top() - GAP - size.y).max(bounds.top() + 4.0) };
    // Selection spanning the whole screen: tuck the toolbar inside it.
    let y = if y + size.y > bounds.bottom() { bounds.bottom() - size.y - GAP } else { y };
    let x = (anchor.center().x - size.x / 2.0).clamp(bounds.left() + 4.0, (bounds.right() - size.x - 4.0).max(bounds.left() + 4.0));
    floating_toolbar(ctx, id, Pos2::new(x, y), add)
}

/// Thin vertical divider between toolbar groups.
pub fn toolbar_separator(ui: &mut Ui) -> Response {
    let p = Palette::of(ui);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(9.0, 20.0), Sense::hover());
    ui.painter().vline(rect.center().x.floor() + 0.5, rect.y_range(), Stroke::new(1.0, p.border));
    resp
}
