//! The right panel of the diff view: the two image slots, the comparison mode, tolerance, statistics
//! and the export buttons.

use egui::{Align, Layout, Pos2, Rect, ScrollArea, Sense, Stroke, StrokeKind, Ui, Vec2};
use snap_core::{t, tf};
use snap_ui::{
    Palette, Tone, icon, space, text,
    widgets::{Button, ButtonKind, Slider, badge, card_frame, chip, divider, icon_button, tooltip, util},
};

use super::{AFTER, BEFORE, DiffView, Mode, ViewCx, slot_key};
use crate::views::combine::source;

const SLOT_H: f32 = 60.0;

fn title(ui: &mut Ui, key: &str) {
    ui.label(text::body_strong(t(key)));
}

impl DiffView {
    pub(super) fn side(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        let before = self.params;
        ui.spacing_mut().item_spacing = Vec2::splat(space::SM);
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            self.export(ui, cx);
            ui.add_space(space::MD);
            divider(ui);
            ui.add_space(space::MD);
            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.options(ui, cx));
            });
        });
        if self.params != before {
            self.dirty = true;
        }
    }

    fn options(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = 24.0;
            ui.horizontal(|ui| {
                title(ui, "diff.images");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_enabled_ui(self.pair().is_some(), |ui| {
                        if icon_button(ui, icon::SWAP, &t("diff.swap"), None).clicked() {
                            self.swap();
                        }
                    });
                });
            });
        });
        for slot in [BEFORE, AFTER] {
            self.slot_card(ui, cx, slot);
        }

        ui.add_space(space::MD);
        title(ui, "diff.mode.title");
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(6.0);
            let modes = [
                (Mode::Highlight, "diff.mode.highlight"),
                (Mode::Overlay, "diff.mode.overlay"),
                (Mode::SideBySide, "diff.mode.side"),
                (Mode::Difference, "diff.mode.difference"),
                (Mode::Swipe, "diff.mode.swipe"),
            ];
            for (mode, key) in modes {
                if chip(ui, &t(key), self.params.mode == mode).clicked() {
                    self.params.mode = mode;
                }
            }
        });
        match self.params.mode {
            Mode::Overlay => {
                let mut percent = self.params.opacity * 100.0;
                ui.label(t("diff.opacity"));
                let rail = ui.available_width() - 56.0;
                if ui.add(Slider::new(&mut percent, 0.0..=100.0).suffix(" %").width(rail)).changed() {
                    self.params.opacity = percent / 100.0;
                }
            }
            Mode::Swipe => {
                ui.label(text::caption(t("diff.swipe_hint")));
            }
            _ => {}
        }

        ui.add_space(space::MD);
        title(ui, "diff.tolerance");
        let mut threshold = f32::from(self.params.threshold);
        let rail = ui.available_width() - 56.0;
        if ui.add(Slider::new(&mut threshold, 0.0..=64.0).width(rail)).changed() {
            self.params.threshold = threshold.round() as u8;
        }

        self.stats(ui);
    }

    /// One image slot: thumbnail, name and size; clicking opens the source menu.
    fn slot_card(&mut self, ui: &mut Ui, cx: &mut ViewCx, index: usize) {
        let p = Palette::of(ui);
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), SLOT_H), Sense::click());
        let hover = util::hover_anim(ui, &resp);
        let label = t(slot_key(index));
        let thumb = Rect::from_min_size(rect.min + Vec2::splat(8.0), Vec2::new(72.0, SLOT_H - 16.0));
        if ui.is_rect_visible(rect) {
            let painter = ui.painter();
            painter.rect_filled(rect, 10.0, p.surface_2);
            painter.rect_filled(rect, 10.0, p.hover().gamma_multiply(hover));
            painter.rect_stroke(rect, 10.0, Stroke::new(1.0, util::lerp_color(p.border, p.border_strong(), hover)), StrokeKind::Inside);
            let text_x = thumb.right() + 12.0;
            match &mut self.slots[index] {
                Some(slot) => {
                    let tex = slot.thumb_tex.get_or_insert_with(|| source::upload(ui.ctx(), "diff-slot", &slot.thumb));
                    let image = source::fit_inside(thumb, tex.size_vec2());
                    ui.painter().image(tex.id(), image, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), egui::Color32::WHITE);
                    ui.painter().rect_stroke(image, 3.0, Stroke::new(1.0, p.border), StrokeKind::Outside);
                    let caption = util::layout(ui, &label, &text::caption_style(), p.text_dim);
                    let size = format!("{} × {}", slot.image.width(), slot.image.height());
                    let size = util::layout(ui, &size, &text::mono(), p.text);
                    let name = util::layout(ui, &slot.name, &text::body_style(), p.text);
                    let clip = Rect::from_min_max(Pos2::new(text_x, rect.top()), Pos2::new(rect.right() - 30.0, rect.bottom()));
                    let top = rect.center().y - (caption.size().y + name.size().y + size.size().y + 4.0) / 2.0;
                    let painter = ui.painter().with_clip_rect(clip);
                    painter.galley(util::snap(ui, Pos2::new(text_x, top)), caption.clone(), p.text_dim);
                    painter.galley(util::snap(ui, Pos2::new(text_x, top + caption.size().y + 2.0)), name.clone(), p.text);
                    painter.galley(util::snap(ui, Pos2::new(text_x, top + caption.size().y + name.size().y + 4.0)), size, p.text_dim);
                }
                None => {
                    let painter = ui.painter();
                    painter.rect_stroke(thumb, 6.0, Stroke::new(1.0, p.border_strong()), StrokeKind::Inside);
                    util::paint_icon(ui, thumb.center(), icon::PLUS, 18.0, p.text_dim);
                    let caption = util::layout(ui, &label, &text::caption_style(), p.text_dim);
                    let choose = util::layout(ui, &t("diff.choose"), &text::body_style(), p.text);
                    let top = rect.center().y - (caption.size().y + choose.size().y + 2.0) / 2.0;
                    let painter = ui.painter();
                    painter.galley(util::snap(ui, Pos2::new(text_x, top)), caption.clone(), p.text_dim);
                    painter.galley(util::snap(ui, Pos2::new(text_x, top + caption.size().y + 2.0)), choose, p.text);
                }
            }
            util::paint_icon(ui, Pos2::new(rect.right() - 16.0, rect.center().y), icon::CARET_DOWN, 12.0, p.text_faint);
            util::focus_ring(ui, &resp, rect, 10.0);
        }
        let resp = tooltip(resp, &t("diff.replace"), None);
        if let Some(pick) = source::source_menu(ui, &resp, true) {
            self.choose(cx, index, pick);
        }
    }

    /// Share of changed pixels, their count and bounding box, and a note when the sizes differ.
    fn stats(&mut self, ui: &mut Ui) {
        let (Some((a, b)), Some(outcome)) = (self.pair(), &self.outcome) else { return };
        let stats = outcome.stats;
        let differ = a.image.dimensions() != b.image.dimensions();
        let p = Palette::of(ui);
        ui.add_space(space::MD);
        title(ui, "diff.stats.title");
        card_frame(ui, space::LG as i8).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = space::SM;
            ui.horizontal(|ui| {
                let percent = stats.changed_ratio * 100.0;
                let shown = if stats.changed_pixels == 0 {
                    "0%".to_owned()
                } else if percent < 0.01 {
                    "<0.01%".to_owned()
                } else {
                    format!("{percent:.2}%")
                };
                ui.label(text::mono_text(shown).size(22.0).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if stats.changed_pixels == 0 {
                        badge(ui, &t("diff.stats.identical"), Tone::Success);
                    }
                });
            });
            ui.label(text::caption(t("diff.stats.changed")));
            if let Some(bbox) = stats.bbox {
                divider(ui);
                stat_row(ui, &t("diff.stats.pixels"), &source::group_digits(stats.changed_pixels, &t("diff.stats.thousands")));
                ui.label(text::caption(t("diff.stats.region")));
                ui.label(text::mono_text(format!("{}, {}  ·  {} × {}", bbox.x, bbox.y, bbox.w, bbox.h)));
            }
            if differ {
                divider(ui);
                let note = tf("diff.stats.sizes", &[("a", &dims(&a.image)), ("b", &dims(&b.image))]);
                ui.horizontal_wrapped(|ui| {
                    ui.label(text::caption(note).color(p.warning));
                });
            }
        });
    }

    /// Size of the result and the Copy / Save / Edit buttons, anchored to the bottom of the panel.
    fn export(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        let enabled = self.can_export();
        let width = ui.available_width();
        let half = (width - space::SM) / 2.0;
        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal(|ui| {
                let save = ui.add(Button::new(&t("common.save")).icon(icon::FLOPPY_DISK).min_width(half));
                if tooltip(save, "", Some("CmdOrCtrl+S")).clicked() {
                    self.save(cx);
                }
                let edit = ui.add(Button::new(&t("diff.edit")).icon(icon::PENCIL_SIMPLE).min_width(half));
                if edit.clicked() {
                    self.edit(cx);
                }
            });
            let copy = ui.add(Button::new(&t("common.copy")).icon(icon::COPY).kind(ButtonKind::Primary).min_width(width));
            if tooltip(copy, "", Some("CmdOrCtrl+C")).clicked() {
                self.copy(cx);
            }
        });
        if !enabled {
            ui.add_space(space::XS);
            ui.label(text::caption(t("diff.need_two")));
        }
    }
}

fn dims(image: &image::RgbaImage) -> String {
    format!("{} × {}", image.width(), image.height())
}

/// A label on the left, a mono value on the right.
fn stat_row(ui: &mut Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(text::caption(label));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(text::mono_text(value));
        });
    });
}
