//! Color swatches with a custom-color popover (HSV picker), plus the inline picker itself.

use egui::{
    Color32, ColorImage, Context, Frame, Id, Margin, Popup, PopupCloseBehavior, Pos2, Rect, Response, Sense, Stroke, StrokeKind,
    TextureHandle, TextureOptions, Ui, Vec2, Widget, ecolor::Hsva, epaint::RectShape,
};
use snap_core::t;

use super::{
    input::TextField,
    util::{self, focus_ring, hover_anim},
};
use crate::{
    icon,
    theme::{Palette, with_alpha},
};

const SWATCH: f32 = 20.0;
const CELL: f32 = 28.0;
const PICKER_W: f32 = 216.0;

/// The default annotation palette: red, orange, yellow, green, teal, blue, violet, pink, white, black.
pub const DEFAULT_PALETTE: [[u8; 4]; 10] = [
    [0xFF, 0x4D, 0x52, 0xFF],
    [0xFF, 0x8A, 0x3D, 0xFF],
    [0xFF, 0xD1, 0x3D, 0xFF],
    [0x30, 0xC5, 0x7A, 0xFF],
    [0x24, 0xC6, 0xD6, 0xFF],
    [0x3D, 0x5A, 0xFE, 0xFF],
    [0x8E, 0x5B, 0xFF, 0xFF],
    [0xFF, 0x5C, 0xB8, 0xFF],
    [0xFF, 0xFF, 0xFF, 0xFF],
    [0x17, 0x18, 0x1C, 0xFF],
];

/// Row of color swatches with a selected ring and a trailing custom-color well.
#[must_use = "add the swatches with `ui.add(..)`"]
pub struct ColorSwatches<'a> {
    color: &'a mut [u8; 4],
    palette: &'a [[u8; 4]],
    alpha: bool,
}

impl<'a> ColorSwatches<'a> {
    pub fn new(color: &'a mut [u8; 4], palette: &'a [[u8; 4]]) -> Self {
        Self { color, palette, alpha: true }
    }

    /// Whether the custom popover offers an opacity slider (default on).
    pub fn alpha(mut self, alpha: bool) -> Self {
        self.alpha = alpha;
        self
    }
}

impl Widget for ColorSwatches<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let p = Palette::of(ui);
        let mut changed = false;
        let in_palette = self.palette.contains(self.color);
        let mut group = ui
            .horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::splat(0.0);
                for &c in self.palette {
                    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(CELL), Sense::click());
                    resp.widget_info(|| {
                        egui::WidgetInfo::selected(egui::Role::RadioButton, ui.is_enabled(), c == *self.color, hex_string(c, true))
                    });
                    if ui.is_rect_visible(rect) {
                        paint_swatch(ui, rect.center(), c, c == *self.color, hover_anim(ui, &resp), &p);
                        focus_ring(ui, &resp, Rect::from_center_size(rect.center(), Vec2::splat(SWATCH + 4.0)), (SWATCH + 4.0) / 2.0);
                    }
                    if resp.clicked() && c != *self.color {
                        *self.color = c;
                        changed = true;
                    }
                }
                changed |= custom_well(ui, self.color, !in_palette, self.alpha, &p);
            })
            .response;
        if changed {
            group.mark_changed();
        }
        group
    }
}

pub fn color_swatches(ui: &mut Ui, color: &mut [u8; 4], palette: &[[u8; 4]]) -> Response {
    ui.add(ColorSwatches::new(color, palette))
}

fn paint_swatch(ui: &Ui, center: Pos2, c: [u8; 4], selected: bool, hover: f32, p: &Palette) {
    let painter = ui.painter();
    let r = SWATCH / 2.0;
    let fill = Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
    if c[3] == 0 {
        painter.circle_filled(center, r, p.surface_2);
        painter.line_segment([center + Vec2::new(-r * 0.6, r * 0.6), center + Vec2::new(r * 0.6, -r * 0.6)], Stroke::new(1.5, p.danger));
    } else {
        painter.circle_filled(center, r, fill);
    }
    // A hairline keeps white and black swatches visible on either surface.
    painter.circle_stroke(
        center,
        r - 0.5,
        Stroke::new(1.0, if p.dark { Color32::from_white_alpha(40) } else { Color32::from_black_alpha(36) }),
    );
    if selected {
        painter.circle_stroke(center, r + 3.0, Stroke::new(2.0, p.accent));
    } else if hover > 0.0 {
        painter.circle_stroke(center, r + 2.5, Stroke::new(1.5, with_alpha(p.text_faint, hover)));
    }
}

/// Conic "custom" well; returns true when the color changed.
fn custom_well(ui: &mut Ui, color: &mut [u8; 4], selected: bool, alpha: bool, p: &Palette) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(CELL), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), t("ui.color.custom")));
    if ui.is_rect_visible(rect) {
        let hover = hover_anim(ui, &resp);
        let tex = hue_wheel(ui.ctx());
        let r = SWATCH / 2.0;
        let wheel = Rect::from_center_size(rect.center(), Vec2::splat(SWATCH));
        ui.painter()
            .add(RectShape::filled(wheel, r, Color32::WHITE).with_texture(tex.id(), Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0))));
        let inner = if selected { Color32::from_rgba_unmultiplied(color[0], color[1], color[2], 255) } else { p.surface };
        ui.painter().circle_filled(rect.center(), r - 4.5, inner);
        if !selected {
            util::paint_icon(ui, rect.center(), icon::PLUS, 9.0, p.text_dim);
        }
        if selected {
            ui.painter().circle_stroke(rect.center(), r + 3.0, Stroke::new(2.0, p.accent));
        } else if hover > 0.0 {
            ui.painter().circle_stroke(rect.center(), r + 2.5, Stroke::new(1.5, with_alpha(p.text_faint, hover)));
        }
        focus_ring(ui, &resp, Rect::from_center_size(rect.center(), Vec2::splat(SWATCH + 4.0)), (SWATCH + 4.0) / 2.0);
    }

    picker_popup(ui, &resp, color, alpha, p)
}

/// One solid swatch showing `color` that opens the custom-color popover; for a setting that holds a single color.
pub fn color_well(ui: &mut Ui, color: &mut [u8; 4], alpha: bool) -> Response {
    let p = Palette::of(ui);
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::splat(CELL), Sense::click());
    let name = util::control_name(ui);
    resp.widget_info(|| util::valued_info(name.as_deref(), egui::Role::Button, ui.is_enabled(), &hex_string(*color, alpha)));
    let open = Popup::is_id_open(ui.ctx(), Popup::default_response_id(&resp));
    if ui.is_rect_visible(rect) {
        paint_swatch(ui, rect.center(), *color, open, hover_anim(ui, &resp), &p);
        focus_ring(ui, &resp, Rect::from_center_size(rect.center(), Vec2::splat(SWATCH + 4.0)), (SWATCH + 4.0) / 2.0);
    }
    if picker_popup(ui, &resp, color, alpha, &p) {
        resp.mark_changed();
    }
    resp.on_hover_text(hex_string(*color, alpha))
}

/// The HSV picker popover that `resp` toggles; returns true when the color changed.
fn picker_popup(ui: &Ui, resp: &Response, color: &mut [u8; 4], alpha: bool, p: &Palette) -> bool {
    let mut changed = false;
    let frame = Frame::new()
        .fill(p.elevated)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(crate::radius::CARD)
        .inner_margin(Margin::same(12))
        .shadow(p.shadow());
    let style = ui.style().clone();
    // The picker's natural height: field, hue and opacity bars, hex row, and the frame.
    let want = 132.0 + 3.0 * 40.0 + 2.0 * 12.0 + 2.0;
    let (popup, max_h) = super::popup::fitted(Popup::from_toggle_button_response(resp), 6.0, want);
    popup
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
        .frame(frame)
        .style(move |s: &mut egui::Style| *s = (*style).clone())
        .show(|ui| {
            // The scroll area takes all the width it is given, and a popup opening has egui's 600 pt default.
            ui.set_max_width(PICKER_W);
            super::popup::scroll_body(ui, max_h - 2.0 * 12.0 - 2.0, |ui| {
                changed = color_picker(ui, color, alpha).changed();
            });
        });
    changed
}

/// Inline HSV picker: saturation/value field, hue bar, optional opacity bar and a hex input.
pub fn color_picker(ui: &mut Ui, color: &mut [u8; 4], alpha: bool) -> Response {
    let p = Palette::of(ui);
    let id = ui.scope_id().with("snap-color-picker");
    let mut hsva = ui
        .data(|d| d.get_temp::<Hsva>(id))
        .filter(|h| h.to_srgba_unmultiplied() == *color)
        .unwrap_or_else(|| Hsva::from_srgba_unmultiplied(*color));
    let mut changed = false;

    let mut group = ui
        .vertical(|ui| {
            ui.set_width(PICKER_W);
            ui.spacing_mut().item_spacing.y = 10.0;

            // Saturation / value field.
            let (sv_rect, sv) = ui.allocate_exact_size(Vec2::new(PICKER_W, 132.0), Sense::click_and_drag());
            sv.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ColorWell, ui.is_enabled(), t("ui.a11y.color_field")));
            if let Some(pos) = sv.interact_pointer_pos()
                && (sv.dragged() || sv.is_pointer_button_down_on())
            {
                hsva.s = ((pos.x - sv_rect.left()) / sv_rect.width()).clamp(0.0, 1.0);
                hsva.v = 1.0 - ((pos.y - sv_rect.top()) / sv_rect.height()).clamp(0.0, 1.0);
                changed = true;
            }
            let tex = sv_texture(ui.ctx(), hsva.h);
            ui.painter().add(RectShape::filled(sv_rect, 8.0, Color32::WHITE).with_texture(tex.id(), unit_uv()));
            ui.painter().rect_stroke(sv_rect, 8.0, Stroke::new(1.0, with_alpha(Color32::BLACK, 0.18)), StrokeKind::Inside);
            let knob = Pos2::new(sv_rect.left() + hsva.s * sv_rect.width(), sv_rect.top() + (1.0 - hsva.v) * sv_rect.height());
            let knob_fill = Color32::from_rgb(hsva.to_srgb()[0], hsva.to_srgb()[1], hsva.to_srgb()[2]);
            let painter = ui.painter();
            painter.circle_filled(knob, 7.0, knob_fill);
            painter.circle_stroke(knob, 7.0, Stroke::new(2.0, Color32::WHITE));
            painter.circle_stroke(knob, 8.5, Stroke::new(1.0, Color32::from_black_alpha(70)));

            // Hue bar.
            let hue_bar = bar(ui, &mut hsva.h, &hue_texture(ui.ctx()), None, &t("ui.a11y.hue"), false);
            changed |= hue_bar;

            if alpha {
                let base = Hsva { a: 1.0, ..hsva };
                let tex = alpha_texture(ui.ctx(), base.to_srgb());
                let mut a = hsva.a;
                if bar(ui, &mut a, &tex, Some(base.to_srgb()), &t("ui.color.opacity"), true) {
                    hsva.a = a;
                    changed = true;
                }
            }

            // Hex input and preview.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (preview, _) = ui.allocate_exact_size(Vec2::splat(util::control_height(ui)), Sense::hover());
                let [r, g, b, a] = hsva.to_srgba_unmultiplied();
                if a < 255 {
                    let tex = checker_texture(ui.ctx());
                    ui.painter().add(
                        RectShape::filled(preview, 8.0, Color32::WHITE)
                            .with_texture(tex.id(), Rect::from_min_max(Pos2::ZERO, Pos2::new(2.0, 2.0))),
                    );
                }
                ui.painter().rect_filled(preview, 8.0, Color32::from_rgba_unmultiplied(r, g, b, a));
                ui.painter().rect_stroke(preview, 8.0, Stroke::new(1.0, p.border), StrokeKind::Inside);

                let hex_id = id.with("hex");
                let mut s = ui.data(|d| d.get_temp::<String>(hex_id)).unwrap_or_default();
                let editing = ui.memory(|m| m.has_focus(hex_id.with("edit")));
                if !editing {
                    s = hex_string(hsva.to_srgba_unmultiplied(), alpha);
                }
                let before = s.clone();
                let hex = t("ui.color.hex");
                let field = util::with_control_name(ui, &hex, |ui| {
                    ui.add(TextField::new(&mut s).mono(true).width(ui.available_width()).hint(&hex))
                });
                if field.changed()
                    && s != before
                    && let Some(c) = parse_color(&s)
                {
                    hsva = Hsva::from_srgba_unmultiplied([c[0], c[1], c[2], if alpha { c[3] } else { 255 }]);
                    changed = true;
                }
                ui.data_mut(|d| d.insert_temp(hex_id, s));
            });
        })
        .response;

    if changed {
        *color = hsva.to_srgba_unmultiplied();
        group.mark_changed();
    }
    ui.data_mut(|d| d.insert_temp(id, hsva));
    group
}

/// A 12 px gradient bar with a round handle, named `name` (also its tooltip with `tip`). `value` is 0..1 (hue or
/// alpha); returns true when it changed.
fn bar(ui: &mut Ui, value: &mut f32, tex: &TextureHandle, handle_fill: Option<[u8; 3]>, name: &str, tip: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(PICKER_W, 14.0), Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::slider(ui.is_enabled(), f64::from(*value), name));
    if tip {
        resp.clone().on_hover_text(name);
    }
    let mut changed = false;
    let r = 7.0;
    if let Some(pos) = resp.interact_pointer_pos()
        && (resp.dragged() || resp.is_pointer_button_down_on())
    {
        *value = ((pos.x - (rect.left() + r)) / (rect.width() - 2.0 * r)).clamp(0.0, 1.0);
        changed = true;
    }
    let track = rect.shrink2(Vec2::new(0.0, 1.0));
    ui.painter().add(RectShape::filled(track, 6.0, Color32::WHITE).with_texture(tex.id(), unit_uv()));
    ui.painter().rect_stroke(track, 6.0, Stroke::new(1.0, with_alpha(Color32::BLACK, 0.16)), StrokeKind::Inside);
    let cx = rect.left() + r + *value * (rect.width() - 2.0 * r);
    let center = Pos2::new(cx, rect.center().y);
    let fill = match handle_fill {
        Some(rgb) => Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
        None => {
            let c = Hsva::new(*value, 1.0, 1.0, 1.0).to_srgb();
            Color32::from_rgb(c[0], c[1], c[2])
        }
    };
    ui.painter().circle_filled(center + Vec2::new(0.0, 1.0), r, Color32::from_black_alpha(60));
    ui.painter().circle_filled(center, r, Color32::WHITE);
    ui.painter().circle_filled(center, r - 2.5, fill);
    changed
}

fn unit_uv() -> Rect {
    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0))
}

fn load(ctx: &Context, key: &str, id: Id, image: ColorImage, options: TextureOptions) -> TextureHandle {
    ctx.load_texture_cached(key, id, image, options)
}

fn hue_wheel(ctx: &Context) -> TextureHandle {
    const N: usize = 64;
    let mut pixels = Vec::with_capacity(N * N);
    let c = (N as f32 - 1.0) / 2.0;
    for y in 0..N {
        for x in 0..N {
            let (dx, dy) = (x as f32 - c, y as f32 - c);
            let angle = dy.atan2(dx) / std::f32::consts::TAU + 0.5;
            let edge = (c + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
            let [r, g, b] = Hsva::new(angle, 0.78, 1.0, 1.0).to_srgb();
            pixels.push(Color32::from_rgba_unmultiplied(r, g, b, (edge * 255.0) as u8));
        }
    }
    load(ctx, "snap-ui/hue-wheel", Id::unique("snap-ui/hue-wheel"), ColorImage::new([N, N], pixels), TextureOptions::LINEAR)
}

fn hue_texture(ctx: &Context) -> TextureHandle {
    const N: usize = 256;
    let pixels = (0..N)
        .map(|x| {
            let [r, g, b] = Hsva::new(x as f32 / (N - 1) as f32, 1.0, 1.0, 1.0).to_srgb();
            Color32::from_rgb(r, g, b)
        })
        .collect();
    load(ctx, "snap-ui/hue-bar", Id::unique("snap-ui/hue-bar"), ColorImage::new([N, 1], pixels), TextureOptions::LINEAR)
}

fn sv_texture(ctx: &Context, hue: f32) -> TextureHandle {
    const W: usize = 64;
    const H: usize = 40;
    let mut pixels = Vec::with_capacity(W * H);
    for y in 0..H {
        for x in 0..W {
            let [r, g, b] = Hsva::new(hue, x as f32 / (W - 1) as f32, 1.0 - y as f32 / (H - 1) as f32, 1.0).to_srgb();
            pixels.push(Color32::from_rgb(r, g, b));
        }
    }
    load(ctx, "snap-ui/sv", Id::unique("snap-ui/sv"), ColorImage::new([W, H], pixels), TextureOptions::LINEAR)
}

/// Opacity bar: checkerboard under a transparent→opaque ramp of `rgb`, baked at 2x for crisp checks.
fn alpha_texture(ctx: &Context, rgb: [u8; 3]) -> TextureHandle {
    const W: usize = (PICKER_W as usize) * 2;
    const H: usize = 28;
    let mut pixels = Vec::with_capacity(W * H);
    for y in 0..H {
        for x in 0..W {
            let cell = ((x / 10) + (y / 10)) % 2 == 0;
            let base = if cell { 255.0 } else { 204.0 };
            let a = x as f32 / (W - 1) as f32;
            let mix = |c: u8| (c as f32 * a + base * (1.0 - a)).round() as u8;
            pixels.push(Color32::from_rgb(mix(rgb[0]), mix(rgb[1]), mix(rgb[2])));
        }
    }
    load(ctx, "snap-ui/alpha-bar", Id::unique("snap-ui/alpha-bar"), ColorImage::new([W, H], pixels), TextureOptions::LINEAR)
}

/// 2x2 checkerboard repeated by UVs in 0..2.
fn checker_texture(ctx: &Context) -> TextureHandle {
    let (a, b) = (Color32::from_gray(255), Color32::from_gray(204));
    let options = TextureOptions { wrap_mode: egui::TextureWrapMode::Repeat, ..TextureOptions::NEAREST };
    load(ctx, "snap-ui/checker", Id::unique("snap-ui/checker"), ColorImage::new([2, 2], vec![a, b, b, a]), options)
}

/// `#RRGGBB`, or `#RRGGBBAA` when `alpha` and the color is translucent.
pub fn hex_string(c: [u8; 4], alpha: bool) -> String {
    if alpha && c[3] != 255 {
        format!("#{:02X}{:02X}{:02X}{:02X}", c[0], c[1], c[2], c[3])
    } else {
        format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
    }
}

/// Parses `#RGB`, `#RRGGBB` and `#RRGGBBAA` (leading `#` optional).
pub fn parse_color(s: &str) -> Option<[u8; 4]> {
    let h = s.trim().trim_start_matches('#');
    if !h.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    match h.len() {
        3 => {
            let n = |i: usize| u8::from_str_radix(&h[i..i + 1], 16).ok().map(|v| v * 17);
            Some([n(0)?, n(1)?, n(2)?, 255])
        }
        6 => Some([byte(0)?, byte(2)?, byte(4)?, 255]),
        8 => Some([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        _ => None,
    }
}
