//! What the primary actions do: render on a worker, hand the result to the clipboard, the engine's pipeline
//! (save, upload: recorded in the library with their editable project), the pin window or the waiting pipeline.

use std::path::{Path, PathBuf};

use snap_annotate::{Document, save_project};
use snap_core::{Capture, CaptureId, Error, Result, RgbaImage, Settings, naming, pipeline::Step, settings::ImageFormat, t, tf};
use snap_engine::{Annotated, Engine, Notice, NoticeKind, PipelineUi, SaveOptions};
use snap_image::encode;
use snap_ui::{Toast, ToastKind};

use super::{
    EditorView,
    output::{ExportParams, Msg, Saved, export_scale, render_export},
};
use crate::{
    commands::Command,
    views::{ViewCx, ViewHandle, settings::SettingsPage},
};

/// The engine's hooks while the editor runs a pipeline for the user: the document is already finished, so
/// `Annotate` just hands it over (which is what links the editable project to the library entry) and the
/// progress of long steps comes back as a toast.
struct OutputUi {
    handle: ViewHandle,
    document: Document,
}

impl PipelineUi for OutputUi {
    fn annotate(&self, cap: &Capture) -> Result<Option<Annotated>> {
        Ok(Some(Annotated { image: cap.image.clone(), document: Some(self.document.clone()) }))
    }

    fn pin(&self, _img: &RgbaImage, _cap: &Capture) {}

    fn notify(&self, _notice: &Notice) {}

    fn progress(&self, label: &str, frac: Option<f32>) {
        self.handle.send(Msg::Progress { label: label.to_owned(), frac });
    }
}

const SAVE: Step = Step::Save { dir: None, template: None, format: None };

/// Run `steps` over the finished picture as a new capture of its own (a new library entry).
fn run(
    engine: &Engine,
    handle: &ViewHandle,
    doc: &Document,
    meta: &Capture,
    image: RgbaImage,
    steps: &[Step],
    save: &SaveOptions,
) -> Result<snap_engine::Outcome> {
    let mut capture = meta.clone();
    capture.id = CaptureId::new();
    // The picture is the export, perhaps shrunk to the maximum width: copies and files record its own density.
    capture.scale_factor = export_scale(doc, &image);
    // The project remembers which library entry it belongs to, so that opening it again finds the entry.
    let mut document = doc.clone();
    document.meta.capture_id = Some(capture.id.clone());
    let ui = OutputUi { handle: handle.clone(), document };
    capture.image = image;
    let steps: Vec<Step> = std::iter::once(Step::Annotate).chain(steps.iter().cloned()).collect();
    engine.run_steps_with_save(capture, &steps, save, &ui)
}

impl EditorView {
    fn params(cx: &ViewCx) -> ExportParams {
        ExportParams::from_settings(cx.settings)
    }

    /// Everything that finishes the user's work counts as "saved" for the unsaved-changes guard.
    pub(super) fn mark_exported(&mut self) {
        self.saved_rev = self.export_rev;
    }

    pub(super) fn copy(&mut self, cx: &mut ViewCx) {
        self.canvas.commit_text(&mut self.s);
        if self.working.copy {
            return;
        }
        self.working.copy = true;
        self.export_rev = self.s.rev;
        let (doc, params, engine) = (self.s.doc.clone(), Self::params(cx), cx.engine.clone());
        cx.spawn(move || {
            let image = render_export(&doc, &params);
            Msg::Copied(engine.copy_image(&image, export_scale(&doc, &image)))
        });
    }

    /// Save: the first time as a new library entry (picture plus editable project) through the engine's naming and
    /// folders; afterwards in place, so the file, the project and the library entry stay one thing.
    pub(super) fn save(&mut self, cx: &mut ViewCx) {
        self.canvas.commit_text(&mut self.s);
        if self.working.save {
            return;
        }
        self.working.save = true;
        self.export_rev = self.s.rev;
        let (doc, params, engine, meta) = (self.s.doc.clone(), Self::params(cx), cx.engine.clone(), self.meta.clone());
        let (project, file) = (self.project.clone(), self.output_file.clone());
        let handle = cx.handle();
        cx.spawn(move || {
            let image = render_export(&doc, &params);
            Msg::Saved(match project {
                Some(project) => save_in_place(&engine, &doc, image, &params, &meta, project, file),
                None => run(&engine, &handle, &doc, &meta, image, &[SAVE], &SaveOptions::default()).map(|o| Saved {
                    id: o.id.unwrap_or(meta.id.clone()),
                    file: o.file,
                    project: o.project,
                }),
            })
        });
    }

    /// Ask where to save, as a project (`.snap`) or an image, and write it.
    pub(super) fn save_as(&mut self, cx: &mut ViewCx) {
        self.canvas.commit_text(&mut self.s);
        if self.working.save {
            return;
        }
        self.working.save = true;
        self.export_rev = self.s.rev;
        let (doc, params, engine, meta) = (self.s.doc.clone(), Self::params(cx), cx.engine.clone(), self.meta.clone());
        let settings: Settings = cx.settings.clone();
        let start = self.output_file.clone().or_else(|| self.project.clone());
        let handle = cx.handle();
        cx.spawn(move || Msg::SavedAs(pick_and_write(&engine, &handle, &doc, &settings, &params, &meta, start)));
    }

    pub(super) fn upload(&mut self, cx: &mut ViewCx) {
        self.canvas.commit_text(&mut self.s);
        if self.working.upload {
            return;
        }
        let s = cx.settings;
        if s.destinations.is_empty() || (s.default_destination.is_none() && s.destinations.len() != 1) {
            cx.toast_action(
                ToastKind::Warning,
                t("editor.upload_no_destination"),
                t("editor.upload_set_up"),
                Command::OpenSettings { page: Some(SettingsPage::Destinations) },
            );
            return;
        }
        self.working.upload = true;
        self.export_rev = self.s.rev;
        let (doc, params, engine, meta) = (self.s.doc.clone(), Self::params(cx), cx.engine.clone(), self.meta.clone());
        let handle = cx.handle();
        // Right after a save the file is the picture: send it and add the link to the entry it belongs to.
        let saved_file = self.output_file.clone().filter(|f| self.output_rev == self.s.rev && f.exists());
        cx.spawn(move || {
            if let Some(file) = saved_file {
                let progress = |sent: u64, total: u64| {
                    handle.send(Msg::Progress { label: t("editor.uploading").into_owned(), frac: Some(sent as f32 / total.max(1) as f32) })
                };
                let result = engine.upload_file(&file, None, Some(&meta.id), &progress).inspect(|link| {
                    if let Err(e) = engine.copy_link(link, None, meta.context.window_title.as_deref()) {
                        tracing::warn!("could not copy the link: {e}");
                    }
                });
                return Msg::Uploaded(result);
            }
            let steps = [Step::Upload { destination: None }, Step::CopyLink { format: None }];
            let outcome = run(&engine, &handle, &doc, &meta, render_export(&doc, &params), &steps, &SaveOptions::default());
            Msg::Uploaded(outcome.and_then(|o| o.link.ok_or_else(|| Error::invalid(t("editor.upload_no_link")))))
        });
    }

    pub(super) fn pin(&mut self, cx: &mut ViewCx) {
        self.canvas.commit_text(&mut self.s);
        let (doc, params) = (self.s.doc.clone(), Self::params(cx));
        cx.spawn(move || Msg::Pinned(render_export(&doc, &params)));
    }

    /// Hand the result to the waiting pipeline and close.
    pub(super) fn done(&mut self, cx: &mut ViewCx) {
        self.canvas.commit_text(&mut self.s);
        if self.working.done {
            return;
        }
        self.working.done = true;
        self.export_rev = self.s.rev;
        let (doc, params) = (self.s.doc.clone(), Self::params(cx));
        cx.spawn(move || Msg::Finished(render_export(&doc, &params)));
    }

    pub(super) fn paste_image(&mut self, cx: &mut ViewCx) {
        match snap_platform::clipboard::get_image() {
            Some(image) => self.add_pasted_image(image),
            None => cx.toast(ToastKind::Info, t("editor.paste.no_image")),
        }
    }

    pub(super) fn add_pasted_image(&mut self, image: image::RgbaImage) {
        self.canvas.commit_text(&mut self.s);
        let (w, h) = self.s.doc.canvas_size();
        let around = self.visible_center();
        self.s.add_image(image, around, (w as f32 * 0.6, h as f32 * 0.6), snap_annotate::Style::default());
    }

    /// Show or advance the progress toast of a long step; an empty label ends it.
    fn progress(&mut self, cx: &mut ViewCx, label: &str, frac: Option<f32>) {
        if label.is_empty() {
            if let Some(id) = self.progress_toast.take() {
                cx.toasts.dismiss(id);
            }
            return;
        }
        let mut toast = Toast::new(ToastKind::Info, label).lifetime(std::time::Duration::from_secs(30));
        if let Some(frac) = frac {
            toast = toast.progress(frac);
        }
        match self.progress_toast {
            Some(id) if cx.toasts.update(id, toast.clone()) => {}
            _ => self.progress_toast = Some(cx.toasts.push_toast(toast)),
        }
    }

    /// An output finished: the progress toast gives way to the result.
    fn settle(&mut self, cx: &mut ViewCx) {
        self.progress(cx, "", None);
    }

    /// The answer to a finished worker.
    pub(super) fn handle(&mut self, cx: &mut ViewCx, msg: Msg) {
        match msg {
            Msg::Progress { label, frac } => self.progress(cx, &label, frac),
            Msg::Copied(result) => {
                self.working.copy = false;
                match result {
                    Ok(()) => {
                        self.mark_exported();
                        cx.toast(ToastKind::Success, t("common.copied"));
                    }
                    Err(e) => cx.toast_error(&e),
                }
            }
            Msg::Saved(result) => {
                self.working.save = false;
                self.settle(cx);
                match result {
                    Ok(saved) => self.saved(cx, saved),
                    Err(e) => cx.toast_error(&e),
                }
            }
            Msg::SavedAs(result) => {
                self.working.save = false;
                self.settle(cx);
                match result {
                    Ok(Some(saved)) => self.saved(cx, saved),
                    Ok(None) => {}
                    Err(e) => cx.toast_error(&e),
                }
            }
            Msg::Uploaded(result) => {
                self.working.upload = false;
                self.settle(cx);
                match result {
                    Ok(link) => {
                        self.mark_exported();
                        cx.notify(Notice {
                            kind: NoticeKind::Success,
                            title: t("editor.uploaded").into_owned(),
                            body: link.best().to_string(),
                            url: Some(link.best().to_string()),
                            ..Notice::default()
                        });
                    }
                    Err(e) => cx.toast_error(&e),
                }
            }
            Msg::Pinned(image) => {
                let exact = self.s.doc.crop.is_none()
                    && self.s.doc.beautify.is_none()
                    && image.dimensions() == self.s.doc.canvas_size()
                    && !self.meta.screen_rect.is_empty();
                let origin = exact.then_some(self.meta.screen_rect);
                cx.push(Command::OpenPin { image, origin });
            }
            Msg::Finished(image) => {
                self.working.done = false;
                self.mark_exported();
                self.finish(Some(Annotated { image, document: Some(self.s.doc.clone()) }));
                self.closing = true;
                cx.close();
            }
            Msg::Estimate(estimate) => self.previews.on_estimate(estimate),
            Msg::Thumbs(images) => self.previews.on_thumbs(images),
            Msg::Ocr(result) => self.panel.on_recognised(result, cx),
            Msg::Translated(result) => self.panel.on_translated(result, cx),
            Msg::AltText(result) => self.panel.on_alt_text(result, cx),
            Msg::Redactions(result) => self.add_redactions(cx, result),
        }
    }

    /// Remember what a save wrote and say so.
    fn saved(&mut self, cx: &mut ViewCx, saved: Saved) {
        self.mark_exported();
        self.meta.id = saved.id;
        if saved.project.is_some() {
            self.project = saved.project.clone();
        }
        if saved.file.is_some() {
            self.output_file = saved.file.clone();
            self.output_rev = self.export_rev;
        }
        let shown = saved.file.as_ref().or(saved.project.as_ref()).cloned();
        let label = if saved.file.is_some() { t("editor.saved") } else { t("editor.project_saved") };
        match shown {
            Some(path) => cx.toast_action(ToastKind::Success, label, t("app.notice.show_in_folder"), Command::Reveal(path)),
            None => cx.toast(ToastKind::Success, label),
        }
    }

    /// Cover what the engine found with redact shapes the user can review, move and delete.
    fn add_redactions(&mut self, cx: &mut ViewCx, result: Result<Vec<snap_engine::Hit>>) {
        self.searching = false;
        let hits = match result {
            Ok(hits) => hits,
            Err(e) => {
                cx.toast_error(&e);
                return;
            }
        };
        let existing: Vec<snap_core::RectF> = self
            .s
            .doc
            .shapes
            .iter()
            .filter_map(|s| match s.kind {
                snap_annotate::ShapeKind::Redact { rect, .. } => Some(rect),
                _ => None,
            })
            .collect();
        let fresh: Vec<&snap_engine::Hit> = hits.iter().filter(|h| !existing.iter().any(|r| overlap(*r, h.rect) > 0.5)).collect();
        if fresh.is_empty() {
            cx.toast(ToastKind::Info, t("editor.redact.none"));
            return;
        }
        self.s.checkpoint();
        let style = self.tools.styles.get(&super::tools::Tool::Redact).copied().unwrap_or_default();
        let kind = self.tools.extras.redact;
        let ids: Vec<u64> =
            fresh.iter().map(|h| self.s.add(snap_annotate::ShapeKind::Redact { rect: h.rect, style: kind }, style)).collect();
        let count = ids.len().to_string();
        self.s.selection = ids;
        cx.toast(ToastKind::Success, tf("editor.redact.found", &[("n", &count)]));
    }

    /// `smart` also sends the recognised text to the AI endpoint, to find what no pattern matches.
    pub(super) fn auto_redact(&mut self, cx: &mut ViewCx, smart: bool) {
        if self.searching {
            return;
        }
        self.searching = true;
        let (engine, base) = (cx.engine.clone(), self.s.doc.base.clone());
        cx.spawn(move || {
            Msg::Redactions(if smart { engine.find_redactions_smart(&base, &[]) } else { engine.find_redactions(&base, &[]) })
        });
    }
}

/// Share of the smaller rectangle covered by the other.
fn overlap(a: snap_core::RectF, b: snap_core::RectF) -> f32 {
    let w = (a.right().min(b.right()) - a.x.max(b.x)).max(0.0);
    let h = (a.bottom().min(b.bottom()) - a.y.max(b.y)).max(0.0);
    let smaller = (a.w * a.h).min(b.w * b.h).max(1.0);
    w * h / smaller
}

/// Quality setting for a file format, as the capture settings define it.
fn quality(settings: &Settings, format: ImageFormat) -> u8 {
    match format {
        ImageFormat::Jpeg => settings.capture.jpeg_quality,
        ImageFormat::Webp => settings.capture.webp_quality,
        ImageFormat::Png => 100,
    }
}

/// Write the project again and bring the picture it was saved with (and its library thumbnail) up to date.
fn save_in_place(
    engine: &Engine,
    doc: &Document,
    image: RgbaImage,
    params: &ExportParams,
    meta: &Capture,
    project: PathBuf,
    known_file: Option<PathBuf>,
) -> Result<Saved> {
    save_project(doc, &project)?;
    let file = known_file.filter(|f| f.exists());
    if let Some(file) = &file {
        let format = encode::format_from_path(file).unwrap_or(params.format);
        encode::save(&image, file, format, quality(&engine.settings(), format), params.optimize_png, Some(export_scale(doc, &image)))?;
        if let Err(e) = snap_history::write_thumbnail(&image, &meta.id) {
            tracing::warn!("could not refresh the library thumbnail: {e}");
        }
    }
    Ok(Saved { id: meta.id.clone(), file, project: Some(project) })
}

/// The Save as dialog and the write behind it. `Ok(None)` when the dialog was dismissed.
fn pick_and_write(
    engine: &Engine,
    handle: &ViewHandle,
    doc: &Document,
    settings: &Settings,
    params: &ExportParams,
    meta: &Capture,
    current: Option<PathBuf>,
) -> Result<Option<Saved>> {
    let dir = current
        .as_deref()
        .and_then(Path::parent)
        .map(PathBuf::from)
        .unwrap_or_else(|| snap_core::paths::save_dir(&settings.capture.save_dir));
    let stem = current
        .as_deref()
        .and_then(|p| p.file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| naming::expand(&settings.capture.filename_template, &naming::NameContext::for_capture(meta, 1)));
    // The first filter is what the dialog starts with: the project when that is what was last saved, else the picture format.
    let is_project = current.as_deref().and_then(Path::extension).is_some_and(|e| e.eq_ignore_ascii_case(snap_core::PROJECT_EXT));
    let mut filters =
        vec![(t("editor.dialog.png"), vec!["png"]), (t("editor.dialog.jpg"), vec!["jpg", "jpeg"]), (t("editor.dialog.webp"), vec!["webp"])];
    let preferred = filters.iter().position(|(_, ext)| ext.contains(&params.format.ext())).unwrap_or(0);
    let first = filters.remove(preferred);
    filters.insert(0, first);
    filters.insert(if is_project { 0 } else { filters.len() }, (t("editor.dialog.project"), vec![snap_core::PROJECT_EXT]));
    let extension = if is_project { snap_core::PROJECT_EXT } else { params.format.ext() };
    let mut dialog =
        rfd::FileDialog::new().set_title(t("editor.dialog.title")).set_directory(dir).set_file_name(format!("{stem}.{extension}"));
    for (name, extensions) in &filters {
        dialog = dialog.add_filter(name.as_ref(), extensions);
    }
    let Some(path) = dialog.save_file() else { return Ok(None) };
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case(snap_core::PROJECT_EXT)) {
        save_project(doc, &path)?;
        return Ok(Some(Saved { id: meta.id.clone(), file: None, project: Some(path) }));
    }
    let save = SaveOptions { path: Some(path), format: Some(params.format), ..SaveOptions::default() };
    let outcome = run(engine, handle, doc, meta, render_export(doc, params), &[SAVE], &save)?;
    Ok(Some(Saved { id: outcome.id.unwrap_or_else(|| meta.id.clone()), file: outcome.file, project: outcome.project }))
}
