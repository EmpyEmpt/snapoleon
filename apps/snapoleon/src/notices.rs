//! Notices, the progress pill and the end of jobs.

use std::path::PathBuf;

use egui::{Align2, Context, Id, Margin, Order, Stroke};
use snap_core::{Error, ipc::Response, t};
use snap_engine::{Notice, NoticeKind};
use snap_platform::notify::{ActionLabels, NotifyAction, OsNotice};
use snap_shell::Shell;
use snap_ui::{
    Toast, ToastKind, palette, radius,
    widgets::{progress_indeterminate, progress_sized, spinner},
};

use crate::{
    app::App,
    commands::Command,
    events::{AppEvent, JobDone},
    jobs,
    routing::reply_with,
    tray,
};

/// User-facing text for an error: the bare message for the kinds whose `Display` only adds an English prefix
/// ("not found: ..."), `Display` otherwise.
pub fn error_text(e: &Error) -> String {
    match e {
        Error::NotFound(message) | Error::Invalid(message) | Error::Network(message) | Error::Policy(message) => message.clone(),
        Error::Permission(message) | Error::Unsupported(message) => message.to_string(),
        other => other.to_string(),
    }
}

/// The titles of the buttons on OS notifications, in the current language.
pub(crate) fn action_labels() -> ActionLabels {
    ActionLabels {
        open: t("app.notify.open").into_owned(),
        reveal: t("app.notify.show_in_finder").into_owned(),
        copy_link: t("app.notice.copy_link").into_owned(),
        open_link: t("app.notify.open_link").into_owned(),
    }
}

/// What a click on an OS notification (or one of its buttons) does.
pub(crate) fn notify_command(action: NotifyAction) -> Command {
    match action {
        NotifyAction::Open { file: Some(file), .. } => open_file(file),
        NotifyAction::Open { file: None, url: Some(url) } => Command::OpenUrl(url),
        NotifyAction::Open { file: None, url: None } => Command::OpenLibrary,
        NotifyAction::Reveal(file) => Command::Reveal(file),
        NotifyAction::CopyLink(url) => Command::CopyText(url),
        NotifyAction::OpenLink(url) => Command::OpenUrl(url),
    }
}

/// Captures, projects and videos open in Snapoleon's editors; anything else (a GIF recording, a crash report) in
/// its default app, as the tray's recent list does.
fn open_file(path: PathBuf) -> Command {
    let project = path.extension().is_some_and(|e| e.eq_ignore_ascii_case(snap_core::PROJECT_EXT));
    let picture = image::ImageFormat::from_path(&path).is_ok_and(|f| f != image::ImageFormat::Gif);
    if project || picture || crate::routing::is_video(&path) { Command::EditFile(path) } else { Command::OpenPath(path) }
}

/// The toast button that reveals a file, named like the platform's file manager.
fn show_in_folder() -> std::borrow::Cow<'static, str> {
    if cfg!(target_os = "macos") { t("app.notify.show_in_finder") } else { t("app.notice.show_in_folder") }
}

pub(crate) struct Progress {
    label: String,
    frac: Option<f32>,
}

impl App {
    /// Show a notice: a toast when an app window is focused, an OS notification otherwise.
    pub(crate) fn notice(&mut self, sh: &mut Shell<AppEvent>, notice: Notice) {
        let focused = self.focused.filter(|win| self.slots.contains_key(win));
        if let Some((win, slot)) = focused.and_then(|win| Some((win, self.slots.get_mut(&win)?))) {
            let kind = match notice.kind {
                NoticeKind::Info => ToastKind::Info,
                NoticeKind::Success => ToastKind::Success,
                NoticeKind::Warning => ToastKind::Warning,
                NoticeKind::Error => ToastKind::Error,
            };
            let text = if notice.body.is_empty() { notice.title.clone() } else { format!("{}: {}", notice.title, notice.body) };
            let toast = Toast::new(kind, text);
            let toast = match (&notice.url, &notice.file) {
                (Some(url), _) => toast
                    .action(t("app.notice.copy_link"), self.toast_actions.register(Command::CopyText(url.clone())))
                    .action(t("app.notice.show_qr"), self.toast_actions.register(Command::ShowQr(url.clone()))),
                (None, Some(file)) => toast.action(show_in_folder(), self.toast_actions.register(Command::Reveal(file.clone()))),
                (None, None) => toast,
            };
            slot.toasts.push_toast(toast);
            sh.repaint(win);
        } else if notice.kind == NoticeKind::Error || self.settings.general.show_notifications {
            // Off the UI thread: the preview is copied for the notification (macOS) and the OS call may block.
            std::thread::spawn(move || {
                let os = OsNotice { title: notice.title, body: notice.body, image: notice.image, file: notice.file, url: notice.url };
                if let Err(e) = snap_platform::notify::show(&os) {
                    tracing::warn!("notification failed: {e}");
                }
            });
        }
    }

    pub(crate) fn report_error(&mut self, sh: &mut Shell<AppEvent>, e: &snap_core::Error) {
        tracing::warn!("{e}");
        self.notice(
            sh,
            Notice { kind: NoticeKind::Error, title: t("common.error").into_owned(), body: error_text(e), ..Notice::default() },
        );
    }

    pub(crate) fn on_job_progress(&mut self, label: String, frac: Option<f32>) {
        self.progress = (!label.is_empty()).then_some(Progress { label, frac });
        if let Some(tray) = &mut self.tray {
            let tooltip = match &self.progress {
                Some(p) => format!("{}: {}", t("app.tray.tooltip"), p.label),
                None => tray::tooltip(self.recording),
            };
            tray.set_tooltip(&tooltip);
        }
    }

    pub(crate) fn on_job_done(&mut self, sh: &mut Shell<AppEvent>, done: JobDone) {
        self.jobs_running = self.jobs_running.saturating_sub(1);
        if self.jobs_running == 0 {
            self.on_job_progress(String::new(), None);
        }
        match done.result {
            Ok(outcome) => {
                tracing::info!("job '{}' finished", done.label);
                reply_with(done.reply, jobs::response(&outcome));
                if done.show_text && !done.text_shown && !outcome.cancelled {
                    match outcome.text.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                        Some(text) => self.cmds.push(Command::ShowText { title: t("app.text.title").into_owned(), text: text.to_string() }),
                        None => {
                            self.notice(sh, Notice { kind: NoticeKind::Info, title: t("app.text.none").into_owned(), ..Notice::default() })
                        }
                    }
                }
            }
            Err(message) => {
                tracing::warn!("job '{}' failed: {message}", done.label);
                reply_with(done.reply, Response::err(&message));
                self.notice(
                    sh,
                    Notice { kind: NoticeKind::Error, title: t("app.error.job").into_owned(), body: message, ..Notice::default() },
                );
            }
        }
        self.refresh_tray();
    }

    pub(crate) fn draw_progress(&self, ctx: &Context) {
        let Some(progress) = &self.progress else { return };
        let p = palette(ctx);
        egui::Area::new(Id::unique("snap-progress"))
            .order(Order::Tooltip)
            .anchor(Align2::LEFT_BOTTOM, egui::vec2(16.0, -16.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(p.elevated)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(radius::CARD + 2.0)
                    .inner_margin(Margin::symmetric(14, 10))
                    .shadow(p.shadow())
                    .show(ui, |ui| {
                        ui.set_width(240.0);
                        ui.horizontal(|ui| {
                            spinner(ui);
                            ui.label(&progress.label);
                        });
                        match progress.frac {
                            Some(frac) => progress_sized(ui, frac.clamp(0.0, 1.0), 240.0),
                            None => progress_indeterminate(ui),
                        };
                    });
            });
    }
}
