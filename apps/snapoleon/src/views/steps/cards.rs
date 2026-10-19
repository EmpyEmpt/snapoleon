//! The step cards of the guide editor: picture with the click ring, editable caption, per-step
//! actions and drag-to-reorder by the handle.

use egui::{
    Align, Color32, CursorIcon, Frame, Layout, Margin, Pos2, Rect, Response, RichText, Sense, Stroke, StrokeKind, TextEdit, Ui, Vec2,
};
use snap_core::{t, tf};
use snap_ui::{
    Palette, icon, space, text,
    widgets::{card_frame, icon_button, util},
};

use super::guide::{GuideView, Step};
use crate::views::{ViewCx, combine::source};

/// Picture height as a share of its width.
const ASPECT: f32 = 0.625;
const GUTTER: f32 = 32.0;
const CARD_GAP: f32 = space::MD;

#[derive(Default)]
struct Actions {
    remove: Option<u64>,
    up: Option<u64>,
    down: Option<u64>,
    annotate: Option<u64>,
    recapture: Option<u64>,
    grab: Option<u64>,
    edited: bool,
}

impl GuideView {
    pub(super) fn cards(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        let width = ui.available_width();
        let image_w = (width * 0.42).clamp(200.0, 320.0);
        let (count, idle, dragging) = (self.steps.len(), self.busy.is_none(), self.dragging);
        let mut slots: Vec<(u64, Rect)> = Vec::with_capacity(count);
        let mut actions = Actions::default();
        ui.spacing_mut().item_spacing.y = CARD_GAP;
        for (index, step) in self.steps.iter_mut().enumerate() {
            let card = card_frame(ui, space::MD as i8).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add_enabled_ui(idle, |ui| card_body(ui, step, index, count, image_w, dragging == Some(step.id), &mut actions));
            });
            slots.push((step.id, card.response.rect));
        }
        if actions.edited {
            self.touch();
        }
        if let Some(id) = actions.grab {
            self.dragging = Some(id);
        }
        if let Some(id) = actions.remove {
            self.remove(id);
        }
        if let Some(id) = actions.up {
            self.move_step(id, -1);
        }
        if let Some(id) = actions.down {
            self.move_step(id, 1);
        }
        if let Some(id) = actions.annotate {
            self.annotate(cx, id);
        }
        if let Some(id) = actions.recapture {
            self.capture_screen(cx, Some(id));
        }
        self.drag_reorder(ui, &slots);
    }

    /// While a card is held: show where it would land, scroll near the edges, move it on release.
    fn drag_reorder(&mut self, ui: &mut Ui, slots: &[(u64, Rect)]) {
        let Some(id) = self.dragging else { return };
        let (pointer, released, down) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.any_released(), i.pointer.any_down()));
        let Some(pointer) = pointer.filter(|_| down || released) else {
            self.dragging = None;
            return;
        };
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        let p = Palette::of(ui);
        let before = slots.iter().position(|(_, r)| pointer.y < r.center().y).unwrap_or(slots.len());
        let from = slots.iter().position(|(slot, _)| *slot == id).unwrap_or(0);
        if before != from && before != from + 1 {
            let y = match before {
                0 => slots[0].1.top() - CARD_GAP / 2.0,
                n => slots[n - 1].1.bottom() + CARD_GAP / 2.0,
            };
            let x = slots[0].1.x_range();
            ui.painter().rect_filled(Rect::from_x_y_ranges(x, (y - 1.0)..=(y + 1.0)), 1.0, p.accent);
        }
        if let Some(step) = self.steps.iter().find(|s| s.id == id) {
            ghost(ui, pointer, from + 1, &step.shot.caption);
        }
        // Scroll the list while the pointer is held near its top or bottom edge.
        let view = ui.clip_rect();
        let edge = 56.0;
        if pointer.y < view.top() + edge {
            ui.scroll_with_delta(Vec2::new(0.0, 14.0));
        } else if pointer.y > view.bottom() - edge {
            ui.scroll_with_delta(Vec2::new(0.0, -14.0));
        }
        ui.ctx().request_repaint();
        if released {
            self.move_before(id, before);
            self.dragging = None;
        }
    }
}

fn card_body(ui: &mut Ui, step: &mut Step, index: usize, count: usize, image_w: f32, lifted: bool, out: &mut Actions) {
    let p = Palette::of(ui);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = space::MD;
        ui.vertical(|ui| {
            ui.set_width(GUTTER - 4.0);
            ui.spacing_mut().item_spacing.y = space::SM;
            number_badge(ui, index + 1);
            let (rect, resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::drag());
            let hover = util::hover_anim(ui, &resp);
            util::paint_icon(ui, rect.center(), icon::DOTS_SIX_VERTICAL, 18.0, util::lerp_color(p.text_faint, p.text, hover));
            if resp.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::Grab);
            }
            if resp.drag_started() {
                out.grab = Some(step.id);
            }
            snap_ui::widgets::tooltip(resp, &t("steps.card.drag"), None);
        });

        picture(ui, step, image_w, lifted);

        ui.vertical(|ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = space::SM;
            let hint = tf("steps.card.caption_hint", &[("n", &(index + 1).to_string())]);
            if caption_field(ui, step.id, &mut step.shot.caption, &hint).changed() {
                out.edited = true;
            }
            if let Some(meta) = context_line(step) {
                ui.add(egui::Label::new(text::caption(meta)).truncate());
            }
            ui.add_space(space::XS);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if icon_button(ui, icon::PENCIL_SIMPLE, &t("steps.card.annotate"), None).clicked() {
                    out.annotate = Some(step.id);
                }
                if icon_button(ui, icon::CAMERA, &t("steps.card.recapture"), None).clicked() {
                    out.recapture = Some(step.id);
                }
                ui.add_enabled_ui(index > 0, |ui| {
                    if icon_button(ui, icon::ARROW_UP, &t("steps.card.up"), None).clicked() {
                        out.up = Some(step.id);
                    }
                });
                ui.add_enabled_ui(index + 1 < count, |ui| {
                    if icon_button(ui, icon::ARROW_DOWN, &t("steps.card.down"), None).clicked() {
                        out.down = Some(step.id);
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if icon_button(ui, icon::TRASH, &t("steps.card.delete"), None).clicked() {
                        out.remove = Some(step.id);
                    }
                });
            });
        });
    });
}

/// A three-line input in the look of `snap_ui::widgets::TextField`, with the text at the top (the
/// widget centers short text vertically in a multi-line box).
fn caption_field(ui: &mut Ui, id: u64, text: &mut String, hint: &str) -> Response {
    let p = Palette::of(ui);
    let edit_id = egui::Id::unique(("steps-caption", id));
    let focused = ui.memory(|m| m.has_focus(edit_id));
    let border = if focused { p.accent } else { p.border };
    let frame = Frame::new().fill(p.surface_2).stroke(Stroke::new(1.0, border)).corner_radius(10.0).inner_margin(Margin::symmetric(12, 10));
    let shown = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        let edit = TextEdit::multiline(text)
            .id(edit_id)
            .frame(Frame::NONE)
            .desired_rows(3)
            .desired_width(f32::INFINITY)
            .hint_text(RichText::new(hint).color(p.text_faint))
            .text_color(p.text);
        ui.add(edit)
    });
    if focused {
        ui.painter().rect_stroke(shown.response.rect, 10.0, Stroke::new(2.0, p.accent.gamma_multiply(0.3)), StrokeKind::Outside);
    }
    shown.inner
}

/// "Firefox · Snapoleon: screenshots without the weight" from what the capture knew.
fn context_line(step: &Step) -> Option<String> {
    let c = &step.shot.context;
    let parts: Vec<&str> = [c.app_name.as_deref(), c.window_title.as_deref()].into_iter().flatten().filter(|s| !s.is_empty()).collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// A gold disc with the step number, as in the exports.
fn number_badge(ui: &mut Ui, number: usize) {
    let p = Palette::of(ui);
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 14.0, p.gold);
    let ink = if p.dark { p.bg } else { p.on_accent };
    let galley = util::layout(ui, &number.to_string(), &text::mono(), ink);
    util::paint_centered(ui, rect.center(), galley, ink);
}

/// The step's picture with a ring where the click happened.
fn picture(ui: &mut Ui, step: &mut Step, width: f32, lifted: bool) {
    let p = Palette::of(ui);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, (width * ASPECT).round()), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let tex = step.tex.get_or_insert_with(|| source::upload(ui.ctx(), "steps-card", &step.thumb));
    let painter = ui.painter();
    painter.rect_filled(rect, 8.0, p.surface_2);
    let image = source::fit_inside(rect.shrink(1.0), Vec2::new(step.shot.image.width() as f32, step.shot.image.height() as f32));
    painter.image(tex.id(), image, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
    if let Some(click) = step.shot.click {
        let scale = image.width() / step.shot.image.width() as f32;
        let center = image.min + Vec2::new(click.x as f32, click.y as f32) * scale;
        if image.contains(center) {
            painter.circle_filled(center, 14.0, p.gold.gamma_multiply(0.22));
            painter.circle_stroke(center, 14.0, Stroke::new(3.0, Color32::from_black_alpha(90)));
            painter.circle_stroke(center, 14.0, Stroke::new(2.0, p.gold));
        }
    }
    painter.rect_stroke(rect, 8.0, Stroke::new(1.0, p.border), StrokeKind::Inside);
    if lifted {
        painter.rect_filled(rect, 8.0, p.surface.gamma_multiply(0.6));
    }
}

/// A floating label that follows the pointer while a card is dragged.
fn ghost(ui: &Ui, pointer: Pos2, number: usize, caption: &str) {
    let p = Palette::of(ui);
    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("steps-ghost")));
    let rect = Rect::from_center_size(pointer + Vec2::new(40.0, 0.0), Vec2::new(280.0, 40.0));
    painter.add(p.shadow().as_shape(rect, 10.0));
    painter.rect_filled(rect, 10.0, p.elevated);
    painter.rect_stroke(rect, 10.0, Stroke::new(1.5, p.accent), StrokeKind::Inside);
    let label = if caption.trim().is_empty() {
        tf("steps.step_n", &[("n", &number.to_string())])
    } else {
        format!("{number}.  {}", caption.trim())
    };
    let galley = painter.layout_no_wrap(label, util::font_id(ui, &text::body_style()), p.text);
    painter.with_clip_rect(rect.shrink2(Vec2::new(12.0, 0.0))).galley(
        util::snap(ui, Pos2::new(rect.left() + 12.0, rect.center().y - galley.size().y / 2.0)),
        galley,
        p.text,
    );
}
