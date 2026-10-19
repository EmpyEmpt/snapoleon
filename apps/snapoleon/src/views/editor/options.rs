//! The context bar: the options of the active tool, or of the selected shape when the select tool is active.

use egui::{Frame, Margin, Sense, Stroke, StrokeKind, Ui, Vec2};
use snap_annotate::{ArrowHead, ShapeKind, Stamp, Style, TextBg};
use snap_core::{PointF, settings::RedactStyle, t};
use snap_ui::{
    Palette, icon, palette, radius, text,
    widgets::{
        ColorSwatches, chip_with_icon, number_field, primary_button, secondary_button, segmented, stroke_picker_with, toolbar_separator,
        tooltip,
        util::{Motion, anim, focus_ring, hover_anim, lerp_color, paint_icon},
    },
};

use super::{
    canvas::Canvas,
    crop::Aspect,
    session::Session,
    tools::{Extras, Tool, ToolSet},
};

/// Something the bar asks the editor to do.
pub enum Request {
    /// Cover personal data; `smart` also asks the AI endpoint for what patterns miss.
    AutoRedact { smart: bool },
}

pub struct OptionsCx<'a> {
    pub s: &'a mut Session,
    pub tools: &'a mut ToolSet,
    pub tool: Tool,
    pub canvas: &'a mut Canvas,
    pub ui_scale: f32,
    /// An auto-redact search is running.
    pub searching: bool,
    /// AI is on: the redact bar offers the smart search.
    pub ai: bool,
}

/// Tools that draw with the same stroke width.
const LINE_TOOLS: [Tool; 5] = [Tool::Arrow, Tool::Line, Tool::Rect, Tool::Ellipse, Tool::Pen];

/// Stroke widths offered by the picker, at 1x density.
const WIDTHS: [f32; 6] = [2.0, 3.0, 4.0, 6.0, 8.0, 12.0];

pub fn show(ui: &mut Ui, cx: &mut OptionsCx) -> Option<Request> {
    let p = palette(ui.ctx());
    let mut request = None;
    let frame = Frame::new().fill(p.surface).inner_margin(Margin::symmetric(12, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
        ui.set_min_height(28.0);
        ui.horizontal_wrapped(|ui| request = bar(ui, cx));
    });
    let rect = frame.response.rect;
    ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, p.border));
    request
}

/// The tool whose options apply right now.
fn effective_tool(cx: &OptionsCx) -> Option<Tool> {
    if cx.tool != Tool::Select {
        return Some(cx.tool);
    }
    cx.s.selected_shapes().next().and_then(|s| Tool::of_kind(&s.kind))
}

fn bar(ui: &mut Ui, cx: &mut OptionsCx) -> Option<Request> {
    if cx.tool == Tool::Crop {
        crop_options(ui, cx);
        return None;
    }
    let Some(tool) = effective_tool(cx) else {
        ui.label(text::dim(if cx.tool == Tool::Select && !cx.s.selection.is_empty() {
            t("editor.options.image")
        } else {
            t("editor.options.hint")
        }));
        return None;
    };
    let mut style = current_style(cx, tool);
    let mut extras = cx.tools.extras.clone();
    let (style_before, extras_before) = (style, extras.clone());
    let palette_colors = cx.tools.palette.clone();
    let scale = cx.ui_scale;
    let mut request = None;

    let color = |ui: &mut Ui, style: &mut Style, alpha: bool| {
        ui.add(ColorSwatches::new(&mut style.stroke, &palette_colors).alpha(alpha));
    };
    let width = |ui: &mut Ui, style: &mut Style| {
        let presets: Vec<f32> = WIDTHS.iter().map(|w| (w * scale).round()).collect();
        stroke_picker_with(ui, &mut style.width, &presets);
    };

    match tool {
        Tool::Arrow => {
            color(ui, &mut style, true);
            sep(ui);
            width(ui, &mut style);
            sep(ui);
            let heads = [
                (ArrowHead::Triangle, t("editor.arrow.solid")),
                (ArrowHead::Open, t("editor.arrow.open")),
                (ArrowHead::None, t("editor.arrow.none")),
            ];
            let heads: Vec<(ArrowHead, &str)> = heads.iter().map(|(h, l)| (*h, l.as_ref())).collect();
            segmented(ui, &mut extras.arrow_head, &heads);
            toggle(ui, icon::BEZIER_CURVE, &t("editor.option.curved"), &mut extras.curved);
            toggle(ui, icon::ARROWS_LEFT_RIGHT, &t("editor.option.double"), &mut extras.double);
            toggle(ui, icon::SUN_DIM, &t("editor.option.shadow"), &mut style.shadow);
        }
        Tool::Line => {
            color(ui, &mut style, true);
            sep(ui);
            width(ui, &mut style);
            sep(ui);
            toggle(ui, icon::DOTS_THREE, &t("editor.option.dashed"), &mut style.dashed);
            toggle(ui, icon::SUN_DIM, &t("editor.option.shadow"), &mut style.shadow);
        }
        Tool::Rect | Tool::Ellipse => {
            color(ui, &mut style, true);
            sep(ui);
            width(ui, &mut style);
            sep(ui);
            let mut filled = style.fill.is_some();
            toggle(ui, icon::PAINT_BUCKET, &t("editor.option.fill"), &mut filled);
            if filled != style.fill.is_some() {
                style.fill = filled.then(|| translucent(style.stroke));
            }
            if tool == Tool::Rect {
                toggle(ui, icon::SQUARE_HALF, &t("editor.option.rounded"), &mut extras.rounded);
            }
            toggle(ui, icon::DOTS_THREE, &t("editor.option.dashed"), &mut style.dashed);
            toggle(ui, icon::SUN_DIM, &t("editor.option.shadow"), &mut style.shadow);
        }
        Tool::Pen => {
            color(ui, &mut style, true);
            sep(ui);
            width(ui, &mut style);
            sep(ui);
            toggle(ui, icon::SUN_DIM, &t("editor.option.shadow"), &mut style.shadow);
        }
        Tool::Highlighter => {
            color(ui, &mut style, false);
            sep(ui);
            width(ui, &mut style);
        }
        Tool::Text => {
            color(ui, &mut style, true);
            sep(ui);
            font_size(ui, &mut extras);
            toggle(ui, icon::TEXT_B, &t("editor.option.bold"), &mut extras.bold);
            let backgrounds =
                [(TextBg::None, t("editor.text.plain")), (TextBg::Solid, t("editor.text.box")), (TextBg::Bubble, t("editor.text.bubble"))];
            let backgrounds: Vec<(TextBg, &str)> = backgrounds.iter().map(|(b, l)| (*b, l.as_ref())).collect();
            segmented(ui, &mut extras.text_bg, &backgrounds);
            toggle(ui, icon::SUN_DIM, &t("editor.option.shadow"), &mut style.shadow);
        }
        Tool::Callout => {
            color(ui, &mut style, true);
            sep(ui);
            font_size(ui, &mut extras);
            toggle(ui, icon::SUN_DIM, &t("editor.option.shadow"), &mut style.shadow);
        }
        Tool::Step => {
            color(ui, &mut style, true);
            sep(ui);
            toggle(ui, icon::SUN_DIM, &t("editor.option.shadow"), &mut style.shadow);
            ui.label(text::dim(t("editor.options.step_hint")));
        }
        Tool::Redact => {
            let styles = [
                (RedactStyle::Pixelate, t("editor.redact.pixelate")),
                (RedactStyle::Solid, t("editor.redact.solid")),
                (RedactStyle::Blur, t("editor.redact.blur")),
            ];
            let styles: Vec<(RedactStyle, &str)> = styles.iter().map(|(b, l)| (*b, l.as_ref())).collect();
            segmented(ui, &mut extras.redact, &styles);
            sep(ui);
            let label = if cx.searching { t("editor.redact.searching") } else { t("editor.redact.auto") };
            let button = ui.add_enabled(!cx.searching, snap_ui::widgets::Button::new(&label).icon(icon::SHIELD_CHECK));
            if button.clicked() {
                request = Some(Request::AutoRedact { smart: false });
            }
            tooltip(button, &t("editor.redact.auto_tip"), None);
            if cx.ai {
                let smart = ui.add_enabled(!cx.searching, snap_ui::widgets::Button::new(&t("editor.redact.smart")).icon(icon::MAGIC_WAND));
                if smart.clicked() {
                    request = Some(Request::AutoRedact { smart: true });
                }
                tooltip(smart, &t("editor.redact.smart_tip"), None);
            }
            if cx.searching {
                snap_ui::widgets::spinner(ui);
            }
        }
        Tool::Spotlight => {
            let shapes = [(false, t("editor.spotlight.rect")), (true, t("editor.spotlight.ellipse"))];
            let shapes: Vec<(bool, &str)> = shapes.iter().map(|(b, l)| (*b, l.as_ref())).collect();
            segmented(ui, &mut extras.spot_ellipse, &shapes);
        }
        Tool::Magnifier => {
            color(ui, &mut style, true);
            sep(ui);
            width(ui, &mut style);
        }
        Tool::Stamp => {
            let picked = stamp_row(ui, &mut extras.stamp);
            if picked {
                style.stroke = extras.stamp.default_color();
            }
            sep(ui);
            color(ui, &mut style, true);
        }
        Tool::Measure => {
            color(ui, &mut style, true);
            sep(ui);
            width(ui, &mut style);
        }
        Tool::Select | Tool::Crop => {}
    }

    if style != style_before || extras != extras_before {
        commit(cx, tool, style_before, style, &extras_before, extras);
    }
    request
}

fn crop_options(ui: &mut Ui, cx: &mut OptionsCx) {
    let image = cx.s.doc.canvas_size();
    let aspects: Vec<(Aspect, String)> = Aspect::ALL.iter().map(|a| (*a, a.label())).collect();
    let aspects: Vec<(Aspect, &str)> = aspects.iter().map(|(a, l)| (*a, l.as_str())).collect();
    let mut aspect = cx.canvas.crop.aspect;
    if segmented(ui, &mut aspect, &aspects).changed() {
        cx.canvas.crop.set_aspect(aspect, image);
    }
    sep(ui);
    let r = cx.canvas.crop.rect;
    ui.label(text::mono_text(format!("{} × {}", r.w.round() as i64, r.h.round() as i64)));
    sep(ui);
    if primary_button(ui, &t("editor.crop.apply")).clicked() {
        let Canvas { crop, .. } = cx.canvas;
        crop.apply(cx.s);
    }
    if secondary_button(ui, &t("editor.crop.cancel")).clicked() {
        cx.canvas.crop.cancel();
        cx.canvas.crop.begin(cx.s);
    }
    if cx.s.doc.crop.is_some() && secondary_button(ui, &t("editor.crop.reset")).clicked() {
        cx.canvas.crop.reset(cx.s);
        cx.canvas.crop.begin(cx.s);
    }
}

/// The style the controls start from: the selected shape's own when selecting, the tool's default otherwise.
fn current_style(cx: &OptionsCx, tool: Tool) -> Style {
    if cx.tool == Tool::Select
        && let Some(shape) = cx.s.selected_shapes().next()
    {
        return shape.style;
    }
    cx.tools.styles.get(&tool).copied().unwrap_or_default()
}

/// Make a change in the bar real: remember it as the tool's default and apply it to the selected shapes.
fn commit(cx: &mut OptionsCx, tool: Tool, before: Style, after: Style, extras_before: &Extras, extras: Extras) {
    cx.tools.styles.insert(tool, after);
    // One line weight for the line tools: it is what people expect to keep when they switch between them.
    if after.width != before.width && LINE_TOOLS.contains(&tool) {
        for other in LINE_TOOLS {
            if let Some(style) = cx.tools.styles.get_mut(&other) {
                style.width = after.width;
            }
        }
    }
    // Shape defaults of the other tools follow a colour or width picked in the select state.
    let selecting = cx.tool == Tool::Select;
    let ids: Vec<u64> = cx.s.selection.clone();
    let touched: Vec<u64> =
        ids.into_iter().filter(|id| cx.s.doc.get(*id).is_some_and(|s| selecting || Tool::of_kind(&s.kind) == Some(tool))).collect();
    if !touched.is_empty() {
        cx.s.checkpoint_coalesced("style");
        for id in &touched {
            let Some(shape) = cx.s.doc.get_mut(*id) else { continue };
            let kind_tool = Tool::of_kind(&shape.kind);
            if after.stroke != before.stroke {
                shape.style.stroke = after.stroke;
                if shape.style.fill.is_some() && matches!(shape.kind, ShapeKind::Rect { .. } | ShapeKind::Ellipse { .. }) {
                    shape.style.fill = Some(translucent(after.stroke));
                }
            }
            if after.width != before.width {
                shape.style.width = after.width;
            }
            if after.fill != before.fill && matches!(shape.kind, ShapeKind::Rect { .. } | ShapeKind::Ellipse { .. }) {
                shape.style.fill = after.fill;
            }
            if after.shadow != before.shadow {
                shape.style.shadow = after.shadow;
            }
            if after.dashed != before.dashed {
                shape.style.dashed = after.dashed;
            }
            if kind_tool == Some(tool) {
                apply_extras(&mut shape.kind, extras_before, &extras, cx.ui_scale);
            }
        }
        cx.s.touch();
    }
    cx.tools.extras = extras;
}

fn apply_extras(kind: &mut ShapeKind, before: &Extras, now: &Extras, scale: f32) {
    match kind {
        ShapeKind::Arrow { from, to, ctrl, head, double } => {
            if now.arrow_head != before.arrow_head {
                *head = now.arrow_head;
            }
            if now.double != before.double {
                *double = now.double;
            }
            if now.curved != before.curved {
                *ctrl = now
                    .curved
                    .then(|| PointF::new((from.x + to.x) / 2.0 - (to.y - from.y) * 0.2, (from.y + to.y) / 2.0 + (to.x - from.x) * 0.2));
            }
        }
        ShapeKind::Rect { radius, .. } if now.rounded != before.rounded => *radius = if now.rounded { 10.0 * scale } else { 0.0 },
        ShapeKind::Text { size, bold, bg, max_width, .. } => {
            if now.font_size != before.font_size {
                if let Some(w) = max_width {
                    *w *= now.font_size / *size;
                }
                *size = now.font_size;
            }
            if now.bold != before.bold {
                *bold = now.bold;
            }
            if now.text_bg != before.text_bg {
                *bg = now.text_bg;
            }
        }
        ShapeKind::Callout { size, .. } if now.font_size != before.font_size => *size = now.font_size,
        ShapeKind::Redact { style, .. } if now.redact != before.redact => *style = now.redact,
        ShapeKind::Spotlight { ellipse, .. } if now.spot_ellipse != before.spot_ellipse => *ellipse = now.spot_ellipse,
        ShapeKind::Stamp { glyph, .. } if now.stamp != before.stamp => *glyph = now.stamp,
        _ => {}
    }
}

/// The fill a "Fill" toggle gives a shape: its outline colour at about a third of the strength.
fn translucent(c: [u8; 4]) -> [u8; 4] {
    [c[0], c[1], c[2], 84]
}

fn sep(ui: &mut Ui) {
    toolbar_separator(ui);
}

fn font_size(ui: &mut Ui, extras: &mut Extras) {
    ui.label(text::dim(t("editor.option.size")));
    number_field(ui, &mut extras.font_size, 8.0..=240.0, " px");
}

/// A labelled on/off chip.
fn toggle(ui: &mut Ui, glyph: &str, label: &str, on: &mut bool) {
    if chip_with_icon(ui, glyph, label, *on).clicked() {
        *on = !*on;
    }
}

/// The eight stamps as icon buttons. Returns whether the choice changed.
fn stamp_row(ui: &mut Ui, current: &mut Stamp) -> bool {
    let mut changed = false;
    let p = palette(ui.ctx());
    for stamp in Stamp::ALL {
        let glyph = match stamp {
            Stamp::Check => icon::CHECK_FAT,
            Stamp::Cross => icon::X,
            Stamp::Question => icon::QUESTION,
            Stamp::Warning => icon::WARNING,
            Stamp::Star => icon::STAR,
            Stamp::Heart => icon::HEART,
            Stamp::ThumbsUp => icon::THUMBS_UP,
            Stamp::Info => icon::INFO,
        };
        if icon_toggle(ui, glyph, &stamp.label(), *current == stamp, &p).clicked() && *current != stamp {
            *current = stamp;
            changed = true;
        }
    }
    changed
}

fn icon_toggle(ui: &mut Ui, glyph: &str, tip: &str, selected: bool, p: &Palette) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let sel = anim(ui.ctx(), resp.id.with("sel"), selected, Motion::HOVER);
        let hover = hover_anim(ui, &resp);
        let painter = ui.painter();
        painter.rect_filled(rect, radius::CONTROL, p.hover().gamma_multiply(hover * (1.0 - sel)));
        painter.rect_filled(rect, radius::CONTROL, p.accent_soft.gamma_multiply(sel));
        let color = lerp_color(lerp_color(p.text_dim, p.text, hover), p.accent, sel);
        paint_icon(ui, rect.center(), glyph, 16.0, color);
        if selected {
            painter.rect_stroke(rect, radius::CONTROL, Stroke::new(1.0, p.accent.gamma_multiply(0.45)), StrokeKind::Inside);
        }
        focus_ring(ui, &resp, rect, radius::CONTROL);
    }
    tooltip(resp, tip, None)
}
