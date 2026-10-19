//! Headless orchestration shared by the GUI, the CLI and the MCP server:
//! non-interactive capture, the pipeline executor, saving/uploading/linking,
//! OCR/redaction/AI helpers, scrolling capture and step capture.
//!
//! Everything here is blocking and meant to run on worker threads. The GUI
//! plugs its interactive bits (editor, pins, toasts) in through [`PipelineUi`].
//!
//! CONTRACT NOTE: the public signatures in this file are frozen — the GUI is
//! built against them in parallel. Add API freely; don't change what's here.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};

use snap_core::{
    Capture, CaptureContext, CaptureId, CaptureKind, Error, Point, Rect, Result, RgbaImage, Settings,
    pipeline::Step,
    secrets::SecretStore,
    settings::{ImageFormat, LinkFormat, RedactKind},
};

pub use snap_annotate::Document;
pub use snap_history::History;
pub use snap_ocr::{OcrResult, codes::Code};
pub use snap_record::{RecordOptions, RecordTarget, Recording};
pub use snap_redact::Hit;

pub mod cli;
pub mod mcp;
pub mod steps;

/// A non-interactive capture.
#[derive(Debug, Clone, PartialEq)]
pub enum CaptureRequest {
    Monitor(usize),
    /// The monitor under the cursor.
    ActiveMonitor,
    AllMonitors,
    /// `None` = the foreground window.
    Window(Option<u64>),
    /// First window whose title contains this text (case-insensitive).
    WindowTitled(String),
    Rect(Rect),
    LastRegion,
    /// Smallest UI element under the cursor.
    ElementAtCursor,
    Clipboard,
    File(PathBuf),
}

/// What the editor hands back to a pipeline.
pub struct Annotated {
    /// Final flattened render (crop, shapes, beautify applied).
    pub image: RgbaImage,
    /// The editable document, saved as a `.snap` project next to history.
    pub document: Option<Document>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NoticeKind {
    #[default]
    Info,
    Success,
    Warning,
    Error,
}

/// A user-facing result notification (toast in-app, OS notification otherwise).
#[derive(Debug, Clone, Default)]
pub struct Notice {
    pub kind: NoticeKind,
    pub title: String,
    pub body: String,
    /// Preview image for OS notifications.
    pub image: Option<PathBuf>,
    /// Offered as "Open link" / "Copy link".
    pub url: Option<String>,
    /// Offered as "Show in folder".
    pub file: Option<PathBuf>,
}

/// Interactive hooks a pipeline may need. Called from worker threads; the GUI
/// implementation forwards to the UI thread and blocks on the reply.
pub trait PipelineUi: Sync {
    /// Open the editor on `cap` and wait. `Ok(None)` = the user cancelled.
    fn annotate(&self, cap: &Capture) -> Result<Option<Annotated>>;
    fn pin(&self, img: &RgbaImage, cap: &Capture);
    fn notify(&self, notice: &Notice);
    /// Long-running step progress; `frac = None` = indeterminate. Empty label = done.
    fn progress(&self, label: &str, frac: Option<f32>);
    /// Show recognised/translated text in a result window (GUI) or stdout (CLI).
    fn show_text(&self, _title: &str, _text: &str) {}
}

/// For CLI/MCP: annotate passes the capture through, pin is a no-op, notices go to stderr.
#[derive(Debug, Clone, Copy, Default)]
pub struct HeadlessUi {
    pub quiet: bool,
}

impl PipelineUi for HeadlessUi {
    fn annotate(&self, cap: &Capture) -> Result<Option<Annotated>> {
        Ok(Some(Annotated { image: cap.image.clone(), document: None }))
    }
    fn pin(&self, _img: &RgbaImage, _cap: &Capture) {}
    fn notify(&self, n: &Notice) {
        if !self.quiet {
            eprintln!("{}: {}", n.title, n.body);
        }
    }
    fn progress(&self, _label: &str, _frac: Option<f32>) {}
}

/// An uploaded file's links.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Link {
    pub url: String,
    pub short_url: Option<String>,
    pub delete_url: Option<String>,
    pub thumbnail_url: Option<String>,
    /// Destination id it went to.
    pub destination: String,
}

impl Link {
    /// The link to hand out: short if we have one.
    pub fn best(&self) -> &str {
        self.short_url.as_deref().unwrap_or(&self.url)
    }
}

/// Result of a pipeline run.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    pub id: Option<CaptureId>,
    pub file: Option<PathBuf>,
    pub project: Option<PathBuf>,
    pub link: Option<Link>,
    /// OCR text (or translation when a Translate step ran).
    pub text: Option<String>,
    pub codes: Vec<Code>,
    pub alt_text: Option<String>,
    /// The user cancelled an interactive step; nothing after it ran.
    pub cancelled: bool,
}

/// Overrides for [`Engine::save`]; `None` falls back to settings.
#[derive(Debug, Clone, Default)]
pub struct SaveOptions {
    pub dir: Option<PathBuf>,
    pub template: Option<String>,
    pub format: Option<ImageFormat>,
    /// Exact target path ("Save as"); wins over dir/template, format from extension.
    pub path: Option<PathBuf>,
}

/// Scrolling-capture progress callback payload.
#[derive(Debug, Clone)]
pub struct ScrollProgress {
    /// Downscaled preview of the stitched page so far (max 1600 px tall).
    pub preview: RgbaImage,
    /// Full stitched height in pixels.
    pub height: u32,
    pub frames: u32,
}

/// Cheap to clone; all state is shared.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

struct Inner {
    settings: parking_lot::RwLock<Settings>,
    secrets: Arc<dyn SecretStore>,
    history: Option<Arc<History>>,
}

impl Engine {
    /// Uses the OS keyring and the default history database (if enabled).
    pub fn new(settings: Settings) -> Self {
        let secrets = snap_platform::secrets::store();
        let history = if settings.history.enabled { History::open_default().ok().map(Arc::new) } else { None };
        Self::with_parts(settings, secrets, history)
    }

    pub fn with_parts(settings: Settings, secrets: Arc<dyn SecretStore>, history: Option<Arc<History>>) -> Self {
        Self { inner: Arc::new(Inner { settings: parking_lot::RwLock::new(settings), secrets, history }) }
    }

    pub fn settings(&self) -> Settings {
        self.inner.settings.read().clone()
    }

    /// Mutate and persist settings (policy re-applied).
    pub fn update_settings(&self, f: impl FnOnce(&mut Settings)) -> Result<()> {
        let _ = f;
        Err(todo())
    }

    /// Re-read settings from disk.
    pub fn reload_settings(&self) -> Result<()> {
        Err(todo())
    }

    pub fn secrets(&self) -> Arc<dyn SecretStore> {
        self.inner.secrets.clone()
    }

    pub fn history(&self) -> Option<Arc<History>> {
        self.inner.history.clone()
    }

    pub fn ai(&self) -> snap_ai::Ai {
        snap_ai::Ai::new(&self.settings().ai, &*self.inner.secrets)
    }

    pub fn capture(&self, req: &CaptureRequest) -> Result<Capture> {
        let _ = req;
        Err(todo())
    }

    /// Run a pipeline by id over a capture; records history. Policy-aware.
    pub fn run(&self, cap: Capture, pipeline: &str, ui: &dyn PipelineUi) -> Result<Outcome> {
        let _ = (cap, pipeline, ui);
        Err(todo())
    }

    /// Run ad-hoc steps (instant-toolbar actions, editor buttons).
    pub fn run_steps(&self, cap: Capture, steps: &[Step], ui: &dyn PipelineUi) -> Result<Outcome> {
        let _ = (cap, steps, ui);
        Err(todo())
    }

    /// Run steps over an existing file (recordings, CLI `upload`): image
    /// steps are skipped for non-images, Save is a no-op, Upload sends the file.
    pub fn run_file(&self, path: &Path, kind: CaptureKind, steps: &[Step], ui: &dyn PipelineUi) -> Result<Outcome> {
        let _ = (path, kind, steps, ui);
        Err(todo())
    }

    pub fn save(&self, img: &RgbaImage, cap: &Capture, opts: &SaveOptions) -> Result<PathBuf> {
        let _ = (img, cap, opts);
        Err(todo())
    }

    /// Save an editable project for `id` (in the data dir) and link it in history.
    pub fn save_project(&self, doc: &Document, id: &CaptureId) -> Result<PathBuf> {
        let _ = (doc, id);
        Err(todo())
    }

    /// Upload a file to `dest` (None = default destination), shorten if configured,
    /// record the link in history when `id` is known.
    pub fn upload_file(&self, path: &Path, dest: Option<&str>, id: Option<&CaptureId>, progress: &dyn Fn(u64, u64)) -> Result<Link> {
        let _ = (path, dest, id, progress);
        Err(todo())
    }

    /// Encode with the configured format and upload.
    pub fn upload_image(&self, img: &RgbaImage, cap: &Capture, dest: Option<&str>, progress: &dyn Fn(u64, u64)) -> Result<Link> {
        let _ = (img, cap, dest, progress);
        Err(todo())
    }

    /// Format per settings (or `format`), put on the clipboard, return the text.
    pub fn copy_link(&self, link: &Link, format: Option<LinkFormat>, title: Option<&str>) -> Result<String> {
        let _ = (link, format, title);
        Err(todo())
    }

    pub fn copy_image(&self, img: &RgbaImage) -> Result<()> {
        snap_platform::clipboard::set_image(img)
    }

    pub fn ocr(&self, img: &RgbaImage) -> Result<OcrResult> {
        snap_ocr::recognize(img, &self.settings().ocr)
    }

    pub fn decode_codes(&self, img: &RgbaImage) -> Vec<Code> {
        snap_ocr::codes::decode(img)
    }

    /// OCR + PII detection; `kinds` empty = settings.privacy.redact_kinds.
    /// Returns image-space hits without touching the image (the editor turns them into shapes).
    pub fn find_redactions(&self, img: &RgbaImage, kinds: &[RedactKind]) -> Result<Vec<Hit>> {
        let _ = (img, kinds);
        Err(todo())
    }

    /// [`find_redactions`](Self::find_redactions) then cover hits destructively with the configured style.
    pub fn auto_redact(&self, img: &mut RgbaImage) -> Result<Vec<Hit>> {
        let _ = img;
        Err(todo())
    }

    /// `target = None` = settings.translate.target_lang.
    pub fn translate(&self, text: &str, target: Option<&str>) -> Result<String> {
        let _ = (text, target);
        Err(todo())
    }

    pub fn alt_text(&self, img: &RgbaImage, ctx: &CaptureContext) -> Result<String> {
        self.ai().alt_text(img, ctx)
    }

    /// Auto-scroll the content under `rect` and stitch until the page stops
    /// moving, `cancel` is set, or the height cap is reached.
    pub fn scrolling(&self, rect: Rect, cancel: &AtomicBool, on_progress: &dyn Fn(&ScrollProgress)) -> Result<Capture> {
        let _ = (rect, cancel, on_progress);
        Err(todo())
    }

    /// Recording options from settings for `target` (output path in the video dir).
    pub fn record_options(&self, target: RecordTarget, format: snap_core::settings::VideoFormat) -> RecordOptions {
        let _ = (target, format);
        unimplemented!()
    }

    /// Add a finished recording to history (with a poster thumbnail).
    pub fn add_recording(&self, rec: &Recording, context: CaptureContext) -> Result<CaptureId> {
        let _ = (rec, context);
        Err(todo())
    }

    /// The point is a physical virtual-desktop pixel; used by step capture and MCP.
    pub fn context_at(&self, p: Point) -> CaptureContext {
        snap_capture::context_at(p)
    }
}

fn todo() -> Error {
    Error::unsupported("not implemented yet")
}
