//! The pin's right-click menu. It is a window of its own rather than an egui popup: a pin can be a few
//! dozen pixels wide, and a popup inside it would be clipped.

use egui::{Context, CursorIcon, Frame, Id, Key, LayerId, Margin, Order, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2};
use snap_core::{Point, t};
use snap_shell::WindowSpec;
use snap_ui::{
    icon, palette, text,
    widgets::{accel_text, segmented, util},
};

use super::{PinAction, native};
use crate::views::{OpenCx, View, ViewCx, ViewHandle, app_icon};

const TAG: &str = "pin-menu";
const WIDTH: f32 = 248.0;
const MARGIN: f32 = 6.0;
const ROW_H: f32 = 30.0;
const SEPARATOR_H: f32 = 9.0;
const OPACITY_H: f32 = 64.0;
const OPACITY_PRESETS: [(u32, &str); 4] = [(100, "100%"), (75, "75%"), (50, "50%"), (25, "25%")];

/// What the menu needs to know about the pin it belongs to.
pub struct MenuInit {
    pub pin: ViewHandle,
    /// Where the pointer was, in physical screen pixels.
    pub at: Point,
    pub opacity: f32,
    pub click_through: bool,
}

pub fn open(cx: &mut OpenCx, init: MenuInit) {
    for old in cx.shell.windows_with_tag(TAG) {
        cx.shell.close(old);
    }
    let height = menu_height();
    let monitors = cx.shell.monitors();
    let (screen, scale) = monitors.iter().copied().find(|(m, _)| m.contains(init.at)).or(monitors.first().copied()).unwrap_or_default();
    let (w, h) = ((WIDTH * scale).round() as i32, (height * scale).round() as i32);
    // Open down and to the right of the pointer, flipping where that would leave the monitor.
    let x = if init.at.x + w > screen.right() { init.at.x - w } else { init.at.x };
    let y = if init.at.y + h > screen.bottom() { init.at.y - h } else { init.at.y };
    let spec = WindowSpec {
        title: t("pin.title").into_owned(),
        size: Some((WIDTH, height)),
        position: Some(Point::new(x.max(screen.x), y.max(screen.y))),
        decorations: false,
        resizable: false,
        rounded: true,
        always_on_top: true,
        tool_window: true,
        icon: Some(app_icon()),
        tag: TAG.into(),
        ..WindowSpec::default()
    };
    cx.open(spec, MenuView { init, was_focused: false });
}

/// Rows and separators add up to the window height; the shell has no auto-size, so it is computed.
fn menu_height() -> f32 {
    let mut h = MARGIN * 2.0 + ROW_H * 4.0 + SEPARATOR_H * 2.0 + ROW_H * 2.0;
    if native::SUPPORTED {
        h += OPACITY_H + ROW_H;
    }
    h
}

struct MenuView {
    init: MenuInit,
    /// Closing on focus loss must wait until the window has been focused once.
    was_focused: bool,
}

impl View for MenuView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let focused = ctx.input(|i| i.viewport().focused).unwrap_or(false);
        if !self.was_focused {
            cx.shell.focus(cx.win);
        }
        self.was_focused |= focused;
        if (self.was_focused && !focused) || ctx.input(|i| i.key_pressed(Key::Escape)) {
            cx.close();
            return;
        }

        let p = palette(ctx);
        let mut picked: Option<PinAction> = None;
        ctx.root_ui(|ui| {
            egui::CentralPanel::default().frame(Frame::new().fill(p.elevated).inner_margin(Margin::same(MARGIN as i8))).show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                let items = [
                    (icon::COPY, t("pin.menu.copy"), Some("CmdOrCtrl+C"), PinAction::Copy),
                    (icon::FLOPPY_DISK, t("pin.menu.save"), Some("CmdOrCtrl+S"), PinAction::SaveAs),
                    (icon::PENCIL_SIMPLE, t("pin.menu.edit"), Some("Enter"), PinAction::Edit),
                ];
                for (glyph, label, accel, action) in items {
                    if row(ui, glyph, &label, accel, false).clicked() {
                        picked = Some(action);
                    }
                }
                separator(ui);
                if row(ui, icon::MAGNIFYING_GLASS, &t("pin.menu.zoom_reset"), Some("CmdOrCtrl+0"), false).clicked() {
                    picked = Some(PinAction::ResetZoom);
                }
                if native::SUPPORTED {
                    let percent = (self.init.opacity * 100.0).round() as u32;
                    let mut preset = OPACITY_PRESETS.iter().map(|&(v, _)| v).min_by_key(|v| v.abs_diff(percent)).unwrap_or(100);
                    let (block, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), OPACITY_H), Sense::hover());
                    let caption = util::layout(ui, &t("pin.menu.opacity"), &text::caption_style(), p.text_dim);
                    ui.painter().galley(util::snap(ui, block.min + Vec2::new(10.0, 8.0)), caption, p.text_dim);
                    let control = Rect::from_min_size(block.min + Vec2::new(4.0, 30.0), Vec2::new(block.width() - 8.0, 32.0));
                    ui.scope_builder(UiBuilder::new().max_rect(control), |ui| {
                        if segmented(ui, &mut preset, &OPACITY_PRESETS).changed() {
                            picked = Some(PinAction::SetOpacity(preset as f32 / 100.0));
                        }
                    });
                    if row(ui, icon::LOCK_SIMPLE, &t("pin.menu.click_through"), None, self.init.click_through).clicked() {
                        picked = Some(PinAction::ToggleClickThrough);
                    }
                }
                separator(ui);
                if row(ui, icon::X, &t("pin.menu.close"), Some("Esc"), false).clicked() {
                    picked = Some(PinAction::Close);
                }
                if row(ui, icon::STACK, &t("pin.menu.close_all"), None, false).clicked() {
                    picked = Some(PinAction::CloseAll);
                }
            });
        });
        let outline = ctx.layer_painter(LayerId::new(Order::Foreground, Id::unique("pin-menu-outline")));
        outline.rect_stroke(ctx.content_rect(), 0.0, Stroke::new(1.0, p.border), StrokeKind::Inside);

        if let Some(action) = picked {
            self.init.pin.send(action);
            cx.close();
        }
    }
}

/// One menu row: icon, label, optional accelerator at the right, a check mark when `on`.
fn row(ui: &mut Ui, icon: &str, label: &str, accel: Option<&str>, on: bool) -> egui::Response {
    let p = palette(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
    if ui.is_rect_visible(rect) {
        let hover = util::hover_anim(ui, &resp);
        ui.painter().rect_filled(rect, 6.0, p.hover().gamma_multiply(hover));
        let icon_color = util::lerp_color(p.text_dim, p.text, hover);
        util::paint_icon(ui, Pos2::new(rect.left() + 18.0, rect.center().y), icon, 15.0, icon_color);
        let galley = util::layout(ui, label, &text::body_style(), p.text);
        ui.painter().galley(util::snap(ui, Pos2::new(rect.left() + 38.0, rect.center().y - galley.size().y / 2.0)), galley, p.text);
        if on {
            util::paint_icon(ui, Pos2::new(rect.right() - 16.0, rect.center().y), icon::CHECK, 15.0, p.accent);
        }
        if let Some(accel) = accel {
            let shown = accel_text(accel);
            let galley = ui.painter().layout_no_wrap(shown, text::mono().resolve(ui.style()), p.text_faint);
            let pos = Pos2::new(rect.right() - 12.0 - galley.size().x, rect.center().y - galley.size().y / 2.0);
            ui.painter().galley(util::snap(ui, pos), galley, p.text_faint);
        }
    }
    resp.on_hover_cursor(CursorIcon::Default)
}

fn separator(ui: &mut Ui) {
    let p = palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), SEPARATOR_H), Sense::hover());
    ui.painter().hline(rect.x_range().shrink(6.0), rect.center().y.floor() + 0.5, Stroke::new(1.0, p.border));
}
