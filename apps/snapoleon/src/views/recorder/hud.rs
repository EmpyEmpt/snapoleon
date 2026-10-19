//! The control pill: red dot and timer, pause, audio state and level, stop, discard. The same window
//! turns into a small card when recording cannot start.

use std::f32::consts::TAU;

use egui::{Align, Color32, Context, Frame, Id, LayerId, Layout, Margin, Order, PointerButton, Sense, Stroke, StrokeKind, Ui, Vec2, pos2};
use snap_core::{t, tf};
use snap_ui::{
    icon, palette, text,
    widgets::{Button, ButtonKind, ghost_button, icon_button, primary_button, secondary_button, spinner, toolbar_separator, tooltip, util},
};

use super::{Failure, Phase, RecorderView, ViewCx, apply_fix};

/// Segments of the level meter and their size in logical pixels.
const METER_SEGMENTS: usize = 8;
const METER_SEGMENT: Vec2 = Vec2::new(3.0, 12.0);
const METER_GAP: f32 = 2.0;
/// Seconds per beat of the recording dot.
const PULSE_PERIOD: f64 = 1.4;

enum Act {
    TogglePause,
    Stop,
    AskDiscard,
    KeepRecording,
    Discard,
    CancelCountdown,
    Fix,
    CloseCard,
}

impl RecorderView {
    pub(super) fn draw(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let p = palette(ctx);
        let mut act = None;
        let mut drag = false;
        let margin = if matches!(self.phase, Phase::Failed) { Margin::same(16) } else { Margin::symmetric(12, 0) };
        ctx.root_ui(|ui| {
            egui::CentralPanel::default().frame(Frame::new().fill(p.elevated).inner_margin(margin)).show(ui, |ui| {
                // Behind every control: dragging anywhere else moves the pill.
                let surface = ui.interact(ctx.content_rect(), Id::unique("recorder-drag"), Sense::click_and_drag());
                drag = surface.drag_started_by(PointerButton::Primary);
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.spacing_mut().interact_size.y = 32.0;
                match &self.phase {
                    Phase::Countdown { .. } => self.countdown_row(ui, &mut act),
                    Phase::Starting => busy_row(ui, &t("recorder.starting")),
                    Phase::Recording if self.discard_asked.is_some() => self.discard_row(ui, &mut act),
                    Phase::Recording => self.recording_row(ui, &mut act),
                    Phase::Saving { discarding } => busy_row(ui, &t(if *discarding { "recorder.discarding" } else { "recorder.saving" })),
                    Phase::Failed => {
                        if let Some(failure) = &self.failure {
                            card(ui, failure, &mut act);
                        }
                    }
                }
            });
        });
        let outline = ctx.layer_painter(LayerId::new(Order::Foreground, Id::unique("recorder-outline")));
        outline.rect_stroke(ctx.content_rect(), 0.0, Stroke::new(1.0, p.border), StrokeKind::Inside);

        if drag {
            cx.shell.drag_window(cx.win);
        }
        match act {
            Some(Act::TogglePause) => self.toggle_pause(cx),
            Some(Act::Stop) => self.stop(cx),
            Some(Act::AskDiscard) => self.discard_asked = Some(std::time::Instant::now()),
            Some(Act::KeepRecording) => self.discard_asked = None,
            Some(Act::Discard) => self.discard(cx),
            Some(Act::CancelCountdown) => cx.close(),
            Some(Act::Fix) => {
                if let Some(fix) = self.failure.as_ref().and_then(|f| f.fix) {
                    apply_fix(cx, fix);
                }
                cx.close();
            }
            Some(Act::CloseCard) => cx.close(),
            None => {}
        }
    }

    fn countdown_row(&self, ui: &mut Ui, act: &mut Option<Act>) {
        let Phase::Countdown { began, secs } = self.phase else { return };
        let left = (secs as f32 - began.elapsed().as_secs_f32()).ceil().max(1.0) as u32;
        ui.horizontal_centered(|ui| {
            recording_dot(ui, DotState::Idle);
            ui.label(text::body_strong(tf("recorder.starting_in", &[("n", &left.to_string())])));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if secondary_button(ui, &t("common.cancel")).clicked() {
                    *act = Some(Act::CancelCountdown);
                }
            });
        });
    }

    fn discard_row(&self, ui: &mut Ui, act: &mut Option<Act>) {
        ui.horizontal_centered(|ui| {
            ui.label(text::body_strong(t("recorder.discard_ask")));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(Button::new(&t("recorder.discard")).kind(ButtonKind::Danger)).clicked() {
                    *act = Some(Act::Discard);
                }
                if secondary_button(ui, &t("recorder.keep")).clicked() {
                    *act = Some(Act::KeepRecording);
                }
            });
        });
    }

    fn recording_row(&self, ui: &mut Ui, act: &mut Option<Act>) {
        let Some(recorder) = &self.recorder else { return };
        let paused = recorder.is_paused();
        let p = palette(ui.ctx());
        ui.horizontal_centered(|ui| {
            recording_dot(ui, if paused { DotState::Paused } else { DotState::Live });
            let clock = clock(recorder.elapsed_ms());
            let color = if paused { p.text_dim } else { p.text };
            let (rect, _) = ui.allocate_exact_size(Vec2::new(62.0, 20.0), Sense::hover());
            let galley = util::layout(ui, &clock, &text::mono(), color);
            ui.painter().galley(util::snap(ui, pos2(rect.left(), rect.center().y - galley.size().y / 2.0)), galley, color);
            // The clock keeps its slot whether it reads 59:59 or 1:00:00, so nothing shifts.
            toolbar_separator(ui);
            let (glyph, tip) = if paused { (icon::PLAY, "recorder.resume") } else { (icon::PAUSE, "recorder.pause") };
            if icon_button(ui, glyph, &t(tip), None).clicked() {
                *act = Some(Act::TogglePause);
            }
            if self.options.microphone || self.options.system_audio {
                toolbar_separator(ui);
                self.audio_group(ui);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icon_button(ui, icon::TRASH, &t("recorder.discard"), None).clicked() {
                    *act = Some(Act::AskDiscard);
                }
                let stop = ui.add(Button::new(&t("recorder.stop")).kind(ButtonKind::Danger).icon(icon::STOP));
                if tooltip(stop, &t("recorder.stop_tip"), self.stop_hotkey.as_deref()).clicked() {
                    *act = Some(Act::Stop);
                }
            });
        });
    }

    /// Microphone and system audio as state, then the level of whichever is being listened to.
    fn audio_group(&self, ui: &mut Ui) {
        let (mic, system) = (self.options.microphone, self.options.system_audio);
        state_icon(
            ui,
            if mic { icon::MICROPHONE } else { icon::MICROPHONE_SLASH },
            mic,
            &t(if mic { "recorder.mic_on" } else { "recorder.mic_off" }),
        );
        state_icon(
            ui,
            if system { icon::SPEAKER_HIGH } else { icon::SPEAKER_SLASH },
            system,
            &t(if system { "recorder.system_on" } else { "recorder.system_off" }),
        );
        level_meter(ui, self.level);
    }
}

#[derive(Clone, Copy)]
enum DotState {
    /// Counting down: not recording yet.
    Idle,
    Live,
    Paused,
}

/// The 12 px status mark: a pulsing red dot while recording, two bars while paused.
fn recording_dot(ui: &mut Ui, state: DotState) {
    let p = palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
    let center = rect.center();
    match state {
        DotState::Live => {
            let beat = (ui.input(|i| i.time) % PULSE_PERIOD / PULSE_PERIOD) as f32;
            let strength = 0.78 + 0.22 * (beat * TAU).cos();
            ui.painter().circle_filled(center, 8.0 * (0.45 + 0.1 * strength), p.danger.gamma_multiply(0.18 * strength));
            ui.painter().circle_filled(center, 5.0, p.danger);
        }
        DotState::Paused => {
            for dx in [-2.5, 2.5] {
                ui.painter().rect_filled(egui::Rect::from_center_size(center + Vec2::new(dx, 0.0), Vec2::new(2.5, 9.0)), 1.0, p.text_dim);
            }
        }
        DotState::Idle => {
            ui.painter().circle_stroke(center, 4.5, Stroke::new(1.5, p.text_faint));
        }
    }
}

/// 16 px glyph that shows whether a source is on, with a tooltip saying so.
fn state_icon(ui: &mut Ui, glyph: &str, on: bool, tip: &str) {
    let p = palette(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
    util::paint_icon(ui, rect.center(), glyph, 16.0, if on { p.text } else { p.text_faint });
    tooltip(resp, tip, None);
}

/// Eight small bars that light up from the left; green, then amber, then red near the top.
fn level_meter(ui: &mut Ui, level: f32) {
    let p = palette(ui.ctx());
    let width = METER_SEGMENTS as f32 * METER_SEGMENT.x + (METER_SEGMENTS - 1) as f32 * METER_GAP;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, METER_SEGMENT.y), Sense::hover());
    let lit = (level * METER_SEGMENTS as f32).round() as usize;
    for i in 0..METER_SEGMENTS {
        let x = rect.left() + i as f32 * (METER_SEGMENT.x + METER_GAP);
        let segment = egui::Rect::from_min_size(pos2(x, rect.top()), METER_SEGMENT);
        let on: Color32 = match i {
            0..=4 => p.success,
            5..=6 => p.warning,
            _ => p.danger,
        };
        ui.painter().rect_filled(segment, 1.0, if i < lit { on } else { p.border });
    }
}

/// `m:ss`, or `h:mm:ss` from the first hour on.
fn clock(ms: u64) -> String {
    let secs = ms / 1000;
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
    } else {
        format!("{:02}:{:02}", secs / 60, secs % 60)
    }
}

fn busy_row(ui: &mut Ui, label: &str) {
    ui.horizontal_centered(|ui| {
        spinner(ui);
        ui.label(text::body_strong(label));
    });
}

/// The error card: what happened, and one button that fixes it when we know how.
fn card(ui: &mut Ui, failure: &Failure, act: &mut Option<Act>) {
    let p = palette(ui.ctx());
    ui.horizontal_top(|ui| {
        util::paint_icon(ui, ui.cursor().min + Vec2::new(10.0, 12.0), icon::WARNING, 20.0, p.danger);
        ui.add_space(30.0);
        ui.vertical(|ui| {
            ui.label(text::body_strong(t("recorder.error.title")));
            ui.add_space(2.0);
            ui.label(text::dim(failure.message.clone()));
        });
    });
    ui.with_layout(Layout::bottom_up(Align::Max), |ui| {
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| match failure.fix {
                Some(fix) => {
                    let label = t(match fix {
                        super::Fix::Permission(_) => "recorder.error.open_system_settings",
                        super::Fix::SettingsPage => "recorder.error.open_settings",
                    });
                    if primary_button(ui, &label).clicked() {
                        *act = Some(Act::Fix);
                    }
                    if ghost_button(ui, &t("common.close")).clicked() {
                        *act = Some(Act::CloseCard);
                    }
                }
                None => {
                    if secondary_button(ui, &t("common.close")).clicked() {
                        *act = Some(Act::CloseCard);
                    }
                }
            });
        });
    });
}
