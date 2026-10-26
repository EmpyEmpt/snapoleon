//! Everything the UI thread can be woken with. Worker threads, the tray, the
//! hotkey manager and the IPC server all talk to the app through these.

use std::{any::Any, time::Instant};

use crossbeam_channel::Sender;
use snap_core::ipc::{Request, Response};
use snap_engine::{Notice, Outcome};
use snap_platform::{notify::NotifyAction, tray::TrayEvent};
use snap_shell::WindowKey;

use crate::commands::Command;

// See `Command`: transient messages, not worth boxing.
#[allow(clippy::large_enum_variant)]
pub enum AppEvent {
    /// Index into `settings.hotkeys` as registered.
    Hotkey(usize),
    Tray(TrayEvent),
    /// The latest capture's quick actions ran out (see `App::arm_tray_expiry`): rebuild the tray menu.
    RefreshTray,
    /// A request from the CLI, a second instance or a `snapoleon://` URL. The
    /// sender is answered exactly once, possibly after a job has finished.
    Ipc(Request, Sender<Response>),
    Command(Command),
    /// The user clicked an OS notification or one of its buttons (macOS).
    Notified(NotifyAction),
    /// A reopen of the app (`OsRequest::Reopen`, macOS) made at that moment, held back for a notification click it may
    /// have come with (see `ipc::defer_reopen`).
    Reopen(Instant),
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
    /// A capture, recording or imported file was added to (or changed in) the history. The app refreshes the tray's recent
    /// list and delivers [`HistoryChanged`] to every open window's `View::on_msg`.
    HistoryChanged,
    /// Dev harness: the screenshot delay elapsed.
    DevShot,
}

/// The message every open window gets in `View::on_msg` after [`AppEvent::HistoryChanged`]. The Library reloads on it.
/// Views that write to the history themselves send the event with `cx.proxy.send(AppEvent::HistoryChanged)`.
#[derive(Debug, Clone, Copy)]
pub struct HistoryChanged;

pub struct JobDone {
    pub label: String,
    pub result: Result<Outcome, snap_core::Error>,
    /// The job's steps produce text for the text window (OCR without copy, translation).
    pub show_text: bool,
    /// The engine already showed the text through `PipelineUi::show_text`.
    pub text_shown: bool,
    /// Answered with the outcome (CLI `capture --region`, `process`).
    pub reply: Option<Sender<Response>>,
}
