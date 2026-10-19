//! The strip of image tiles under the preview: select, remove, drag to reorder, add more.

use egui::{Color32, CursorIcon, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use snap_core::{t, tf};
use snap_ui::{
    Palette, icon, text,
    widgets::{tooltip, util},
};

use super::{CombineView, source};

pub const HEIGHT: f32 = 140.0;
const TILE_W: f32 = 120.0;
const THUMB_H: f32 = 76.0;
const TILE_H: f32 = THUMB_H + 22.0;
const GAP: f32 = 12.0;

impl CombineView {
    pub(super) fn tray(&mut self, ui: &mut Ui, cx: &mut crate::views::ViewCx) {
        let mut slots: Vec<(u64, Rect)> = Vec::new();
        let mut remove = None;
        let mut select = None;
        let mut grab = None;
        let mut add = None;
        let dragging = self.dragging;
        egui::ScrollArea::horizontal().auto_shrink([false, false]).show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                let count = self.tiles.len();
                for (index, tile) in self.tiles.iter_mut().enumerate() {
                    let tex = tile.tex.get_or_insert_with(|| source::upload(ui.ctx(), "combine-tile", &tile.thumb));
                    let tile_ui = TileUi {
                        id: tile.id,
                        index,
                        name: &tile.name,
                        selected: self.selected == Some(tile.id),
                        lifted: dragging == Some(tile.id),
                    };
                    let (rect, out) = paint_tile(ui, &tile_ui, tex, count);
                    slots.push((tile.id, rect));
                    if out.removed {
                        remove = Some(tile.id);
                    } else if out.clicked {
                        select = Some(tile.id);
                    }
                    if out.grabbed {
                        grab = Some(tile.id);
                    }
                }
                add = add_tile(ui);
            });
        });

        if let Some(id) = remove {
            self.remove(id);
        }
        if let Some(id) = select {
            self.selected = Some(id);
        }
        if let Some(id) = grab {
            self.selected = Some(id);
            self.dragging = Some(id);
        }
        if let Some(pick) = add {
            self.add_from(cx, pick);
        }
        self.drag_reorder(ui, &slots);
    }

    /// While a tile is held: show where it would land and move it on release.
    fn drag_reorder(&mut self, ui: &mut Ui, slots: &[(u64, Rect)]) {
        let Some(id) = self.dragging else { return };
        let (pointer, released, down) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.any_released(), i.pointer.any_down()));
        let Some(pointer) = pointer.filter(|_| down || released) else {
            self.dragging = None;
            return;
        };
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        // The tile goes in front of the first one whose center is right of the cursor.
        let before = slots.iter().position(|(_, r)| pointer.x < r.center().x).unwrap_or(slots.len());
        let from = slots.iter().position(|(slot, _)| *slot == id).unwrap_or(0);
        let p = Palette::of(ui);
        if before != from && before != from + 1 {
            let x = match before {
                0 => slots[0].1.left() - 6.0,
                n => slots[n - 1].1.right() + 6.0,
            };
            let line = Rect::from_min_max(Pos2::new(x - 1.0, slots[0].1.top()), Pos2::new(x + 1.0, slots[0].1.top() + THUMB_H));
            ui.painter().rect_filled(line, 1.0, p.accent);
        }
        if let Some((_, rect)) = slots.get(from) {
            ghost(ui, pointer, rect.size(), self.tiles.iter().find(|t| t.id == id).and_then(|t| t.tex.as_ref()));
        }
        if released {
            self.move_tile(id, before);
            self.dragging = None;
        }
    }
}

struct TileUi<'a> {
    id: u64,
    index: usize,
    name: &'a str,
    selected: bool,
    /// Being dragged: the slot stays but dims.
    lifted: bool,
}

#[derive(Default)]
struct TileOut {
    clicked: bool,
    grabbed: bool,
    removed: bool,
}

fn paint_tile(ui: &mut Ui, tile: &TileUi, tex: &egui::TextureHandle, count: usize) -> (Rect, TileOut) {
    let p = Palette::of(ui);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(TILE_W, TILE_H), Sense::click_and_drag());
    let hover = util::hover_anim(ui, &resp);
    let thumb = Rect::from_min_size(rect.min, Vec2::new(TILE_W, THUMB_H));
    let mut out = TileOut { clicked: resp.clicked(), grabbed: resp.drag_started(), removed: false };

    // The remove button sits on top of the tile, so it must be registered after the tile itself.
    let close = Rect::from_center_size(Pos2::new(thumb.right() - 14.0, thumb.top() + 14.0), Vec2::splat(22.0));
    let close_resp = (hover > 0.0 || tile.selected).then(|| ui.interact(close, resp.id.with(("remove", tile.id)), Sense::click()));

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(thumb, 8.0, p.surface_2);
        let image = source::fit_inside(thumb.shrink(5.0), tex.size_vec2());
        painter.image(tex.id(), image, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        let border = util::lerp_color(p.border, p.border_strong(), hover);
        painter.rect_stroke(thumb, 8.0, Stroke::new(1.0, border), StrokeKind::Inside);
        if tile.selected {
            painter.rect_stroke(thumb.expand(2.0), 10.0, Stroke::new(2.0, p.accent), StrokeKind::Outside);
        }
        if tile.lifted {
            painter.rect_filled(thumb, 8.0, p.surface.gamma_multiply(0.7));
        }

        // Position badge, top-left.
        let badge = Rect::from_min_size(thumb.min + Vec2::splat(6.0), Vec2::splat(18.0));
        painter.rect_filled(badge, 9.0, Color32::from_black_alpha(150));
        let number = util::layout(ui, &(tile.index + 1).to_string(), &text::mono(), Color32::WHITE);
        util::paint_centered(ui, badge.center(), number, Color32::WHITE);

        if let Some(close_resp) = &close_resp {
            let hot = util::hover_anim(ui, close_resp);
            let fill = util::lerp_color(Color32::from_black_alpha(150), p.danger, hot);
            ui.painter().circle_filled(close.center(), 11.0, fill);
            util::paint_icon(ui, close.center(), icon::X, 12.0, Color32::WHITE);
        }

        let caption = util::layout(ui, tile.name, &text::caption_style(), p.text_dim);
        let strip = Rect::from_min_size(Pos2::new(rect.left(), thumb.bottom()), Vec2::new(TILE_W, TILE_H - THUMB_H));
        ui.painter().with_clip_rect(strip).galley(util::snap(ui, Pos2::new(rect.left() + 2.0, thumb.bottom() + 5.0)), caption, p.text_dim);
    }
    if let Some(close_resp) = close_resp {
        out.removed = tooltip(close_resp, &t("combine.tile.remove"), Some("Del")).clicked();
    }
    if count > 1 {
        resp.on_hover_text(tf("combine.tile.position", &[("n", &(tile.index + 1).to_string()), ("total", &count.to_string())]));
    }
    (rect, out)
}

/// The dashed-looking "add" tile at the end of the strip; opens the source menu.
fn add_tile(ui: &mut Ui) -> Option<source::Pick> {
    let p = Palette::of(ui);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(TILE_W, THUMB_H), Sense::click());
    let hover = util::hover_anim(ui, &resp);
    let painter = ui.painter();
    painter.rect_filled(rect, 8.0, p.hover().gamma_multiply(hover));
    painter.rect_stroke(rect, 8.0, Stroke::new(1.0, util::lerp_color(p.border, p.border_strong(), hover)), StrokeKind::Inside);
    util::paint_icon(ui, rect.center() - Vec2::new(0.0, 9.0), icon::PLUS, 20.0, util::lerp_color(p.text_dim, p.text, hover));
    let label = util::layout(ui, &t("combine.add"), &text::caption_style(), p.text_dim);
    util::paint_centered(ui, rect.center() + Vec2::new(0.0, 15.0), label, p.text_dim);
    util::focus_ring(ui, &resp, rect, 8.0);
    let resp = tooltip(resp, &t("combine.add_images"), Some("CmdOrCtrl+O"));
    source::source_menu(ui, &resp, false)
}

/// A floating copy of the held tile that follows the pointer.
fn ghost(ui: &Ui, pointer: Pos2, size: Vec2, tex: Option<&egui::TextureHandle>) {
    let p = Palette::of(ui);
    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("combine-ghost")));
    let rect = Rect::from_center_size(pointer, Vec2::new(size.x, THUMB_H));
    painter.add(p.shadow().as_shape(rect, 8.0));
    painter.rect_filled(rect, 8.0, p.elevated);
    if let Some(tex) = tex {
        let image = source::fit_inside(rect.shrink(5.0), tex.size_vec2());
        painter.image(tex.id(), image, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
    }
    painter.rect_stroke(rect, 8.0, Stroke::new(1.5, p.accent), StrokeKind::Inside);
}
