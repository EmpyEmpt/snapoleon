//! The cross-view command vocabulary. Views never open each other directly:
//! they push a [`Command`] (via `ViewCx::cmds`) and the app routes it. Commands
//! are processed on the UI thread after the current frame.

use std::path::PathBuf;

use crossbeam_channel::Sender;
use snap_core::{Capture, Rect, RgbaImage, hotkey::Action, ipc::Response, pipeline::Step, settings::VideoFormat};
use snap_engine::{CaptureRequest, Code, RecordTarget};

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
    /// Close every pin (tray item).
    ClosePins,
    /// Make every click-through pin take mouse input again (tray item).
    UnlockPins,
    /// Load an image from disk (worker thread), then pin it.
    PinFile(PathBuf),
    OpenLibrary,
    OpenSettings {
        page: Option<SettingsPage>,
    },
    OpenOnboarding,
    /// The first run just ended and its window is closing: say where the app went (the menu bar or the tray).
    FirstRunDone,
    OpenAbout,
    /// Recognised or translated text.
    ShowText {
        title: String,
        text: String,
    },
    /// Decoded QR codes and barcodes; `text` is their text, one per line.
    ShowCodes {
        title: String,
        text: String,
        codes: Vec<Code>,
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
    /// Empty `images` opens the tool with its own "add images" state. Each image comes with its pixels per point.
    OpenCombine {
        images: Vec<(RgbaImage, f32)>,
    },
    /// `None` for an image leaves that side for the tool's own picker (tray: "Compare images"; a view that has the
    /// two images, like a two-card selection in the Library, passes them).
    OpenDiff {
        a: Option<RgbaImage>,
        b: Option<RgbaImage>,
    },
    OpenColorTool(ColorToolInput),

    /// The hotkey recorder widget is focused: global hotkeys must not fire.
    PauseHotkeys(bool),
    /// A capture was just taken: play the shutter sound when "play sounds" is on.
    Shutter,
    /// Show a link as a QR code to scan with a phone.
    ShowQr(String),
    /// A notice's "Open System Settings": the system prompt while the permission is undecided, else its settings page.
    FixPermission(snap_platform::permissions::Perm),
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
