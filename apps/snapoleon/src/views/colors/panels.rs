//! The sections of the colors window: selected color, recent colors, contrast checker, measurements.

use std::time::{Duration, Instant};

use egui::{Align, Color32, Layout, Margin, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2};
use snap_core::{Rect as PxRect, t, tf};
use snap_image::color::{self, WcagLevel};
use snap_ui::{
    Palette, Tone, icon, space, text,
    widgets::{self, badge, card_frame, icon_button, tooltip, util},
};

use super::{ColorSample, Measurement};

pub const WHITE: [u8; 4] = [255, 255, 255, 255];
pub const BLACK: [u8; 4] = [0, 0, 0, 255];

const COPIED_FOR: Duration = Duration::from_millis(1400);
const VERDICT_LABEL_W: f32 = 112.0;

/// What the user did during one frame; the view applies it afterwards.
#[derive(Default)]
pub struct Out {
    pub copy: Option<(String, String)>,
    pub select: Option<usize>,
    pub clear_colors: bool,
    pub clear_measurements: bool,
    pub swap: bool,
}

/// Transient state the panels read.
pub struct Shown<'a> {
    pub colors: &'a [ColorSample],
    pub measurements: &'a [Measurement],
    pub selected: usize,
    pub text: &'a mut [u8; 4],
    pub background: &'a mut [u8; 4],
    pub copied: Option<&'a (String, Instant)>,
}

impl Shown<'_> {
    fn is_copied(&self, key: &str) -> bool {
        self.copied.is_some_and(|(k, at)| k == key && at.elapsed() < COPIED_FOR)
    }
}

fn rgb(c: [u8; 4]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

fn header(ui: &mut Ui, title: &str, trailing: impl FnOnce(&mut Ui)) {
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = 24.0;
        ui.horizontal(|ui| {
            ui.label(text::body_strong(title));
            ui.with_layout(Layout::right_to_left(Align::Center), trailing);
        });
    });
    ui.add_space(space::XS);
}

fn clear_button(ui: &mut Ui, label: &str) -> bool {
    widgets::ghost_button(ui, label).clicked()
}

/// A rounded color chip with a hairline so white and black stay visible on either surface.
fn paint_chip(ui: &Ui, rect: Rect, c: [u8; 4], round: f32) {
    let p = Palette::of(ui);
    let hairline = if p.dark { Color32::from_white_alpha(40) } else { Color32::from_black_alpha(36) };
    ui.painter().rect_filled(rect, round, rgb(c));
    ui.painter().rect_stroke(rect, round, Stroke::new(1.0, hairline), StrokeKind::Inside);
}

/// The big swatch plus every format with a copy button.
pub fn selected(ui: &mut Ui, shown: &Shown, out: &mut Out) {
    let Some(sample) = shown.colors.get(shown.selected) else { return };
    let c = sample.rgba;
    let p = Palette::of(ui);
    header(ui, &t("colors.current.title"), |_| {});
    card_frame(ui, 0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 88.0), Sense::hover());
        let top_round = egui::CornerRadius { nw: 10, ne: 10, sw: 0, se: 0 };
        ui.painter().rect_filled(rect, top_round, rgb(c));
        let ink = if color::contrast_ratio(c, WHITE) >= color::contrast_ratio(c, BLACK) { Color32::WHITE } else { Color32::BLACK };
        let hex = ui.painter().layout_no_wrap(color::hex(c), util::font_id(ui, &text::mono()), ink);
        ui.painter().galley(util::snap(ui, rect.left_bottom() + Vec2::new(16.0, -12.0 - hex.size().y)), hex, ink);
        let at = tf("colors.current.at", &[("x", &sample.at.x.to_string()), ("y", &sample.at.y.to_string())]);
        let at = ui.painter().layout_no_wrap(at, util::font_id(ui, &text::caption_style()), ink.gamma_multiply(0.75));
        ui.painter().galley(util::snap(ui, rect.right_bottom() - Vec2::new(16.0 + at.size().x, 12.0 + at.size().y)), at, ink);
        ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, p.border));

        let rows =
            [("hex", color::hex(c)), ("rgb", color::rgb(c)), ("hsl", color::hsl(c)), ("hsv", color::hsv(c)), ("oklch", color::oklch(c))];
        for (i, (key, value)) in rows.into_iter().enumerate() {
            let label = t(&format!("colors.format.{key}"));
            let resp = copy_row(ui, i > 0, &label, &value, shown.is_copied(key));
            if resp.clicked() {
                out.copy = Some((key.to_owned(), value));
            }
        }
    });
    ui.add_space(space::XL);
}

/// The copy tooltip hangs off the glyph at the row's right edge, not the row's bottom-left corner.
fn copy_tooltip(ui: &Ui, row: &Response, rect: Rect, copied: bool) {
    let glyph = Rect::from_center_size(Pos2::new(rect.right() - 20.0, rect.center().y), Vec2::splat(24.0));
    tooltip(ui.interact(glyph, row.id.with("copy-glyph"), Sense::hover()), &t(if copied { "colors.copied" } else { "colors.copy" }), None);
}

/// A format row: label, mono value, and a copy glyph that turns into a check after copying.
fn copy_row(ui: &mut Ui, divider: bool, label: &str, value: &str, copied: bool) -> Response {
    let p = Palette::of(ui);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::click());
    let hover = util::hover_anim(ui, &resp);
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if divider {
            painter.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, p.border));
        }
        painter.rect_filled(rect.shrink2(Vec2::new(4.0, 2.0)), 6.0, p.hover().gamma_multiply(hover));
        let label = ui.painter().layout_no_wrap(label.to_owned(), util::font_id(ui, &text::caption_style()), p.text_dim);
        painter.galley(util::snap(ui, Pos2::new(rect.left() + 16.0, rect.center().y - label.size().y / 2.0)), label, p.text_dim);
        let value = ui.painter().layout_no_wrap(value.to_owned(), util::font_id(ui, &text::mono()), p.text);
        painter.galley(util::snap(ui, Pos2::new(rect.left() + 80.0, rect.center().y - value.size().y / 2.0)), value, p.text);
        let (glyph, tint) = if copied { (icon::CHECK, p.success) } else { (icon::COPY, util::lerp_color(p.text_faint, p.text, hover)) };
        util::paint_icon(ui, Pos2::new(rect.right() - 20.0, rect.center().y), glyph, 15.0, tint);
    }
    copy_tooltip(ui, &resp, rect, copied);
    resp
}

/// Swatches of the recent picks; clicking one selects it.
pub fn recent(ui: &mut Ui, shown: &Shown, out: &mut Out) {
    header(ui, &t("colors.history.title"), |ui| {
        if clear_button(ui, &t("colors.history.clear")) {
            out.clear_colors = true;
        }
    });
    let p = Palette::of(ui);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(space::SM);
        for (i, sample) in shown.colors.iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::click());
            let hover = util::hover_anim(ui, &resp);
            paint_chip(ui, rect, sample.rgba, 6.0);
            if i == shown.selected {
                ui.painter().rect_stroke(rect.expand(2.0), 8.0, Stroke::new(2.0, p.accent), StrokeKind::Outside);
            } else if hover > 0.0 {
                ui.painter().rect_stroke(rect.expand(2.0), 8.0, Stroke::new(1.5, p.text_faint.gamma_multiply(hover)), StrokeKind::Outside);
            }
            if tooltip(resp, &color::hex(sample.rgba), None).clicked() {
                out.select = Some(i);
            }
        }
    });
    ui.add_space(space::XL);
}

/// Two color slots, a live sample and the WCAG verdicts.
pub fn contrast(ui: &mut Ui, shown: &mut Shown, out: &mut Out) {
    header(ui, &t("colors.contrast.title"), |_| {});
    let mut choices: Vec<[u8; 4]> = shown.colors.iter().map(|c| c.rgba).collect();
    for fixed in [WHITE, BLACK] {
        if !choices.contains(&fixed) {
            choices.push(fixed);
        }
    }
    card_frame(ui, space::LG as i8).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing = Vec2::splat(space::SM);
        let slot_w = (ui.available_width() - 32.0 - space::SM * 2.0) / 2.0;
        ui.horizontal(|ui| {
            slot(ui, slot_w, &t("colors.contrast.text"), shown.text, &choices);
            if icon_button(ui, icon::SWAP, &t("colors.contrast.swap"), None).clicked() {
                out.swap = true;
            }
            slot(ui, slot_w, &t("colors.contrast.background"), shown.background, &choices);
        });
        sample(ui, *shown.text, *shown.background);

        let ratio = color::contrast_ratio(*shown.text, *shown.background);
        let level = color::wcag_level(ratio);
        ui.add_space(space::XS);
        ui.label(text::mono_text(format!("{ratio:.2} : 1")).size(22.0).strong());
        verdict_row(ui, &t("colors.contrast.normal"), level >= WcagLevel::Aa, level >= WcagLevel::Aaa);
        verdict_row(ui, &t("colors.contrast.large"), level >= WcagLevel::AaLarge, level >= WcagLevel::Aa);
    });
    ui.add_space(space::XL);
}

/// One color slot: a swatch and its hex. Opens a popup with the recent colors.
fn slot(ui: &mut Ui, width: f32, label: &str, value: &mut [u8; 4], choices: &[[u8; 4]]) {
    let p = Palette::of(ui);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 48.0), Sense::click());
    let hover = util::hover_anim(ui, &resp);
    let popup_open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, 10.0, p.surface_2);
        painter.rect_filled(rect, 10.0, p.hover().gamma_multiply(hover));
        let border = if popup_open { p.accent } else { util::lerp_color(p.border, p.border_strong(), hover) };
        painter.rect_stroke(rect, 10.0, Stroke::new(1.0, border), StrokeKind::Inside);
        paint_chip(ui, Rect::from_min_size(rect.left_top() + Vec2::new(10.0, 10.0), Vec2::splat(28.0)), *value, 6.0);
        let caption = ui.painter().layout_no_wrap(label.to_owned(), util::font_id(ui, &text::caption_style()), p.text_dim);
        let hex = ui.painter().layout_no_wrap(color::hex(*value), util::font_id(ui, &text::mono()), p.text);
        let x = rect.left() + 48.0;
        ui.painter().galley(util::snap(ui, Pos2::new(x, rect.top() + 9.0)), caption, p.text_dim);
        ui.painter().galley(util::snap(ui, Pos2::new(x, rect.top() + 24.0)), hex, p.text);
        util::focus_ring(ui, &resp, rect, 10.0);
    }
    let frame = egui::Frame::new()
        .fill(p.elevated)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(10.0)
        .inner_margin(Margin::same(12))
        .shadow(p.shadow());
    let style = ui.style().clone();
    // Eight 32 px chips a row, 6 px apart, in a 12 px frame.
    let rows = choices.len().div_ceil(8).max(1) as f32;
    let (popup, max_h) = snap_ui::widgets::popup::fitted(egui::Popup::menu(&resp), 4.0, rows * 38.0 - 6.0 + 26.0);
    popup.close_behavior(egui::PopupCloseBehavior::CloseOnClick).frame(frame).style(move |s: &mut egui::Style| *s = (*style).clone()).show(
        |ui| {
            ui.set_max_width(8.0 * 32.0 + 7.0 * 6.0);
            snap_ui::widgets::popup::scroll_body(ui, max_h - 26.0, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::splat(6.0);
                    for &c in choices {
                        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::click());
                        paint_chip(ui, rect, c, 6.0);
                        if c == *value {
                            ui.painter().rect_stroke(rect.expand(2.0), 8.0, Stroke::new(2.0, p.accent), StrokeKind::Outside);
                        }
                        if tooltip(resp, &color::hex(c), None).clicked() {
                            *value = c;
                        }
                    }
                })
            });
        },
    );
}

/// "Aa" and a sentence in the chosen text color on the chosen background.
fn sample(ui: &mut Ui, fg: [u8; 4], bg: [u8; 4]) {
    let p = Palette::of(ui);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 72.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 10.0, rgb(bg));
    painter.rect_stroke(rect, 10.0, Stroke::new(1.0, p.border), StrokeKind::Inside);
    let clip = painter.with_clip_rect(rect.shrink(8.0));
    let big = ui.painter().layout_no_wrap(t("colors.contrast.sample").into_owned(), util::font_id(ui, &text::headline_style()), rgb(fg));
    let small = ui.painter().layout_no_wrap(t("colors.contrast.sample_text").into_owned(), util::font_id(ui, &text::body_style()), rgb(fg));
    let big_pos = Pos2::new(rect.left() + 20.0, rect.center().y - big.size().y / 2.0);
    let small_x = big_pos.x + big.size().x + 16.0;
    clip.galley(util::snap(ui, big_pos), big, rgb(fg));
    clip.galley(util::snap(ui, Pos2::new(small_x, rect.center().y - small.size().y / 2.0)), small, rgb(fg));
}

/// "Normal text  AA pass  AAA fail": the label in a fixed column so the badges line up.
fn verdict_row(ui: &mut Ui, label: &str, aa: bool, aaa: bool) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::SM;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(VERDICT_LABEL_W, 20.0), Sense::hover());
        let p = Palette::of(ui);
        let galley = ui.painter().layout_no_wrap(label.to_owned(), util::font_id(ui, &text::caption_style()), p.text_dim);
        ui.painter().galley(util::snap(ui, Pos2::new(rect.left(), rect.center().y - galley.size().y / 2.0)), galley, p.text_dim);
        for (name, pass) in [("AA", aa), ("AAA", aaa)] {
            let verdict = t(if pass { "colors.contrast.pass" } else { "colors.contrast.fail" });
            badge(ui, &format!("{name} {verdict}"), if pass { Tone::Success } else { Tone::Danger });
        }
    });
}

/// Recent measurements, newest first.
pub fn measurements(ui: &mut Ui, shown: &Shown, out: &mut Out) {
    header(ui, &t("colors.measurements.title"), |ui| {
        if clear_button(ui, &t("colors.measurements.clear")) {
            out.clear_measurements = true;
        }
    });
    card_frame(ui, 0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0;
        for (i, m) in shown.measurements.iter().enumerate() {
            let (value, note, glyph) = describe(m);
            let key = format!("measure-{i}");
            let resp = measurement_row(ui, i > 0, glyph, &value, &note, shown.is_copied(&key));
            if resp.clicked() {
                out.copy = Some((key, value));
            }
        }
    });
    ui.add_space(space::XL);
}

/// Mono value, a caption that says what it is, and the icon for the kind of measurement.
fn describe(m: &Measurement) -> (String, String, &'static str) {
    match m.b {
        None => (
            format!("{} × {}", m.a.w, m.a.h),
            tf("colors.measurements.area", &[("x", &m.a.x.to_string()), ("y", &m.a.y.to_string())]),
            icon::RULER,
        ),
        Some(b) => {
            let (dx, dy) = gap(m.a, b);
            (format!("{dx} × {dy}"), t("colors.measurements.gap").into_owned(), icon::ARROWS_LEFT_RIGHT)
        }
    }
}

/// Empty space between two rectangles along each axis (zero where they overlap).
fn gap(a: PxRect, b: PxRect) -> (u32, u32) {
    let between = |a0: i32, a1: i32, b0: i32, b1: i32| (b0 - a1).max(a0 - b1).max(0) as u32;
    (between(a.x, a.right(), b.x, b.right()), between(a.y, a.bottom(), b.y, b.bottom()))
}

fn measurement_row(ui: &mut Ui, divider: bool, glyph: &str, value: &str, note: &str, copied: bool) -> Response {
    let p = Palette::of(ui);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 52.0), Sense::click());
    let hover = util::hover_anim(ui, &resp);
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if divider {
            painter.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, p.border));
        }
        painter.rect_filled(rect.shrink2(Vec2::new(4.0, 2.0)), 6.0, p.hover().gamma_multiply(hover));
        util::paint_icon(ui, Pos2::new(rect.left() + 28.0, rect.center().y), glyph, 18.0, p.text_dim);
        let value = ui.painter().layout_no_wrap(value.to_owned(), util::font_id(ui, &text::mono()), p.text);
        let wrap = rect.width() - 56.0 - 44.0;
        let note = util::layout_wrapped(ui, note, &text::caption_style(), p.text_dim, wrap);
        let block = value.size().y + 2.0 + note.size().y;
        let top = rect.center().y - block / 2.0;
        let note_y = top + value.size().y + 2.0;
        painter.galley(util::snap(ui, Pos2::new(rect.left() + 56.0, top)), value, p.text);
        painter.galley(util::snap(ui, Pos2::new(rect.left() + 56.0, note_y)), note, p.text_dim);
        let (glyph, tint) = if copied { (icon::CHECK, p.success) } else { (icon::COPY, util::lerp_color(p.text_faint, p.text, hover)) };
        util::paint_icon(ui, Pos2::new(rect.right() - 20.0, rect.center().y), glyph, 15.0, tint);
    }
    copy_tooltip(ui, &resp, rect, copied);
    resp
}
