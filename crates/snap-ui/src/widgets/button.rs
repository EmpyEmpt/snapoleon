//! Buttons: text buttons in four kinds, icon buttons and editor tool buttons.

use egui::{Color32, Pos2, Rect, Response, Role, Sense, Stroke, StrokeKind, Ui, Vec2, Widget, WidgetInfo};

use super::{
    tooltip::tooltip,
    util::{self, control_height, focus_ring, hover_anim, lerp_color},
};
use crate::{
    text::body_strong_style,
    theme::{Palette, radius},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonKind {
    /// The one confident action: accent fill.
    Primary,
    #[default]
    Secondary,
    /// No chrome until hovered.
    Ghost,
    /// Destructive: danger fill.
    Danger,
}

/// A text button with an optional leading icon.
#[must_use = "add the button with `ui.add(..)`"]
pub struct Button<'a> {
    label: &'a str,
    icon: Option<&'a str>,
    kind: ButtonKind,
    min_width: f32,
}

impl<'a> Button<'a> {
    pub fn new(label: &'a str) -> Self {
        Self { label, icon: None, kind: ButtonKind::Secondary, min_width: 0.0 }
    }

    pub fn kind(mut self, kind: ButtonKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn icon(mut self, icon: &'a str) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn min_width(mut self, width: f32) -> Self {
        self.min_width = width;
        self
    }
}

/// Paints a button background for `kind` and returns the foreground color.
fn paint_background(ui: &Ui, rect: Rect, resp: &Response, kind: ButtonKind, round: f32) -> Color32 {
    let p = Palette::of(ui);
    let hover = hover_anim(ui, resp);
    let pressed = resp.is_pointer_button_down_on() && ui.is_enabled();
    let painter = ui.painter();
    match kind {
        ButtonKind::Primary | ButtonKind::Danger => {
            let base = if kind == ButtonKind::Primary { p.accent } else { p.danger };
            let hovered = lerp_color(base, p.on_accent, 0.14);
            let mut fill = lerp_color(base, hovered, hover);
            if pressed {
                fill = lerp_color(base, Color32::BLACK, 0.16);
            }
            painter.rect_filled(rect, round, fill);
            // A faint top sheen keeps the flat fill from looking dead.
            let inset = round.min(rect.width() / 2.0);
            painter.hline(
                (rect.left() + inset)..=(rect.right() - inset),
                rect.top() + 0.5,
                Stroke::new(1.0, Color32::from_white_alpha(if p.dark { 34 } else { 46 })),
            );
            p.on_accent
        }
        ButtonKind::Secondary => {
            painter.rect_filled(rect, round, p.surface_2);
            painter.rect_filled(rect, round, p.hover().gamma_multiply(hover));
            if pressed {
                painter.rect_filled(rect, round, p.pressed());
            }
            let stroke = lerp_color(p.border, p.border_strong(), hover);
            painter.rect_stroke(rect, round, Stroke::new(1.0, stroke), StrokeKind::Inside);
            p.text
        }
        ButtonKind::Ghost => {
            painter.rect_filled(rect, round, p.hover().gamma_multiply(hover));
            if pressed {
                painter.rect_filled(rect, round, p.pressed());
            }
            lerp_color(p.text_dim, p.text, hover)
        }
    }
}

impl Widget for Button<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let p = Palette::of(ui);
        let height = control_height(ui);
        let style = body_strong_style();
        let galley = util::layout(ui, self.label, &style, p.text);
        let icon_size = if height < 30.0 { 14.0 } else { 16.0 };
        let icon_w = self.icon.map_or(0.0, |_| icon_size + 6.0);
        let pad = if self.kind == ButtonKind::Ghost { 10.0 } else { 14.0 };
        let width = (galley.size().x + icon_w + pad * 2.0).max(self.min_width).max(height);
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, ui.is_enabled(), self.label));

        if ui.is_rect_visible(rect) {
            let fg = paint_background(ui, rect, &resp, self.kind, radius::CONTROL);
            let content_w = galley.size().x + icon_w;
            let mut x = rect.center().x - content_w / 2.0;
            if let Some(icon) = self.icon {
                util::paint_icon(ui, Pos2::new(x + icon_size / 2.0, rect.center().y), icon, icon_size, fg);
                x += icon_w;
            }
            let galley = util::layout(ui, self.label, &style, fg);
            let pos = util::snap(ui, Pos2::new(x, rect.center().y - galley.size().y / 2.0));
            ui.painter().galley(pos, galley, fg);
            focus_ring(ui, &resp, rect, radius::CONTROL);
        }
        resp
    }
}

pub fn primary_button(ui: &mut Ui, label: &str) -> Response {
    ui.add(Button::new(label).kind(ButtonKind::Primary))
}

pub fn secondary_button(ui: &mut Ui, label: &str) -> Response {
    ui.add(Button::new(label))
}

pub fn ghost_button(ui: &mut Ui, label: &str) -> Response {
    ui.add(Button::new(label).kind(ButtonKind::Ghost))
}

pub fn danger_button(ui: &mut Ui, label: &str) -> Response {
    ui.add(Button::new(label).kind(ButtonKind::Danger))
}

/// Square, chrome-less button holding a Phosphor icon. `tip` becomes its tooltip; `hotkey` adds a kbd chip.
pub fn icon_button(ui: &mut Ui, icon: &str, tip: &str, hotkey: Option<&str>) -> Response {
    let size = control_height(ui);
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, ui.is_enabled(), tip));
    if ui.is_rect_visible(rect) {
        let fg = paint_background(ui, rect, &resp, ButtonKind::Ghost, radius::CONTROL);
        util::paint_icon(ui, rect.center(), icon, if size < 30.0 { 15.0 } else { 17.0 }, fg);
        focus_ring(ui, &resp, rect, radius::CONTROL);
    }
    tooltip(resp, tip, hotkey)
}

/// Editor tool-rail button: 36 px square with an 18 px icon; the selected tool gets the soft accent wash.
pub fn tool_button(ui: &mut Ui, icon: &str, tip: &str, hotkey: Option<&str>, selected: bool) -> Response {
    let p = Palette::of(ui);
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(36.0), Sense::click());
    resp.widget_info(|| WidgetInfo::selected(Role::Button, ui.is_enabled(), selected, tip));
    if ui.is_rect_visible(rect) {
        let sel = util::anim(ui.ctx(), resp.id.with("sel"), selected, util::Motion::HOVER);
        let hover = hover_anim(ui, &resp);
        let round = 8.0;
        let painter = ui.painter();
        painter.rect_filled(rect, round, p.hover().gamma_multiply(hover * (1.0 - sel)));
        painter.rect_filled(rect, round, p.accent_soft.gamma_multiply(sel));
        if resp.is_pointer_button_down_on() {
            painter.rect_filled(rect, round, p.pressed());
        }
        let fg = lerp_color(lerp_color(p.text_dim, p.text, hover), p.accent, sel);
        util::paint_icon(ui, rect.center(), icon, 18.0, fg);
        focus_ring(ui, &resp, rect, round);
    }
    tooltip(resp, tip, hotkey)
}
