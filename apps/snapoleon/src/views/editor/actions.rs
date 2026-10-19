//! What the primary actions do: render on a worker, hand the result to the clipboard, disk, upload, pin or the pipeline.

use std::path::PathBuf;

use snap_annotate::save_project;
use snap_core::{Result, Settings, naming, t, tf};
use snap_engine::{Annotated, Notice, NoticeKind, SaveOptions};
use snap_image::encode;
use snap_ui::ToastKind;

use super::{
    EditorView,
    output::{ExportParams, Msg, render_export},
};
use crate::{commands::Command, views::ViewCx};

impl EditorView {
    fn params(cx: &ViewCx) -> ExportParams {
        ExportParams::from_settings(cx.settings)
    }

    /// Everything that finishes the user's work counts as "saved" for the unsaved-changes guard.
    pub(super) fn mark_exported(&mut self) {
        self.saved_rev = self.s.rev;
    }

    pub(super) fn copy(&mut self, cx: &mut ViewCx) {
        self.canvas.commit_text(&mut self.s);
        if self.working.copy {
            return;
        }
        self.working.copy = true;
        let (doc, params, engine) = (self.s.doc.clone(), Self::params(cx), cx.engine.clone());
        cx.spawn(move || Msg::Copied(engine.copy_image(&render_export(&doc, &params))));
    }

    /// Save the project when there is one, otherwise the image through the engine's naming and folders.
    pub(super) fn save(&mut self, cx: &mut ViewCx) {
        self.canvas.commit_text(&mut self.s);
        if self.working.save {
            return;
        }
        self.working.save = true;
        let doc = self.s.doc.clone();
        if let Some(path) = self.project.clone() {
            cx.spawn(move || Msg::ProjectSaved(save_project(&doc, &path).map(|()| path)));
            return;
        }
        let (params, engine, meta) = (Self::params(cx), cx.engine.clone(), self.meta.clone());
        cx.spawn(move || {
            let mut capture = meta;
            capture.image = render_export(&doc, &params);
            Msg::Saved(engine.save(&capture.image, &capture, &SaveOptions::default()))
        });
    }

    /// Ask where to save, as a project (`.snap`) or an image, and write it.
    pub(super) fn save_as(&mut self, cx: &mut ViewCx) {
        self.canvas.commit_text(&mut self.s);
        if self.working.save {
            return;
        }
        self.working.save = true;
        let doc = self.s.doc.clone();
        let settings: Settings = cx.settings.clone();
        let params = Self::params(cx);
        let meta = self.meta.clone();
        let start = self.project.clone();
        cx.spawn(move || Msg::SavedAs(pick_and_write(&doc, &settings, params, &meta, start)));
    }

    pub(super) fn upload(&mut self, cx: &mut ViewCx) {
        self.canvas.commit_text(&mut self.s);
        if self.working.upload {
            return;
        }
        self.working.upload = true;
        let (doc, params, engine, meta) = (self.s.doc.clone(), Self::params(cx), cx.engine.clone(), self.meta.clone());
        cx.spawn(move || {
            let mut capture = meta;
            capture.image = render_export(&doc, &params);
            let result = engine.upload_image(&capture.image, &capture, None, &|_, _| {});
            if let Ok(link) = &result
                && let Err(e) = engine.copy_link(link, None, capture.context.window_title.as_deref())
            {
                tracing::warn!("could not copy the link: {e}");
            }
            Msg::Uploaded(result)
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

    /// The answer to a finished worker.
    pub(super) fn handle(&mut self, cx: &mut ViewCx, msg: Msg) {
        match msg {
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
                match result {
                    Ok(path) => {
                        self.mark_exported();
                        cx.toast_action(ToastKind::Success, t("editor.saved"), t("app.notice.show_in_folder"), Command::Reveal(path));
                    }
                    Err(e) => cx.toast_error(&e),
                }
            }
            Msg::ProjectSaved(result) => {
                self.working.save = false;
                match result {
                    Ok(path) => {
                        self.mark_exported();
                        cx.toast_action(
                            ToastKind::Success,
                            t("editor.project_saved"),
                            t("app.notice.show_in_folder"),
                            Command::Reveal(path),
                        );
                    }
                    Err(e) => cx.toast_error(&e),
                }
            }
            Msg::SavedAs(result) => {
                self.working.save = false;
                match result {
                    Ok(Some((path, project))) => {
                        self.mark_exported();
                        if project {
                            self.project = Some(path.clone());
                        }
                        cx.toast_action(ToastKind::Success, t("editor.saved"), t("app.notice.show_in_folder"), Command::Reveal(path));
                    }
                    Ok(None) => {}
                    Err(e) => cx.toast_error(&e),
                }
            }
            Msg::Uploaded(result) => {
                self.working.upload = false;
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

    pub(super) fn auto_redact(&mut self, cx: &mut ViewCx) {
        if self.searching {
            return;
        }
        self.searching = true;
        let (engine, base) = (cx.engine.clone(), self.s.doc.base.clone());
        cx.spawn(move || Msg::Redactions(engine.find_redactions(&base, &[])));
    }
}

/// Share of the smaller rectangle covered by the other.
fn overlap(a: snap_core::RectF, b: snap_core::RectF) -> f32 {
    let w = (a.right().min(b.right()) - a.x.max(b.x)).max(0.0);
    let h = (a.bottom().min(b.bottom()) - a.y.max(b.y)).max(0.0);
    let smaller = (a.w * a.h).min(b.w * b.h).max(1.0);
    w * h / smaller
}

/// The Save as dialog and the write behind it. `Ok(None)` when the dialog was dismissed.
fn pick_and_write(
    doc: &snap_annotate::Document,
    settings: &Settings,
    params: ExportParams,
    meta: &snap_core::Capture,
    current: Option<PathBuf>,
) -> Result<Option<(PathBuf, bool)>> {
    let dir = current
        .as_deref()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_else(|| snap_core::paths::save_dir(&settings.capture.save_dir));
    let stem = current
        .as_deref()
        .and_then(|p| p.file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| naming::expand(&settings.capture.filename_template, &naming::NameContext::for_capture(meta, 1)));
    let dialog = rfd::FileDialog::new()
        .set_title(t("editor.dialog.title"))
        .set_directory(dir)
        .set_file_name(format!("{stem}.{}", params.format.ext()))
        .add_filter(t("editor.dialog.project"), &[snap_core::PROJECT_EXT])
        .add_filter(t("editor.dialog.png"), &["png"])
        .add_filter(t("editor.dialog.jpg"), &["jpg", "jpeg"])
        .add_filter(t("editor.dialog.webp"), &["webp"]);
    let Some(mut path) = dialog.save_file() else { return Ok(None) };
    let project = path.extension().is_some_and(|e| e.eq_ignore_ascii_case(snap_core::PROJECT_EXT));
    if project {
        save_project(doc, &path)?;
        return Ok(Some((path, true)));
    }
    let format = encode::format_from_path(&path).unwrap_or(params.format);
    if path.extension().is_none() {
        path.set_extension(format.ext());
    }
    let quality = match format {
        snap_core::settings::ImageFormat::Jpeg => settings.capture.jpeg_quality,
        snap_core::settings::ImageFormat::Webp => settings.capture.webp_quality,
        snap_core::settings::ImageFormat::Png => 100,
    };
    let image = render_export(doc, &params);
    encode::save(&image, &path, format, quality, settings.capture.optimize_png)?;
    Ok(Some((path, false)))
}
