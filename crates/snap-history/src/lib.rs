//! Capture history: a WAL-mode SQLite library with FTS5 full-text search over
//! OCR text, window titles, app names, URLs and tags, plus link records,
//! favourites, retention and 320 px PNG thumbnails.

mod entry;
mod history;
mod schema;
mod thumb;

use std::borrow::Cow;

use snap_core::{capture::CaptureKind, t};

pub use entry::{Entry, LinkRecord, Query};
pub use history::History;
pub use thumb::write_thumbnail;

/// Localized display name of a capture kind, for filter menus and lists.
pub fn kind_label(kind: CaptureKind) -> Cow<'static, str> {
    t(&format!("history.kind.{}", entry::kind_name(kind)))
}
