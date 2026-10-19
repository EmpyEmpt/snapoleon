//! The preview area of the diff view: the computed map, or the swipe comparison.

use egui::{Color32, CursorIcon, Pos2, Rect, Sense, Stroke, TextureHandle, Ui, Vec2};
use image::{RgbaImage, imageops};
use snap_core::t;
use snap_ui::{
    Palette, icon, text,
    widgets::{Button, ButtonKind, empty_state_with, spinner, util},
};

use super::{AFTER, BEFORE, DiffView, Mode, ViewCx};
use crate::views::combine::{source, viewer::Placed};

const FULL_UV: Rect = Rect { min: Pos2::ZERO, max: Pos2 { x: 1.0, y: 1.0 } };
/// Half the width of the swipe handle's grab area.
const GRAB: f32 = 14.0;

impl DiffView {
    pub(super) fn canvas(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        let rect = ui.available_rect_before_wrap();
        ui.allocate_rect(rect, Sense::hover());
        if self.pair().is_none() {
            self.empty(ui, rect, cx);
        } else if self.params.mode == Mode::Swipe {
            self.swipe_view(ui, rect);
        } else {
            self.result_view(ui, rect);
        }
    }

    fn empty(&mut self, ui: &mut Ui, rect: Rect, cx: &mut ViewCx) {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        child.add_space(((rect.height() - 200.0) / 2.0).max(0.0));
        let mut compare_last = false;
        empty_state_with(&mut child, icon::GIT_DIFF, &t("diff.empty.title"), &t("diff.empty.body"), |ui| {
            compare_last =
                ui.add(Button::new(&t("diff.empty.action")).icon(icon::CLOCK_COUNTER_CLOCKWISE).kind(ButtonKind::Primary)).clicked();
        });
        if compare_last {
            // The newest capture is the "after" image, the one before it the "before" image.
            source::load_recent(cx, &[(BEFORE, 1), (AFTER, 0)]);
        }
    }

    fn result_view(&mut self, ui: &mut Ui, rect: Rect) {
        let Some(outcome) = &mut self.outcome else {
            ui.put(Rect::from_center_size(rect.center(), Vec2::splat(24.0)), spinner);
            return;
        };
        let tex = outcome.tex.get_or_insert_with(|| source::upload(ui.ctx(), "diff-result", &outcome.image));
        let px = Vec2::new(outcome.image.width() as f32, outcome.image.height() as f32);
        let placed = self.viewer.place(ui, rect, px);
        placed.paint(ui, tex);
        placed.paint_outline(ui);
        self.viewer.controls(ui, rect, px);
    }

    /// "Before" on the left of the divider, "after" on the right; drag the handle to compare.
    fn swipe_view(&mut self, ui: &mut Ui, rect: Rect) {
        let ctx = ui.ctx().clone();
        let [Some(a), Some(b)] = &mut self.slots else { return };
        let (a_px, b_px) = (image_size(&a.image), image_size(&b.image));
        let a_tex = a.tex.get_or_insert_with(|| source::upload(&ctx, "diff-before", &a.image)).clone();
        let b_tex = b.tex.get_or_insert_with(|| source::upload(&ctx, "diff-after", &b.image)).clone();
        let px = a_px.max(b_px);
        let placed = self.viewer.place(ui, rect, px);
        let image = placed.image_rect;
        let scale = image.width() / px.x;
        let split = image.left() + self.swipe * image.width();

        paint_band(ui, &placed, &a_tex, Rect::from_min_size(image.min, a_px * scale), image.left(), split);
        paint_band(ui, &placed, &b_tex, Rect::from_min_size(image.min, b_px * scale), split, image.right());
        placed.paint_outline(ui);

        // Drag handle: registered after the viewer's own interaction so it wins where they overlap.
        let visible = image.intersect(rect);
        let grab = Rect::from_x_y_ranges((split - GRAB)..=(split + GRAB), visible.y_range());
        let resp = ui.interact(grab, ui.scope_id().with("swipe"), Sense::drag());
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        }
        if resp.dragged() {
            self.swipe = (self.swipe + resp.drag_delta().x / image.width()).clamp(0.0, 1.0);
        }
        paint_handle(ui, &placed, split, visible, resp.hovered() || resp.dragged());
        corner_label(ui, &placed, t("diff.before").as_ref(), image.left_top() + Vec2::splat(10.0), false);
        corner_label(ui, &placed, t("diff.after").as_ref(), image.right_top() + Vec2::new(-10.0, 10.0), true);
        self.viewer.controls(ui, rect, px);
    }
}

fn image_size(image: &RgbaImage) -> Vec2 {
    Vec2::new(image.width() as f32, image.height() as f32)
}

/// Draw the part of `tex` (placed at `dest`) that lies between two x positions.
fn paint_band(ui: &Ui, placed: &Placed, tex: &TextureHandle, dest: Rect, from_x: f32, to_x: f32) {
    let band = Rect::from_x_y_ranges(from_x..=to_x, dest.y_range());
    let visible = dest.intersect(band);
    if visible.width() <= 0.0 || visible.height() <= 0.0 {
        return;
    }
    let uv = Rect::from_min_max(
        Pos2::new((visible.left() - dest.left()) / dest.width(), (visible.top() - dest.top()) / dest.height()),
        Pos2::new((visible.right() - dest.left()) / dest.width(), (visible.bottom() - dest.top()) / dest.height()),
    );
    placed.paint_clipped(ui, tex, visible, uv);
}

fn paint_handle(ui: &Ui, placed: &Placed, x: f32, visible: Rect, active: bool) {
    let p = Palette::of(ui);
    let painter = placed.painter(ui);
    let shadow = Stroke::new(3.0, Color32::from_black_alpha(70));
    painter.vline(x, visible.y_range(), shadow);
    painter.vline(x, visible.y_range(), Stroke::new(1.5, Color32::WHITE));
    let center = Pos2::new(x, visible.center().y);
    painter.add(p.shadow_small().as_shape(Rect::from_center_size(center, Vec2::splat(32.0)), 16.0));
    painter.circle_filled(center, 16.0, if active { p.accent } else { p.elevated });
    painter.circle_stroke(center, 16.0, Stroke::new(1.0, p.border));
    util::paint_icon(ui, center, icon::ARROWS_LEFT_RIGHT, 16.0, if active { p.on_accent } else { p.text });
}

/// A small dark pill naming a side of the swipe comparison.
fn corner_label(ui: &Ui, placed: &Placed, label: &str, anchor: Pos2, right: bool) {
    let painter = placed.painter(ui);
    let galley = painter.layout_no_wrap(label.to_owned(), util::font_id(ui, &text::caption_style()), Color32::WHITE);
    let size = galley.size() + Vec2::new(16.0, 8.0);
    let rect = if right { Rect::from_min_size(Pos2::new(anchor.x - size.x, anchor.y), size) } else { Rect::from_min_size(anchor, size) };
    painter.rect_filled(rect, rect.height() / 2.0, Color32::from_black_alpha(150));
    painter.galley(util::snap(ui, rect.min + Vec2::new(8.0, 4.0)), galley, Color32::WHITE);
}

/// The two images cut at `split` (0..=1 of the width): "before" on the left, "after" on the right.
pub(super) fn compose_swipe(a: &RgbaImage, b: &RgbaImage, split: f32) -> RgbaImage {
    let (w, h) = (a.width().max(b.width()), a.height().max(b.height()));
    let cut = (split * w as f32).round() as u32;
    let mut out = RgbaImage::new(w, h);
    let left = imageops::crop_imm(a, 0, 0, cut.min(a.width()), a.height()).to_image();
    imageops::replace(&mut out, &left, 0, 0);
    let right_x = cut.min(b.width());
    let right = imageops::crop_imm(b, right_x, 0, b.width() - right_x, b.height()).to_image();
    imageops::replace(&mut out, &right, i64::from(right_x), 0);
    out
}
