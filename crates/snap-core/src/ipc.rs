//! Messages between the CLI / URL-scheme handler / second instance and the
//! running app. Transport (named pipe / unix socket, newline-delimited JSON)
//! lives in `snap-platform::ipc`.

use serde::{Deserialize, Serialize};

use crate::{geom::Rect, hotkey::Action};

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
