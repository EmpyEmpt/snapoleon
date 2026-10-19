//! Everything the UI thread can be woken with. Worker threads, the tray, the
//! hotkey manager and the IPC server all talk to the app through these.

use std::any::Any;

use crossbeam_channel::Sender;
use snap_core::ipc::{Request, Response};
use snap_engine::{Notice, Outcome};
use snap_platform::tray::TrayEvent;
use snap_shell::WindowKey;

use crate::commands::Command;

// See `Command`: transient messages, not worth boxing.
#[allow(clippy::large_enum_variant)]
pub enum AppEvent {
    /// Index into `settings.hotkeys` as registered.
    Hotkey(usize),
    Tray(TrayEvent),
    /// A request from the CLI, a second instance or a `snapoleon://` URL. The
    /// sender is answered exactly once, possibly after a job has finished.
    Ipc(Request, Sender<Response>),
    Command(Command),
    /// Progress of a running job; an empty label clears the progress pill.
    JobProgress {
        label: String,
        frac: Option<f32>,
    },
    JobDone(JobDone),
    /// A user-facing result: an in-app toast when an app window is focused, an OS notification otherwise.
    Notice(Notice),
    /// Settings were written (by a view or by `ReloadSettings`); re-read and re-apply them.
    SettingsChanged,
    /// A message for one view, delivered to `View::on_msg`.
    ViewMsg {
        win: WindowKey,
        msg: Box<dyn Any + Send>,
    },
    /// Dev harness: the screenshot delay elapsed.
    DevShot,
}

pub struct JobDone {
    pub label: String,
    pub result: Result<Outcome, String>,
    /// The job's steps produce text for the text window (OCR without copy, translation).
    pub show_text: bool,
    /// The engine already showed the text through `PipelineUi::show_text`.
    pub text_shown: bool,
    /// Answered with the outcome (CLI `capture --region`, `process`).
    pub reply: Option<Sender<Response>>,
}
