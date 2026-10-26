//! The scrolling capture panel: a small always-on-top window next to the capture area (hidden from
//! the capture itself) with a live preview of the stitched page, its height and frame count, and
//! the Stop and Discard buttons. The capture itself runs `Engine::scrolling` on a worker thread;
//! the stitched result goes through the pipeline.

mod demo;

use std::{
    any::Any,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use egui::{Align, Context, Frame, Key, Layout, Margin, Pos2, Rect as UiRect, Sense, Stroke, TextureHandle, Ui, Vec2};
use snap_core::{Capture, Point, Rect, Result, t, tf};
use snap_engine::{Notice, NoticeKind, ScrollProgress};
use snap_shell::WindowSpec;
use snap_ui::{
    Palette, icon, space, text,
    widgets::{Button, ButtonKind, icon_button, spinner, tooltip, util},
};

use super::{OpenCx, View, ViewCx, app_icon, combine::source};
use crate::commands::Command;

pub const TAG: &str = "scroll";

/// Logical size of the panel.
const SIZE: (f32, f32) = (288.0, 520.0);
/// Gap between the capture area and the panel, in physical pixels.
const GAP: i32 = 16;

pub struct ScrollInput {
    /// The area to scroll, in virtual-desktop physical pixels.
    pub rect: Rect,
    /// Pipeline for the stitched capture; `None` = the default pipeline.
    pub pipeline: Option<String>,
}

/// Starts a scrolling capture. Ignored while one is already running.
pub fn open(cx: &mut OpenCx, input: ScrollInput) {
    if cx.focus_tagged(TAG) {
        return;
    }
    let (position, _) = placement(&cx.shell.monitors(), input.rect);
    let spec = WindowSpec {
        title: t("scroll.title").into_owned(),
        size: Some(SIZE),
        position: Some(position),
        decorations: false,
        resizable: false,
        rounded: true,
        always_on_top: true,
        tool_window: true,
        // The content under the area is what the user is looking at: keep its focus.
        activate: false,
        exclude_from_capture: true,
        icon: Some(app_icon()),
        tag: TAG.into(),
        ..WindowSpec::default()
    };
    cx.open(spec, ScrollView::new(input));
}

/// Where the panel goes: right of the area if it fits on the area's monitor, else left of it, else
/// against the monitor's right edge. Returns the top-left in physical pixels and the scale used.
fn placement(monitors: &[(Rect, f32)], area: Rect) -> (Point, f32) {
    let center = Point::new(area.x + area.w as i32 / 2, area.y + area.h as i32 / 2);
    let &(monitor, scale) = monitors.iter().find(|(m, _)| m.contains(center)).or_else(|| monitors.first()).unwrap_or(&(area, 1.0));
    let (w, h) = ((SIZE.0 * scale) as i32, (SIZE.1 * scale) as i32);
    let mut x = area.right() + GAP;
    if x + w > monitor.right() {
        x = area.x - GAP - w;
    }
    if x < monitor.x {
        x = monitor.right() - w - GAP;
    }
    let y = area.y.min(monitor.bottom() - h - GAP).max(monitor.y);
    (Point::new(x, y), scale)
}

/// Worker to view messages.
enum Msg {
    Progress(ScrollProgress),
    Done(Result<Capture>),
}

struct ScrollView {
    input: ScrollInput,
    cancel: Arc<AtomicBool>,
    started: bool,
    /// Stop was pressed: the engine is finishing the stitch.
    finishing: bool,
    /// The user gave up: whatever the engine returns is dropped.
    discarded: bool,
    latest: Option<ScrollProgress>,
    /// The preview of `latest` has not been uploaded yet.
    stale: bool,
    tex: Option<TextureHandle>,
}

impl ScrollView {
    fn new(input: ScrollInput) -> Self {
        Self {
            input,
            cancel: Arc::new(AtomicBool::new(false)),
            started: false,
            finishing: false,
            discarded: false,
            latest: None,
            stale: false,
            tex: None,
        }
    }

    /// Start the capture on the first frame, when the panel exists and can report back.
    fn start(&mut self, cx: &ViewCx) {
        if std::mem::replace(&mut self.started, true) {
            return;
        }
        let (engine, handle, cancel, rect) = (cx.engine.clone(), cx.handle(), self.cancel.clone(), self.input.rect);
        cx.spawn(move || {
            let report = |progress: &ScrollProgress| handle.send(Msg::Progress(progress.clone()));
            Msg::Done(if demo::enabled() { demo::run(rect, &cancel, &report) } else { engine.scrolling(rect, &cancel, &report) })
        });
    }

    fn stop(&mut self) {
        self.finishing = true;
        self.cancel.store(true, Ordering::Relaxed);
    }

    fn discard(&mut self, cx: &mut ViewCx) {
        self.discarded = true;
        self.cancel.store(true, Ordering::Relaxed);
        cx.close();
    }

    fn hud(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        ui.spacing_mut().item_spacing = Vec2::splat(space::SM);
        self.header(ui, cx);
        // The preview takes what the footer leaves.
        let footer = 28.0 + 32.0 + space::SM * 3.0;
        let preview = UiRect::from_min_size(ui.cursor().min, Vec2::new(ui.available_width(), (ui.available_height() - footer).max(80.0)));
        ui.allocate_rect(preview, Sense::hover());
        self.preview(ui, preview);
        self.footer(ui, cx);
    }

    fn header(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        let p = Palette::of(ui);
        let row = ui.horizontal(|ui| {
            let (glyph, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
            util::paint_icon(ui, glyph.center(), icon::ARROWS_VERTICAL, 18.0, p.accent);
            ui.label(text::body_strong(t("scroll.title")));
        });
        // The panel has no title bar: the header moves it.
        if ui.interact(row.response.rect, ui.scope_id().with("drag"), Sense::drag()).drag_started() {
            cx.shell.drag_window(cx.win);
        }
        let area = &self.input.rect;
        ui.label(text::caption(format!("{} × {} px", area.w, area.h)));
        ui.label(text::caption(t("scroll.hint")));
    }

    fn preview(&mut self, ui: &mut Ui, rect: UiRect) {
        let p = Palette::of(ui);
        let painter = ui.painter();
        painter.rect_filled(rect, 10.0, p.surface_2);
        painter.rect_stroke(rect, 10.0, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
        if self.stale {
            self.tex = self.latest.as_ref().map(|l| source::upload(ui.ctx(), "scroll-preview", &l.preview));
            self.stale = false;
        }
        match &self.tex {
            Some(tex) => {
                let image = source::fit_inside(rect.shrink(8.0), tex.size_vec2());
                let uv = UiRect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                ui.painter().image(tex.id(), image, uv, egui::Color32::WHITE);
                ui.painter().rect_stroke(image, 2.0, Stroke::new(1.0, p.border), egui::StrokeKind::Outside);
            }
            None => {
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(12.0)).layout(Layout::top_down(Align::Center)));
                child.add_space((rect.height() / 2.0 - 36.0).max(0.0));
                spinner(&mut child);
                child.label(text::caption(t("scroll.waiting")));
            }
        }
    }

    fn footer(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        let (height, frames) = self.latest.as_ref().map_or((0, 0), |l| (l.height, l.frames));
        ui.horizontal(|ui| {
            ui.set_min_height(28.0);
            ui.label(text::caption(t("scroll.height")));
            ui.label(text::mono_text(tf("scroll.px", &[("n", &source::group_digits(height.into(), &t("scroll.thousands")))])));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(text::mono_text(frames.to_string()));
                ui.label(text::caption(t("scroll.frames")));
            });
        });
        ui.horizontal(|ui| {
            let stop_width = ui.available_width() - 32.0 - space::SM;
            if self.finishing {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(stop_width, 32.0), Sense::hover());
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(Layout::left_to_right(Align::Center)));
                spinner(&mut child);
                child.label(text::caption(t("scroll.finishing")));
            } else {
                let stop = ui.add(Button::new(&t("scroll.stop")).icon(icon::STOP).kind(ButtonKind::Primary).min_width(stop_width));
                if tooltip(stop, &t("scroll.stop_tip"), Some("Enter")).clicked() {
                    self.stop();
                }
            }
            if icon_button(ui, icon::TRASH, &t("scroll.discard"), Some("Esc")).clicked() {
                self.discard(cx);
            }
        });
    }
}

impl View for ScrollView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        self.start(cx);
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.discard(cx);
            return;
        }
        if !self.finishing && ctx.input(|i| i.key_pressed(Key::Enter)) {
            self.stop();
        }
        let p = snap_ui::palette(ctx);
        ctx.root_ui(|ui| {
            egui::CentralPanel::default()
                .frame(Frame::new().fill(p.elevated).inner_margin(Margin::same(14)))
                .show(ui, |ui| self.hud(ui, cx));
        });
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("scroll-outline")));
        painter.rect_stroke(ctx.content_rect(), 0.0, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
    }

    fn close_requested(&mut self, _cx: &mut ViewCx) -> bool {
        self.discarded = true;
        true
    }

    fn closed(&mut self, _cx: &mut ViewCx) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    fn on_msg(&mut self, cx: &mut ViewCx, msg: Box<dyn Any + Send>) {
        let Ok(msg) = msg.downcast::<Msg>() else { return };
        match *msg {
            Msg::Progress(progress) => {
                self.latest = Some(progress);
                self.stale = true;
            }
            Msg::Done(result) => {
                if !self.discarded {
                    match result {
                        Ok(capture) => {
                            let pipeline = self.input.pipeline.take().unwrap_or_else(|| cx.settings.default_pipeline.clone());
                            cx.push(Command::RunPipeline { capture, pipeline });
                        }
                        Err(snap_core::Error::Cancelled) => {}
                        Err(e) => {
                            tracing::warn!("scrolling capture failed: {e}");
                            // Scrolling needs Accessibility, its frames Screen Recording. The first time, the system
                            // prompt (which also lists the app in System Settings); every time, a notice that leads there.
                            let permission = crate::notices::missing_permission(&e);
                            if let Some(perm) = permission {
                                snap_platform::permissions::request(perm);
                            }
                            cx.notify(Notice {
                                kind: NoticeKind::Error,
                                title: t("scroll.error").into_owned(),
                                body: crate::notices::error_text(&e),
                                permission,
                                ..Notice::default()
                            });
                        }
                    }
                }
                cx.close();
            }
        }
    }
}
