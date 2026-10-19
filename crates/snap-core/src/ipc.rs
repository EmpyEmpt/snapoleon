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

/// Parse a `snapoleon://<action>?pipeline=x&region=x,y,w,h` URL.
pub fn parse_url(url: &str) -> Option<Request> {
    let rest = url.strip_prefix(&format!("{}://", crate::URL_SCHEME))?;
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let path = path.trim_matches('/');
    let param = |k: &str| query.split('&').find_map(|kv| kv.split_once('=').filter(|(a, _)| *a == k).map(|(_, v)| percent_decode(v)));
    match path {
        "edit" => Some(Request::Edit { path: param("path")? }),
        "pin" => Some(Request::Pin { path: param("path")? }),
        "library" => Some(Request::ShowWindow { window: AppWindow::Library }),
        "settings" => Some(Request::ShowWindow { window: AppWindow::Settings }),
        key => Some(Request::Action {
            action: Action::from_key(key)?,
            pipeline: param("pipeline"),
            region: param("region").and_then(|r| Rect::parse(&r)),
            window_title: param("window"),
            monitor: param("monitor").and_then(|m| m.parse().ok()),
        }),
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
                out.push(b'%');
            }
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
