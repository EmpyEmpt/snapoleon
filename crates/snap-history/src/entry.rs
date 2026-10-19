use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use snap_core::{
    Capture,
    capture::{CaptureContext, CaptureId, CaptureKind},
};

/// A share link created for a capture (upload destination, shortener, ...).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkRecord {
    pub url: String,
    pub destination: String,
    pub delete_url: Option<String>,
    pub created_ms: i64,
}

/// One history row. `bytes` is the size of the saved file, `thumb` the
/// thumbnail written by [`crate::write_thumbnail`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: CaptureId,
    pub kind: CaptureKind,
    pub file: Option<PathBuf>,
    pub project: Option<PathBuf>,
    pub thumb: Option<PathBuf>,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub created_ms: i64,
    pub context: CaptureContext,
    /// OCR or other extracted text, indexed for search.
    pub text: Option<String>,
    pub links: Vec<LinkRecord>,
    pub tags: Vec<String>,
    pub favorite: bool,
    pub duration_ms: Option<u64>,
}

impl Entry {
    /// Build a row for a fresh capture. `file` and `bytes` describe the saved
    /// file, if one was written.
    pub fn from_capture(c: &Capture, file: Option<PathBuf>, bytes: u64) -> Self {
        Self {
            id: c.id.clone(),
            kind: c.kind,
            file,
            project: None,
            thumb: None,
            width: c.image.width(),
            height: c.image.height(),
            bytes,
            created_ms: c.taken_at_ms,
            context: c.context.clone(),
            text: None,
            links: Vec::new(),
            tags: Vec::new(),
            favorite: false,
            duration_ms: None,
        }
    }
}

/// Search filter. Empty `text` lists matching rows newest first. Non-empty
/// `text` is split on whitespace; each word matches as a prefix and all words
/// must match, ranked by bm25 and then recency.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Query {
    pub text: String,
    pub kind: Option<CaptureKind>,
    /// Exact app name, compared case-insensitively.
    pub app: Option<String>,
    pub from_ms: Option<i64>,
    pub to_ms: Option<i64>,
    pub favorites: bool,
    /// Maximum rows returned; 0 means the default of 100.
    pub limit: u32,
    pub offset: u32,
}

/// Stable snake_case name stored in the database and used as a locale key.
pub(crate) fn kind_name(kind: CaptureKind) -> &'static str {
    match kind {
        CaptureKind::Region => "region",
        CaptureKind::Window => "window",
        CaptureKind::Element => "element",
        CaptureKind::Monitor => "monitor",
        CaptureKind::AllMonitors => "all_monitors",
        CaptureKind::Scrolling => "scrolling",
        CaptureKind::Clipboard => "clipboard",
        CaptureKind::File => "file",
        CaptureKind::Recording => "recording",
    }
}

pub(crate) fn kind_from_name(name: &str) -> Option<CaptureKind> {
    Some(match name {
        "region" => CaptureKind::Region,
        "window" => CaptureKind::Window,
        "element" => CaptureKind::Element,
        "monitor" => CaptureKind::Monitor,
        "all_monitors" => CaptureKind::AllMonitors,
        "scrolling" => CaptureKind::Scrolling,
        "clipboard" => CaptureKind::Clipboard,
        "file" => CaptureKind::File,
        "recording" => CaptureKind::Recording,
        _ => return None,
    })
}
