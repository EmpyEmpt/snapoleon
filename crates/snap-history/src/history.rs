use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, Params, Row, ToSql, params};
use snap_core::{Error, Result, capture::CaptureContext, capture::CaptureId, now_ms, paths};

use crate::{
    Entry, LinkRecord, Query,
    entry::{kind_from_name, kind_name},
    schema::MIGRATIONS,
};

const DEFAULT_LIMIT: u32 = 100;
const DAY_MS: i64 = 86_400_000;
const COLUMNS: &str = "c.id, c.kind, c.file, c.project, c.thumb, c.width, c.height, c.bytes, \
    c.created_ms, c.app_name, c.app_id, c.window_title, c.url, c.element, c.monitor_name, \
    c.text, c.tags, c.favorite, c.duration_ms";

/// Converts `rusqlite` results into the workspace error type.
trait SqlResult<T> {
    fn db(self) -> Result<T>;
}

impl<T> SqlResult<T> for rusqlite::Result<T> {
    fn db(self) -> Result<T> {
        self.map_err(|e| Error::Other(e.into()))
    }
}

/// Capture history backed by one SQLite file. The connection sits behind a
/// mutex, so `History` is `Send + Sync` and can be shared through an `Arc`.
pub struct History {
    conn: Mutex<Connection>,
}

impl History {
    /// Open or create the database at `path`, enabling WAL and foreign keys
    /// and applying pending migrations.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut conn = Connection::open(path).db()?;
        conn.busy_timeout(Duration::from_secs(5)).db()?;
        conn.pragma_update(None, "journal_mode", "WAL").db()?;
        conn.pragma_update(None, "synchronous", "NORMAL").db()?;
        conn.pragma_update(None, "foreign_keys", true).db()?;
        migrate(&mut conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// Open the database at [`paths::history_db`].
    pub fn open_default() -> Result<Self> {
        Self::open(&paths::history_db())
    }

    /// Insert a new entry with its tags and links. Fails if the id exists.
    pub fn add(&self, e: &Entry) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction().db()?;
        tx.execute(
            "INSERT INTO captures (id, kind, file, project, thumb, width, height, bytes, created_ms, \
             app_name, app_id, window_title, url, element, monitor_name, text, tags, favorite, duration_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
            params![
                e.id.0,
                kind_name(e.kind),
                path_text(e.file.as_deref())?,
                path_text(e.project.as_deref())?,
                path_text(e.thumb.as_deref())?,
                i64::from(e.width),
                i64::from(e.height),
                to_i64(e.bytes),
                e.created_ms,
                e.context.app_name,
                e.context.app_id,
                e.context.window_title,
                e.context.url,
                e.context.element,
                e.context.monitor_name,
                e.text,
                tags_json(&normalize_tags(&e.tags)),
                e.favorite,
                e.duration_ms.map(to_i64),
            ],
        )
        .db()?;
        for link in &e.links {
            insert_link(&tx, &e.id, link)?;
        }
        tx.commit().db()
    }

    /// Fetch one entry with its links, or `None` if it does not exist.
    pub fn get(&self, id: &CaptureId) -> Result<Option<Entry>> {
        let conn = self.conn.lock();
        let sql = format!("SELECT {COLUMNS} FROM captures c WHERE c.id = ?1");
        let Some(mut entry) = conn.query_row(&sql, [&id.0], row_entry).optional().db()? else {
            return Ok(None);
        };
        entry.links = load_links(&conn, &entry.id)?;
        Ok(Some(entry))
    }

    /// Replace the extracted text (OCR output) and reindex it.
    pub fn set_text(&self, id: &CaptureId, text: &str) -> Result<()> {
        self.update(id, "UPDATE captures SET text = ?1 WHERE id = ?2", params![text, id.0])
    }

    /// Attach a share link to an existing entry.
    pub fn add_link(&self, id: &CaptureId, link: &LinkRecord) -> Result<()> {
        let conn = self.conn.lock();
        insert_link(&conn, id, link)
    }

    /// Forget a share link, for one that was deleted on its server.
    pub fn remove_link(&self, id: &CaptureId, url: &str) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM links WHERE capture_id = ?1 AND url = ?2", params![id.0, url]).db()?;
        Ok(())
    }

    /// Replace the tags. Tags are trimmed, empty ones dropped and
    /// case-insensitive duplicates removed.
    pub fn set_tags(&self, id: &CaptureId, tags: &[String]) -> Result<()> {
        let json = tags_json(&normalize_tags(tags));
        self.update(id, "UPDATE captures SET tags = ?1 WHERE id = ?2", params![json, id.0])
    }

    pub fn set_favorite(&self, id: &CaptureId, favorite: bool) -> Result<()> {
        self.update(id, "UPDATE captures SET favorite = ?1 WHERE id = ?2", params![favorite, id.0])
    }

    /// Set the saved image and/or project file paths.
    pub fn set_file(&self, id: &CaptureId, file: Option<&Path>, project: Option<&Path>) -> Result<()> {
        let file = path_text(file)?;
        let project = path_text(project)?;
        self.update(id, "UPDATE captures SET file = ?1, project = ?2 WHERE id = ?3", params![file, project, id.0])
    }

    /// Remove an entry and its links. The thumbnail is always removed, since
    /// history owns it. The image and project files are removed only when
    /// `delete_files` is set. File removal is best-effort and never fails the call.
    pub fn delete(&self, id: &CaptureId, delete_files: bool) -> Result<()> {
        let (file, project, thumb) = {
            let conn = self.conn.lock();
            let row: Option<(Option<String>, Option<String>, Option<String>)> = conn
                .query_row("SELECT file, project, thumb FROM captures WHERE id = ?1", [&id.0], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .optional()
                .db()?;
            let Some(found) = row else {
                return Err(Error::NotFound(id.0.clone()));
            };
            conn.execute("DELETE FROM captures WHERE id = ?1", [&id.0]).db()?;
            found
        };
        if let Some(thumb) = thumb {
            remove_quietly(Path::new(&thumb));
        }
        if delete_files {
            for file in [file, project].into_iter().flatten() {
                remove_quietly(Path::new(&file));
            }
        }
        Ok(())
    }

    /// Search entries. See [`Query`] for matching and ordering rules.
    pub fn search(&self, q: &Query) -> Result<Vec<Entry>> {
        let conn = self.conn.lock();
        let fts = fts_query(&q.text);
        let kind = q.kind.map(kind_name);
        let app = q.app.as_deref().filter(|a| !a.is_empty());
        let favorites = i64::from(q.favorites);
        let limit = i64::from(if q.limit == 0 { DEFAULT_LIMIT } else { q.limit });
        let offset = i64::from(q.offset);

        let filters = "(:kind IS NULL OR c.kind = :kind) \
            AND (:app IS NULL OR c.app_name COLLATE NOCASE = :app) \
            AND (:from IS NULL OR c.created_ms >= :from) \
            AND (:to IS NULL OR c.created_ms <= :to) \
            AND (:fav = 0 OR c.favorite = 1)";
        let mut bound: Vec<(&str, &dyn ToSql)> = vec![
            (":kind", &kind),
            (":app", &app),
            (":from", &q.from_ms),
            (":to", &q.to_ms),
            (":fav", &favorites),
            (":limit", &limit),
            (":offset", &offset),
        ];
        let sql = match &fts {
            Some(m) => {
                bound.push((":q", m));
                format!(
                    "SELECT {COLUMNS} FROM captures_fts JOIN captures c ON c.seq = captures_fts.rowid \
                     WHERE captures_fts MATCH :q AND {filters} \
                     ORDER BY bm25(captures_fts), c.created_ms DESC LIMIT :limit OFFSET :offset"
                )
            }
            None => format!(
                "SELECT {COLUMNS} FROM captures c WHERE {filters} \
                 ORDER BY c.created_ms DESC LIMIT :limit OFFSET :offset"
            ),
        };
        query_entries(&conn, &sql, bound.as_slice())
    }

    /// Distinct app names with their entry counts, most used first.
    pub fn apps(&self) -> Result<Vec<(String, u32)>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT app_name, COUNT(*) AS n FROM captures \
                 WHERE app_name IS NOT NULL AND app_name <> '' \
                 GROUP BY app_name ORDER BY n DESC, app_name",
            )
            .db()?;
        let rows =
            stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))).db()?.collect::<rusqlite::Result<Vec<_>>>().db()?;
        Ok(rows.into_iter().map(|(app, n)| (app, n as u32)).collect())
    }

    /// Total number of entries.
    pub fn count(&self) -> Result<u64> {
        let conn = self.conn.lock();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM captures", [], |r| r.get(0)).db()?;
        Ok(n as u64)
    }

    /// Delete entries created more than `older_than_days` days ago, with their
    /// thumbnails and links. Image files are kept. Returns the number of
    /// entries removed. Zero days means retention is disabled, so nothing is removed.
    pub fn purge(&self, older_than_days: u32) -> Result<u32> {
        if older_than_days == 0 {
            return Ok(0);
        }
        let cutoff = now_ms() - i64::from(older_than_days) * DAY_MS;
        let (removed, thumbs) = {
            let mut conn = self.conn.lock();
            let tx = conn.transaction().db()?;
            let thumbs = tx
                .prepare("SELECT thumb FROM captures WHERE created_ms < ?1 AND thumb IS NOT NULL")
                .db()?
                .query_map([cutoff], |r| r.get::<_, String>(0))
                .db()?
                .collect::<rusqlite::Result<Vec<_>>>()
                .db()?;
            let removed = tx.execute("DELETE FROM captures WHERE created_ms < ?1", [cutoff]).db()?;
            tx.commit().db()?;
            (removed, thumbs)
        };
        for thumb in thumbs {
            remove_quietly(Path::new(&thumb));
        }
        Ok(u32::try_from(removed).unwrap_or(u32::MAX))
    }

    /// Run an UPDATE keyed by `id`. Reports `NotFound` if no row matched.
    fn update(&self, id: &CaptureId, sql: &str, p: impl Params) -> Result<()> {
        let changed = self.conn.lock().execute(sql, p).db()?;
        if changed == 0 {
            return Err(Error::NotFound(id.0.clone()));
        }
        Ok(())
    }
}

/// Bring the schema up to date using `PRAGMA user_version` as the version.
fn migrate(conn: &mut Connection) -> Result<()> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).db()?;
    let latest = MIGRATIONS.len() as i64;
    if current > latest {
        return Err(Error::invalid(format!("history database schema v{current} is newer than this build supports (v{latest})")));
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let tx = conn.transaction().db()?;
        tx.execute_batch(sql).db()?;
        tx.pragma_update(None, "user_version", index as i64 + 1).db()?;
        tx.commit().db()?;
    }
    Ok(())
}

fn insert_link(conn: &Connection, id: &CaptureId, link: &LinkRecord) -> Result<()> {
    // The SELECT form makes the insert a no-op when the capture is missing.
    let inserted = conn
        .execute(
            "INSERT INTO links (capture_id, url, destination, delete_url, created_ms) \
             SELECT ?1, ?2, ?3, ?4, ?5 WHERE EXISTS (SELECT 1 FROM captures WHERE id = ?1)",
            params![id.0, link.url, link.destination, link.delete_url, link.created_ms],
        )
        .db()?;
    if inserted == 0 {
        return Err(Error::NotFound(id.0.clone()));
    }
    Ok(())
}

fn load_links(conn: &Connection, id: &CaptureId) -> Result<Vec<LinkRecord>> {
    let mut stmt = conn.prepare("SELECT url, destination, delete_url, created_ms FROM links WHERE capture_id = ?1 ORDER BY seq").db()?;
    let links = stmt
        .query_map([&id.0], |r| Ok(LinkRecord { url: r.get(0)?, destination: r.get(1)?, delete_url: r.get(2)?, created_ms: r.get(3)? }))
        .db()?
        .collect::<rusqlite::Result<Vec<_>>>()
        .db()?;
    Ok(links)
}

/// Run a query returning `captures` columns and attach each entry's links.
fn query_entries(conn: &Connection, sql: &str, bound: &[(&str, &dyn ToSql)]) -> Result<Vec<Entry>> {
    let mut stmt = conn.prepare(sql).db()?;
    let mut entries = stmt.query_map(bound, row_entry).db()?.collect::<rusqlite::Result<Vec<_>>>().db()?;
    for entry in &mut entries {
        entry.links = load_links(conn, &entry.id)?;
    }
    Ok(entries)
}

/// Map a row selected with [`COLUMNS`]. Links are filled in by the caller.
fn row_entry(r: &Row<'_>) -> rusqlite::Result<Entry> {
    let kind: String = r.get(1)?;
    let kind = kind_from_name(&kind).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, format!("unknown capture kind `{kind}`").into())
    })?;
    let tags: String = r.get(16)?;
    Ok(Entry {
        id: CaptureId(r.get(0)?),
        kind,
        file: r.get::<_, Option<String>>(2)?.map(PathBuf::from),
        project: r.get::<_, Option<String>>(3)?.map(PathBuf::from),
        thumb: r.get::<_, Option<String>>(4)?.map(PathBuf::from),
        width: r.get::<_, i64>(5)? as u32,
        height: r.get::<_, i64>(6)? as u32,
        bytes: r.get::<_, i64>(7)? as u64,
        created_ms: r.get(8)?,
        context: CaptureContext {
            app_name: r.get(9)?,
            app_id: r.get(10)?,
            window_title: r.get(11)?,
            url: r.get(12)?,
            element: r.get(13)?,
            monitor_name: r.get(14)?,
        },
        text: r.get(15)?,
        links: Vec::new(),
        tags: serde_json::from_str(&tags).unwrap_or_default(),
        favorite: r.get::<_, i64>(17)? != 0,
        duration_ms: r.get::<_, Option<i64>>(18)?.map(|v| v as u64),
    })
}

/// Turn user input into an FTS5 query: each word becomes a quoted prefix term
/// (`"word"*`), and the terms are ANDed. Words without letters or digits are
/// dropped. Returns `None` when nothing searchable is left.
fn fts_query(text: &str) -> Option<String> {
    let terms: Vec<String> = text
        .split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .map(|w| format!("\"{}\"*", w.replace('"', "\"\"")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

fn normalize_tags(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tag in tags.iter().map(|t| t.trim()).filter(|t| !t.is_empty()) {
        if !out.iter().any(|o| o.eq_ignore_ascii_case(tag)) {
            out.push(tag.to_string());
        }
    }
    out
}

fn tags_json(tags: &[String]) -> String {
    serde_json::to_string(tags).unwrap_or_else(|_| "[]".into())
}

fn path_text(path: Option<&Path>) -> Result<Option<String>> {
    path.map(|p| p.to_str().map(str::to_owned).ok_or_else(|| Error::invalid(format!("path is not valid UTF-8: {}", p.display()))))
        .transpose()
}

fn to_i64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// Delete a file the history no longer needs. A missing file is fine; any
/// other failure is logged and ignored.
fn remove_quietly(path: &Path) {
    if let Err(e) = fs::remove_file(path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("could not remove {}: {e}", path.display());
    }
}
