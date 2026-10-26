//! Pinned images: borderless always-on-top windows that sit exactly over the pixels they came from.
//!
//! Drag moves a pin, the wheel zooms around the pointer, Ctrl+wheel changes its opacity, a double click
//! opens the editor, Esc or a middle click closes it and a right click opens the menu (a window of its
//! own, see [`menu`]). Click-through hands the mouse to whatever is below; holding Esc brings it back.
//! Nothing about a pin is persisted.

use std::{
    any::Any,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use egui::{
    Align2, Area, Color32, ColorImage, Context, CursorIcon, Frame, Id, Key, Order, PointerButton, Sense, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Ui, Vec2, pos2,
};
use snap_core::{
    Capture, CaptureKind, Error, Point, Rect, RgbaImage,
    naming::{self, NameContext},
    paths,
    settings::{ImageFormat, PinSettings, parse_hex_color},
    t,
};
use snap_engine::{Notice, NoticeKind};
use snap_shell::WindowSpec;
use snap_ui::{
    icon, palette, text,
    widgets::{
        icon_button, toolbar_pill,
        util::{self, Motion, anim},
    },
};

use self::menu::MenuInit;
use super::{OpenCx, View, ViewCx, app_icon};
use crate::{commands::Command, views::editor::EditorInput};

mod menu;
mod native;

pub const TAG: &str = "pin";

const MIN_ZOOM: f32 = 0.1;
const MAX_ZOOM: f32 = 8.0;
const MIN_OPACITY: f32 = 0.1;
/// The window never gets smaller than this many physical pixels per side, so a pin stays grabbable.
const MIN_SIDE: u32 = 24;
/// One wheel notch is 40 points in egui; this makes it about 10% of zoom.
const ZOOM_PER_POINT: f32 = 0.0025;
/// ln(zoom_delta) of one Ctrl+wheel notch is 0.2 (egui divides by 200); one notch moves opacity 5%.
const OPACITY_PER_LN: f32 = 0.25;
const OPACITY_STEPS: [f32; 4] = [1.0, 0.75, 0.5, 0.25];
const UNLOCK_HOLD: Duration = Duration::from_millis(600);
const FLASH_SHOW: Duration = Duration::from_millis(1100);
const FLASH_FADE: Duration = Duration::from_millis(300);
/// Re-apply opacity this long after opening: showing the window rewrites its style.
const STYLE_SETTLE: Duration = Duration::from_millis(800);
/// The accent border colour the settings ship with; it means "follow the theme accent".
const DEFAULT_BORDER: [u8; 4] = [0x3D, 0x5A, 0xFE, 0xFF];

/// Pins `image`. `origin` is the physical screen rect the pixels came from: the pin appears right
/// over them. `None` centres it on the monitor under the cursor.
pub fn open(cx: &mut OpenCx, image: RgbaImage, origin: Option<Rect>) {
    let monitors = cx.shell.monitors();
    let anchor = origin.map_or_else(snap_capture::cursor_position, |r| Point::new(r.x + r.w as i32 / 2, r.y + r.h as i32 / 2));
    let (screen, scale) = monitors.iter().copied().find(|(m, _)| m.contains(anchor)).or(monitors.first().copied()).unwrap_or_default();
    let (iw, ih) = image.dimensions();
    let (zoom, rect) = match origin {
        Some(o) => (1.0, Rect::new(o.x, o.y, iw, ih)),
        None => {
            let fit = (screen.w as f32 * 0.8 / iw as f32).min(screen.h as f32 * 0.8 / ih as f32).min(1.0);
            let (w, h) = scaled(iw, ih, fit);
            (fit, Rect::new(screen.x + (screen.w as i32 - w as i32) / 2, screen.y + (screen.h as i32 - h as i32) / 2, w, h))
        }
    };
    let spec = WindowSpec {
        title: t("pin.title").into_owned(),
        size: Some((rect.w as f32 / scale, rect.h as f32 / scale)),
        position: Some(rect.origin()),
        decorations: false,
        resizable: false,
        always_on_top: true,
        skip_taskbar: true,
        // A pin sits over what the user works on: it must not take the keyboard from it.
        activate: false,
        icon: Some(app_icon()),
        tag: TAG.into(),
        ..WindowSpec::default()
    };
    let view = PinView::new(image, rect, scale, zoom, cx.settings.pins.clone());
    let win = cx.open(spec, view);
    // The logical size above is rounded; the OS gets the exact physical rect.
    cx.shell.set_outer_rect(win, rect);
}

/// Window size in physical pixels for an image shown at `zoom`.
fn scaled(width: u32, height: u32, zoom: f32) -> (u32, u32) {
    (((width as f32 * zoom).round() as u32).max(MIN_SIDE), ((height as f32 * zoom).round() as u32).max(MIN_SIDE))
}

/// Message for a pin window: leave click-through mode (the tray's "Unlock pins").
pub struct Unlock;

/// What the menu (and the keyboard) can ask a pin to do.
enum PinAction {
    Copy,
    SaveAs,
    Edit,
    ResetZoom,
    SetOpacity(f32),
    ToggleClickThrough,
    Close,
    CloseAll,
}

/// Results of work done off the UI thread.
enum PinMsg {
    Copied(Result<(), Error>),
    /// `Ok(None)`: the dialog was dismissed.
    Saved(Result<Option<PathBuf>, Error>),
}

struct Flash {
    text: String,
    since: Instant,
}

struct PinView {
    image: Arc<RgbaImage>,
    texture: Option<TextureHandle>,
    /// Where the pixels came from; the editor gets it as the capture's screen rect.
    origin: Rect,
    /// Scale factor of the monitor the pin was opened on.
    scale: f32,
    /// Window pixels per image pixel.
    zoom: f32,
    opacity: f32,
    click_through: bool,
    esc_held_since: Option<Instant>,
    flash: Option<Flash>,
    border: Color32,
    opened: Instant,
}

impl PinView {
    fn new(image: RgbaImage, origin: Rect, scale: f32, zoom: f32, settings: PinSettings) -> Self {
        let custom = parse_hex_color(&settings.border_color).filter(|c| *c != DEFAULT_BORDER);
        Self {
            image: Arc::new(image),
            texture: None,
            origin,
            scale,
            zoom,
            opacity: if native::SUPPORTED { settings.opacity.clamp(MIN_OPACITY, 1.0) } else { 1.0 },
            click_through: false,
            esc_held_since: None,
            flash: None,
            // No border override: the theme accent at 60% (DESIGN.md), resolved against the palette when painting.
            border: custom.map_or(Color32::TRANSPARENT, |[r, g, b, a]| Color32::from_rgba_unmultiplied(r, g, b, a)),
            opened: Instant::now(),
        }
    }

    fn texture(&mut self, ctx: &Context) -> &TextureHandle {
        self.texture.get_or_insert_with(|| {
            let max_side = ctx.input(|i| i.max_texture_side) as u32;
            let (w, h) = self.image.dimensions();
            let color = if w.max(h) > max_side {
                // A pin larger than the GPU's texture limit is shown from a downscaled copy.
                let k = max_side as f32 / w.max(h) as f32;
                let small = image::imageops::resize(
                    &*self.image,
                    ((w as f32 * k) as u32).max(1),
                    ((h as f32 * k) as u32).max(1),
                    image::imageops::FilterType::Triangle,
                );
                ColorImage::from_rgba_unmultiplied([small.width() as usize, small.height() as usize], small.as_raw())
            } else {
                ColorImage::from_rgba_unmultiplied([w as usize, h as usize], self.image.as_raw())
            };
            ctx.load_texture("pin-image", color, TextureOptions::LINEAR)
        })
    }

    fn flash(&mut self, text: impl Into<String>) {
        self.flash = Some(Flash { text: text.into(), since: Instant::now() });
    }

    /// Zoom to `zoom`, keeping the screen point `anchor` (physical pixels) fixed; the window centre by default.
    fn set_zoom(&mut self, cx: &mut ViewCx, zoom: f32, anchor: Option<Point>) {
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let Some(current) = cx.shell.outer_rect(cx.win) else { return };
        let (w, h) = scaled(self.image.width(), self.image.height(), zoom);
        let anchor = anchor.unwrap_or(Point::new(current.x + current.w as i32 / 2, current.y + current.h as i32 / 2));
        let fx = f64::from(anchor.x - current.x) / f64::from(current.w.max(1));
        let fy = f64::from(anchor.y - current.y) / f64::from(current.h.max(1));
        let x = f64::from(anchor.x) - fx * f64::from(w);
        let y = f64::from(anchor.y) - fy * f64::from(h);
        cx.shell.set_outer_rect(cx.win, Rect::new(x.round() as i32, y.round() as i32, w, h));
        self.zoom = zoom;
        self.flash(format!("{}%", (zoom * 100.0).round()));
    }

    fn set_opacity(&mut self, opacity: f32) {
        if !native::SUPPORTED {
            return;
        }
        self.opacity = opacity.clamp(MIN_OPACITY, 1.0);
        self.flash(format!("{}%", (self.opacity * 100.0).round()));
    }

    fn set_click_through(&mut self, cx: &mut ViewCx, on: bool) {
        if !native::SUPPORTED || self.click_through == on {
            return;
        }
        self.click_through = on;
        self.esc_held_since = None;
        cx.shell.set_passthrough(cx.win, on);
        self.flash(t(if on { "pin.flash.click_through_on" } else { "pin.flash.click_through_off" }));
    }

    /// Keeps the window's opacity applied. The OS drops it whenever the shell rewrites window flags
    /// (showing the window, toggling click-through), so while the value is not 1 it is simply re-sent.
    fn sync_opacity(&self, cx: &ViewCx, ctx: &Context) {
        if !native::SUPPORTED || self.opacity >= 1.0 {
            return;
        }
        if let Some(id) = cx.shell.native_id(cx.win) {
            native::set_opacity(id, self.opacity);
        }
        if self.opened.elapsed() < STYLE_SETTLE {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// A click-through pin gets no input, so leaving the mode is polled globally: hold Esc.
    fn poll_unlock(&mut self, cx: &mut ViewCx, ctx: &Context) {
        if !self.click_through {
            return;
        }
        if native::escape_down() {
            let since = *self.esc_held_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= UNLOCK_HOLD {
                self.set_click_through(cx, false);
                return;
            }
        } else {
            self.esc_held_since = None;
        }
        ctx.request_repaint_after(Duration::from_millis(50));
    }

    fn apply(&mut self, cx: &mut ViewCx, action: PinAction) {
        match action {
            PinAction::Copy => {
                let image = self.image.clone();
                cx.spawn(move || PinMsg::Copied(snap_platform::clipboard::set_image(&image)));
            }
            PinAction::SaveAs => self.save_as(cx),
            PinAction::Edit => {
                let mut capture = Capture::new(CaptureKind::Region, (*self.image).clone(), self.origin, self.scale);
                capture.context.window_title = Some(t("pin.title").into_owned());
                cx.push(Command::OpenEditor(EditorInput::from_capture(capture)));
            }
            PinAction::ResetZoom => self.set_zoom(cx, 1.0, None),
            PinAction::SetOpacity(opacity) => self.set_opacity(opacity),
            PinAction::ToggleClickThrough => self.set_click_through(cx, !self.click_through),
            PinAction::Close => cx.close(),
            PinAction::CloseAll => {
                for pin in cx.shell.windows_with_tag(TAG) {
                    cx.shell.close(pin);
                }
            }
        }
    }

    fn save_as(&self, cx: &mut ViewCx) {
        let image = self.image.clone();
        let capture = &cx.settings.capture;
        let (dir, template, default_format) = (paths::save_dir(&capture.save_dir), capture.filename_template.clone(), capture.format);
        let (jpeg_quality, webp_quality, optimize_png) = (capture.jpeg_quality, capture.webp_quality, capture.optimize_png);
        cx.spawn(move || {
            let name = naming::expand(
                &template,
                &NameContext { kind: Some(CaptureKind::Region), size: Some(image.dimensions()), ..NameContext::now() },
            );
            let picked = rfd::FileDialog::new()
                .set_title(t("pin.save.title"))
                .set_directory(dir)
                .set_file_name(format!("{name}.{}", default_format.ext()))
                .add_filter("PNG", &["png"])
                .add_filter("JPEG", &["jpg", "jpeg"])
                .add_filter("WebP", &["webp"])
                .save_file();
            let Some(mut path) = picked else { return PinMsg::Saved(Ok(None)) };
            let format = snap_image::encode::format_from_path(&path).unwrap_or_else(|| {
                path.set_extension(default_format.ext());
                default_format
            });
            let quality = if format == ImageFormat::Webp { webp_quality } else { jpeg_quality };
            PinMsg::Saved(snap_image::encode::save(&image, &path, format, quality, optimize_png).map(|_| Some(path)))
        });
    }

    fn open_menu(&self, cx: &mut ViewCx) {
        let init =
            MenuInit { pin: cx.handle(), at: snap_capture::cursor_position(), opacity: self.opacity, click_through: self.click_through };
        cx.push(Command::Open(Box::new(move |ocx| menu::open(ocx, init))));
    }

    /// Wheel zoom around the pointer, Ctrl+wheel opacity.
    fn handle_wheel(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let (scroll, zoom_delta, command, hover) =
            ctx.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta(), i.modifiers.command, i.pointer.hover_pos()));
        let ppp = ctx.pixels_per_point();
        let anchor = hover
            .zip(cx.shell.outer_rect(cx.win))
            .map(|(p, r)| Point::new(r.x + (p.x * ppp).round() as i32, r.y + (p.y * ppp).round() as i32));
        if command && zoom_delta != 1.0 {
            self.set_opacity(self.opacity + zoom_delta.ln() * OPACITY_PER_LN);
        } else if zoom_delta != 1.0 {
            // A trackpad pinch.
            self.set_zoom(cx, self.zoom * zoom_delta, anchor);
        } else if scroll != 0.0 {
            self.set_zoom(cx, self.zoom * (scroll * ZOOM_PER_POINT).exp(), anchor);
        }
    }

    fn handle_keys(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let pressed = |key: Key, command: bool| ctx.input(|i| i.key_pressed(key) && i.modifiers.command == command);
        let action = if pressed(Key::Escape, false) {
            Some(PinAction::Close)
        } else if ctx.input(|i| i.events.contains(&egui::Event::Copy)) {
            // egui-winit turns Ctrl+C into this event and swallows the key press itself.
            Some(PinAction::Copy)
        } else if pressed(Key::S, true) {
            Some(PinAction::SaveAs)
        } else if pressed(Key::Num0, true) {
            Some(PinAction::ResetZoom)
        } else if pressed(Key::Enter, false) {
            Some(PinAction::Edit)
        } else {
            None
        };
        if let Some(action) = action {
            self.apply(cx, action);
        }
    }

    fn border_color(&self, ctx: &Context) -> Color32 {
        if self.border == Color32::TRANSPARENT { palette(ctx).accent.gamma_multiply(0.6) } else { self.border }
    }
}

impl View for PinView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        self.sync_opacity(cx, ctx);
        self.poll_unlock(cx, ctx);
        if !self.click_through {
            self.handle_keys(cx, ctx);
            self.handle_wheel(cx, ctx);
        }

        let texture = self.texture(ctx).id();
        let border = self.border_color(ctx);
        let mut action = None;
        let mut drag = false;
        let mut surface_rect = egui::Rect::NOTHING;
        let mut hovered = false;
        ctx.root_ui(|ui| {
            egui::CentralPanel::default().frame(Frame::new()).show(ui, |ui| {
                let rect = ui.max_rect();
                surface_rect = rect;
                hovered = ui.ctx().pointer_hover_pos().is_some_and(|p| rect.contains(p));
                let surface = ui.interact(rect, Id::unique("pin-surface"), Sense::click_and_drag());
                drag = surface.drag_started_by(PointerButton::Primary);
                if surface.double_clicked() {
                    action = Some(PinAction::Edit);
                } else if surface.clicked_by(PointerButton::Middle) {
                    action = Some(PinAction::Close);
                } else if surface.secondary_clicked() {
                    self.open_menu(cx);
                }
                if hovered && !self.click_through {
                    ui.set_cursor_icon(CursorIcon::Grab);
                }
                let painter = ui.painter();
                painter.image(texture, rect, egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                painter.rect_stroke(rect, 0.0, Stroke::new(1.0, border), StrokeKind::Inside);
            });
        });
        if hovered && !self.click_through {
            action = action.or(hover_bar(ctx, surface_rect, self));
        } else {
            // Let the bar fade out.
            hover_bar_fade(ctx, surface_rect, self);
        }
        if self.click_through {
            lock_badge(ctx, surface_rect);
        }
        self.show_flash(ctx, surface_rect);

        if drag {
            cx.shell.drag_window(cx.win);
        }
        if let Some(action) = action {
            self.apply(cx, action);
        }
    }

    fn on_msg(&mut self, cx: &mut ViewCx, msg: Box<dyn Any + Send>) {
        let msg = match msg.downcast::<Unlock>() {
            Ok(_) => return self.set_click_through(cx, false),
            Err(msg) => msg,
        };
        let msg = match msg.downcast::<PinAction>() {
            Ok(action) => return self.apply(cx, *action),
            Err(msg) => msg,
        };
        let Ok(msg) = msg.downcast::<PinMsg>() else { return };
        match *msg {
            PinMsg::Copied(Ok(())) => self.flash(t("common.copied")),
            PinMsg::Saved(Ok(Some(path))) => {
                self.flash(t("pin.flash.saved"));
                cx.notify(Notice {
                    kind: NoticeKind::Success,
                    title: t("pin.flash.saved").into_owned(),
                    body: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    file: Some(path),
                    ..Notice::default()
                });
            }
            PinMsg::Saved(Ok(None)) => {}
            PinMsg::Copied(Err(e)) | PinMsg::Saved(Err(e)) => cx.notify(Notice {
                kind: NoticeKind::Error,
                title: t("common.error").into_owned(),
                body: crate::notices::error_text(&e),
                ..Notice::default()
            }),
        }
    }
}

impl PinView {
    /// A short readout (zoom, opacity, "Copied") at the bottom of the pin that fades away.
    fn show_flash(&mut self, ctx: &Context, rect: egui::Rect) {
        let Some(flash) = &self.flash else { return };
        let age = flash.since.elapsed();
        if age >= FLASH_SHOW + FLASH_FADE {
            self.flash = None;
            return;
        }
        let fade = 1.0 - (age.saturating_sub(FLASH_SHOW).as_secs_f32() / FLASH_FADE.as_secs_f32()).clamp(0.0, 1.0);
        let p = palette(ctx);
        Area::new(Id::unique("pin-flash"))
            .order(Order::Foreground)
            .interactable(false)
            .pivot(Align2::CENTER_BOTTOM)
            .fixed_pos(pos2(rect.center().x, rect.bottom() - 10.0))
            .show(ctx, |ui| {
                ui.set_opacity(fade);
                Frame::new()
                    .fill(p.elevated)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(255)
                    .inner_margin(egui::Margin::symmetric(12, 5))
                    .shadow(p.shadow_small())
                    .show(ui, |ui| ui.label(text::mono_text(flash.text.clone()).color(p.text)));
            });
        ctx.request_repaint_after(Duration::from_millis(33));
    }
}

/// The compact bar that fades in at the top while the pointer is over the pin: opacity, zoom, close.
fn hover_bar(ctx: &Context, rect: egui::Rect, pin: &PinView) -> Option<PinAction> {
    let fade = anim(ctx, Id::unique("pin-bar"), true, Motion::SLIDE);
    paint_bar(ctx, rect, pin, fade)
}

fn hover_bar_fade(ctx: &Context, rect: egui::Rect, pin: &PinView) {
    let fade = anim(ctx, Id::unique("pin-bar"), false, Motion::SLIDE);
    if fade > 0.0 {
        paint_bar(ctx, rect, pin, fade);
    }
}

fn paint_bar(ctx: &Context, rect: egui::Rect, pin: &PinView, fade: f32) -> Option<PinAction> {
    // Small pins get a reduced bar, tiny ones none: the bar must never cover more than a third of the image.
    let (full, any) = (rect.width() >= 210.0, rect.width() >= 90.0 && rect.height() >= 64.0);
    if !any {
        return None;
    }
    let mut action = None;
    Area::new(Id::unique("pin-bar-area"))
        .order(Order::Foreground)
        .pivot(Align2::CENTER_TOP)
        .fixed_pos(pos2(rect.center().x, rect.top() + 8.0))
        .show(ctx, |ui| {
            ui.set_opacity(fade);
            ui.spacing_mut().interact_size.y = 28.0;
            toolbar_pill(ui, |ui| {
                if full {
                    if native::SUPPORTED {
                        let label = format!("{}%", (pin.opacity * 100.0).round());
                        if readout(ui, icon::DROP_HALF, &label, &t("pin.tip.opacity"), Some("CmdOrCtrl+Scroll")).clicked() {
                            let next = OPACITY_STEPS.iter().copied().find(|s| *s < pin.opacity - 0.01).unwrap_or(1.0);
                            action = Some(PinAction::SetOpacity(next));
                        }
                    }
                    let label = format!("{}%", (pin.zoom * 100.0).round());
                    if readout(ui, icon::MAGNIFYING_GLASS, &label, &t("pin.tip.zoom"), Some("CmdOrCtrl+0")).clicked() {
                        action = Some(PinAction::ResetZoom);
                    }
                }
                if icon_button(ui, icon::X, &t("pin.tip.close"), Some("Esc")).clicked() {
                    action = Some(PinAction::Close);
                }
            });
        });
    action
}

/// Icon plus a mono value, as one ghost button.
fn readout(ui: &mut Ui, icon: &str, value: &str, tip: &str, accel: Option<&str>) -> egui::Response {
    let p = palette(ui.ctx());
    let galley = util::layout(ui, value, &text::mono(), p.text);
    let size = Vec2::new(10.0 + 14.0 + 6.0 + galley.size().x.max(30.0) + 10.0, util::control_height(ui));
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let hover = util::hover_anim(ui, &resp);
        let fg = util::lerp_color(p.text_dim, p.text, hover);
        ui.painter().rect_filled(rect, 6.0, p.hover().gamma_multiply(hover));
        util::paint_icon(ui, pos2(rect.left() + 17.0, rect.center().y), icon, 14.0, fg);
        let galley = util::layout(ui, value, &text::mono(), fg);
        ui.painter().galley(util::snap(ui, pos2(rect.left() + 10.0 + 14.0 + 6.0, rect.center().y - galley.size().y / 2.0)), galley, fg);
    }
    snap_ui::widgets::tooltip(resp, tip, accel)
}

/// Small lock in the top-right corner while click-through is on.
fn lock_badge(ctx: &Context, rect: egui::Rect) {
    if rect.width() < 40.0 || rect.height() < 40.0 {
        return;
    }
    let p = palette(ctx);
    Area::new(Id::unique("pin-lock"))
        .order(Order::Foreground)
        .interactable(false)
        .pivot(Align2::RIGHT_TOP)
        .fixed_pos(pos2(rect.right() - 8.0, rect.top() + 8.0))
        .show(ctx, |ui| {
            let (badge, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
            ui.painter().circle_filled(badge.center(), 12.0, p.elevated);
            ui.painter().circle_stroke(badge.center(), 12.0, Stroke::new(1.0, p.border));
            util::paint_icon(ui, badge.center(), icon::LOCK_SIMPLE, 13.0, p.accent);
        });
}
