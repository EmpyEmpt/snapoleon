//! Stub for the editor view: a standard window that names itself and answers the pipeline
//! reply, so the `Annotate` step works end to end. Keep `TAG`, `EditorInput` and the `open`
//! signature: the rest of the app already calls them.

use std::path::{Path, PathBuf};

use crossbeam_channel::Sender;
use egui::Context;
use snap_annotate::Document;
use snap_core::{Capture, CaptureKind, Rect, Result, t};
use snap_engine::Annotated;
use snap_ui::{
    icon, text,
    widgets::{empty_state, primary_button},
};

use super::{Chrome, OpenCx, View, ViewCx, window_spec};

pub const TAG: &str = "editor";

/// Everything the editor opens with.
pub struct EditorInput {
    pub capture: Capture,
    /// An existing annotation document (a re-opened `.snap` project).
    pub document: Option<Document>,
    /// Where the project is saved; `None` until the first "Save project".
    pub project_path: Option<PathBuf>,
    /// Set when a pipeline's `Annotate` step is waiting for the result. The editor sends exactly
    /// once when it closes: `Some(Annotated)` when the user finished, `None` when they discarded.
    /// Dropping the sender without sending counts as discarding.
    pub reply: Option<Sender<Option<Annotated>>>,
}

impl EditorInput {
    pub fn from_capture(capture: Capture) -> Self {
        Self { capture, document: None, project_path: None, reply: None }
    }

    /// An image file, or a `.snap` project.
    pub fn from_file(path: &Path) -> Result<Self> {
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case(snap_core::PROJECT_EXT)) {
            return Self::from_project(path);
        }
        let image = snap_image::encode::load(path)?;
        let mut capture = Capture::from_image(CaptureKind::File, image);
        capture.context.window_title = path.file_name().map(|n| n.to_string_lossy().into_owned());
        Ok(Self::from_capture(capture))
    }

    pub fn from_project(path: &Path) -> Result<Self> {
        let document = snap_annotate::load_project(path)?;
        let (w, h) = document.canvas_size();
        let mut capture = Capture::new(CaptureKind::File, (*document.base).clone(), Rect::new(0, 0, w, h), document.scale_factor);
        if let Some(id) = &document.meta.capture_id {
            capture.id = id.clone();
        }
        capture.context = document.meta.context.clone();
        capture.taken_at_ms = document.meta.created_ms;
        Ok(Self { capture, document: Some(document), project_path: Some(path.to_path_buf()), reply: None })
    }
}

pub fn open(cx: &mut OpenCx, input: EditorInput) {
    cx.open(window_spec(t("app.stub.editor.title"), TAG, (1100.0, 760.0), (720.0, 480.0)), EditorView { input: Some(input) });
}

struct EditorView {
    /// Taken when the window finishes, so the reply is sent once.
    input: Option<EditorInput>,
}

impl EditorView {
    fn finish(&mut self, done: bool) {
        let Some(input) = self.input.take() else { return };
        if let Some(reply) = input.reply {
            let result = done.then(|| Annotated { image: input.capture.image, document: input.document });
            let _ = reply.send(result);
        }
    }
}

impl View for EditorView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let title = t("app.stub.editor.title");
        let size = self.input.as_ref().map(|i| i.capture.image.dimensions());
        let mut done = false;
        Chrome::new(&title).show(cx, ctx, |ui, _cx| {
            ui.add_space(((ui.available_height() - 190.0) / 2.0).max(0.0));
            empty_state(ui, icon::PENCIL_SIMPLE, &title, &t("app.stub.body"));
            ui.vertical_centered(|ui| {
                if let Some((w, h)) = size {
                    ui.label(text::mono_text(format!("{w} × {h}")));
                }
                done = primary_button(ui, &t("common.done")).clicked();
            });
        });
        if done {
            self.finish(true);
            cx.close();
        }
    }

    fn closed(&mut self, _cx: &mut ViewCx) {
        self.finish(false);
    }
}
