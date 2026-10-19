//! What the Library shows: filter chips, the history query behind them and paged fetching.

use snap_core::{CaptureKind, Result};
use snap_history::{Entry, History, Query};

use super::format;

/// Rows fetched per request.
pub const PAGE: u32 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KindFilter {
    #[default]
    All,
    Screenshots,
    Recordings,
    Favorites,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateRange {
    Today,
    Week,
    Month,
}

impl DateRange {
    fn start(self, now_ms: i64) -> i64 {
        match self {
            DateRange::Today => format::day_start(now_ms),
            DateRange::Week => format::week_start(now_ms),
            DateRange::Month => format::month_start(now_ms),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Filter {
    pub kind: KindFilter,
    pub range: Option<DateRange>,
    pub app: Option<String>,
}

impl Filter {
    /// Anything narrower than "everything".
    pub fn is_active(&self) -> bool {
        *self != Filter::default()
    }

    fn query(&self, text: &str, offset: u32) -> Query {
        Query {
            text: text.trim().to_owned(),
            // "Screenshots" is every kind but recordings; history can only filter on one kind, so that one is filtered in `fetch`.
            kind: (self.kind == KindFilter::Recordings).then_some(CaptureKind::Recording),
            app: self.app.clone(),
            from_ms: self.range.map(|r| r.start(snap_core::now_ms())),
            to_ms: None,
            favorites: self.kind == KindFilter::Favorites,
            limit: PAGE,
            offset,
        }
    }
}

/// One fetched page.
pub struct Page {
    pub entries: Vec<Entry>,
    /// Absolute database row where the next request starts.
    pub consumed: u32,
    pub more: bool,
}

/// Fetch at least `want` entries (fewer at the end of the data) starting at database row `offset`.
pub fn fetch(history: &History, filter: &Filter, text: &str, offset: u32, want: usize) -> Result<Page> {
    let screenshots = filter.kind == KindFilter::Screenshots;
    let mut entries = Vec::new();
    let mut consumed = offset;
    loop {
        let rows = history.search(&filter.query(text, consumed))?;
        let got = rows.len() as u32;
        consumed += got;
        entries.extend(rows.into_iter().filter(|e| !screenshots || e.kind != CaptureKind::Recording));
        if got < PAGE {
            return Ok(Page { entries, consumed, more: false });
        }
        if entries.len() >= want {
            return Ok(Page { entries, consumed, more: true });
        }
    }
}
