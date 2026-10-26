//! The pipeline executor: runs [`Step`]s in order over a capture or an existing file, threading the current image,
//! saved file, link and text through, and records the result in history.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use snap_core::{
    Capture, CaptureId, CaptureKind, Error, Result, RgbaImage, Settings, paths,
    pipeline::Step,
    settings::{ImageFormat, RedactKind, RedactStyle},
    t, tf,
};
use snap_history::Entry;

use crate::{Annotated, Code, Document, Engine, Link, Notice, NoticeKind, Outcome, PipelineUi, SaveOptions, recording, redact, share};

pub(crate) fn run_capture(engine: &Engine, cap: Capture, steps: &[Step], ui: &dyn PipelineUi, save: &SaveOptions) -> Result<Outcome> {
    let mut run = Run::new(engine, ui, cap);
    run.save_defaults = save.clone();
    run.execute(steps)
}

pub(crate) fn run_file(engine: &Engine, path: &Path, kind: CaptureKind, steps: &[Step], ui: &dyn PipelineUi) -> Result<Outcome> {
    let meta = std::fs::metadata(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => Error::NotFound(path.display().to_string()),
        _ => Error::Io(e),
    })?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
    let image = (kind != CaptureKind::Recording).then(|| snap_image::encode::load(path).ok()).flatten();
    let still = image.is_some();
    let mut cap = Capture::from_image(kind, image.unwrap_or_else(|| RgbaImage::new(1, 1)));
    cap.context.window_title = name;
    let mut run = Run::new(engine, ui, cap);
    run.file = Some(path.to_path_buf());
    run.file_bytes = meta.len();
    run.file_fresh = true;
    run.source_file = true;
    run.file_only = !still;
    if kind == CaptureKind::Recording {
        run.video = snap_record::edit::probe(path).map_err(|e| tracing::warn!("cannot probe {}: {e}", path.display())).ok();
    }
    run.execute(steps)
}

/// Things that happened during a run, summarised by the Notify step.
enum Done {
    ImageCopied,
    Saved(PathBuf),
    Uploaded(String),
    LinkCopied,
    TextCopied(usize),
    CodeCopied,
    NoText,
    NoCodes,
    Redacted(usize),
}

struct Run<'a> {
    engine: &'a Engine,
    ui: &'a dyn PipelineUi,
    settings: Settings,
    /// `cap.image` is the current image; steps replace it.
    cap: Capture,
    document: Option<Document>,
    project: Option<PathBuf>,
    file: Option<PathBuf>,
    file_bytes: u64,
    /// `file` holds exactly the current image, so an upload can send it as is.
    file_fresh: bool,
    /// `file` came from the caller and the image is untouched: Save has nothing to do.
    source_file: bool,
    /// The subject is not a still image (a recording): image steps are skipped.
    file_only: bool,
    video: Option<snap_record::edit::VideoInfo>,
    link: Option<Link>,
    /// OCR text of the current image.
    ocr_text: Option<String>,
    /// OCR text, or its translation after a Translate step.
    text: Option<String>,
    codes: Vec<Code>,
    alt_text: Option<String>,
    thumb: Option<PathBuf>,
    /// Scratch copy written for RunCommand when nothing was saved.
    scratch: Option<PathBuf>,
    /// Defaults for Save steps from the caller (CLI `--out`).
    save_defaults: SaveOptions,
    done: Vec<Done>,
    cancelled: bool,
}

impl<'a> Run<'a> {
    fn new(engine: &'a Engine, ui: &'a dyn PipelineUi, cap: Capture) -> Self {
        Self {
            engine,
            ui,
            settings: engine.settings(),
            cap,
            document: None,
            project: None,
            file: None,
            file_bytes: 0,
            file_fresh: false,
            source_file: false,
            file_only: false,
            video: None,
            link: None,
            ocr_text: None,
            text: None,
            codes: Vec::new(),
            alt_text: None,
            thumb: None,
            scratch: None,
            save_defaults: SaveOptions::default(),
            done: Vec::new(),
            cancelled: false,
        }
    }

    fn execute(mut self, steps: &[Step]) -> Result<Outcome> {
        for step in steps {
            if self.cancelled {
                break;
            }
            match self.step(step) {
                Ok(()) => {}
                Err(Error::Cancelled) => self.cancelled = true,
                Err(e) => {
                    self.ui.progress("", None);
                    self.ui.notify(&Notice {
                        kind: NoticeKind::Error,
                        title: t("engine.notice.failed").into_owned(),
                        body: error_text(&e),
                        ..Notice::default()
                    });
                    // What already ran (a saved file, an edited document) still belongs in the library.
                    self.finish();
                    return Err(e);
                }
            }
        }
        self.ui.progress("", None);
        Ok(self.finish())
    }

    /// The image changed: whatever was derived from the old pixels is stale.
    fn touch(&mut self) {
        self.file_fresh = false;
        self.source_file = false;
        self.ocr_text = None;
        self.thumb = None;
    }

    fn step(&mut self, step: &Step) -> Result<()> {
        if self.file_only && skipped_for_files(step) {
            tracing::debug!("skipping {step:?}: the subject is not a still image");
            return Ok(());
        }
        match step {
            Step::Annotate => self.annotate(),
            Step::AutoRedact { kinds, style } => self.redact(kinds, *style),
            Step::Beautify { preset } => self.beautify(preset),
            Step::Resize { max_width, max_height, readable } => {
                let fitted = snap_image::resize::fit(&self.cap.image, *max_width, *max_height, *readable);
                if fitted.dimensions() != self.cap.image.dimensions() {
                    // Fewer pixels for the same on-screen size, down to one pixel per point: the clipboard and the
                    // editor keep showing it at the size it had.
                    let ratio = fitted.width() as f32 / self.cap.image.width() as f32;
                    self.cap.scale_factor = (self.cap.scale_factor * ratio).max(1.0);
                    self.cap.image = fitted;
                    self.touch();
                }
                Ok(())
            }
            Step::Ocr { copy, as_markdown_table } => self.ocr(*copy, *as_markdown_table),
            Step::DecodeCodes { copy } => self.decode_codes(*copy),
            Step::Translate { target_lang } => self.translate(target_lang.as_deref()),
            Step::Save { dir, template, format } => self.save(dir.as_deref(), template, *format),
            Step::CopyImage => {
                // The clipboard may be held by another process; that must not cost the user the later steps.
                match self.engine.copy_image(&self.cap.image, self.cap.scale_factor) {
                    Ok(()) => self.done.push(Done::ImageCopied),
                    Err(e) => {
                        tracing::warn!("cannot copy the image: {e}");
                        self.ui.notify(&Notice {
                            kind: NoticeKind::Warning,
                            title: t("engine.notice.copy_failed").into_owned(),
                            body: error_text(&e),
                            ..Notice::default()
                        });
                    }
                }
                Ok(())
            }
            Step::Upload { destination } => self.upload(destination.as_deref()),
            Step::Shorten => {
                let engine = self.engine;
                share::shorten_link(engine, self.link_mut()?)
            }
            Step::CopyLink { format } => {
                let link = self.link.as_ref().ok_or_else(|| Error::Invalid(t("engine.error.no_link").into_owned()))?;
                share::copy_link(self.engine, link, *format, self.cap.context.window_title.as_deref(), !self.file_only)?;
                self.done.push(Done::LinkCopied);
                Ok(())
            }
            Step::Pin => {
                self.ui.pin(&self.cap.image, &self.cap);
                Ok(())
            }
            Step::AltText { copy } => {
                self.ui.progress(&t("engine.progress.alt_text"), None);
                let alt = self.engine.alt_text(&self.cap.image, &self.cap.context)?;
                if *copy {
                    snap_platform::clipboard::set_text(&alt)?;
                    self.done.push(Done::TextCopied(alt.chars().count()));
                }
                self.alt_text = Some(alt);
                Ok(())
            }
            Step::RunCommand { program, args } => self.run_command(program, args),
            Step::Notify => {
                self.notify();
                Ok(())
            }
        }
    }

    fn link_mut(&mut self) -> Result<&mut Link> {
        self.link.as_mut().ok_or_else(|| Error::Invalid(t("engine.error.no_link").into_owned()))
    }

    fn annotate(&mut self) -> Result<()> {
        match self.ui.annotate(&self.cap)? {
            None => self.cancelled = true,
            Some(Annotated { image, document }) => {
                self.cap.image = image;
                self.document = document;
                self.touch();
            }
        }
        Ok(())
    }

    fn redact(&mut self, kinds: &[RedactKind], style: Option<RedactStyle>) -> Result<()> {
        self.ui.progress(&t("engine.progress.redacting"), None);
        let hits = redact::auto_redact(self.engine, &mut self.cap.image, kinds, style)?;
        if !hits.is_empty() {
            self.touch();
            self.done.push(Done::Redacted(hits.len()));
        }
        Ok(())
    }

    fn beautify(&mut self, name: &str) -> Result<()> {
        let preset = self.settings.beautify_preset(name).ok_or_else(|| Error::NotFound(tf("engine.error.no_preset", &[("name", name)])))?;
        self.cap.image = snap_image::beautify::apply_with(&self.cap.image, preset, &self.cap.context);
        self.touch();
        Ok(())
    }

    /// The pill while text is read; the first request after an install waits about half a minute for the system to
    /// prepare recognition, which must not look like a hang.
    fn ocr_progress(&self) {
        let label = if snap_ocr::warming_up(&self.settings.ocr) { "ocr.warmup.label" } else { "engine.progress.ocr" };
        self.ui.progress(&t(label), None);
    }

    fn ocr(&mut self, copy: bool, as_markdown_table: bool) -> Result<()> {
        self.ocr_progress();
        let result = self.engine.ocr(&self.cap.image)?;
        let plain = result.text();
        let text = if as_markdown_table { result.to_markdown_table().unwrap_or_else(|| plain.clone()) } else { plain.clone() };
        self.ocr_text = Some(plain);
        if text.trim().is_empty() {
            self.done.push(Done::NoText);
            return Ok(());
        }
        if copy {
            snap_platform::clipboard::set_text(&text)?;
            self.done.push(Done::TextCopied(text.chars().count()));
        } else {
            self.ui.show_text(&t("engine.text.recognized"), &text);
        }
        self.text = Some(text);
        Ok(())
    }

    fn decode_codes(&mut self, copy: bool) -> Result<()> {
        self.codes = self.engine.decode_codes(&self.cap.image);
        let Some(first) = self.codes.first() else {
            self.done.push(Done::NoCodes);
            return Ok(());
        };
        if copy {
            snap_platform::clipboard::set_text(&first.text)?;
            self.done.push(Done::CodeCopied);
        } else {
            self.ui.show_codes(&t("engine.text.codes"), &self.codes);
        }
        Ok(())
    }

    fn translate(&mut self, target: Option<&str>) -> Result<()> {
        let source = match self.text.clone() {
            Some(text) => text,
            None => {
                self.ocr_progress();
                let text = self.engine.ocr(&self.cap.image)?.text();
                self.ocr_text = Some(text.clone());
                text
            }
        };
        if source.trim().is_empty() {
            self.done.push(Done::NoText);
            return Ok(());
        }
        self.ui.progress(&t("engine.progress.translating"), None);
        let translated = self.engine.translate(&source, target)?;
        self.ui.show_text(&t("engine.text.translated"), &translated);
        self.text = Some(translated);
        Ok(())
    }

    fn save(&mut self, dir: Option<&str>, template: &Option<String>, format: Option<ImageFormat>) -> Result<()> {
        if self.source_file || self.file_only {
            return Ok(());
        }
        let defaults = &self.save_defaults;
        let opts = SaveOptions {
            dir: dir.map(paths::save_dir).or_else(|| defaults.dir.clone()),
            template: template.clone().or_else(|| defaults.template.clone()),
            format: format.or(defaults.format),
            path: defaults.path.clone(),
        };
        let path = self.engine.save(&self.cap.image, &self.cap, &opts)?;
        self.file_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        self.file_fresh = true;
        self.done.push(Done::Saved(path.clone()));
        self.file = Some(path);
        Ok(())
    }

    fn upload(&mut self, destination: Option<&str>) -> Result<()> {
        // `share` enforces the redaction and metadata policy on every upload, whoever calls it.
        let label = t("engine.progress.uploading");
        let ui = self.ui;
        let progress =
            |sent: u64, total: u64| ui.progress(&label, Some(if total == 0 { 0.0 } else { (sent as f32 / total as f32).min(1.0) }));
        ui.progress(&label, Some(0.0));
        let link = match (&self.file, self.file_fresh) {
            (Some(file), true) => share::upload_file(self.engine, file, destination, None, &progress, false)?,
            _ => share::upload_image(self.engine, &self.cap.image, &self.cap, destination, &progress, false)?,
        };
        self.done.push(Done::Uploaded(link.url.clone()));
        self.link = Some(link);
        Ok(())
    }

    fn run_command(&mut self, program: &str, args: &[String]) -> Result<()> {
        let needs_file = std::iter::once(program).chain(args.iter().map(String::as_str)).any(|a| a.contains("{file}"));
        let file = if needs_file { Some(self.file_for_command()?) } else { self.file.clone() };
        let url = self.link.as_ref().map(|l| l.best().to_string()).unwrap_or_default();
        let text = self.text.clone().unwrap_or_default();
        let fill = |s: &str| {
            s.replace("{file}", &file.as_ref().map(|f| f.display().to_string()).unwrap_or_default())
                .replace("{url}", &url)
                .replace("{text}", &text)
        };
        let mut child = Command::new(fill(program))
            .args(args.iter().map(|a| fill(a)))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                Error::Other(anyhow::anyhow!(tf("engine.error.command_failed", &[("program", program), ("error", &e.to_string())])))
            })?;
        // The program keeps running on its own; this only reaps it so no zombie is left behind.
        std::thread::spawn(move || child.wait());
        Ok(())
    }

    /// The saved file when it is current, otherwise a scratch copy of the current image.
    fn file_for_command(&mut self) -> Result<PathBuf> {
        if let (Some(file), true) = (&self.file, self.file_fresh) {
            return Ok(file.clone());
        }
        let dir = paths::cache_dir().join("run");
        let path = dir.join(format!("{}.png", self.cap.id));
        snap_image::encode::save(&self.cap.image, &path, ImageFormat::Png, 100, false)?;
        self.scratch = Some(path.clone());
        Ok(path)
    }

    fn notify(&mut self) {
        if !self.settings.general.show_notifications {
            return;
        }
        if self.done.is_empty() {
            return;
        }
        let nothing_found = self.done.iter().any(|d| matches!(d, Done::NoText | Done::NoCodes));
        let has = |f: fn(&Done) -> bool| self.done.iter().any(f);
        let title = if has(|d| matches!(d, Done::LinkCopied)) {
            t("engine.notice.link_copied")
        } else if has(|d| matches!(d, Done::Uploaded(_))) {
            t("engine.notice.uploaded")
        } else if has(|d| matches!(d, Done::TextCopied(_))) {
            t("engine.notice.text_copied")
        } else if has(|d| matches!(d, Done::CodeCopied)) {
            t("engine.notice.code_copied")
        } else if nothing_found {
            t("engine.notice.nothing_found")
        } else if has(|d| matches!(d, Done::Saved(_))) {
            t("engine.notice.saved")
        } else if has(|d| matches!(d, Done::ImageCopied)) {
            t("engine.notice.copied")
        } else {
            t("engine.notice.done")
        };
        let parts: Vec<String> = self
            .done
            .iter()
            .map(|d| match d {
                Done::ImageCopied => t("engine.notice.part.image_copied").into_owned(),
                Done::Saved(path) => tf(
                    "engine.notice.part.saved",
                    &[("name", &path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())],
                ),
                Done::Uploaded(url) => url.clone(),
                Done::LinkCopied => t("engine.notice.part.link_copied").into_owned(),
                Done::TextCopied(n) => tf("engine.notice.part.text_copied", &[("n", &n.to_string())]),
                Done::CodeCopied => t("engine.notice.part.code_copied").into_owned(),
                Done::NoText => t("engine.notice.part.no_text").into_owned(),
                Done::NoCodes => t("engine.notice.part.no_codes").into_owned(),
                Done::Redacted(n) => tf("engine.notice.part.redacted", &[("n", &n.to_string())]),
            })
            .collect();
        let image = self.preview_image();
        self.ui.notify(&Notice {
            kind: if nothing_found { NoticeKind::Warning } else { NoticeKind::Success },
            title: title.into_owned(),
            body: parts.join(" \u{00b7} "),
            image,
            url: self.link.as_ref().map(|l| l.best().to_string()),
            file: self.file.clone(),
            permission: None,
        });
    }

    /// Picture for OS notifications: the history thumbnail, or the saved file when it is a format toasts can show.
    fn preview_image(&mut self) -> Option<PathBuf> {
        if self.file_only {
            return None;
        }
        if self.history_enabled() {
            return self.thumbnail();
        }
        self.file.clone().filter(|f| matches!(snap_image::encode::format_from_path(f), Some(ImageFormat::Png | ImageFormat::Jpeg)))
    }

    fn thumbnail(&mut self) -> Option<PathBuf> {
        if self.thumb.is_none() {
            self.thumb = snap_history::write_thumbnail(&self.cap.image, &self.cap.id)
                .map_err(|e| tracing::warn!("cannot write thumbnail: {e}"))
                .ok();
        }
        self.thumb.clone()
    }

    fn history_enabled(&self) -> bool {
        self.settings.history.enabled && !self.engine.policy().disable_history && self.engine.history().is_some()
    }

    fn finish(mut self) -> Outcome {
        let mut outcome = Outcome {
            file: self.file.clone(),
            link: self.link.clone(),
            text: self.text.clone(),
            codes: std::mem::take(&mut self.codes),
            alt_text: self.alt_text.take(),
            cancelled: self.cancelled,
            ..Outcome::default()
        };
        if !self.cancelled {
            outcome.id = self.record_history();
            outcome.project = self.project.clone();
        }
        if let Some(scratch) = self.scratch.take() {
            let _ = std::fs::remove_file(scratch);
        }
        if outcome.id.is_some() && self.ocr_text.is_none() && self.settings.history.index_text && !self.file_only {
            self.index_text_in_background();
        }
        outcome
    }

    /// Add the run to history. `None` when history is off or the subject is not worth keeping.
    fn record_history(&mut self) -> Option<CaptureId> {
        if !self.history_enabled() {
            return None;
        }
        let history = self.engine.history()?;
        let id = self.cap.id.clone();
        let mut entry = if self.cap.kind == CaptureKind::Recording {
            let (duration, size) = self.video.as_ref().map_or((0, (0, 0)), |v| (v.duration_ms, (v.width, v.height)));
            let file = self.file.clone()?;
            recording::recording_entry(&id, &file, duration, size, self.file_bytes, self.cap.context.clone(), &self.settings)
        } else if self.file_only {
            return None;
        } else {
            let mut entry = Entry::from_capture(&self.cap, self.file.clone(), self.file_bytes);
            if !self.settings.privacy.record_context {
                entry.context = Default::default();
            }
            entry.thumb = self.thumbnail();
            entry.text = self.ocr_text.clone().or_else(|| self.text.clone());
            entry
        };
        if let Some(doc) = &self.document {
            match self.engine.save_project(doc, &id) {
                Ok(path) => self.project = Some(path),
                Err(e) => tracing::warn!("cannot save the editable project: {e}"),
            }
            entry.project = self.project.clone();
        }
        if let Some(link) = &self.link {
            let record = |url: &str| snap_history::LinkRecord {
                url: url.to_string(),
                destination: link.destination.clone(),
                delete_url: link.delete_url.clone(),
                created_ms: snap_core::now_ms(),
            };
            entry.links = std::iter::once(link.url.as_str()).chain(link.short_url.as_deref()).map(record).collect();
        }
        match history.add(&entry) {
            Ok(()) => {
                self.engine.history_changed();
                Some(id)
            }
            Err(e) => {
                tracing::warn!("cannot add the capture to history: {e}");
                None
            }
        }
    }

    /// OCR the final image off-thread and store the text for search.
    fn index_text_in_background(self) {
        let (engine, id, image) = (self.engine.clone(), self.cap.id, self.cap.image);
        let spawned = std::thread::Builder::new().name("snap-index-text".into()).spawn(move || {
            let Some(history) = engine.history() else { return };
            match engine.ocr(&image) {
                Ok(result) => {
                    let text = result.text();
                    if !text.trim().is_empty() {
                        match history.set_text(&id, &text) {
                            Ok(()) => engine.history_changed(),
                            Err(e) => tracing::debug!("cannot store indexed text: {e}"),
                        }
                    }
                }
                Err(e) => tracing::debug!("text indexing skipped: {e}"),
            }
        });
        if let Err(e) = spawned {
            tracing::warn!("cannot start text indexing: {e}");
        }
    }
}

/// Steps that make no sense for a subject that is not a still image.
fn skipped_for_files(step: &Step) -> bool {
    matches!(
        step,
        Step::Annotate
            | Step::AutoRedact { .. }
            | Step::Beautify { .. }
            | Step::Resize { .. }
            | Step::Ocr { .. }
            | Step::DecodeCodes { .. }
            | Step::Translate { .. }
            | Step::CopyImage
            | Step::Pin
            | Step::AltText { .. }
    )
}

/// A user-facing sentence for an error. Library errors carry their own localised message in the variant.
pub(crate) fn error_text(e: &Error) -> String {
    match e {
        Error::Cancelled => t("engine.error.cancelled").into_owned(),
        Error::Permission(what) => what.to_string(),
        Error::Unsupported(what) => tf("engine.error.unsupported", &[("what", what)]),
        Error::Network(what) => tf("engine.error.network", &[("what", what)]),
        Error::NotFound(m) | Error::Invalid(m) | Error::Policy(m) => m.clone(),
        Error::Io(e) => e.to_string(),
        Error::Image(e) => e.to_string(),
        Error::Other(e) => e.to_string(),
    }
}
