//! What happens when you act on captures: the work (on threads) and what the view does with the answers.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use egui::ColorImage;
use snap_core::{CaptureKind, Error, t};
use snap_history::Entry;
use snap_ui::{Toast, ToastKind};

use super::{Act, LibraryView, Load, Msg, PREVIEW_EDGE, format, paint};
use crate::{commands::Command, views::ViewCx};

/// The result of a one-shot action.
pub enum Outcome {
    Toast(String),
    Commands(Vec<Command>),
}

pub struct Uploaded {
    /// The last link, for the toast's Open button.
    pub url: String,
    /// The uploaded captures re-read from history, which now list their link.
    pub entries: Vec<Entry>,
}

fn missing() -> Error {
    Error::NotFound(t("library.error.missing").into_owned())
}

fn existing(path: Option<&PathBuf>) -> Option<PathBuf> {
    path.filter(|p| p.exists()).cloned()
}

/// The file an editor should open: the editable project when it is still there, else the saved file.
fn editable(entry: &Entry) -> Result<PathBuf, Error> {
    existing(entry.project.as_ref()).or_else(|| existing(entry.file.as_ref())).ok_or_else(missing)
}

fn saved_file(entry: &Entry) -> Result<PathBuf, Error> {
    existing(entry.file.as_ref()).ok_or_else(missing)
}

/// Decode the capture (or a video's first frame) for the drawer and quick look.
pub fn load_preview(file: &Path, video: bool) -> Option<ColorImage> {
    let image = if video { snap_record::edit::frame_at(file, 0, PREVIEW_EDGE).ok()? } else { snap_image::encode::load(file).ok()? };
    let image = snap_image::resize::fit(&image, Some(PREVIEW_EDGE), Some(PREVIEW_EDGE), false);
    Some(ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], image.as_raw()))
}

impl LibraryView {
    /// Run `act` on `targets`. Actions that need files check them on the worker and answer with a toast.
    pub(super) fn run(&mut self, cx: &mut ViewCx, act: Act, targets: Vec<Entry>) {
        let Some(first) = targets.first().cloned() else { return };
        match act {
            Act::Edit => cx.spawn(move || {
                Msg::Done((|| {
                    let path = editable(&first)?;
                    let command =
                        if first.kind == CaptureKind::Recording { Command::OpenVideoEditor { path } } else { Command::EditFile(path) };
                    Ok(Outcome::Commands(vec![command]))
                })())
            }),
            Act::Pin => {
                let pinnable: Vec<Entry> = targets.into_iter().filter(|e| e.kind != CaptureKind::Recording).collect();
                cx.spawn(move || {
                    Msg::Done((|| {
                        let files = pinnable.iter().map(saved_file).collect::<Result<Vec<_>, _>>()?;
                        Ok(Outcome::Commands(files.into_iter().map(Command::PinFile).collect()))
                    })())
                });
            }
            Act::Copy => cx.spawn(move || {
                Msg::Done((|| {
                    // One image goes on the clipboard as pixels; videos and several captures as files.
                    if let [only] = targets.as_slice()
                        && only.kind != CaptureKind::Recording
                    {
                        snap_platform::clipboard::set_image(&snap_image::encode::load(&saved_file(only)?)?)?;
                    } else {
                        copy_files(&targets)?;
                    }
                    Ok(Outcome::Toast(t("common.copied").into_owned()))
                })())
            }),
            Act::CopyFile => cx.spawn(move || {
                Msg::Done((|| {
                    copy_files(&targets)?;
                    Ok(Outcome::Toast(t("common.copied").into_owned()))
                })())
            }),
            Act::CopyPath => {
                let paths: Vec<String> = targets.iter().filter_map(|e| e.file.as_ref()).map(|f| f.display().to_string()).collect();
                if paths.is_empty() {
                    cx.toast(ToastKind::Error, t("library.error.missing"));
                } else {
                    cx.push(Command::CopyText(paths.join("\n")));
                    cx.toast(ToastKind::Success, t("common.copied"));
                }
            }
            Act::Reveal => match first.file.clone().or(first.project.clone()) {
                Some(path) => cx.push(Command::Reveal(path)),
                None => cx.toast(ToastKind::Error, t("library.error.missing")),
            },
            Act::Favorite => {
                let favorite = !targets.iter().all(|e| e.favorite);
                let ids: Vec<snap_core::CaptureId> = targets.iter().map(|e| e.id.clone()).collect();
                for entry in self.entries.iter_mut().filter(|e| ids.contains(&e.id)) {
                    entry.favorite = favorite;
                }
                if !favorite && self.filter.kind == super::KindFilter::Favorites {
                    self.entries.retain(|e| !ids.contains(&e.id));
                    self.selected.retain(|s| !ids.iter().any(|i| i.0 == *s));
                }
                self.write(cx, move |history| ids.iter().try_for_each(|id| history.set_favorite(id, favorite)));
            }
            Act::Upload => {
                for entry in &targets {
                    self.uploading.insert(entry.id.0.clone());
                }
                let engine = cx.engine.clone();
                let format = cx.settings.links.copy_format;
                let toast = Toast::new(ToastKind::Info, t("library.upload.started")).lifetime(Duration::from_secs(120)).progress(0.0);
                self.upload_toast = Some(cx.toasts.push_toast(toast));
                let handle = cx.handle();
                cx.spawn(move || {
                    let ids = targets.iter().map(|e| e.id.0.clone()).collect();
                    let progress = |frac: f32| handle.send(Msg::UploadProgress(frac));
                    Msg::Uploaded { ids, result: upload(&engine, &targets, format, &progress) }
                });
            }
            Act::Recognize => {
                if self.recognizing.insert(first.id.0.clone()) {
                    let engine = cx.engine.clone();
                    cx.spawn(move || Msg::Recognized { id: first.id.0.clone(), result: recognize(&engine, &first) });
                }
            }
            Act::Delete => {
                if self.selected.len() <= 1 {
                    if self.selected.is_empty() {
                        self.select_only(&first.id.0);
                    }
                    self.drawer.confirm_delete = true;
                } else {
                    self.bulk_confirm = true;
                }
            }
        }
    }

    /// Run a history write on a worker. The screen already shows the change; a failure reloads the truth.
    fn write(&self, cx: &ViewCx, work: impl FnOnce(&snap_history::History) -> snap_core::Result<()> + Send + 'static) {
        if let Some(history) = cx.engine.history() {
            cx.spawn(move || match work(&history) {
                Ok(()) => Msg::Written,
                Err(e) => Msg::WriteFailed(e),
            });
        }
    }

    pub(super) fn set_tags(&mut self, cx: &mut ViewCx, id: &str, tags: Vec<String>) {
        let Some(entry) = self.entries.iter_mut().find(|e| e.id.0 == id) else { return };
        entry.tags.clone_from(&tags);
        let id = entry.id.clone();
        self.write(cx, move |history| history.set_tags(&id, &tags));
    }

    /// Delete the selected captures (the confirmation has been given).
    pub(super) fn delete_selected(&mut self, cx: &mut ViewCx) {
        let ids: Vec<snap_core::CaptureId> = self.selected_entries().into_iter().map(|e| e.id).collect();
        let files = self.drawer.delete_files;
        let Some(history) = cx.engine.history() else { return };
        if ids.is_empty() {
            return;
        }
        // Keep the keyboard cursor near where the deleted cards were.
        let next = ids.iter().filter_map(|id| self.index_of(&id.0)).max().map(|i| i + 1);
        let count = ids.len();
        self.entries.retain(|e| !ids.contains(&e.id));
        for id in &ids {
            self.thumbs.forget(&id.0);
        }
        self.clear_selection();
        if let Some(entry) = next.and_then(|i| self.entries.get(i.saturating_sub(count)).or_else(|| self.entries.last())) {
            let id = entry.id.0.clone();
            self.select_only(&id);
        }
        let toast = format::counted("library.deleted", count as u64);
        cx.toast(ToastKind::Success, toast);
        cx.spawn(move || match ids.iter().try_for_each(|id| history.delete(id, files)) {
            Ok(()) => Msg::Written,
            Err(e) => Msg::WriteFailed(e),
        });
        self.fetch_apps(cx);
    }

    /// Open the combine tool with the selected images.
    pub(super) fn combine_selected(&mut self, cx: &mut ViewCx) {
        let files: Vec<PathBuf> =
            self.selected_entries().iter().filter(|e| e.kind != CaptureKind::Recording).filter_map(|e| e.file.clone()).collect();
        cx.toast(ToastKind::Info, t("library.combine.loading"));
        cx.spawn(move || {
            let images: Vec<_> = files.iter().filter_map(|f| snap_image::encode::load(f).ok()).collect();
            if images.len() < 2 {
                return Msg::Done(Err(missing()));
            }
            Msg::Done(Ok(Outcome::Commands(vec![Command::OpenCombine { images }])))
        });
    }

    /// Handle an answer from a worker thread.
    pub(super) fn message(&mut self, cx: &mut ViewCx, msg: Msg) {
        match msg {
            Msg::Page { generation, mode, result } => self.page_arrived(cx, generation, mode, result),
            Msg::Apps(apps) => self.apps = apps,
            Msg::Poll { count, newest } => {
                self.polling = false;
                let signature = Some((count, newest));
                if self.signature.is_some() && self.signature != signature {
                    self.load(cx, Load::Refresh);
                    self.fetch_apps(cx);
                }
                self.signature = signature;
            }
            Msg::Preview { id, image, file_ok } => {
                if self.preview_requested.as_deref() == Some(id.as_str()) {
                    self.decoded_preview = Some((id, image, file_ok));
                }
            }
            Msg::Entries(fresh) => {
                for entry in fresh {
                    if let Some(slot) = self.entries.iter_mut().find(|e| e.id == entry.id) {
                        *slot = entry;
                    }
                }
            }
            Msg::Done(Ok(Outcome::Toast(text))) => cx.toast(ToastKind::Success, text),
            Msg::Done(Ok(Outcome::Commands(commands))) => {
                for command in commands {
                    cx.push(command);
                }
            }
            Msg::Done(Err(e)) => cx.toast_error(&e),
            Msg::UploadProgress(frac) => {
                if let Some(id) = self.upload_toast {
                    let toast = Toast::new(ToastKind::Info, t("library.upload.started")).lifetime(Duration::from_secs(120)).progress(frac);
                    cx.toasts.update(id, toast);
                }
            }
            Msg::Uploaded { ids, result } => {
                for id in &ids {
                    self.uploading.remove(id);
                }
                if let Some(id) = self.upload_toast.take() {
                    cx.toasts.dismiss(id);
                }
                match result {
                    Ok(done) => {
                        self.message(cx, Msg::Entries(done.entries));
                        cx.toast_action(ToastKind::Success, t("library.upload.done"), t("common.open"), Command::OpenUrl(done.url));
                    }
                    Err(e) => cx.toast_error(&e),
                }
            }
            Msg::Recognized { id, result } => {
                self.recognizing.remove(&id);
                match result {
                    Ok(text) if text.trim().is_empty() => cx.toast(ToastKind::Info, t("library.text.found_none")),
                    Ok(text) => {
                        if let Some(entry) = self.entries.iter_mut().find(|e| e.id.0 == id) {
                            entry.text = Some(text);
                        }
                    }
                    Err(e) => cx.toast_error(&e),
                }
            }
            Msg::Written => {}
            Msg::WriteFailed(e) => {
                cx.toast_error(&e);
                self.load(cx, Load::Refresh);
            }
        }
    }
}

fn copy_files(entries: &[Entry]) -> Result<(), Error> {
    let files = entries.iter().map(saved_file).collect::<Result<Vec<_>, _>>()?;
    snap_platform::clipboard::set_files(&files)
}

/// Upload each capture, put the links on the clipboard in the configured format and re-read the entries.
fn upload(
    engine: &snap_engine::Engine,
    entries: &[Entry],
    format: snap_core::settings::LinkFormat,
    progress: &dyn Fn(f32),
) -> Result<Uploaded, Error> {
    let mut links = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        let file = saved_file(entry)?;
        let of_all = |sent: u64, total: u64| progress((i as f32 + sent as f32 / total.max(1) as f32) / entries.len() as f32);
        let link = engine.upload_file(&file, None, Some(&entry.id), &of_all)?;
        let image = entry.kind != CaptureKind::Recording;
        links.push((snap_upload::format_link(link.best(), format, Some(&paint::title_of(entry)), image), link.best().to_owned()));
    }
    let copied = links.iter().map(|(text, _)| text.as_str()).collect::<Vec<_>>().join("\n");
    snap_platform::clipboard::set_text(&copied)?;
    let url = links.last().map(|(_, url)| url.clone()).unwrap_or_default();
    let fresh = engine.history().map(|h| entries.iter().filter_map(|e| h.get(&e.id).ok().flatten()).collect()).unwrap_or_default();
    Ok(Uploaded { url, entries: fresh })
}

/// OCR the capture and store the text in history so search finds it.
fn recognize(engine: &snap_engine::Engine, entry: &Entry) -> Result<String, Error> {
    let image = snap_image::encode::load(&saved_file(entry)?)?;
    let text = engine.ocr(&image)?.text();
    if let Some(history) = engine.history()
        && !text.trim().is_empty()
    {
        history.set_text(&entry.id, &text)?;
    }
    Ok(text)
}
