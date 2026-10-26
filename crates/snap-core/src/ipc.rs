//! Messages between the CLI / URL-scheme handler / second instance and the
//! running app. Transport (named pipe / unix socket, newline-delimited JSON)
//! lives in `snap-platform::ipc`.

use serde::{Deserialize, Serialize};

use crate::{
    capture::{CaptureContext, CaptureKind},
    geom::Rect,
    hotkey::Action,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Ping,
    /// Run an action as if its hotkey was pressed.
    Action {
        action: Action,
        #[serde(default)]
        pipeline: Option<String>,
        /// Skip the interactive overlay for region actions.
        #[serde(default)]
        region: Option<Rect>,
        #[serde(default)]
        window_title: Option<String>,
        #[serde(default)]
        monitor: Option<usize>,
    },
    /// A `snapoleon://` link. A web page can fire one, so it names only the action: the app runs
    /// the interactive form of it (overlay, picker) with the default pipeline, never a target,
    /// a pipeline or a path taken from the URL.
    Link {
        action: Action,
    },
    /// Open a file (image or `.snap` project) in the editor.
    Edit {
        path: String,
    },
    Pin {
        path: String,
    },
    /// Run a pipeline over an existing image file.
    Process {
        path: String,
        pipeline: String,
    },
    ShowWindow {
        window: AppWindow,
    },
    ReloadSettings,
    Quit,
    /// Take a screenshot for a headless `snapoleon capture` or `snapoleon mcp` and write it as a PNG to `out`; the
    /// answer carries the path and a [`GrabInfo`]. macOS checks Screen Recording against the app that started a
    /// process, so a command run from a terminal or an agent host cannot use Snapoleon's grant itself: it hands the
    /// grab to the running app, which holds it.
    Grab {
        target: GrabTarget,
        #[serde(default)]
        cursor: bool,
        out: String,
    },
    /// The running app's version and privacy permissions, answered in [`Response::status`].
    Status,
}

/// What a [`Request::Grab`] captures. Coordinates are virtual-desktop physical pixels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GrabTarget {
    Monitor {
        index: usize,
    },
    AllMonitors,
    Window {
        id: u64,
    },
    /// The first window whose title, then app name, contains this text (case-insensitive).
    WindowTitled {
        title: String,
    },
    Region {
        rect: Rect,
    },
    ElementAtCursor,
}

/// Everything about a grabbed capture except its pixels, which are in the file the request named.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GrabInfo {
    pub kind: CaptureKind,
    pub screen_rect: Rect,
    pub scale_factor: f32,
    #[serde(default)]
    pub context: CaptureContext,
    #[serde(default)]
    pub tone_mapped: bool,
}

/// The privacy permissions of one process. Always true where the OS has no such per-app permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Permissions {
    pub screen_recording: bool,
    pub accessibility: bool,
    pub input_monitoring: bool,
    pub microphone: bool,
}

/// The running app, as [`Request::Status`] reports it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AppStatus {
    pub version: String,
    pub pid: u32,
    pub exe: String,
    pub permissions: Permissions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppWindow {
    Library,
    Settings,
    About,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The answer to [`Request::Grab`]; the image is the file at `path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grab: Option<GrabInfo>,
    /// The answer to [`Request::Status`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<AppStatus>,
}

impl Response {
    pub fn ok() -> Self {
        Self { ok: true, ..Default::default() }
    }
    pub fn err(e: impl std::fmt::Display) -> Self {
        Self { ok: false, error: Some(e.to_string()), ..Default::default() }
    }
}

/// Parse a `snapoleon://<action>` URL. Anything after `?` is ignored: a URL is untrusted input,
/// so it cannot carry a region, a window, a pipeline or a file path.
pub fn parse_url(url: &str) -> Option<Request> {
    let rest = url.strip_prefix(&format!("{}://", crate::URL_SCHEME))?;
    let path = rest.split(['?', '#']).next().unwrap_or_default().trim_matches('/');
    match path {
        "library" => Some(Request::ShowWindow { window: AppWindow::Library }),
        "settings" => Some(Request::ShowWindow { window: AppWindow::Settings }),
        key => Some(Request::Link { action: Action::from_key(key)? }),
    }
}
