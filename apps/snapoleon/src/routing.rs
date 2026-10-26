//! Command routing: what the app does for every `Command`.

use snap_core::{CaptureKind, ipc::Response, pipeline::Step, t, tf};
use snap_engine::{CaptureRequest, Engine, Notice, NoticeKind, Outcome, RecordTarget};
use snap_shell::Shell;

use crate::{
    app::App,
    commands::{Command, Work},
    events::AppEvent,
    jobs,
    notices::error_text,
    views,
};

/// Pixels per module of a QR code shown on screen.
const QR_MODULE_PX: u32 = 8;

impl App {
    pub(crate) fn run_command(&mut self, sh: &mut Shell<AppEvent>, cmd: Command) {
        match cmd {
            Command::StartCapture { action, pipeline } => self.start_action(sh, action, pipeline),
            Command::OpenOverlay(request) => self.open_with(sh, |cx| views::overlay::open(cx, request)),
            Command::CaptureAndRun { request, work, reply } => self.capture_and_run(sh, request, work, reply),
            Command::RunPipeline { capture, pipeline } => {
                let show_text = self.settings.pipeline(&pipeline).is_some_and(|p| shows_text(&p.steps));
                self.run_job(sh, t("app.job.working").into_owned(), None, show_text, move |engine, ui| {
                    jobs::run_work(engine, capture, Work::Pipeline(pipeline), ui)
                });
            }
            Command::RunSteps { capture, steps } => {
                let show_text = shows_text(&steps);
                self.run_job(sh, t("app.job.working").into_owned(), None, show_text, move |engine, ui| {
                    jobs::run_work(engine, capture, Work::Steps(steps), ui)
                });
            }
            Command::ProcessFile { path, pipeline, reply } => {
                let Some(steps) = self.settings.pipeline(&pipeline).map(|p| p.steps.clone()) else {
                    reply_with(reply, Response::err(tf("app.error.no_pipeline", &[("id", &pipeline)])));
                    return;
                };
                let kind = file_kind(&path);
                self.run_job(sh, t("app.job.working").into_owned(), reply, false, move |engine, ui| {
                    engine.run_file(&path, kind, &steps, ui)
                });
            }
            Command::OpenEditor(input) => self.open_with(sh, |cx| views::editor::open(cx, input)),
            // The editor is for pictures and projects; a video goes to the video editor.
            Command::EditFile(path) if is_video(&path) => self.cmds.push(Command::OpenVideoEditor { path }),
            Command::EditFile(path) => {
                load_in_background(sh, path, |path| views::editor::EditorInput::from_file(&path).map(Command::OpenEditor))
            }
            Command::OpenPin { image, origin } => {
                let opened = self.open_windows(sh, |cx| views::pin::open(cx, image, origin));
                self.pins.extend(opened);
                self.refresh_tray();
            }
            Command::ClosePins => {
                for win in sh.windows_with_tag(views::pin::TAG) {
                    sh.close(win);
                }
            }
            Command::UnlockPins => {
                for win in sh.windows_with_tag(views::pin::TAG) {
                    sh.proxy().send(AppEvent::ViewMsg { win, msg: Box::new(views::pin::Unlock) });
                }
            }
            Command::PinFile(path) => load_in_background(sh, path, |path| {
                let image = snap_image::encode::load(&path)?;
                Ok(Command::OpenPin { image, origin: None })
            }),
            Command::OpenLibrary => self.open_with(sh, views::library::open),
            Command::OpenSettings { page } => self.open_with(sh, |cx| views::settings::open(cx, page)),
            Command::OpenOnboarding => self.open_with(sh, views::onboarding::open),
            Command::OpenAbout => self.open_with(sh, views::about::open),
            Command::ShowText { title, text } => self.open_with(sh, |cx| views::text::open(cx, title, text)),
            Command::ShowCodes { title, text, codes } => self.open_with(sh, |cx| views::text::open_with_codes(cx, title, text, codes)),
            Command::StartRecording { target, format } => self.start_recording(sh, target, format),
            Command::StopRecording => {
                for win in sh.windows_with_tag(views::recorder::TAG) {
                    sh.proxy().send(AppEvent::ViewMsg { win, msg: Box::new(views::recorder::RecorderMsg::Stop) });
                }
            }
            Command::RecordingState { active } => self.set_recording(sh, active),
            Command::OpenVideoEditor { path } => self.open_with(sh, |cx| views::video::open(cx, path)),
            Command::StartScrolling { rect, pipeline } => {
                self.open_with(sh, |cx| views::scroll::open(cx, views::scroll::ScrollInput { rect, pipeline }))
            }
            Command::StartStepCapture => self.open_with(sh, views::steps::open),
            Command::OpenCombine { images } => self.open_with(sh, |cx| views::combine::open(cx, images)),
            Command::OpenDiff { a, b } => self.open_with(sh, |cx| views::diff::open(cx, a, b)),
            Command::OpenColorTool(input) => self.open_with(sh, |cx| views::colors::open(cx, input)),
            Command::PauseHotkeys(paused) => self.pause_hotkeys(sh, paused),
            Command::Shutter => {
                if self.settings.general.play_sounds {
                    snap_platform::sound::shutter();
                }
            }
            // A pin is the lightest window there is: it floats above everything and closes with a click.
            Command::ShowQr(url) => match snap_ocr::codes::try_qr(&url, QR_MODULE_PX) {
                Ok(image) => self.cmds.push(Command::OpenPin { image, origin: None }),
                Err(e) => self.report_error(sh, &e),
            },
            Command::CopyText(text) => {
                if let Err(e) = snap_platform::clipboard::set_text(&text) {
                    self.report_error(sh, &e);
                }
            }
            Command::Reveal(path) => {
                if let Err(e) = snap_platform::open::reveal(&path) {
                    self.report_error(sh, &e);
                }
            }
            Command::OpenPath(path) => {
                if let Err(e) = snap_platform::open::path(&path) {
                    self.report_error(sh, &e);
                }
            }
            Command::OpenUrl(url) => {
                if let Err(e) = snap_platform::open::url(&url) {
                    self.report_error(sh, &e);
                }
            }
            Command::Open(open) => self.open_with(sh, open),
            Command::Quit => self.quit(sh),
        }
    }

    pub(crate) fn capture_and_run(
        &mut self,
        sh: &mut Shell<AppEvent>,
        request: CaptureRequest,
        work: Work,
        reply: Option<crossbeam_channel::Sender<Response>>,
    ) {
        let show_text = match &work {
            Work::Steps(steps) => shows_text(steps),
            Work::Pipeline(id) => self.settings.pipeline(id).is_some_and(|p| shows_text(&p.steps)),
        };
        let sound = self.settings.general.play_sounds;
        self.run_job(sh, t("app.job.capturing").into_owned(), reply, show_text, move |engine, ui| {
            let capture = engine.capture(&request)?;
            if sound {
                snap_platform::sound::shutter();
            }
            jobs::run_work(engine, capture, work, ui)
        });
    }

    /// Start an engine job on a worker thread.
    pub(crate) fn run_job(
        &mut self,
        sh: &mut Shell<AppEvent>,
        label: String,
        reply: Option<crossbeam_channel::Sender<Response>>,
        show_text: bool,
        run: impl FnOnce(&Engine, &jobs::GuiPipelineUi) -> snap_core::Result<Outcome> + Send + 'static,
    ) {
        self.jobs_running += 1;
        let engine = self.engine.clone();
        jobs::spawn(sh.proxy(), label, reply, show_text, move |ui| run(&engine, ui));
    }

    pub(crate) fn start_recording(&mut self, sh: &mut Shell<AppEvent>, target: RecordTarget, format: snap_core::settings::VideoFormat) {
        if self.policy.disable_recording {
            self.notice(sh, Notice { kind: NoticeKind::Warning, title: t("app.recording.disabled").into_owned(), ..Notice::default() });
            return;
        }
        self.open_with(sh, |cx| views::recorder::open(cx, views::recorder::RecorderInput { target, format }));
    }

    pub(crate) fn set_recording(&mut self, sh: &mut Shell<AppEvent>, active: bool) {
        self.recording = active;
        self.refresh_tray();
        if !active && self.quit_pending {
            self.shutdown(sh);
        }
    }
}

pub(crate) fn reply_with(reply: Option<crossbeam_channel::Sender<Response>>, response: Response) {
    if let Some(reply) = reply {
        let _ = reply.send(response);
    }
}

/// Whether running these steps produces text that belongs in the text window rather than on the clipboard.
fn shows_text(steps: &[Step]) -> bool {
    steps.iter().any(|s| matches!(s, Step::Ocr { copy: false, .. } | Step::Translate { .. }))
}

fn file_kind(path: &std::path::Path) -> CaptureKind {
    if is_video(path) || extension(path) == "gif" { CaptureKind::Recording } else { CaptureKind::File }
}

/// A file the video editor opens.
pub(crate) fn is_video(path: &std::path::Path) -> bool {
    matches!(extension(path).as_str(), "mp4" | "m4v" | "mov" | "webm" | "mkv")
}

fn extension(path: &std::path::Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// Decode a file on a worker thread, then queue the command built from it (or an error notice).
fn load_in_background(
    sh: &mut Shell<AppEvent>,
    path: std::path::PathBuf,
    load: impl FnOnce(std::path::PathBuf) -> snap_core::Result<Command> + Send + 'static,
) {
    let proxy = sh.proxy();
    std::thread::spawn(move || match load(path) {
        Ok(command) => proxy.send(AppEvent::Command(command)),
        Err(e) => proxy.send(AppEvent::Notice(Notice {
            kind: NoticeKind::Error,
            title: t("app.error.open_file").into_owned(),
            body: error_text(&e),
            ..Notice::default()
        })),
    });
}
