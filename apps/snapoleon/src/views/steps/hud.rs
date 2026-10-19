//! The session HUD: a compact always-on-top panel (hidden from the capture) that counts the steps
//! and offers Snap now, Pause, Finish and Discard while the engine records clicks.

use std::{any::Any, time::Instant};

use egui::{Align, Color32, Context, Frame, Layout, Margin, Pos2, Rect, Sense, Stroke, StrokeKind, TextureHandle, Ui, Vec2};
use snap_core::{hotkey::Action, t};
use snap_engine::{Notice, NoticeKind};
use snap_ui::{
    Palette, icon, text,
    widgets::{Button, ButtonKind, icon_button, tooltip, util},
};

use super::{
    StepsMsg, guide,
    session::{Session, Shot},
};
use crate::{
    commands::Command,
    views::{View, ViewCx, combine::source},
};

/// How long a new step flashes in the HUD.
const FLASH_SECS: f32 = 0.7;

pub struct HudView {
    session: Option<Session>,
    shots: Vec<Shot>,
    /// Pause button state; the engine is also paused while the pointer is over the HUD (see `sync_pause`).
    paused: bool,
    engine_paused: bool,
    last_shot_at: Option<Instant>,
    thumb_tex: Option<TextureHandle>,
    started: bool,
}

impl HudView {
    pub fn new() -> Self {
        Self { session: None, shots: Vec::new(), paused: false, engine_paused: false, last_shot_at: None, thumb_tex: None, started: false }
    }

    fn start(&mut self, cx: &mut ViewCx) {
        if std::mem::replace(&mut self.started, true) {
            return;
        }
        match Session::start(cx) {
            Ok(session) => self.session = Some(session),
            Err(e) => {
                tracing::warn!("cannot start step capture: {e}");
                cx.notify(Notice {
                    kind: NoticeKind::Error,
                    title: t("steps.error.start").into_owned(),
                    body: crate::notices::error_text(&e),
                    ..Notice::default()
                });
                cx.close();
            }
        }
    }

    /// Clicks on the HUD itself must not become steps, so the recorder pauses while the pointer is over it.
    fn sync_pause(&mut self, ctx: &Context) {
        let over = ctx.input(|i| i.pointer.has_pointer());
        let want = self.paused || over;
        if want != self.engine_paused {
            self.engine_paused = want;
            if let Some(session) = &self.session {
                session.pause(want);
            }
        }
    }

    /// An explicit shot works while the recorder is paused for the pointer; it pauses again on the next frame.
    fn snap_now(&mut self) {
        if let Some(session) = &self.session {
            session.pause(false);
            session.snap_now();
            self.engine_paused = false;
        }
    }

    fn stop_session(&mut self) {
        if let Some(session) = self.session.take() {
            session.stop();
        }
    }

    fn finish(&mut self, cx: &mut ViewCx) {
        self.stop_session();
        let shots = std::mem::take(&mut self.shots);
        if shots.is_empty() {
            cx.notify(Notice { kind: NoticeKind::Info, title: t("steps.none").into_owned(), ..Notice::default() });
        } else {
            cx.push(Command::Open(Box::new(move |cx| guide::open(cx, shots))));
        }
        cx.close();
    }

    fn discard(&mut self, cx: &mut ViewCx) {
        self.stop_session();
        cx.close();
    }

    fn body(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        let p = Palette::of(ui);
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.horizontal_centered(|ui| {
            // Counter group; also the handle for moving the HUD.
            let counter = ui.horizontal_centered(|ui| {
                self.thumb(ui, &p);
                // The label comes first so the count never has to agree with a plural.
                let (label, tone) = if self.paused { ("steps.hud.paused", p.warning) } else { ("steps.hud.steps", p.text_dim) };
                ui.label(text::caption(t(label)).color(tone));
                ui.label(text::mono_text(self.shots.len().to_string()).size(18.0).strong().color(p.gold));
            });
            if ui.interact(counter.response.rect, ui.scope_id().with("drag"), Sense::drag()).drag_started() {
                cx.shell.drag_window(cx.win);
            }
            ui.add_space(4.0);

            if icon_button(ui, icon::CAMERA, &t("steps.hud.snap"), None).clicked() {
                self.snap_now();
            }
            let (glyph, tip) = if self.paused { (icon::PLAY, "steps.hud.resume") } else { (icon::PAUSE, "steps.hud.pause") };
            if icon_button(ui, glyph, &t(tip), None).clicked() {
                self.paused = !self.paused;
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icon_button(ui, icon::TRASH, &t("steps.hud.discard"), None).clicked() {
                    self.discard(cx);
                }
                let finish = ui.add(Button::new(&t("steps.hud.finish")).icon(icon::CHECK).kind(ButtonKind::Primary));
                let accel =
                    cx.settings.hotkeys.iter().find(|b| b.action == Action::StepCaptureToggle && b.enabled).map(|b| b.accel.clone());
                if tooltip(finish, "", accel.as_deref()).clicked() {
                    self.finish(cx);
                }
            });
        });
    }

    /// The texture of the latest thumbnail; made here because messages have no `Context`.
    fn ensure_thumb(&mut self, ctx: &Context) {
        if self.thumb_tex.is_none() {
            self.thumb_tex = self.shots.last().map(|s| source::upload(ctx, "steps-hud-thumb", &s.thumb));
        }
    }

    /// Thumbnail of the latest step with a gold ring that fades after each new step.
    fn thumb(&mut self, ui: &mut Ui, p: &Palette) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(56.0, 36.0), Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, 6.0, p.surface_2);
        match &self.thumb_tex {
            Some(tex) => {
                let image = source::fit_inside(rect.shrink(1.0), tex.size_vec2());
                painter.image(tex.id(), image, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            }
            None => util::paint_icon(ui, rect.center(), icon::CAMERA, 16.0, p.text_faint),
        }
        let flash = self.last_shot_at.map_or(0.0, |at| (1.0 - at.elapsed().as_secs_f32() / FLASH_SECS).max(0.0));
        let ring = Stroke::new(if flash > 0.0 { 2.0 } else { 1.0 }, util::lerp_color(p.border, p.gold, flash));
        ui.painter().rect_stroke(rect, 6.0, ring, StrokeKind::Inside);
        if flash > 0.0 {
            ui.ctx().request_repaint();
        }
    }
}

impl View for HudView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        self.start(cx);
        self.sync_pause(ctx);
        self.ensure_thumb(ctx);
        let p = snap_ui::palette(ctx);
        ctx.root_ui(|ui| {
            egui::CentralPanel::default()
                .frame(Frame::new().fill(p.elevated).inner_margin(Margin::symmetric(14, 8)))
                .show(ui, |ui| self.body(ui, cx));
        });
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("steps-hud-outline")));
        painter.rect_stroke(ctx.content_rect(), 0.0, Stroke::new(1.0, p.border), StrokeKind::Inside);
    }

    fn close_requested(&mut self, _cx: &mut ViewCx) -> bool {
        self.stop_session();
        true
    }

    fn closed(&mut self, _cx: &mut ViewCx) {
        self.stop_session();
    }

    fn on_msg(&mut self, cx: &mut ViewCx, msg: Box<dyn Any + Send>) {
        let msg = match msg.downcast::<Shot>() {
            Ok(shot) => {
                self.thumb_tex = None;
                self.shots.push(*shot);
                self.last_shot_at = Some(Instant::now());
                return;
            }
            Err(msg) => msg,
        };
        if msg.downcast::<StepsMsg>().is_ok() {
            self.finish(cx);
        }
    }
}
