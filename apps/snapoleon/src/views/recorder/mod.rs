//! Stub for the recorder HUD: a small always-on-top pill that is hidden from captures. It tells
//! the app when recording starts and stops (tray icon) but does not record yet.
//! Keep `TAG`, `RecorderInput`, `RecorderMsg` and the `open` signature: the app already calls them.

use std::{any::Any, time::Instant};

use egui::{Context, Margin};
use snap_core::{Point, settings::VideoFormat, t};
use snap_engine::{Notice, NoticeKind, RecordTarget};
use snap_shell::WindowSpec;
use snap_ui::{
    palette, text,
    widgets::{danger_button, secondary_button},
};

use super::{OpenCx, View, ViewCx, app_icon};
use crate::commands::Command;

pub const TAG: &str = "recorder";

pub struct RecorderInput {
    pub target: RecordTarget,
    pub format: VideoFormat,
}

/// Messages the app sends to the recorder window.
pub enum RecorderMsg {
    /// Finish and keep the recording (`Command::StopRecording`, the stop hotkey).
    Stop,
}

/// Starts a recording session. Ignored (with a notice) while one is already running.
pub fn open(cx: &mut OpenCx, input: RecorderInput) {
    if !cx.shell.windows_with_tag(TAG).is_empty() {
        cx.notify(Notice { kind: NoticeKind::Warning, title: t("app.recording.already").into_owned(), ..Notice::default() });
        return;
    }
    let (area, _) = cx.shell.monitors().first().copied().unwrap_or_default();
    let spec = WindowSpec {
        title: t("app.stub.recorder.title").into_owned(),
        size: Some((300.0, 64.0)),
        position: Some(Point::new(area.x + area.w as i32 / 2 - 150, area.y + area.h as i32 - 140)),
        decorations: false,
        resizable: false,
        rounded: true,
        always_on_top: true,
        tool_window: true,
        exclude_from_capture: true,
        icon: Some(app_icon()),
        tag: TAG.into(),
        ..WindowSpec::default()
    };
    cx.open(spec, RecorderView { _input: input, started: Instant::now() });
    cx.proxy.send(crate::events::AppEvent::Command(Command::RecordingState { active: true }));
}

struct RecorderView {
    _input: RecorderInput,
    started: Instant,
}

impl View for RecorderView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let p = palette(ctx);
        let elapsed = self.started.elapsed().as_secs();
        let mut stop = false;
        let mut cancel = false;
        ctx.root_ui(|ui| {
            egui::CentralPanel::default().frame(egui::Frame::new().fill(p.elevated).inner_margin(Margin::symmetric(14, 8))).show(
                ui,
                |ui| {
                    ui.horizontal_centered(|ui| {
                        let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        ui.painter().circle_filled(dot.center(), 5.0, p.danger);
                        ui.label(text::mono_text(format!("{:02}:{:02}", elapsed / 60, elapsed % 60)));
                        stop = danger_button(ui, &t("app.stub.recorder.stop")).clicked();
                        cancel = secondary_button(ui, &t("common.cancel")).clicked();
                    });
                },
            );
        });
        if stop || cancel {
            cx.close();
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }

    fn closed(&mut self, cx: &mut ViewCx) {
        cx.push(Command::RecordingState { active: false });
    }

    fn on_msg(&mut self, cx: &mut ViewCx, msg: Box<dyn Any + Send>) {
        if msg.downcast::<RecorderMsg>().is_ok() {
            cx.close();
        }
    }
}
