//! The cross-view command vocabulary. Views never open each other directly:
//! they push a [`Command`] (via `ViewCx::cmds`) and the app routes it. Commands
//! are processed on the UI thread after the current frame.

use std::path::PathBuf;

use crossbeam_channel::Sender;
use snap_core::{Capture, Rect, RgbaImage, hotkey::Action, ipc::Response, pipeline::Step, settings::VideoFormat};
use snap_engine::{CaptureRequest, RecordTarget};

use crate::views::{OpenCx, colors::ColorToolInput, editor::EditorInput, overlay::OverlayRequest, settings::SettingsPage};

/// What to run over a capture.
#[derive(Clone)]
pub enum Work {
    /// A pipeline by id.
    Pipeline(String),
    /// Ad-hoc steps (instant-toolbar buttons, OCR and QR shortcuts).
    Steps(Vec<Step>),
}

// Commands are short-lived messages moved through a channel; boxing the image-carrying variants would only
// complicate every view that builds them.
#[allow(clippy::large_enum_variant)]
pub enum Command {
    /// Run a hotkey action as if its hotkey was pressed. Capture actions open the
    /// overlay (or capture right away for monitor/all/last-region); the rest
    /// open their views. `pipeline: None` = the action's default.
    StartCapture {
        action: Action,
        pipeline: Option<String>,
    },
    /// Open the selection overlay. Normally reached through `StartCapture`.
    OpenOverlay(OverlayRequest),
    /// Capture without UI on a worker thread, then run `work` over the result.
    CaptureAndRun {
        request: CaptureRequest,
        work: Work,
        reply: Option<Sender<Response>>,
    },
    /// Run a pipeline over a finished capture on a worker thread.
    RunPipeline {
        capture: Capture,
        pipeline: String,
    },
    /// Run ad-hoc steps over a finished capture on a worker thread.
    RunSteps {
        capture: Capture,
        steps: Vec<Step>,
    },
    /// Run a pipeline over an image or video file on disk.
    ProcessFile {
        path: PathBuf,
        pipeline: String,
        reply: Option<Sender<Response>>,
    },

    /// Open the editor. `EditorInput::reply` is answered when the user finishes (see docs/APP.md).
    OpenEditor(EditorInput),
    /// Load an image or `.snap` project from disk (worker thread), then open the editor.
    EditFile(PathBuf),
    /// `origin` is where the pixels came from, so the pin appears over them.
    OpenPin {
        image: RgbaImage,
        origin: Option<Rect>,
    },
    /// Load an image from disk (worker thread), then pin it.
    PinFile(PathBuf),
    OpenLibrary,
    OpenSettings {
        page: Option<SettingsPage>,
    },
    OpenOnboarding,
    OpenAbout,
    /// Recognised or translated text.
    ShowText {
        title: String,
        text: String,
    },
    StartRecording {
        target: RecordTarget,
        format: VideoFormat,
    },
    StopRecording,
    /// Sent by the recorder view when a recording starts or ends (tray icon, hotkey routing).
    RecordingState {
        active: bool,
    },
    OpenVideoEditor {
        path: PathBuf,
    },
    /// Auto-scroll and stitch the content under `rect`; the stitched capture goes through `pipeline`
    /// (`None` = the default pipeline).
    StartScrolling {
        rect: Rect,
        pipeline: Option<String>,
    },
    StartStepCapture,
    /// Empty `images` opens the tool with its own "add images" state.
    OpenCombine {
        images: Vec<RgbaImage>,
    },
    OpenDiff {
        a: Option<RgbaImage>,
        b: Option<RgbaImage>,
    },
    OpenColorTool(ColorToolInput),

    /// The hotkey recorder widget is focused: global hotkeys must not fire.
    PauseHotkeys(bool),
    CopyText(String),
    Reveal(PathBuf),
    OpenPath(PathBuf),
    OpenUrl(String),
    /// Escape hatch for a view that needs another window of its own: the closure runs on the UI
    /// thread with an `OpenCx`. Build the new view *inside* the closure (views are not `Send`).
    Open(Box<dyn FnOnce(&mut OpenCx) + Send>),
    Quit,
}

impl Work {
    /// The command that runs this work over `capture`.
    pub fn command(self, capture: Capture) -> Command {
        match self {
            Work::Pipeline(pipeline) => Command::RunPipeline { capture, pipeline },
            Work::Steps(steps) => Command::RunSteps { capture, steps },
        }
    }
}
