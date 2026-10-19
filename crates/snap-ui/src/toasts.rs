//! Bottom-right toast stack: auto-dismiss after 4 s, optional action button, slide and fade.

use std::time::Duration;

use egui::{
    Align2, Area, Context, Frame, Id, Margin, Order, Pos2, Rect, Sense, Stroke, Vec2, emath::easing,
};
use snap_core::t;

use crate::{
    icon, text,
    theme::{Palette, Tone, radius},
    widgets::util::{self, hover_anim},
};

const WIDTH: f32 = 340.0;
const MARGIN: f32 = 16.0;
const GAP: f32 = 8.0;
const ENTER: f64 = 0.2;
const EXIT: f64 = 0.16;
const LIFETIME: Duration = Duration::from_secs(4);
const MAX_VISIBLE: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToastKind {
    #[default]
    Info,
    Success,
    Warning,
    Error,
}

impl ToastKind {
    fn tone(self) -> Tone {
        match self {
            ToastKind::Info => Tone::Accent,
            ToastKind::Success => Tone::Success,
            ToastKind::Warning => Tone::Warning,
            ToastKind::Error => Tone::Danger,
        }
    }

    fn icon(self) -> &'static str {
        match self {
            ToastKind::Info => icon::INFO,
            ToastKind::Success => icon::CHECK_CIRCLE,
            ToastKind::Warning => icon::WARNING,
            ToastKind::Error => icon::X_CIRCLE,
        }
    }
}

/// A toast to show. `action` is a button label plus an id reported back by [`Toasts::show`] when clicked.
#[derive(Debug, Clone)]
pub struct Toast {
    pub kind: ToastKind,
    pub text: String,
    pub action: Option<(String, String)>,
    pub lifetime: Duration,
}

impl Toast {
    pub fn new(kind: ToastKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
            action: None,
            lifetime: LIFETIME,
        }
    }

    /// Adds an action button (e.g. "Open"); `id` comes back from [`Toasts::show`] when it is clicked.
    pub fn action(mut self, label: impl Into<String>, id: impl Into<String>) -> Self {
        self.action = Some((label.into(), id.into()));
        self
    }

    pub fn lifetime(mut self, lifetime: Duration) -> Self {
        self.lifetime = lifetime;
        self
    }
}

struct Item {
    id: u64,
    toast: Toast,
    /// Seconds this toast has been visible, paused while hovered.
    age: f64,
    /// Seconds since it entered.
    since_enter: f64,
    /// Seconds since dismissal began.
    since_exit: Option<f64>,
    height: f32,
}

/// Owner of the toast stack. Keep one per window; call [`Toasts::show`] every frame.
#[derive(Default)]
pub struct Toasts {
    items: Vec<Item>,
    next_id: u64,
}

impl Toasts {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, kind: ToastKind, text: impl Into<String>) {
        self.push_toast(Toast::new(kind, text));
    }

    pub fn push_toast(&mut self, toast: Toast) {
        self.next_id += 1;
        self.items.push(Item {
            id: self.next_id,
            toast,
            age: 0.0,
            since_enter: 0.0,
            since_exit: None,
            height: 44.0,
        });
        // Never stack more than a handful: retire the oldest.
        let live = self.items.iter().filter(|i| i.since_exit.is_none()).count();
        for item in self
            .items
            .iter_mut()
            .filter(|i| i.since_exit.is_none())
            .take(live.saturating_sub(MAX_VISIBLE))
        {
            item.since_exit = Some(0.0);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Draws the stack and advances timers. Returns the ids of actions clicked this frame.
    pub fn show(&mut self, ctx: &Context) -> Vec<String> {
        if self.items.is_empty() {
            return Vec::new();
        }
        let p = crate::palette(ctx);
        let dt = ctx.input(|i| i.unstable_dt).min(0.1) as f64;
        let instant = ctx.global_style().animation_time <= 0.0;
        let screen = ctx.content_rect();
        let mut activated = Vec::new();

        let mut offset = 0.0;
        for item in self.items.iter_mut().rev() {
            item.since_enter += dt;
            if let Some(t) = &mut item.since_exit {
                *t += dt;
            }
            let enter = if instant {
                1.0
            } else {
                easing::cubic_out(((item.since_enter) / ENTER).clamp(0.0, 1.0) as f32)
            };
            let exit = match item.since_exit {
                Some(_) if instant => 1.0,
                Some(t) => (t / EXIT).clamp(0.0, 1.0) as f32,
                None => 0.0,
            };
            let visible = enter * (1.0 - exit);
            let slide = (1.0 - enter) * 28.0 + exit * 28.0;

            let id = Id::unique(("snap-toast", item.id));
            let target = util::anim_value(ctx, id.with("offset"), offset, 0.16);
            let area = Area::new(id)
                .order(Order::Tooltip)
                .anchor(
                    Align2::RIGHT_BOTTOM,
                    Vec2::new(-MARGIN + slide, -MARGIN - target),
                )
                .interactable(true)
                .sense(Sense::hover());
            let out = area.show(ctx, |ui| {
                ui.set_opacity(visible);
                let width = WIDTH.min(screen.width() - MARGIN * 2.0);
                toast_ui(ui, &p, &item.toast, width)
            });
            let clicked = out.inner;
            item.height = out.response.rect.height();
            offset += (item.height + GAP) * (1.0 - exit);

            let hovered = ctx.rect_contains_pointer(out.response.layer_id, out.response.rect);
            if item.since_exit.is_none() {
                if !hovered {
                    item.age += dt;
                }
                if item.age >= item.toast.lifetime.as_secs_f64() {
                    item.since_exit = Some(0.0);
                }
                match clicked {
                    Clicked::Action => {
                        if let Some((_, id)) = &item.toast.action {
                            activated.push(id.clone());
                        }
                        item.since_exit = Some(0.0);
                    }
                    Clicked::Close => item.since_exit = Some(0.0),
                    Clicked::None => {}
                }
            }
        }

        self.items
            .retain(|i| i.since_exit.is_none_or(|t| !instant && t < EXIT));
        if !self.items.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        activated
    }
}

enum Clicked {
    None,
    Action,
    Close,
}

fn toast_ui(ui: &mut egui::Ui, p: &Palette, toast: &Toast, width: f32) -> Clicked {
    let tone = toast.kind.tone();
    let frame = Frame::new()
        .fill(p.elevated)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(radius::CARD + 2.0)
        .inner_margin(Margin::symmetric(14, 12))
        .shadow(p.shadow());
    let mut clicked = Clicked::None;
    frame.show(ui, |ui| {
        let inner_w = width - 28.0;
        ui.set_width(inner_w);
        let icon_w = 18.0 + 10.0;
        let action = toast.action.as_ref().map(|(label, _)| {
            let g = util::layout(ui, label, &text::body_strong_style(), p.accent);
            (g.size().x + 20.0, g)
        });
        let action_w = action.as_ref().map_or(0.0, |(w, _)| w + 8.0);
        let close_w = 24.0;
        let text_w = (inner_w - icon_w - action_w - close_w - 6.0).max(80.0);
        let galley = util::layout_wrapped(ui, &toast.text, &text::body_style(), p.text, text_w);
        let height = galley.size().y.max(24.0);
        let (rect, hover) = ui.allocate_exact_size(Vec2::new(inner_w, height), Sense::hover());
        let hovered = hover.hovered() || ui.rect_contains_pointer(rect.expand(8.0));

        let cy = rect.center().y;
        ui.painter()
            .circle_filled(Pos2::new(rect.left() + 9.0, cy), 12.0, p.tone_soft(tone));
        util::paint_icon(
            ui,
            Pos2::new(rect.left() + 9.0, cy),
            toast.kind.icon(),
            16.0,
            p.tone(tone),
        );
        let text_pos = Pos2::new(rect.left() + icon_w, cy - galley.size().y / 2.0);
        ui.painter()
            .galley(util::snap(ui, text_pos), galley, p.text);

        let mut x = rect.right();
        let close_rect =
            Rect::from_center_size(Pos2::new(x - close_w / 2.0, cy), Vec2::splat(24.0));
        x -= close_w;
        let close = ui.interact(close_rect, ui.scope_id().with("close"), Sense::click());
        let close_t = util::anim(ui.ctx(), close.id.with("vis"), hovered, 0.1);
        if close_t > 0.0 {
            let h = hover_anim(ui, &close);
            ui.painter().circle_filled(
                close_rect.center(),
                11.0,
                p.hover().gamma_multiply(h * close_t),
            );
            util::paint_icon(
                ui,
                close_rect.center(),
                icon::X,
                12.0,
                util::lerp_color(p.text_faint, p.text, h).gamma_multiply(close_t),
            );
        }
        if close.clicked() {
            clicked = Clicked::Close;
        }
        close.on_hover_text(t("ui.toast.dismiss"));

        if let Some((w, galley)) = action {
            let r = Rect::from_center_size(Pos2::new(x - w / 2.0 - 4.0, cy), Vec2::new(w, 28.0));
            let resp = ui.interact(r, ui.scope_id().with("action"), Sense::click());
            let h = hover_anim(ui, &resp);
            ui.painter()
                .rect_filled(r, 6.0, p.accent_soft.gamma_multiply(0.5 + 0.5 * h));
            let color = if resp.is_pointer_button_down_on() {
                p.accent.gamma_multiply(0.8)
            } else {
                p.accent
            };
            let pos = Pos2::new(
                r.center().x - galley.size().x / 2.0,
                r.center().y - galley.size().y / 2.0,
            );
            ui.painter().galley(util::snap(ui, pos), galley, color);
            if resp.clicked() {
                clicked = Clicked::Action;
            }
        }
    });
    clicked
}
