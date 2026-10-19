//! Worker threads that run engine calls, and the GUI side of the engine's `PipelineUi`.
//!
//! Engine calls block (capture, OCR, upload, ...), so they never run on the UI thread. A job
//! reports progress and its final result back as `AppEvent`s.

use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::Sender;
use snap_core::{Capture, Error, Result, RgbaImage, ipc::Response, t};
use snap_engine::{Annotated, Code, Engine, Notice, Outcome, PipelineUi};
use snap_shell::Proxy;

use crate::{
    commands::{Command, Work},
    events::{AppEvent, JobDone},
    notices::error_text,
    views::editor::EditorInput,
};

/// Implements the engine's interactive hooks by sending events to the UI thread.
pub struct GuiPipelineUi {
    proxy: Proxy<AppEvent>,
    text_shown: AtomicBool,
}

impl GuiPipelineUi {
    pub fn new(proxy: Proxy<AppEvent>) -> Self {
        Self { proxy, text_shown: AtomicBool::new(false) }
    }
}

impl PipelineUi for GuiPipelineUi {
    /// Opens the editor and blocks this worker thread until the user finishes or discards.
    fn annotate(&self, cap: &Capture) -> Result<Option<Annotated>> {
        let (reply, answer) = crossbeam_channel::bounded(1);
        let input = EditorInput { reply: Some(reply), ..EditorInput::from_capture(cap.clone()) };
        // The user is editing, not waiting for work: no "Working" pill while the editor is open.
        self.progress("", None);
        self.proxy.send(AppEvent::Command(Command::OpenEditor(input)));
        // A dropped sender (window closed without answering, app quitting) counts as a discard.
        Ok(answer.recv().unwrap_or(None))
    }

    fn pin(&self, img: &RgbaImage, cap: &Capture) {
        self.proxy.send(AppEvent::Command(Command::OpenPin { image: img.clone(), origin: Some(cap.screen_rect) }));
    }

    fn notify(&self, notice: &Notice) {
        self.proxy.send(AppEvent::Notice(notice.clone()));
    }

    fn progress(&self, label: &str, frac: Option<f32>) {
        self.proxy.send(AppEvent::JobProgress { label: label.to_string(), frac });
    }

    fn show_text(&self, title: &str, text: &str) {
        self.text_shown.store(true, Ordering::Relaxed);
        self.proxy.send(AppEvent::Command(Command::ShowText { title: title.to_string(), text: text.to_string() }));
    }

    fn show_codes(&self, title: &str, codes: &[Code]) {
        self.text_shown.store(true, Ordering::Relaxed);
        let text = codes.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join(
            "
",
        );
        self.proxy.send(AppEvent::Command(Command::ShowCodes { title: title.to_string(), text, codes: codes.to_vec() }));
    }
}

/// Run `run` on a worker thread. The progress pill shows `label` until the job reports otherwise;
/// the result arrives as `AppEvent::JobDone`. `show_text` asks the app to show the outcome's text
/// in the text window unless the engine already did.
pub fn spawn(
    proxy: Proxy<AppEvent>,
    label: String,
    reply: Option<Sender<Response>>,
    show_text: bool,
    run: impl FnOnce(&GuiPipelineUi) -> Result<Outcome> + Send + 'static,
) {
    let spawned = std::thread::Builder::new().name("snap-job".into()).spawn({
        let proxy = proxy.clone();
        move || {
            let ui = GuiPipelineUi::new(proxy.clone());
            ui.progress(&label, None);
            // A panic inside the engine ends this job, not the app, and must not leave the progress pill spinning.
            let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&ui)));
            let result = match ran.unwrap_or_else(|_| Err(Error::Other(anyhow::anyhow!(t("app.error.job"))))) {
                Ok(outcome) => Ok(outcome),
                // The user dismissed an interactive step: not an error.
                Err(Error::Cancelled) => Ok(Outcome { cancelled: true, ..Outcome::default() }),
                Err(e) => Err(error_text(&e)),
            };
            let text_shown = ui.text_shown.load(Ordering::Relaxed);
            proxy.send(AppEvent::JobDone(JobDone { label, result, show_text, text_shown, reply }));
        }
    });
    if let Err(e) = spawned {
        tracing::error!("cannot start a job thread: {e}");
    }
}

/// Run `work` over a finished capture.
pub fn run_work(engine: &Engine, capture: Capture, work: Work, ui: &dyn PipelineUi) -> Result<Outcome> {
    match work {
        Work::Pipeline(id) => engine.run(capture, &id, ui),
        Work::Steps(steps) => engine.run_steps(capture, &steps, ui),
    }
}

/// What the CLI gets back for a finished job.
pub fn response(outcome: &Outcome) -> Response {
    if outcome.cancelled {
        return Response::err(t("app.job.cancelled"));
    }
    Response {
        ok: true,
        error: None,
        path: outcome.file.as_ref().map(|p| p.display().to_string()),
        url: outcome.link.as_ref().map(|l| l.best().to_string()),
        text: outcome.text.clone(),
    }
}
