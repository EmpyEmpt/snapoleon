//! A small popup menu (card "more" menu, right click, "copy as"). Unlike a stock egui popup it is driven by the
//! caller's own state, so it can stay open while the widget that opened it is hidden or re-created every frame.

use egui::{
    Area, Color32, Context, CornerRadius, FontId, Frame, Id, Key, LayerId, Margin, Order, Pos2, Rect, Role, Sense, Stroke, Ui, Vec2,
    WidgetInfo,
};
use snap_ui::{
    Palette,
    text::body_style,
    widgets::{accel_text, popup, util},
};

const ROW_H: f32 = 30.0;
const PAD: f32 = 4.0;
const SEPARATOR_H: f32 = 9.0;

pub struct Item<T> {
    pub icon: &'static str,
    pub label: String,
    pub value: T,
    pub hotkey: Option<&'static str>,
    pub danger: bool,
    pub enabled: bool,
    /// A hairline above this row.
    pub separator: bool,
}

impl<T> Item<T> {
    pub fn new(icon: &'static str, label: impl Into<String>, value: T) -> Self {
        Self { icon, label: label.into(), value, hotkey: None, danger: false, enabled: true, separator: false }
    }

    pub fn hotkey(mut self, hotkey: &'static str) -> Self {
        self.hotkey = Some(hotkey);
        self
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn separated(mut self) -> Self {
        self.separator = true;
        self
    }
}

/// Where the menu hangs.
#[derive(Clone, Copy)]
pub enum Anchor {
    /// Under (or above, near the window bottom) a widget, right edges aligned.
    Widget(Rect),
    /// Like [`Anchor::Widget`] with the left edges aligned.
    Start(Rect),
    /// At the pointer.
    Point(Pos2),
}

impl Anchor {
    fn widget(self) -> Option<Rect> {
        match self {
            Anchor::Widget(rect) | Anchor::Start(rect) => Some(rect),
            Anchor::Point(_) => None,
        }
    }
}

pub enum Outcome<T> {
    Open,
    Chosen(T),
    Closed,
}

/// Show the menu for one frame. A press outside (other than on the anchor widget, which toggles the menu itself) or Esc closes it.
pub fn show<T: Copy>(ctx: &Context, id: Id, anchor: Anchor, items: &[Item<T>]) -> Outcome<T> {
    let p = snap_ui::palette(ctx);
    let width = menu_width(ctx, id, items);
    let height = items.iter().map(|i| ROW_H + if i.separator { SEPARATOR_H } else { 0.0 }).sum::<f32>() + PAD * 2.0 + 2.0;
    let (pos, max_h) = place(ctx, anchor, Vec2::new(width, height));

    let mut chosen = None;
    let frame = Frame::new()
        .fill(p.elevated)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(PAD as i8))
        .shadow(p.shadow());
    let area = Area::new(id).order(Order::Foreground).fixed_pos(pos).sense(Sense::hover()).show(ctx, |ui| {
        frame.show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            ui.set_width(width - PAD * 2.0 - 2.0);
            popup::scroll_body(ui, max_h - PAD * 2.0 - 2.0, |ui| {
                for item in items {
                    if item.separator {
                        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), SEPARATOR_H), Sense::hover());
                        ui.painter().hline(rect.x_range().shrink(6.0), rect.center().y, Stroke::new(1.0, p.border));
                    }
                    if row(ui, &p, item) {
                        chosen = Some(item.value);
                    }
                }
            });
        });
    });

    if let Some(value) = chosen {
        return Outcome::Chosen(value);
    }
    let esc = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape));
    let pressed_outside = ctx.input(|i| {
        i.pointer.any_pressed()
            && i.pointer
                .interact_pos()
                .is_some_and(|pos| !area.response.rect.contains(pos) && !anchor.widget().is_some_and(|r| r.contains(pos)))
    });
    if esc || pressed_outside { Outcome::Closed } else { Outcome::Open }
}

/// Where the menu's top-left corner goes and the most height it may take: under (or above) a widget, never over it,
/// or at the pointer; always inside the window, scrolling when even the larger side is too short.
fn place(ctx: &Context, anchor: Anchor, size: Vec2) -> (Pos2, f32) {
    let gap = 4.0;
    let (x, y, max_h) = match anchor {
        Anchor::Widget(rect) | Anchor::Start(rect) => {
            let (align, max_h) = popup::fit(ctx, rect, gap, size.y);
            let h = size.y.min(max_h);
            let y = if align == egui::RectAlign::BOTTOM_START { rect.bottom() + gap } else { rect.top() - gap - h };
            (if matches!(anchor, Anchor::Start(_)) { rect.left() } else { rect.right() - size.x }, y, max_h)
        }
        Anchor::Point(pos) => {
            let bounds = popup::popup_bounds(ctx, Rect::from_min_size(pos, Vec2::ZERO));
            let h = size.y.min(bounds.height());
            // Below the pointer when it fits, else above it, else as low as the window allows.
            let y = if pos.y + h <= bounds.bottom() {
                pos.y
            } else if pos.y - h >= bounds.top() {
                pos.y - h
            } else {
                bounds.bottom() - h
            };
            (pos.x, y, bounds.height())
        }
    };
    let bounds = ctx.content_rect().shrink(8.0);
    let x = x.clamp(bounds.left(), (bounds.right() - size.x).max(bounds.left()));
    (Pos2::new(x, y), max_h)
}

fn menu_width<T>(ctx: &Context, id: Id, items: &[Item<T>]) -> f32 {
    let painter = ctx.layer_painter(LayerId::new(Order::Foreground, id));
    let measure = |text: &str, font: FontId| painter.layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER).size().x;
    let body = body_style().resolve(&ctx.global_style());
    let widest = items
        .iter()
        .map(|i| {
            let hotkey = i.hotkey.map_or(0.0, |h| 20.0 + measure(&accel_text(h), FontId::monospace(10.5)));
            measure(&i.label, body.clone()) + hotkey
        })
        .fold(0.0_f32, f32::max);
    (widest + 36.0 + 12.0 + PAD * 2.0 + 2.0).max(176.0)
}

/// One row; true when it was clicked.
fn row<T>(ui: &mut Ui, p: &Palette, item: &Item<T>) -> bool {
    let sense = if item.enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), sense);
    resp.widget_info(|| WidgetInfo::labeled(Role::MenuItem, item.enabled, &item.label));
    if ui.is_rect_visible(rect) {
        let hover = if item.enabled { util::hover_anim(ui, &resp) } else { 0.0 };
        ui.painter().rect_filled(rect, 6.0, p.hover().gamma_multiply(hover));
        let fg = match (item.enabled, item.danger) {
            (false, _) => p.text_faint,
            (true, true) => p.danger,
            (true, false) => p.text,
        };
        let icon_fg = if item.enabled && !item.danger { util::lerp_color(p.text_dim, p.text, hover) } else { fg };
        util::paint_icon(ui, Pos2::new(rect.left() + 18.0, rect.center().y), item.icon, 15.0, icon_fg);
        let galley = util::layout(ui, &item.label, &body_style(), fg);
        let pos = Pos2::new(rect.left() + 36.0, rect.center().y - galley.size().y / 2.0);
        ui.painter().galley(util::snap(ui, pos), galley, fg);
        if let Some(hotkey) = item.hotkey {
            let galley = ui.painter().layout_no_wrap(accel_text(hotkey), FontId::monospace(10.5), p.text_faint);
            let pos = Pos2::new(rect.right() - 10.0 - galley.size().x, rect.center().y - galley.size().y / 2.0);
            ui.painter().galley(util::snap(ui, pos), galley, p.text_faint);
        }
    }
    item.enabled && resp.clicked()
}
