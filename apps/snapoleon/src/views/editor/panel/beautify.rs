//! The Beautify tab: a live preview, presets with thumbnails and the controls behind them.

use egui::{Align, Color32, Layout, Pos2, Rect, Sense, Stroke, StrokeKind, TextureHandle, Ui, Vec2};
use snap_core::{
    settings::{Background, BeautifyPreset, FrameStyle},
    t,
};
use snap_ui::{
    Palette, palette, radius, space, text,
    widgets::{
        ColorSwatches, Slider, TextField, dropdown, hex_string, parse_color, secondary_button, spinner, toggle,
        util::{Motion, anim, hover_anim},
    },
};

use super::{PanelCx, field, heading};

/// Preview and preset cells keep this proportion.
const CELL_H: f32 = 76.0;
const PREVIEW_H: f32 = 152.0;

#[derive(Default)]
pub struct State {
    /// The name typed for a new preset.
    name: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fill {
    Gradient,
    Solid,
    Blurred,
    Transparent,
    Image,
}

impl Fill {
    fn of(b: &Background) -> Fill {
        match b {
            Background::Gradient { .. } => Fill::Gradient,
            Background::Solid { .. } => Fill::Solid,
            Background::BlurredCapture => Fill::Blurred,
            Background::Transparent => Fill::Transparent,
            Background::Image { .. } => Fill::Image,
        }
    }

    fn default_background(self) -> Background {
        match self {
            Fill::Gradient => Background::Gradient { from: "#5E5CE6".into(), to: "#0A84FF".into(), angle_deg: 135.0 },
            Fill::Solid => Background::Solid { color: "#1C1C1E".into() },
            Fill::Blurred => Background::BlurredCapture,
            Fill::Transparent | Fill::Image => Background::Transparent,
        }
    }
}

/// Background colours offered under the gradient and solid controls.
const COLORS: [&str; 6] = ["#5E5CE6", "#0A84FF", "#30D158", "#FF9F0A", "#FF375F", "#1C1C1E"];

pub fn ui(ui: &mut Ui, st: &mut State, cx: &mut PanelCx) {
    let on = cx.s.doc.beautify.is_some();
    let mut switch = on;
    ui.horizontal(|ui| {
        ui.label(text::body_strong(t("editor.beautify.title")));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            toggle(ui, &mut switch);
        });
    });
    if switch != on {
        cx.s.checkpoint();
        cx.s.doc.beautify = switch.then(|| cx.vcx.settings.beautify_presets.first().cloned().unwrap_or_default());
        cx.s.touch();
    }

    preview(ui, cx);

    if !switch {
        ui.label(text::caption(t("editor.beautify.off_hint")));
        return;
    }

    heading(ui, &t("editor.beautify.presets"));
    presets(ui, cx);

    let Some(mut b) = cx.s.doc.beautify.clone() else { return };
    let before = b.clone();
    controls(ui, &mut b);
    if b != before {
        cx.s.checkpoint_coalesced("beautify");
        cx.s.doc.beautify = Some(b.clone());
        cx.s.touch();
    }

    heading(ui, &t("editor.beautify.save_title"));
    ui.horizontal(|ui| {
        let width = ui.available_width() - 96.0;
        ui.add(TextField::new(&mut st.name).hint(&t("editor.beautify.preset_name")).width(width.max(80.0)));
        let can_save = !st.name.trim().is_empty();
        ui.add_enabled_ui(can_save, |ui| {
            if secondary_button(ui, &t("editor.beautify.save")).clicked() {
                let mut preset = b.clone();
                preset.name = st.name.trim().to_string();
                cx.s.doc.beautify = Some(preset.clone());
                cx.vcx.update_settings(move |s| match s.beautify_presets.iter_mut().find(|p| p.name == preset.name) {
                    Some(existing) => *existing = preset,
                    None => s.beautify_presets.push(preset),
                });
                st.name.clear();
            }
        });
    });
    let saved = cx.vcx.settings.beautify_presets.iter().any(|p| p.name == b.name);
    if saved && cx.vcx.settings.beautify_presets.len() > 1 {
        let name = b.name.clone();
        if secondary_button(ui, &t("editor.beautify.delete")).clicked() {
            cx.vcx.update_settings(move |s| s.beautify_presets.retain(|p| p.name != name));
        }
    }
}

/// The finished image in a card, so changes are visible as they are made.
fn preview(ui: &mut Ui, cx: &mut PanelCx) {
    let p = palette(ui.ctx());
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, PREVIEW_H), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, radius::CARD, p.surface_2);
    painter.rect_stroke(rect, radius::CARD, Stroke::new(1.0, p.border), StrokeKind::Inside);
    let size = cx.previews.latest().map(|e| e.size);
    match (size, cx.previews.preview_texture(ui.ctx())) {
        (Some((w, h)), Some(texture)) => {
            let inner = rect.shrink(8.0);
            let fit = contain(Vec2::new(w as f32, h as f32), inner.size());
            let image = Rect::from_center_size(inner.center(), fit);
            ui.painter().image(texture.id(), image, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
        _ => {
            let mut child =
                ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(Layout::centered_and_justified(egui::Direction::TopDown)));
            spinner(&mut child);
        }
    }
}

fn presets(ui: &mut Ui, cx: &mut PanelCx) {
    let p = palette(ui.ctx());
    let presets: Vec<BeautifyPreset> = cx.vcx.settings.beautify_presets.clone();
    let current = cx.s.doc.beautify.clone();
    let gap = space::SM;
    let cell_w = ((ui.available_width() - gap) / 2.0).floor();
    let textures: Vec<TextureHandle> = cx.previews.thumb_textures(ui.ctx()).to_vec();
    let mut chosen = None;
    egui::Grid::new("editor-presets").spacing(Vec2::splat(gap)).show(ui, |ui| {
        for (i, preset) in presets.iter().enumerate() {
            let selected = current.as_ref() == Some(preset);
            let cell = ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(cell_w, CELL_H), Sense::click());
                paint_cell(ui, rect, &resp, selected, textures.get(i), &p);
                ui.add_sized([cell_w, 14.0], egui::Label::new(text::caption(&preset.name)).truncate());
                resp
            });
            if cell.inner.clicked() {
                chosen = Some(preset.clone());
            }
            if i % 2 == 1 {
                ui.end_row();
            }
        }
    });
    if let Some(preset) = chosen {
        cx.s.checkpoint();
        cx.s.doc.beautify = Some(preset);
        cx.s.touch();
    }
}

fn paint_cell(ui: &Ui, rect: Rect, resp: &egui::Response, selected: bool, thumb: Option<&TextureHandle>, p: &Palette) {
    let sel = anim(ui.ctx(), resp.id.with("sel"), selected, Motion::HOVER);
    let hover = hover_anim(ui, resp);
    let painter = ui.painter();
    painter.rect_filled(rect, radius::CARD, p.surface_2);
    if let (Some(texture), Some(size)) = (thumb, thumb.map(|t| t.size_vec2())) {
        let inner = rect.shrink(6.0);
        let image = Rect::from_center_size(inner.center(), contain(size, inner.size()));
        painter.image(texture.id(), image, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
    }
    painter.rect_stroke(rect, radius::CARD, Stroke::new(1.0, p.border_strong().gamma_multiply(0.5 + 0.5 * hover)), StrokeKind::Inside);
    if sel > 0.0 {
        painter.rect_stroke(rect.expand(1.0), radius::CARD + 1.0, Stroke::new(2.0, p.accent.gamma_multiply(sel)), StrokeKind::Inside);
    }
}

/// The largest size with `size`'s proportions that fits in `bounds`, never enlarging.
fn contain(size: Vec2, bounds: Vec2) -> Vec2 {
    let k = (bounds.x / size.x.max(1.0)).min(bounds.y / size.y.max(1.0));
    if size.x * k > size.x * 1.0001 { size } else { size * k }
}

fn controls(ui: &mut Ui, b: &mut BeautifyPreset) {
    heading(ui, &t("editor.beautify.background"));
    let mut fill = Fill::of(&b.background);
    let kinds = [
        (Fill::Gradient, t("editor.beautify.gradient")),
        (Fill::Solid, t("editor.beautify.solid")),
        (Fill::Blurred, t("editor.beautify.blurred")),
        (Fill::Transparent, t("editor.beautify.none")),
    ];
    let mut kinds: Vec<(Fill, String)> = kinds.iter().map(|(k, l)| (*k, l.to_string())).collect();
    if fill == Fill::Image {
        kinds.push((Fill::Image, t("editor.beautify.image").into_owned()));
    }
    let kinds: Vec<(Fill, &str)> = kinds.iter().map(|(k, l)| (*k, l.as_str())).collect();
    if dropdown(ui, "editor-bg-kind", &mut fill, &kinds).changed() {
        b.background = fill.default_background();
    }
    let palette: Vec<[u8; 4]> = COLORS.iter().filter_map(|c| parse_color(c)).collect();
    match &mut b.background {
        Background::Gradient { from, to, angle_deg } => {
            color_field(ui, &t("editor.beautify.from"), from, &palette);
            color_field(ui, &t("editor.beautify.to"), to, &palette);
            slider_field(ui, &t("editor.beautify.angle"), angle_deg, 0.0..=360.0, 5.0, "°");
        }
        Background::Solid { color } => color_field(ui, &t("editor.beautify.color"), color, &palette),
        _ => {}
    }

    heading(ui, &t("editor.beautify.layout"));
    for (label, value, max, step) in [
        (t("editor.beautify.padding"), &mut b.padding, 256.0, 4.0),
        (t("editor.beautify.corners"), &mut b.corner_radius, 48.0, 1.0),
        (t("editor.beautify.shadow"), &mut b.shadow, 96.0, 2.0),
    ] {
        let mut v = *value as f32;
        slider_field(ui, &label, &mut v, 0.0..=max, step, " px");
        *value = v as u32;
    }

    let ratios: [(Option<(u32, u32)>, String); 7] = [
        (None, t("editor.beautify.auto").into_owned()),
        (Some((1, 1)), "1:1".into()),
        (Some((4, 3)), "4:3".into()),
        (Some((3, 2)), "3:2".into()),
        (Some((16, 9)), "16:9".into()),
        (Some((2, 1)), "2:1".into()),
        (Some((9, 16)), "9:16".into()),
    ];
    let ratios: Vec<(Option<(u32, u32)>, &str)> = ratios.iter().map(|(r, l)| (*r, l.as_str())).collect();
    field(ui, &t("editor.beautify.aspect"), |ui| {
        dropdown(ui, "editor-aspect", &mut b.aspect, &ratios);
    });

    let frames = [
        (FrameStyle::None, t("editor.frame.none")),
        (FrameStyle::MacWindow, t("editor.frame.mac_light")),
        (FrameStyle::MacWindowDark, t("editor.frame.mac_dark")),
        (FrameStyle::Browser, t("editor.frame.browser_light")),
        (FrameStyle::BrowserDark, t("editor.frame.browser_dark")),
        (FrameStyle::Phone, t("editor.frame.phone")),
    ];
    let frames: Vec<(FrameStyle, &str)> = frames.iter().map(|(f, l)| (*f, l.as_ref())).collect();
    field(ui, &t("editor.beautify.frame"), |ui| {
        dropdown(ui, "editor-frame", &mut b.frame, &frames);
    });
}

fn color_field(ui: &mut Ui, label: &str, hex: &mut String, palette: &[[u8; 4]]) {
    let mut color = parse_color(hex).unwrap_or([28, 28, 30, 255]);
    field(ui, label, |ui| {
        if ui.add(ColorSwatches::new(&mut color, palette).alpha(false)).changed() {
            *hex = hex_string(color, false);
        }
    });
}

fn slider_field(ui: &mut Ui, label: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>, step: f32, suffix: &str) {
    field(ui, label, |ui| {
        let rail = (ui.available_width() - 60.0).max(80.0);
        ui.add(Slider::new(value, range).step(step).suffix(suffix).width(rail));
    });
}
