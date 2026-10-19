//! The export panel: format, size target, width, frame rate and sound; the export itself; and what can
//! be done with the result (copy, reveal, upload, copy link).

use std::{
    cell::Cell,
    path::{Path, PathBuf},
    time::Instant,
};

use egui::{Align, Frame, Layout, Margin, Stroke, Ui, Vec2};
use snap_core::{CaptureContext, CaptureId, Result, settings::VideoFormat, t};
use snap_engine::{Engine, Notice, NoticeKind};
use snap_record::{
    Recording,
    edit::{self, ExportOptions, VideoInfo},
};
use snap_ui::{
    ToastKind, icon, palette, radius, text,
    widgets::{
        Button, ButtonKind, card_frame, dropdown, number_field, progress_indeterminate, progress_sized, segmented, spinner, toggle, tooltip,
    },
};

use super::{Editor, Msg, timecode};
use crate::{commands::Command, views::ViewCx};

/// Size limits are decimal megabytes, which is what the services quote.
const MB: u64 = 1_000_000;
/// Output widths offered; `0` keeps the source width.
const WIDTHS: [u32; 6] = [0, 1920, 1280, 960, 720, 480];
/// Frame rates offered; `0` keeps the source rate (15 for GIF and WebP).
const FRAME_RATES: [u32; 6] = [0, 60, 30, 24, 15, 10];

#[derive(Clone, Copy, PartialEq, Eq)]
enum SizeTarget {
    Unlimited,
    Telegram,
    Email,
    Discord,
    Custom,
}

impl SizeTarget {
    /// Megabytes for the presets.
    fn preset_mb(self) -> Option<f32> {
        match self {
            SizeTarget::Unlimited | SizeTarget::Custom => None,
            SizeTarget::Telegram => Some(2000.0),
            SizeTarget::Email => Some(25.0),
            SizeTarget::Discord => Some(10.0),
        }
    }
}

pub struct ExportForm {
    format: VideoFormat,
    target: SizeTarget,
    custom_mb: f32,
    width: u32,
    fps: u32,
    mute: bool,
}

impl ExportForm {
    pub fn new(source: VideoFormat) -> Self {
        Self { format: source, target: SizeTarget::Unlimited, custom_mb: 8.0, width: 0, fps: 0, mute: false }
    }

    fn limit_bytes(&self) -> Option<u64> {
        let mb = if self.target == SizeTarget::Custom { Some(self.custom_mb) } else { self.target.preset_mb() }?;
        Some((mb.max(0.1) as f64 * MB as f64) as u64)
    }

    /// What `snap_record::edit::export` is asked to do. `estimate` is the expected size of a plain
    /// re-encode of the selection: an MP4 that already fits is not squeezed up to the limit.
    fn options(&self, trim: (u64, u64), duration_ms: u64, estimate: u64) -> ExportOptions {
        let max_bytes = self.limit_bytes().filter(|limit| self.format != VideoFormat::Mp4 || *limit < estimate + estimate / 10);
        ExportOptions {
            format: self.format,
            max_bytes,
            width: (self.width > 0).then_some(self.width),
            fps: (self.fps > 0).then_some(self.fps),
            range: (trim != (0, duration_ms)).then_some(trim),
            mute: self.mute && self.format == VideoFormat::Mp4,
        }
    }
}

/// A finished export.
pub struct ExportDone {
    pub recording: Recording,
    pub id: Option<CaptureId>,
}

pub enum ExportState {
    Idle,
    Running { since: Instant },
    Done(ExportDone),
    Failed(String),
}

pub enum UploadState {
    Idle,
    Running(f32),
    Done { url: String },
}

/// Run the export on the calling (worker) thread, then register the file in the history.
pub fn run_export(engine: &Engine, input: &Path, output: &Path, options: &ExportOptions, source: &VideoInfo) -> Result<ExportDone> {
    edit::export(input, output, options)?;
    // Not every platform can read back what it wrote (animated WebP, for one); the plan is then the answer.
    let info = edit::probe(output).unwrap_or_else(|_| {
        let width = options.width.map_or(source.width, |w| w.min(source.width));
        let duration_ms = options.range.map_or(source.duration_ms, |(start, end)| end - start);
        VideoInfo { duration_ms, width, height: source.height * width / source.width.max(1), ..source.clone() }
    });
    let recording = Recording {
        path: output.to_path_buf(),
        duration_ms: info.duration_ms,
        width: info.width,
        height: info.height,
        bytes: std::fs::metadata(output).map(|m| m.len()).unwrap_or(0),
        format: options.format,
    };
    let id =
        engine.add_recording(&recording, CaptureContext::default()).map_err(|e| tracing::warn!("export not added to history: {e}")).ok();
    Ok(ExportDone { recording, id })
}

/// Upload on the calling thread; `progress` is called with 0..1.
pub fn run_upload(engine: &Engine, done: &ExportDone, progress: &dyn Fn(f32)) -> Result<String> {
    let last = Cell::new(u32::MAX);
    let link = engine.upload_file(&done.recording.path, None, done.id.as_ref(), &|sent, total| {
        let percent = (sent * 100).checked_div(total).unwrap_or(0) as u32;
        if last.replace(percent) != percent {
            progress(percent as f32 / 100.0);
        }
    })?;
    let title = done.recording.path.file_stem().map(|s| s.to_string_lossy().into_owned());
    // The configured link format goes on the clipboard; a failure here is not worth losing the upload over.
    if let Err(e) = engine.copy_link(&link, None, title.as_deref()) {
        tracing::warn!("copy link: {e}");
        snap_platform::clipboard::set_text(link.best())?;
    }
    Ok(link.best().to_string())
}

/// `<name> edit.<ext>` next to the source, numbered when taken.
pub fn output_path(source: &Path, format: VideoFormat) -> PathBuf {
    let ext = match format {
        VideoFormat::Mp4 => "mp4",
        VideoFormat::Gif => "gif",
        VideoFormat::Webp => "webp",
    };
    let dir = source.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "video".into());
    let mut path = dir.join(format!("{stem} edit.{ext}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem} edit {n}.{ext}"));
        n += 1;
    }
    path
}

pub fn format_bytes(bytes: u64) -> String {
    match bytes {
        b if b >= MB * 1000 => format!("{:.1} GB", b as f64 / (MB * 1000) as f64),
        b if b >= MB => format!("{:.1} MB", b as f64 / MB as f64),
        b => format!("{} KB", b.div_ceil(1000)),
    }
}

impl Editor {
    pub(super) fn export_panel(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
        ui.label(text::title(t("video.export.title")));
        ui.label(text::caption(self.summary()));
        ui.add_space(8.0);
        self.form_fields(ui);
        ui.add_space(4.0);
        let running = matches!(self.export, ExportState::Running { .. });
        ui.add_enabled_ui(!running, |ui| {
            let label = t("video.export.run");
            let run = Button::new(&label).kind(ButtonKind::Primary).icon(icon::EXPORT).min_width(ui.available_width());
            if ui.add(run).clicked() {
                self.start_export(cx);
            }
        });
        ui.add_space(4.0);
        match &self.export {
            ExportState::Idle => {}
            ExportState::Running { since } => {
                ui.horizontal(|ui| {
                    spinner(ui);
                    ui.label(text::body_strong(t("video.export.running")));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(text::mono_text(clock(since.elapsed().as_secs()))));
                });
                progress_indeterminate(ui);
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
            }
            ExportState::Failed(message) => {
                let p = palette(ui.ctx());
                ui.label(text::body_strong(format!("{}  {}", icon::X_CIRCLE, t("video.export.failed"))).color(p.danger));
                ui.add(egui::Label::new(text::dim(message.clone())).wrap());
            }
            ExportState::Done(_) => self.result_card(ui, cx),
        }
    }

    fn summary(&self) -> String {
        let info: &VideoInfo = &self.info;
        let audio = if info.has_audio { t("video.export.with_audio") } else { t("video.export.no_audio") };
        format!("{}×{} · {:.0} fps · {}", info.width, info.height, info.fps, audio)
    }

    fn form_fields(&mut self, ui: &mut Ui) {
        let form = &mut self.form;
        field(ui, &t("video.export.format"), |ui| {
            segmented(ui, &mut form.format, &[(VideoFormat::Mp4, "MP4"), (VideoFormat::Gif, "GIF"), (VideoFormat::Webp, "WebP")]);
        });
        field(ui, &t("video.export.size_limit"), |ui| {
            ui.spacing_mut().combo_width = ui.available_width();
            let labels = [
                (SizeTarget::Unlimited, t("video.export.no_limit").into_owned()),
                (SizeTarget::Telegram, t("video.export.telegram").into_owned()),
                (SizeTarget::Email, t("video.export.email").into_owned()),
                (SizeTarget::Discord, t("video.export.discord").into_owned()),
                (SizeTarget::Custom, t("video.export.custom").into_owned()),
            ];
            let options: Vec<(SizeTarget, &str)> = labels.iter().map(|(v, l)| (*v, l.as_str())).collect();
            dropdown(ui, "video-size-limit", &mut form.target, &options);
            if form.target == SizeTarget::Custom {
                number_field(ui, &mut form.custom_mb, 0.5..=4000.0, " MB");
            }
        });
        field(ui, &t("video.export.width"), |ui| {
            ui.spacing_mut().combo_width = ui.available_width();
            let labels: Vec<(u32, String)> =
                WIDTHS.iter().map(|&w| (w, if w == 0 { t("video.export.original").into_owned() } else { format!("{w} px") })).collect();
            let options: Vec<(u32, &str)> = labels.iter().map(|(v, l)| (*v, l.as_str())).collect();
            dropdown(ui, "video-width", &mut form.width, &options);
        });
        field(ui, &t("video.export.fps"), |ui| {
            ui.spacing_mut().combo_width = ui.available_width();
            let labels: Vec<(u32, String)> = FRAME_RATES
                .iter()
                .map(|&f| (f, if f == 0 { t("video.export.original").into_owned() } else { format!("{f} fps") }))
                .collect();
            let options: Vec<(u32, &str)> = labels.iter().map(|(v, l)| (*v, l.as_str())).collect();
            dropdown(ui, "video-fps", &mut form.fps, &options);
        });
        if self.info.has_audio && form.format == VideoFormat::Mp4 {
            ui.horizontal(|ui| {
                ui.label(t("video.export.mute"));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    toggle(ui, &mut form.mute);
                });
            });
        }
    }

    fn start_export(&mut self, cx: &mut ViewCx) {
        let (start, end) = self.trim;
        let kept = (end - start) as f64 / self.info.duration_ms.max(1) as f64;
        let source_bytes = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        let options = self.form.options(self.trim, self.info.duration_ms, (source_bytes as f64 * kept) as u64);
        let output = output_path(&self.path, options.format);
        let (engine, input) = (cx.engine.clone(), self.path.clone());
        self.export = ExportState::Running { since: Instant::now() };
        self.upload = UploadState::Idle;
        let source = self.info.clone();
        cx.spawn(move || Msg::Exported(run_export(&engine, &input, &output, &options, &source)));
    }

    /// File name, size and the actions that follow an export.
    fn result_card(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        let ExportState::Done(done) = &self.export else { return };
        let p = palette(ui.ctx());
        let path = done.recording.path.clone();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let facts = format!(
            "{} · {} · {}",
            format_bytes(done.recording.bytes),
            timecode(done.recording.duration_ms),
            format_label(done.recording.format)
        );
        card_frame(ui, 12).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(text::body_strong(icon::CHECK_CIRCLE).color(p.success));
                ui.add(egui::Label::new(text::body_strong(name)).truncate());
            });
            ui.label(text::mono_text(facts).color(p.text_dim));
            ui.add_space(4.0);
            let (copy, show) = (t("video.result.copy_file"), t("video.result.show"));
            if ui.add(Button::new(&copy).icon(icon::COPY).min_width(ui.available_width())).clicked() {
                match snap_platform::clipboard::set_files(std::slice::from_ref(&path)) {
                    Ok(()) => cx.toast(ToastKind::Success, t("video.result.file_copied")),
                    Err(e) => cx.toast_error(&e),
                }
            }
            if ui.add(Button::new(&show).icon(icon::FOLDER_OPEN).min_width(ui.available_width())).clicked() {
                cx.push(Command::Reveal(path.clone()));
            }
            self.upload_row(ui, cx);
        });
    }

    fn upload_row(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        match &self.upload {
            UploadState::Idle => {
                let label = t("video.result.upload");
                let upload = Button::new(&label).icon(icon::CLOUD_ARROW_UP).min_width(ui.available_width());
                if ui.add(upload).clicked()
                    && let ExportState::Done(done) = &self.export
                {
                    self.upload = UploadState::Running(0.0);
                    let (engine, handle) = (cx.engine.clone(), cx.handle());
                    let (recording, id) = (done.recording.clone(), done.id.clone());
                    cx.spawn(move || {
                        let done = ExportDone { recording, id };
                        Msg::Uploaded(run_upload(&engine, &done, &|fraction| handle.send(Msg::UploadProgress(fraction))))
                    });
                }
            }
            UploadState::Running(fraction) => {
                ui.horizontal(|ui| {
                    ui.label(text::body_strong(t("video.result.uploading")));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(text::mono_text(format!("{:.0}%", fraction * 100.0)))
                    });
                });
                progress_sized(ui, *fraction, ui.available_width());
            }
            UploadState::Done { url } => {
                let p = palette(ui.ctx());
                Frame::new()
                    .fill(p.surface_2)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(radius::CONTROL)
                    .inner_margin(Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.add(egui::Label::new(text::mono_text(url.clone())).truncate());
                    });
                let copy = ui.add(Button::new(&t("video.result.copy_link")).icon(icon::LINK).min_width(ui.available_width()));
                if tooltip(copy, &t("video.result.copy_link_tip"), None).clicked() {
                    cx.push(Command::CopyText(url.clone()));
                    cx.toast(ToastKind::Success, t("video.result.link_copied"));
                }
            }
        }
    }

    /// Apply the result of the export job.
    pub(super) fn exported(&mut self, cx: &mut ViewCx, result: Result<ExportDone>, focused: bool) {
        match result {
            Ok(done) => {
                if !focused {
                    cx.notify(Notice {
                        kind: NoticeKind::Success,
                        title: t("video.export.done").into_owned(),
                        body: done.recording.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                        file: Some(done.recording.path.clone()),
                        ..Notice::default()
                    });
                }
                self.export = ExportState::Done(done);
            }
            Err(e) => self.export = ExportState::Failed(crate::notices::error_text(&e)),
        }
    }

    pub(super) fn uploaded(&mut self, cx: &mut ViewCx, result: Result<String>) {
        match result {
            Ok(url) => {
                cx.toast(ToastKind::Success, t("video.result.link_copied"));
                self.upload = UploadState::Done { url };
            }
            Err(e) => {
                self.upload = UploadState::Idle;
                cx.toast_error(&e);
            }
        }
    }
}

/// Caption above a control.
fn field(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.label(text::caption(label));
        add(ui);
    });
}

fn clock(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

fn format_label(format: VideoFormat) -> &'static str {
    match format {
        VideoFormat::Mp4 => "MP4",
        VideoFormat::Gif => "GIF",
        VideoFormat::Webp => "WebP",
    }
}
