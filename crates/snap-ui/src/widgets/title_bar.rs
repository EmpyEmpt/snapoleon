//! Custom window chrome: drag area, title, and minimize / maximize / close.
//!
//! The bar only reports what the user asked for in [`TitleBarActions`]; the shell performs it.
//! Controls sit on the right on Windows/Linux and as macOS-style circles on the left on macOS
//! (see [`TitleBarActions::mac_style`]). Where the OS draws the controls itself over the bar (the
//! macOS traffic lights of a window with a transparent title bar), the bar only keeps their place free
//! ([`TitleBarActions::native_controls`]).

use egui::{
    Color32, Context, InnerResponse, Pos2, Rect, Response, Role, Sense, Stroke, Ui, UiBuilder, Vec2, ViewportCommand, WidgetInfo,
    text::{LayoutJob, TextWrapping},
};
use snap_core::t;

use super::util::{self, hover_anim, lerp_color};
use crate::{
    brand,
    text::body_strong_style,
    theme::{Palette, space},
};

pub const TITLE_BAR_HEIGHT: f32 = 40.0;

/// Configuration in, requested actions out. The outputs are reset on every [`title_bar`] call.
#[derive(Debug, Clone)]
pub struct TitleBarActions {
    /// Current window state, so the maximize button can show "restore".
    pub maximized: bool,
    pub show_minimize: bool,
    pub show_maximize: bool,
    pub show_close: bool,
    /// Show the brand mark before the title.
    pub show_logo: bool,
    /// macOS-style circles on the left instead of caption buttons on the right. Defaults to the host OS.
    pub mac_style: bool,
    /// The OS draws the window controls over the left end of the bar: draw none and keep this many points free.
    /// A double-click then reports [`Self::double_clicked`] instead of [`Self::toggle_maximize`].
    pub native_controls: Option<f32>,

    /// The user pressed on the drag area: start a window drag.
    pub start_drag: bool,
    pub minimize: bool,
    /// Maximize button or double-click on the bar.
    pub toggle_maximize: bool,
    pub close: bool,
    /// Double-click on the bar while the OS draws the controls: the host does what the OS does on its own title
    /// bars (on macOS a system setting: zoom, minimize or nothing). [`Self::apply`] leaves it to the host.
    pub double_clicked: bool,
}

impl Default for TitleBarActions {
    fn default() -> Self {
        Self {
            maximized: false,
            show_minimize: true,
            show_maximize: true,
            show_close: true,
            show_logo: true,
            mac_style: cfg!(target_os = "macos"),
            native_controls: None,
            start_drag: false,
            minimize: false,
            toggle_maximize: false,
            close: false,
            double_clicked: false,
        }
    }
}

impl TitleBarActions {
    fn reset(&mut self) {
        self.start_drag = false;
        self.minimize = false;
        self.toggle_maximize = false;
        self.close = false;
        self.double_clicked = false;
    }

    /// Performs the requested actions through egui viewport commands, for hosts such as eframe.
    /// The multi-window shell performs them itself.
    pub fn apply(&self, ctx: &Context) {
        if self.start_drag {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }
        if self.minimize {
            ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
        }
        if self.toggle_maximize {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(!self.maximized));
        }
        if self.close {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }
}

pub fn title_bar(ui: &mut Ui, title: &str, actions: &mut TitleBarActions) -> Response {
    title_bar_with(ui, title, actions, |_| ()).response
}

/// [`title_bar`] with a slot between the title and the window controls (e.g. the Library search field).
pub fn title_bar_with<R>(ui: &mut Ui, title: &str, actions: &mut TitleBarActions, center: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
    actions.reset();
    let p = Palette::of(ui);
    let mac = actions.mac_style;
    let focused = ui.input(|i| i.viewport().focused.unwrap_or(true));
    let (rect, bar) = ui.allocate_exact_size(Vec2::new(ui.available_width(), TITLE_BAR_HEIGHT), Sense::click_and_drag());
    bar.widget_info(|| WidgetInfo::labeled(Role::TitleBar, true, title));
    if bar.drag_started_by(egui::PointerButton::Primary) {
        actions.start_drag = true;
    }
    if bar.double_clicked() {
        if actions.native_controls.is_some() {
            actions.double_clicked = true;
        } else if actions.show_maximize {
            actions.toggle_maximize = true;
        }
    }

    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, p.bg);
    painter.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, p.border));

    let mut left = rect.left() + space::MD;
    let mut right = rect.right();
    if let Some(inset) = actions.native_controls {
        left = left.max(rect.left() + inset);
    } else if mac {
        left += window_lights(ui, rect, actions, focused, &p) + space::SM;
    } else {
        right -= window_buttons(ui, rect, actions, focused, &p);
    }

    let text_color = if focused { p.text } else { p.text_dim };
    if actions.show_logo {
        let logo = Rect::from_center_size(Pos2::new(left + 9.0, rect.center().y), Vec2::splat(18.0));
        let logo_painter = ui.painter_at(logo);
        brand::logo_colored(&logo_painter, logo, brand::LogoColors::from_palette(&p));
        left += 18.0 + space::SM;
    }
    // A long title (a file or window name) leaves the slot room for its controls: it is cut with an ellipsis.
    let mut job = LayoutJob::simple_singleline(title.to_owned(), util::font_id(ui, &body_strong_style()), text_color);
    job.wrap =
        TextWrapping { max_width: ((right - left) * 0.4).max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let galley = ui.painter().layout_job(job);
    let pos = Pos2::new(left, rect.center().y - galley.size().y / 2.0);
    left += galley.size().x + space::LG;
    ui.painter().galley(util::snap(ui, pos), galley, text_color);

    let slot = Rect::from_min_max(Pos2::new(left, rect.top()), Pos2::new((right - space::SM).max(left), rect.bottom()));
    let mut slot_ui = ui.new_child(UiBuilder::new().max_rect(slot).layout(egui::Layout::left_to_right(egui::Align::Center)));
    InnerResponse::new(center(&mut slot_ui), bar)
}

/// Windows/Linux caption buttons, flush right. Returns the width used.
fn window_buttons(ui: &mut Ui, rect: Rect, actions: &mut TitleBarActions, focused: bool, p: &Palette) -> f32 {
    const W: f32 = 46.0;
    let kinds = [
        (actions.show_close, CaptionKind::Close),
        (actions.show_maximize, if actions.maximized { CaptionKind::Restore } else { CaptionKind::Maximize }),
        (actions.show_minimize, CaptionKind::Minimize),
    ];
    let mut x = rect.right();
    for (show, kind) in kinds {
        if !show {
            continue;
        }
        x -= W;
        let r = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(W, rect.height() - 1.0));
        let resp = ui.interact(r, ui.scope_id().with(("caption", kind as u8)), Sense::click());
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, kind.label()));
        let hover = hover_anim(ui, &resp);
        let danger = kind == CaptionKind::Close;
        let painter = ui.painter();
        let wash = if danger { p.danger } else { p.hover() };
        painter.rect_filled(r, 0.0, wash.gamma_multiply(hover));
        if resp.is_pointer_button_down_on() {
            painter.rect_filled(r, 0.0, if danger { p.danger.gamma_multiply(0.7) } else { p.pressed() });
        }
        let base = if focused { p.text } else { p.text_faint };
        let color = if danger { lerp_color(base, Color32::WHITE, hover) } else { base };
        paint_caption_glyph(ui, r.center(), kind, color);
        if resp.clicked() {
            match kind {
                CaptionKind::Close => actions.close = true,
                CaptionKind::Minimize => actions.minimize = true,
                CaptionKind::Maximize | CaptionKind::Restore => actions.toggle_maximize = true,
            }
        }
        resp.on_hover_text(kind.label());
    }
    rect.right() - x
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CaptionKind {
    Close,
    Maximize,
    Restore,
    Minimize,
}

impl CaptionKind {
    fn label(self) -> String {
        match self {
            CaptionKind::Close => t("ui.window.close"),
            CaptionKind::Maximize => t("ui.window.maximize"),
            CaptionKind::Restore => t("ui.window.restore"),
            CaptionKind::Minimize => t("ui.window.minimize"),
        }
        .into_owned()
    }
}

/// 10 px glyphs on pixel centers so the 1 px strokes stay crisp.
fn paint_caption_glyph(ui: &Ui, center: Pos2, kind: CaptionKind, color: Color32) {
    let painter = ui.painter();
    let ppp = ui.pixels_per_point();
    let snap = |v: f32| (v * ppp).floor() / ppp + 0.5 / ppp;
    let c = Pos2::new(snap(center.x), snap(center.y));
    let stroke = Stroke::new(1.0, color);
    let h = 5.0;
    match kind {
        CaptionKind::Minimize => {
            painter.hline((c.x - h)..=(c.x + h), c.y, stroke);
        }
        CaptionKind::Maximize => {
            painter.rect_stroke(Rect::from_center_size(c, Vec2::splat(h * 2.0)), 1.5, stroke, egui::StrokeKind::Middle);
        }
        CaptionKind::Restore => {
            let front = Rect::from_center_size(c + Vec2::new(-1.0, 1.0), Vec2::splat(8.0));
            let back = front.translate(Vec2::new(2.0, -2.0));
            painter.rect_stroke(front, 1.5, stroke, egui::StrokeKind::Middle);
            // Only the exposed top and right edges of the rear window.
            painter.line_segment([Pos2::new(back.left() + 2.0, back.top()), Pos2::new(back.right() - 1.5, back.top())], stroke);
            painter.line_segment([Pos2::new(back.right(), back.top() + 1.5), Pos2::new(back.right(), back.bottom() - 2.0)], stroke);
        }
        CaptionKind::Close => {
            painter.line_segment([c + Vec2::new(-h, -h), c + Vec2::new(h, h)], stroke);
            painter.line_segment([c + Vec2::new(-h, h), c + Vec2::new(h, -h)], stroke);
        }
    }
}

/// macOS-style traffic lights on the left. Returns the width used.
fn window_lights(ui: &mut Ui, rect: Rect, actions: &mut TitleBarActions, focused: bool, p: &Palette) -> f32 {
    const R: f32 = 6.0;
    const STEP: f32 = 20.0;
    let lights = [
        (actions.show_close, CaptionKind::Close, Color32::from_rgb(0xFF, 0x5F, 0x57)),
        (actions.show_minimize, CaptionKind::Minimize, Color32::from_rgb(0xFE, 0xBC, 0x2E)),
        (actions.show_maximize, CaptionKind::Maximize, Color32::from_rgb(0x28, 0xC8, 0x40)),
    ];
    let group = Rect::from_min_size(Pos2::new(rect.left() + space::MD - 4.0, rect.top()), Vec2::new(STEP * 3.0, rect.height()));
    let group_hover = ui.rect_contains_pointer(group);
    let mut x = rect.left() + space::MD + R;
    let mut used = 0.0;
    for (show, kind, color) in lights {
        if !show {
            continue;
        }
        let center = Pos2::new(x, rect.center().y);
        let hit = Rect::from_center_size(center, Vec2::splat(STEP - 2.0));
        let resp = ui.interact(hit, ui.scope_id().with(("light", kind as u8)), Sense::click());
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, kind.label()));
        let fill = if focused || group_hover { color } else { p.border_strong() };
        let painter = ui.painter();
        painter.circle_filled(center, R, fill);
        painter.circle_stroke(center, R - 0.5, Stroke::new(1.0, Color32::from_black_alpha(48)));
        if resp.is_pointer_button_down_on() {
            painter.circle_filled(center, R, Color32::from_black_alpha(48));
        }
        if group_hover {
            let ink = Stroke::new(1.2, Color32::from_black_alpha(150));
            match kind {
                CaptionKind::Close => {
                    let d = 2.2;
                    painter.line_segment([center + Vec2::new(-d, -d), center + Vec2::new(d, d)], ink);
                    painter.line_segment([center + Vec2::new(-d, d), center + Vec2::new(d, -d)], ink);
                }
                CaptionKind::Minimize => {
                    painter.hline((center.x - 2.8)..=(center.x + 2.8), center.y, ink);
                }
                _ => {
                    painter.hline((center.x - 2.6)..=(center.x + 2.6), center.y, ink);
                    painter.vline(center.x, (center.y - 2.6)..=(center.y + 2.6), ink);
                }
            }
        }
        if resp.clicked() {
            match kind {
                CaptionKind::Close => actions.close = true,
                CaptionKind::Minimize => actions.minimize = true,
                _ => actions.toggle_maximize = true,
            }
        }
        x += STEP;
        used += STEP;
    }
    used
}
