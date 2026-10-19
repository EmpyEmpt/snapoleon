//! Structural widgets: card, section, setting row, nav item, empty state, spinner and progress.

use egui::{
    Align, Frame, InnerResponse, Layout, Margin, Pos2, Rect, Response, Role, Sense, Stroke, Ui,
    UiBuilder, Vec2, WidgetInfo,
};

use super::util::{self, Motion, anim, anim_value, focus_ring, hover_anim, lerp_color};
use crate::{
    brand,
    text::{self, body_style, caption_style},
    theme::{Palette, radius, space},
};

/// The surface frame used by [`card`]: `surface` fill, 1 px border, 10 px radius.
pub fn card_frame(ui: &Ui, margin: i8) -> Frame {
    let p = Palette::of(ui);
    Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(radius::CARD)
        .inner_margin(Margin::same(margin))
}

/// A bordered surface that fills the available width.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
    card_frame(ui, space::LG as i8).show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui)
    })
}

/// A titled group of rows, as used on the settings pages. Adds a 24 px gap after itself.
pub fn section<R>(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
    let out = ui.vertical(|ui| {
        ui.label(text::body_strong(title));
        ui.add_space(space::XS);
        card_frame(ui, 0)
            .inner_margin(Margin::symmetric(space::LG as i8, 4))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                add(ui)
            })
            .inner
    });
    ui.add_space(space::XL);
    out
}

/// One settings row: label and description on the left, `control` on the right.
/// Rows after the first in a container get a hairline above them.
pub fn setting_row<R>(
    ui: &mut Ui,
    label: &str,
    desc: &str,
    control: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    let p = Palette::of(ui);
    let id = ui.make_persistent_id(("setting_row", label, ui.next_auto_id()));
    let gap = space::LG;
    let first = ui.min_rect().height() < 1.0;
    let full_w = ui.available_width();

    // The control's width is only known after it is laid out, so wrap the text against last frame's width.
    let prev_w = ui.data(|d| d.get_temp::<f32>(id)).unwrap_or(120.0);
    let wrap_w = (full_w - prev_w - gap).max(120.0);
    let label_g = util::layout_wrapped(ui, label, &body_style(), p.text, wrap_w);
    let desc_g = (!desc.is_empty())
        .then(|| util::layout_wrapped(ui, desc, &caption_style(), p.text_dim, wrap_w));
    let text_h = label_g.size().y + desc_g.as_ref().map_or(0.0, |g| g.size().y + 2.0);
    let row_h = (text_h + 20.0).max(52.0);

    let (rect, resp) = ui.allocate_exact_size(Vec2::new(full_w, row_h), Sense::hover());
    if !first && ui.is_rect_visible(rect) {
        ui.painter()
            .hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, p.border));
    }
    let mut y = rect.center().y - text_h / 2.0;
    let label_h = label_g.size().y;
    ui.painter()
        .galley(util::snap(ui, Pos2::new(rect.left(), y)), label_g, p.text);
    y += label_h + 2.0;
    if let Some(g) = desc_g {
        ui.painter()
            .galley(util::snap(ui, Pos2::new(rect.left(), y)), g, p.text_dim);
    }

    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::right_to_left(Align::Center)),
    );
    let inner = control(&mut child);
    let used = child.min_rect().width();
    if (used - prev_w).abs() > 0.5 {
        ui.data_mut(|d| d.insert_temp(id, used));
        ui.ctx().request_repaint();
    }
    // Rows tile edge to edge; cancel the gap the parent would add after this row.
    ui.add_space(-ui.spacing().item_spacing.y);
    InnerResponse::new(inner, resp)
}

/// Sidebar navigation row with an icon, a label and a sliding accent marker when selected.
pub fn nav_item(ui: &mut Ui, icon: &str, label: &str, selected: bool) -> Response {
    let p = Palette::of(ui);
    let height = util::control_height(ui) + 2.0;
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
    resp.widget_info(|| WidgetInfo::selected(Role::Button, ui.is_enabled(), selected, label));
    if ui.is_rect_visible(rect) {
        let sel = anim(ui.ctx(), resp.id.with("sel"), selected, Motion::HOVER);
        let hover = hover_anim(ui, &resp);
        let round = 8.0;
        let painter = ui.painter();
        painter.rect_filled(rect, round, p.hover().gamma_multiply(hover * (1.0 - sel)));
        painter.rect_filled(rect, round, p.accent_soft.gamma_multiply(sel));
        if resp.is_pointer_button_down_on() {
            painter.rect_filled(rect, round, p.pressed());
        }
        if sel > 0.0 {
            let bar_h = 16.0 * sel;
            let bar = Rect::from_center_size(
                Pos2::new(rect.left() + 2.0, rect.center().y),
                Vec2::new(3.0, bar_h),
            );
            painter.rect_filled(bar, 1.5, p.accent);
        }
        let fg = lerp_color(lerp_color(p.text_dim, p.text, hover), p.accent, sel);
        util::paint_icon(
            ui,
            Pos2::new(rect.left() + 20.0, rect.center().y),
            icon,
            18.0,
            fg,
        );
        let galley = util::layout(ui, label, &text::body_strong_style(), fg);
        let pos = util::snap(
            ui,
            Pos2::new(rect.left() + 40.0, rect.center().y - galley.size().y / 2.0),
        );
        ui.painter().galley(pos, galley, fg);
        focus_ring(ui, &resp, rect, round);
    }
    resp
}

/// Centered icon, title, one sentence.
pub fn empty_state(ui: &mut Ui, icon: &str, title: &str, body: &str) -> Response {
    empty_state_with(ui, icon, title, body, |_| ()).response
}

/// [`empty_state`] with one action (typically a primary button) under the text.
pub fn empty_state_with<R>(
    ui: &mut Ui,
    icon: &str,
    title: &str,
    body: &str,
    action: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    let p = Palette::of(ui);
    empty_layout(ui, title, body, action, |ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::hover());
        util::paint_icon(ui, rect.center(), icon, 28.0, p.text_faint);
    })
}

/// Empty state that shows the bicorne brand mark instead of an icon (the Library's first-run state).
pub fn empty_state_brand<R>(
    ui: &mut Ui,
    title: &str,
    body: &str,
    action: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    empty_layout(ui, title, body, action, |ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(56.0), Sense::hover());
        brand::logo_colored(
            &ui.painter_at(rect),
            rect,
            brand::LogoColors::from_palette(&Palette::of(ui)),
        );
    })
}

fn empty_layout<R>(
    ui: &mut Ui,
    title: &str,
    body: &str,
    action: impl FnOnce(&mut Ui) -> R,
    glyph: impl FnOnce(&mut Ui),
) -> InnerResponse<R> {
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = space::SM;
        ui.add_space(space::LG);
        glyph(ui);
        ui.add_space(space::XS);
        ui.label(text::title(title));
        ui.scope(|ui| {
            ui.set_max_width(340.0);
            ui.label(text::dim(body));
        });
        ui.add_space(space::SM);
        let out = action(ui);
        ui.add_space(space::XS);
        out
    })
}

/// 16 px indeterminate spinner in the accent color.
pub fn spinner(ui: &mut Ui) -> Response {
    spinner_sized(ui, 16.0)
}

pub fn spinner_sized(ui: &mut Ui, size: f32) -> Response {
    let p = Palette::of(ui);
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    if ui.is_rect_visible(rect) {
        let time = ui.input(|i| i.time);
        let start = (time * 4.4) as f32;
        let r = size / 2.0 - 1.5;
        let center = rect.center();
        let painter = ui.painter();
        painter.circle_stroke(center, r, Stroke::new(2.0, p.border));
        let sweep = std::f32::consts::TAU * 0.28;
        let points: Vec<Pos2> = (0..=16)
            .map(|i| {
                let a = start + sweep * i as f32 / 16.0;
                center + Vec2::angled(a) * r
            })
            .collect();
        painter.line(points.clone(), Stroke::new(2.0, p.accent));
        // Round caps.
        for end in [points[0], points[16]] {
            painter.circle_filled(end, 1.0, p.accent);
        }
        ui.ctx().request_repaint();
    }
    resp
}

/// Determinate progress bar across the available width. `frac` is 0..=1.
pub fn progress(ui: &mut Ui, frac: f32) -> Response {
    let width = ui.available_width();
    progress_sized(ui, frac, width)
}

pub fn progress_sized(ui: &mut Ui, frac: f32, width: f32) -> Response {
    let p = Palette::of(ui);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 6.0), Sense::hover());
    resp.widget_info(|| WidgetInfo::new(Role::ProgressIndicator));
    if ui.is_rect_visible(rect) {
        let shown = anim_value(
            ui.ctx(),
            resp.id.with("frac"),
            frac.clamp(0.0, 1.0),
            Motion::SLIDE,
        );
        let painter = ui.painter();
        painter.rect_filled(rect, 3.0, p.border);
        if shown > 0.0 {
            let fill = Rect::from_min_size(
                rect.min,
                Vec2::new((rect.width() * shown).max(rect.height()), rect.height()),
            );
            painter.rect_filled(fill, 3.0, p.accent);
        }
    }
    resp
}

/// Progress bar for work with unknown length: a segment sweeping across the track.
pub fn progress_indeterminate(ui: &mut Ui) -> Response {
    let p = Palette::of(ui);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 6.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        let time = ui.input(|i| i.time) as f32;
        let phase = (time * 0.9).fract();
        let seg = rect.width() * 0.3;
        let x = rect.left() - seg + (rect.width() + seg) * phase;
        let painter = ui.painter().with_clip_rect(rect);
        painter.rect_filled(rect, 3.0, p.border);
        painter.rect_filled(
            Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(seg, rect.height())),
            3.0,
            p.accent,
        );
        ui.ctx().request_repaint();
    }
    resp
}

/// Hairline divider in the border color.
pub fn divider(ui: &mut Ui) -> Response {
    let p = Palette::of(ui);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, p.border);
    resp
}
