//! Routes every hotkey `Action` to the right flow, and turns overlay results into commands.

use std::time::Duration;

use snap_core::{Settings, hotkey::Action, pipeline::Step, settings::VideoFormat, t, tf};
use snap_engine::{CaptureRequest, Notice, NoticeKind, RecordTarget};
use snap_shell::{Proxy, Shell};

use crate::{
    app::App,
    commands::{Command, Work},
    events::AppEvent,
    views::{
        colors::ColorToolInput,
        editor::EditorInput,
        overlay::{self, OverlayAction, OverlayMode, OverlayRequest, OverlayResult},
        steps::StepsMsg,
    },
};

impl App {
    /// Run an action as if its hotkey was pressed. Interactive actions queue an overlay or a view;
    /// the rest capture right away on a worker thread.
    pub(crate) fn start_action(&mut self, sh: &mut Shell<AppEvent>, action: Action, pipeline: Option<String>) {
        // A capture now would photograph the overlay that is already up.
        if action.is_capture() && overlay::active() {
            return;
        }
        let settings = &self.settings;
        let overlay = |mode: OverlayMode, pipeline: Option<String>| Command::OpenOverlay(OverlayRequest { mode, pipeline });
        let immediate = |request: CaptureRequest, pipeline: Option<String>| Command::CaptureAndRun {
            request,
            work: Work::Pipeline(pipeline.unwrap_or_else(|| settings.default_pipeline.clone())),
            reply: None,
        };
        let command = match action {
            Action::CaptureRegion => overlay(OverlayMode::Region, pipeline),
            Action::CaptureWindow => overlay(OverlayMode::Window, pipeline),
            Action::CaptureElement => overlay(OverlayMode::Element, pipeline),
            Action::CaptureScrolling => overlay(OverlayMode::Scroll, pipeline),
            Action::CaptureMonitor => immediate(CaptureRequest::ActiveMonitor, pipeline),
            Action::CaptureAllMonitors => immediate(CaptureRequest::AllMonitors, pipeline),
            Action::CaptureLastRegion => immediate(CaptureRequest::LastRegion, pipeline),
            Action::CaptureDelayed => {
                self.countdown(sh.proxy(), Command::StartCapture { action: Action::CaptureRegion, pipeline });
                return;
            }
            Action::RecordRegion => overlay(OverlayMode::Record(settings.recording.format), None),
            Action::RecordGif => overlay(OverlayMode::Record(VideoFormat::Gif), None),
            Action::RecordScreen => {
                Command::StartRecording { target: RecordTarget::Monitor(active_monitor()), format: settings.recording.format }
            }
            Action::StopRecording => Command::StopRecording,
            Action::OcrRegion => overlay(OverlayMode::Ocr, pipeline),
            Action::DecodeCodesRegion => overlay(OverlayMode::Codes, pipeline),
            Action::TranslateRegion => overlay(OverlayMode::Translate, pipeline),
            Action::ColorPicker => overlay(OverlayMode::ColorPicker, None),
            Action::Measure => overlay(OverlayMode::Measure, None),
            Action::PinRegion => overlay(OverlayMode::Pin, None),
            Action::PinClipboard => match snap_platform::clipboard::get_image() {
                Some(image) => Command::OpenPin { image, origin: None },
                None => return self.no_clipboard_image(sh),
            },
            Action::OpenEditorFromClipboard => match snap_platform::clipboard::get_image() {
                Some(image) => {
                    let capture = snap_core::Capture::from_image(snap_core::CaptureKind::Clipboard, image);
                    Command::OpenEditor(EditorInput::from_capture(capture))
                }
                None => return self.no_clipboard_image(sh),
            },
            Action::StepCaptureToggle => {
                // A running session takes the toggle itself; otherwise start one.
                let running = sh.windows_with_tag(crate::views::steps::TAG);
                if let Some(&win) = running.first() {
                    sh.proxy().send(AppEvent::ViewMsg { win, msg: Box::new(StepsMsg::Toggle) });
                    return;
                }
                Command::StartStepCapture
            }
            Action::OpenLibrary => Command::OpenLibrary,
            Action::OpenSettings => Command::OpenSettings { page: None },
        };
        self.cmds.push(command);
    }

    fn no_clipboard_image(&mut self, sh: &mut Shell<AppEvent>) {
        self.notice(sh, Notice { kind: NoticeKind::Info, title: t("app.clipboard.no_image").into_owned(), ..Notice::default() });
    }

    /// Count down on a helper thread (progress pill and tray tooltip), then queue `then`.
    fn countdown(&mut self, proxy: Proxy<AppEvent>, then: Command) {
        let secs = self.settings.capture.delay_secs;
        std::thread::spawn(move || {
            for left in (1..=secs).rev() {
                let label = tf("app.countdown", &[("n", &left.to_string())]);
                proxy.send(AppEvent::JobProgress { label, frac: Some(left as f32 / secs as f32) });
                std::thread::sleep(Duration::from_secs(1));
            }
            proxy.send(AppEvent::JobProgress { label: String::new(), frac: None });
            proxy.send(AppEvent::Command(then));
        });
    }
}

/// The monitor under the cursor, else the primary one.
fn active_monitor() -> usize {
    let monitors = snap_capture::monitors().unwrap_or_default();
    let cursor = snap_capture::cursor_position();
    monitors.iter().find(|m| m.rect.contains(cursor)).or_else(|| monitors.iter().find(|m| m.is_primary)).map_or(0, |m| m.index)
}

/// What a "select, then ..." overlay mode does with the selected capture. `None` for modes that
/// have their own destination (pin, record, scroll, color picker, measure).
pub fn mode_work(mode: OverlayMode, pipeline: Option<String>, settings: &Settings) -> Option<Work> {
    let named = |id: &str, fallback: Vec<Step>| match (pipeline.clone(), settings.pipeline(id)) {
        (Some(chosen), _) => Work::Pipeline(chosen),
        (None, Some(_)) => Work::Pipeline(id.to_string()),
        (None, None) => Work::Steps(fallback),
    };
    match mode {
        OverlayMode::Region | OverlayMode::Window | OverlayMode::Element => {
            Some(Work::Pipeline(pipeline.unwrap_or_else(|| settings.default_pipeline.clone())))
        }
        OverlayMode::Ocr => Some(named("ocr", vec![Step::Ocr { copy: true, as_markdown_table: false }, Step::Notify])),
        OverlayMode::Codes => Some(Work::Steps(vec![Step::DecodeCodes { copy: false }, Step::Notify])),
        OverlayMode::Translate => {
            Some(Work::Steps(vec![Step::Ocr { copy: false, as_markdown_table: false }, Step::Translate { target_lang: None }]))
        }
        OverlayMode::Pin | OverlayMode::Record(_) | OverlayMode::Scroll | OverlayMode::ColorPicker | OverlayMode::Measure => None,
    }
}

/// The commands that follow an overlay session. The overlay view pushes these when it finishes.
pub fn overlay_commands(result: OverlayResult, request: &OverlayRequest, settings: &Settings) -> Vec<Command> {
    let steps = |capture, steps: Vec<Step>| vec![Command::RunSteps { capture, steps }];
    match result {
        OverlayResult::Cancelled => Vec::new(),
        OverlayResult::Color(sample) => vec![Command::OpenColorTool(ColorToolInput::Picked(sample))],
        OverlayResult::Measured(measurement) => vec![Command::OpenColorTool(ColorToolInput::Measured(measurement))],
        OverlayResult::Selected(capture) => match request.mode {
            OverlayMode::Pin => vec![Command::OpenPin { origin: Some(capture.screen_rect), image: capture.image }],
            OverlayMode::Record(format) => vec![Command::StartRecording { target: RecordTarget::Region(capture.screen_rect), format }],
            OverlayMode::Scroll => vec![Command::StartScrolling { rect: capture.screen_rect, pipeline: request.pipeline.clone() }],
            mode => mode_work(mode, request.pipeline.clone(), settings).map(|work| work.command(capture)).into_iter().collect(),
        },
        OverlayResult::Action(action, capture) => match action {
            OverlayAction::Copy => steps(capture, vec![Step::CopyImage, Step::Notify]),
            OverlayAction::Save => steps(capture, vec![Step::Save { dir: None, template: None, format: None }, Step::Notify]),
            OverlayAction::Edit => vec![Command::OpenEditor(EditorInput::from_capture(capture))],
            OverlayAction::Pin => vec![Command::OpenPin { origin: Some(capture.screen_rect), image: capture.image }],
            OverlayAction::Upload => {
                steps(capture, vec![Step::Upload { destination: None }, Step::CopyLink { format: None }, Step::Notify])
            }
            OverlayAction::Text => steps(capture, vec![Step::Ocr { copy: false, as_markdown_table: false }]),
            OverlayAction::Qr => steps(capture, vec![Step::DecodeCodes { copy: false }, Step::Notify]),
            OverlayAction::Redact => steps(capture, vec![Step::AutoRedact { kinds: Vec::new(), style: None }, Step::Annotate]),
            OverlayAction::Record => {
                vec![Command::StartRecording { target: RecordTarget::Region(capture.screen_rect), format: settings.recording.format }]
            }
            OverlayAction::Scroll => vec![Command::StartScrolling { rect: capture.screen_rect, pipeline: None }],
        },
    }
}
