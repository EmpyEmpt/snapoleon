//! Text inputs, number field, slider and dropdown.

use std::ops::RangeInclusive;

use egui::{
    Align, Align2, DragValue, Frame, Key, Layout, Margin, Modifiers, Popup, PopupCloseBehavior,
    Pos2, Rect, Response, RichText, Role, ScrollArea, Sense, Shape, Stroke, StrokeKind, TextEdit,
    Ui, UiBuilder, Vec2, Widget, WidgetInfo, emath::Numeric,
};

use super::util::{self, Motion, anim, control_height, focus_ring, hover_anim, lerp_color};
use snap_core::t;

use crate::{
    icon, text,
    theme::{Palette, radius, with_alpha},
};

/// Single- or multi-line text input with optional leading icon and clear button.
#[must_use = "add the field with `ui.add(..)`"]
pub struct TextField<'a> {
    text: &'a mut String,
    hint: &'a str,
    icon: Option<&'a str>,
    password: bool,
    mono: bool,
    clearable: bool,
    width: Option<f32>,
    rows: usize,
}

impl<'a> TextField<'a> {
    pub fn new(text: &'a mut String) -> Self {
        Self {
            text,
            hint: "",
            icon: None,
            password: false,
            mono: false,
            clearable: false,
            width: None,
            rows: 1,
        }
    }

    pub fn hint(mut self, hint: &'a str) -> Self {
        self.hint = hint;
        self
    }

    pub fn icon(mut self, icon: &'a str) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn password(mut self, password: bool) -> Self {
        self.password = password;
        self
    }

    /// JetBrains Mono, for paths, templates, hex values and tokens.
    pub fn mono(mut self, mono: bool) -> Self {
        self.mono = mono;
        self
    }

    /// Show a clear button while there is text; Esc clears too.
    pub fn clearable(mut self, clearable: bool) -> Self {
        self.clearable = clearable;
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    /// More than one row makes it a multi-line editor.
    pub fn rows(mut self, rows: usize) -> Self {
        self.rows = rows.max(1);
        self
    }
}

impl Widget for TextField<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let p = Palette::of(ui);
        let style = if self.mono {
            text::mono()
        } else {
            text::body_style()
        };
        let row_h = control_height(ui);
        let multiline = self.rows > 1;
        let height = if multiline {
            (ui.text_style_height(&style) * self.rows as f32 + 20.0).max(row_h)
        } else {
            row_h
        };
        let width = self.width.unwrap_or(ui.spacing().text_edit_width);

        let (rect, outer) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
        let edit_id = outer.id.with("edit");
        if outer.clicked() {
            ui.memory_mut(|m| m.request_focus(edit_id));
        }
        let outer = outer.on_hover_cursor(egui::CursorIcon::Text);

        let has_text = !self.text.is_empty();
        let show_clear = self.clearable && has_text;
        let left = 12.0 + self.icon.map_or(0.0, |_| 16.0 + 8.0);
        let right = 12.0 + if show_clear { 22.0 } else { 0.0 };
        let inner = Rect::from_min_max(
            Pos2::new(
                rect.left() + left,
                rect.top() + if multiline { 10.0 } else { 0.0 },
            ),
            Pos2::new(
                rect.right() - right,
                rect.bottom() - if multiline { 10.0 } else { 0.0 },
            ),
        );

        let bg = ui.painter().add(Shape::Noop);
        let hint = RichText::new(self.hint)
            .color(p.text_faint)
            .text_style(style.clone());
        let font = util::font_id(ui, &style);
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(inner)
                .layout(Layout::left_to_right(Align::Center)),
        );
        let edit = if multiline {
            TextEdit::multiline(&mut *self.text).desired_rows(self.rows)
        } else {
            TextEdit::singleline(&mut *self.text).min_size(inner.size())
        }
        .id(edit_id)
        .frame(Frame::NONE)
        .margin(Margin::ZERO)
        .font(font)
        .text_color(p.text)
        .hint_text(hint)
        .password(self.password)
        .desired_width(inner.width())
        .align(if multiline {
            Align2::LEFT_TOP
        } else {
            Align2::LEFT_CENTER
        });
        let mut resp = if multiline {
            ScrollArea::vertical()
                .id_salt(edit_id)
                .auto_shrink([false, false])
                .show(&mut child, |ui| ui.add(edit))
                .inner
        } else {
            child.add(edit)
        };

        let focused = ui.memory(|m| m.has_focus(edit_id));
        let hover = hover_anim(ui, &outer);
        let focus = anim(ui.ctx(), outer.id.with("focus"), focused, Motion::HOVER);
        let round = radius::CARD;
        let border = lerp_color(
            lerp_color(p.border, p.border_strong(), hover),
            p.accent,
            focus,
        );
        let mut shapes = vec![
            egui::epaint::RectShape::filled(rect, round, p.surface_2).into(),
            egui::epaint::RectShape::stroke(
                rect,
                round,
                Stroke::new(1.0, border),
                StrokeKind::Inside,
            )
            .into(),
        ];
        if focus > 0.0 {
            shapes.push(
                egui::epaint::RectShape::stroke(
                    rect,
                    round,
                    Stroke::new(2.0, with_alpha(p.accent, 0.3 * focus)),
                    StrokeKind::Outside,
                )
                .into(),
            );
        }
        ui.painter().set(bg, Shape::Vec(shapes));

        if let Some(icon) = self.icon {
            let color = lerp_color(p.text_faint, p.text_dim, focus);
            util::paint_icon(
                ui,
                Pos2::new(rect.left() + 12.0 + 8.0, rect.center().y),
                icon,
                16.0,
                color,
            );
        }

        if show_clear {
            let r = Rect::from_center_size(
                Pos2::new(rect.right() - 12.0 - 8.0, rect.center().y),
                Vec2::splat(20.0),
            );
            let clear = ui.interact(r, outer.id.with("clear"), Sense::click());
            let h = hover_anim(ui, &clear);
            ui.painter()
                .circle_filled(r.center(), 9.0, p.hover().gamma_multiply(h));
            util::paint_icon(
                ui,
                r.center(),
                icon::X,
                12.0,
                lerp_color(p.text_faint, p.text, h),
            );
            if clear.clicked() {
                self.text.clear();
                resp.mark_changed();
                ui.memory_mut(|m| m.request_focus(edit_id));
            }
            clear.on_hover_text(t("ui.search.clear"));
        }
        if self.clearable
            && focused
            && has_text
            && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
        {
            self.text.clear();
            resp.mark_changed();
        }
        resp
    }
}

pub fn text_field(ui: &mut Ui, text: &mut String, hint: &str) -> Response {
    ui.add(TextField::new(text).hint(hint))
}

/// Full-width search input with a magnifier and a clear button.
pub fn search_field(ui: &mut Ui, text: &mut String, hint: &str) -> Response {
    let width = ui.available_width();
    ui.add(
        TextField::new(text)
            .hint(hint)
            .icon(icon::MAGNIFYING_GLASS)
            .clearable(true)
            .width(width),
    )
}

/// Draggable / editable number box using JetBrains Mono. `suffix` is e.g. `" px"`.
pub fn number_field<N: Numeric>(
    ui: &mut Ui,
    value: &mut N,
    range: RangeInclusive<N>,
    suffix: &str,
) -> Response {
    let span = range.end().to_f64() - range.start().to_f64();
    let speed = if N::INTEGRAL {
        (span / 400.0).clamp(1.0, 10.0)
    } else {
        span / 400.0
    };
    number_field_with(ui, value, range, suffix, speed)
}

pub fn number_field_with<N: Numeric>(
    ui: &mut Ui,
    value: &mut N,
    range: RangeInclusive<N>,
    suffix: &str,
    speed: f64,
) -> Response {
    ui.scope(|ui| {
        let round = egui::CornerRadius::same(radius::CARD as u8);
        let w = &mut ui.visuals_mut().widgets;
        for v in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            v.corner_radius = round;
        }
        ui.spacing_mut().interact_size.x = 72.0;
        ui.add(
            DragValue::new(value)
                .range(range)
                .suffix(suffix)
                .speed(speed),
        )
    })
    .inner
}

/// Horizontal slider with a mono value readout.
#[must_use = "add the slider with `ui.add(..)`"]
pub struct Slider<'a> {
    value: &'a mut f32,
    range: RangeInclusive<f32>,
    step: Option<f32>,
    suffix: &'a str,
    decimals: usize,
    show_value: bool,
    width: Option<f32>,
}

impl<'a> Slider<'a> {
    pub fn new(value: &'a mut f32, range: RangeInclusive<f32>) -> Self {
        Self {
            value,
            range,
            step: None,
            suffix: "",
            decimals: 0,
            show_value: true,
            width: None,
        }
    }

    pub fn step(mut self, step: f32) -> Self {
        self.step = Some(step);
        self
    }

    pub fn suffix(mut self, suffix: &'a str) -> Self {
        self.suffix = suffix;
        self
    }

    pub fn decimals(mut self, decimals: usize) -> Self {
        self.decimals = decimals;
        self
    }

    pub fn show_value(mut self, show: bool) -> Self {
        self.show_value = show;
        self
    }

    /// Width of the rail; the readout is added to the right of it.
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }
}

impl Widget for Slider<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let p = Palette::of(ui);
        let (lo, hi) = (*self.range.start(), *self.range.end());
        let rail_w = self.width.unwrap_or(ui.spacing().slider_width);
        let height = control_height(ui);
        let value_w = if self.show_value { 52.0 } else { 0.0 };

        let (outer, mut resp) =
            ui.allocate_exact_size(Vec2::new(rail_w + value_w, height), Sense::click_and_drag());
        let track = Rect::from_min_size(outer.min, Vec2::new(rail_w, height));
        let knob_r = 8.0;
        let x0 = track.left() + knob_r;
        let x1 = track.right() - knob_r;

        let snap = |v: f32| {
            let v = match self.step {
                Some(s) if s > 0.0 => lo + ((v - lo) / s).round() * s,
                _ => v,
            };
            v.clamp(lo, hi)
        };
        if let Some(pos) = resp.interact_pointer_pos()
            && (resp.dragged() || resp.is_pointer_button_down_on())
        {
            let t = ((pos.x - x0) / (x1 - x0)).clamp(0.0, 1.0);
            let new = snap(lo + (hi - lo) * t);
            if new != *self.value {
                *self.value = new;
                resp.mark_changed();
            }
        }
        if resp.has_focus() {
            let step = self.step.unwrap_or((hi - lo) / 100.0);
            let mut delta = 0.0;
            ui.input_mut(|i| {
                if i.consume_key(Modifiers::NONE, Key::ArrowRight)
                    || i.consume_key(Modifiers::NONE, Key::ArrowUp)
                {
                    delta += step;
                }
                if i.consume_key(Modifiers::NONE, Key::ArrowLeft)
                    || i.consume_key(Modifiers::NONE, Key::ArrowDown)
                {
                    delta -= step;
                }
            });
            if delta != 0.0 {
                *self.value = snap(*self.value + delta);
                resp.mark_changed();
            }
        }
        resp.widget_info(|| WidgetInfo::slider(ui.is_enabled(), *self.value as f64, ""));

        if ui.is_rect_visible(outer) {
            let t = if hi > lo {
                ((*self.value - lo) / (hi - lo)).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let t_anim = util::anim_value(
                ui.ctx(),
                resp.id.with("t"),
                t,
                if resp.dragged() { 0.0 } else { 0.08 },
            );
            let hover = hover_anim(ui, &resp);
            let cx = x0 + (x1 - x0) * t_anim;
            let cy = track.center().y;
            let rail = Rect::from_min_max(
                Pos2::new(x0 - knob_r + 1.0, cy - 2.0),
                Pos2::new(x1 + knob_r - 1.0, cy + 2.0),
            );
            let painter = ui.painter();
            painter.rect_filled(rail, 2.0, p.border);
            let filled = Rect::from_min_max(rail.min, Pos2::new(cx, rail.max.y));
            painter.rect_filled(filled, 2.0, p.accent);

            let active = resp.dragged();
            let r = knob_r + if active { 1.0 } else { hover };
            painter.circle_filled(
                Pos2::new(cx, cy + 1.0),
                r,
                egui::Color32::from_black_alpha(if p.dark { 80 } else { 36 }),
            );
            painter.circle_filled(
                Pos2::new(cx, cy),
                r,
                if p.dark {
                    egui::Color32::WHITE
                } else {
                    p.elevated
                },
            );
            painter.circle_stroke(Pos2::new(cx, cy), r - 0.5, Stroke::new(2.0, p.accent));
            if resp.has_focus() {
                painter.circle_stroke(
                    Pos2::new(cx, cy),
                    r + 1.0,
                    Stroke::new(2.0, with_alpha(p.accent, 0.4)),
                );
            }

            if self.show_value {
                let label = format!("{:.*}{}", self.decimals, *self.value, self.suffix);
                let galley = util::layout(ui, &label, &text::mono(), p.text_dim);
                let pos = Pos2::new(
                    outer.right() - galley.size().x,
                    outer.center().y - galley.size().y / 2.0,
                );
                ui.painter().galley(util::snap(ui, pos), galley, p.text_dim);
            }
        }
        resp
    }
}

/// Convenience for `ui.add(Slider::new(..).step(..).suffix(..))`.
pub fn slider(ui: &mut Ui, value: &mut f32, range: RangeInclusive<f32>) -> Response {
    ui.add(Slider::new(value, range))
}

/// Select-style dropdown. `changed()` is set on the returned response when another option is chosen.
pub fn dropdown<T: PartialEq + Clone>(
    ui: &mut Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    value: &mut T,
    options: &[(T, &str)],
) -> Response {
    ui.push_id(id_salt, |ui| dropdown_inner(ui, value, options))
        .inner
}

fn dropdown_inner<T: PartialEq + Clone>(
    ui: &mut Ui,
    value: &mut T,
    options: &[(T, &str)],
) -> Response {
    let p = Palette::of(ui);
    let style = text::body_style();
    let height = control_height(ui);
    let widest = options
        .iter()
        .map(|(_, l)| util::layout(ui, l, &style, p.text).size().x)
        .fold(0.0_f32, f32::max);
    let width = (widest + 12.0 + 34.0).max(ui.spacing().combo_width);

    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    let selected = options.iter().position(|(v, _)| v == value);
    resp.widget_info(|| {
        WidgetInfo::labeled(
            Role::ComboBox,
            ui.is_enabled(),
            selected.map_or("", |i| options[i].1),
        )
    });

    let popup_id = Popup::default_response_id(&resp);
    let open = Popup::is_id_open(ui.ctx(), popup_id);
    if ui.is_rect_visible(rect) {
        let hover = hover_anim(ui, &resp);
        let open_t = anim(ui.ctx(), resp.id.with("open"), open, Motion::HOVER);
        let round = radius::CARD;
        let painter = ui.painter();
        painter.rect_filled(rect, round, p.surface_2);
        painter.rect_filled(rect, round, p.hover().gamma_multiply(hover));
        let border = lerp_color(
            lerp_color(p.border, p.border_strong(), hover),
            p.accent,
            open_t,
        );
        painter.rect_stroke(rect, round, Stroke::new(1.0, border), StrokeKind::Inside);

        let label = selected.map_or("", |i| options[i].1);
        let galley = util::layout(ui, label, &style, p.text);
        let pos = Pos2::new(rect.left() + 12.0, rect.center().y - galley.size().y / 2.0);
        ui.painter()
            .with_clip_rect(Rect::from_min_max(
                rect.min,
                Pos2::new(rect.right() - 26.0, rect.max.y),
            ))
            .galley(util::snap(ui, pos), galley, p.text);
        let chevron = if open {
            icon::CARET_UP
        } else {
            icon::CARET_DOWN
        };
        util::paint_icon(
            ui,
            Pos2::new(rect.right() - 16.0, rect.center().y),
            chevron,
            12.0,
            lerp_color(p.text_faint, p.text_dim, hover.max(open_t)),
        );
        focus_ring(ui, &resp, rect, round);
    }

    let mut changed = false;
    let popup_style = ui.style().clone();
    let frame = Frame::new()
        .fill(p.elevated)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(radius::CARD)
        .inner_margin(Margin::same(4))
        .shadow(p.shadow());
    Popup::menu(&resp)
        .close_behavior(PopupCloseBehavior::CloseOnClick)
        .frame(frame)
        .gap(4.0)
        .width(rect.width())
        .style(move |s: &mut egui::Style| {
            *s = (*popup_style).clone();
            s.spacing.item_spacing.y = 2.0;
        })
        .show(|ui| {
            ui.set_min_width(rect.width() - 8.0);
            for (i, (opt, label)) in options.iter().enumerate() {
                let chosen = selected == Some(i);
                if menu_row(ui, label, chosen).clicked() && !chosen {
                    *value = opt.clone();
                    changed = true;
                }
            }
        });
    let mut resp = resp;
    if changed {
        resp.mark_changed();
    }
    resp
}

/// One row inside a dropdown popup.
fn menu_row(ui: &mut Ui, label: &str, selected: bool) -> Response {
    let p = Palette::of(ui);
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::click());
    resp.widget_info(|| WidgetInfo::selected(Role::MenuItem, ui.is_enabled(), selected, label));
    if ui.is_rect_visible(rect) {
        let hover = hover_anim(ui, &resp);
        let highlight = resp.has_focus() as u8 as f32;
        let painter = ui.painter();
        painter.rect_filled(rect, 6.0, p.hover().gamma_multiply(hover.max(highlight)));
        let fg = if selected { p.accent } else { p.text };
        let galley = util::layout(ui, label, &text::body_style(), fg);
        let pos = Pos2::new(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0);
        ui.painter().galley(util::snap(ui, pos), galley, fg);
        if selected {
            util::paint_icon(
                ui,
                Pos2::new(rect.right() - 16.0, rect.center().y),
                icon::CHECK,
                14.0,
                p.accent,
            );
        }
    }
    resp
}
