//! Items, comments, view counting, events, short links and sweeper queries.

use rusqlite::{ErrorCode, OptionalExtension, params};
use snap_core::now_ms;

use super::{Comment, Db, Event, Item, Link, Usage};
use crate::{error::AppError, util::DAY_MS};

pub struct NewItem {
    pub id: String,
    pub code: String,
    pub workspace_id: String,
    pub user_id: String,
    pub title: String,
    pub file_name: String,
    pub mime: String,
    pub bytes: i64,
    pub width: i64,
    pub height: i64,
    pub blob_key: String,
    pub thumb_key: Option<String>,
    pub password_hash: Option<String>,
    pub delete_secret: String,
    pub expires_ms: Option<i64>,
}

/// Which items a caller may see: everything, their own, or whole workspaces.
#[derive(Debug, Clone, Default)]
pub struct ItemScope {
    pub all: bool,
    pub user_id: String,
    pub workspaces: Vec<String>,
}

impl ItemScope {
    pub fn own(user_id: &str) -> Self {
        Self { all: false, user_id: user_id.to_string(), workspaces: Vec::new() }
    }
}

fn like_pattern(q: &str) -> String {
    let escaped = q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    format!("%{escaped}%")
}

fn is_conflict(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(f, _) if f.code == ErrorCode::ConstraintViolation)
}

fn event_from_row(r: &rusqlite::Row) -> rusqlite::Result<Event> {
    Ok(Event {
        id: r.get(0)?,
        owner_id: r.get(1)?,
        workspace_id: r.get(2)?,
        item_id: r.get(3)?,
        code: r.get(4)?,
        title: r.get(5)?,
        kind: r.get(6)?,
        actor: r.get(7)?,
        text: r.get(8)?,
        created_ms: r.get(9)?,
    })
}

const EVENT_COLS: &str = "id, owner_id, workspace_id, item_id, code, title, kind, actor, text, created_ms";

impl Db {
    /// Insert an item; `None` when the short code is already taken.
    pub async fn insert_item(&self, n: NewItem) -> Result<Option<Item>, AppError> {
        self.run(move |c| {
            let res = c.execute(
                "INSERT INTO items(id, code, workspace_id, user_id, title, file_name, mime, bytes, width, height, blob_key, thumb_key, password_hash, delete_secret, created_ms, expires_ms) \
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                params![n.id, n.code, n.workspace_id, n.user_id, n.title, n.file_name, n.mime, n.bytes, n.width, n.height, n.blob_key, n.thumb_key, n.password_hash, n.delete_secret, now_ms(), n.expires_ms],
            );
            match res {
                Err(e) if is_conflict(&e) => return Ok(None),
                other => other?,
            };
            c.query_row(&format!("SELECT {} FROM {} WHERE i.id = ?1", Item::COLS, Item::FROM), [&n.id], Item::from_row).optional()
        })
        .await
    }

    pub async fn item_by_code(&self, code: String) -> Result<Option<Item>, AppError> {
        self.run(move |c| {
            c.query_row(&format!("SELECT {} FROM {} WHERE i.code = ?1", Item::COLS, Item::FROM), [&code], Item::from_row).optional()
        })
        .await
    }

    pub async fn item_by_id(&self, id: String) -> Result<Option<Item>, AppError> {
        self.run(move |c| {
            c.query_row(&format!("SELECT {} FROM {} WHERE i.id = ?1", Item::COLS, Item::FROM), [&id], Item::from_row).optional()
        })
        .await
    }

    /// Newest-first page of visible items plus the total match count.
    pub async fn list_items(&self, scope: ItemScope, q: String, limit: i64, offset: i64) -> Result<(Vec<Item>, i64), AppError> {
        self.run(move |c| {
            let workspaces = serde_json::to_string(&scope.workspaces).unwrap_or_else(|_| "[]".into());
            let pattern = like_pattern(&q);
            let filter = "(?1 OR i.user_id = ?2 OR i.workspace_id IN (SELECT value FROM json_each(?3))) \
                          AND (?4 = '' OR i.title LIKE ?5 ESCAPE '\\' OR i.file_name LIKE ?5 ESCAPE '\\' OR i.code = ?4 OR u.username LIKE ?5 ESCAPE '\\' OR u.display_name LIKE ?5 ESCAPE '\\')";
            let total = c.query_row(&format!("SELECT COUNT(*) FROM {} WHERE {filter}", Item::FROM), params![scope.all, scope.user_id, workspaces, q, pattern], |r| r.get(0))?;
            let mut st = c.prepare(&format!("SELECT {} FROM {} WHERE {filter} ORDER BY i.created_ms DESC, i.id LIMIT ?6 OFFSET ?7", Item::COLS, Item::FROM))?;
            let items = st.query_map(params![scope.all, scope.user_id, workspaces, q, pattern, limit, offset], Item::from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((items, total))
        })
        .await
    }

    /// Delete the row and return it so the caller can remove its blobs.
    pub async fn delete_item(&self, id: String) -> Result<Option<Item>, AppError> {
        self.run(move |c| {
            let tx = c.transaction()?;
            let item =
                tx.query_row(&format!("SELECT {} FROM {} WHERE i.id = ?1", Item::COLS, Item::FROM), [&id], Item::from_row).optional()?;
            if item.is_some() {
                tx.execute("DELETE FROM items WHERE id = ?1", [&id])?;
            }
            tx.commit()?;
            Ok(item)
        })
        .await
    }

    /// Record a view unless the same visitor already counted in this hour bucket.
    pub async fn register_view(&self, item_id: String, visitor: String, bucket: i64) -> Result<bool, AppError> {
        self.run(move |c| {
            let tx = c.transaction()?;
            let fresh =
                tx.execute("INSERT OR IGNORE INTO views(item_id, visitor, bucket) VALUES(?1, ?2, ?3)", (&item_id, &visitor, bucket))? > 0;
            if fresh {
                tx.execute("UPDATE items SET views = views + 1 WHERE id = ?1", [&item_id])?;
            }
            tx.commit()?;
            Ok(fresh)
        })
        .await
    }

    pub async fn add_comment(&self, item_id: String, name: String, text: String) -> Result<Comment, AppError> {
        self.run(move |c| {
            let tx = c.transaction()?;
            let created_ms = now_ms();
            tx.execute(
                "INSERT INTO comments(item_id, name, text, created_ms) VALUES(?1, ?2, ?3, ?4)",
                params![item_id, name, text, created_ms],
            )?;
            let id = tx.last_insert_rowid();
            tx.execute("UPDATE items SET comments = comments + 1 WHERE id = ?1", [&item_id])?;
            tx.commit()?;
            Ok(Comment { id, name, text, created_ms })
        })
        .await
    }

    pub async fn comments(&self, item_id: String) -> Result<Vec<Comment>, AppError> {
        self.run(move |c| {
            let mut st = c.prepare("SELECT id, name, text, created_ms FROM comments WHERE item_id = ?1 ORDER BY id")?;
            st.query_map([&item_id], |r| Ok(Comment { id: r.get(0)?, name: r.get(1)?, text: r.get(2)?, created_ms: r.get(3)? }))?.collect()
        })
        .await
    }

    pub async fn delete_comment(&self, item_id: String, comment_id: i64) -> Result<(), AppError> {
        self.run(move |c| {
            let tx = c.transaction()?;
            if tx.execute("DELETE FROM comments WHERE id = ?1 AND item_id = ?2", (comment_id, &item_id))? > 0 {
                tx.execute("UPDATE items SET comments = MAX(comments - 1, 0) WHERE id = ?1", [&item_id])?;
            }
            tx.commit()
        })
        .await
    }

    pub async fn add_event(&self, e: Event) -> Result<(), AppError> {
        self.run(move |c| {
            c.execute(
                "INSERT INTO events(owner_id, workspace_id, item_id, code, title, kind, actor, text, created_ms) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![e.owner_id, e.workspace_id, e.item_id, e.code, e.title, e.kind, e.actor, e.text, e.created_ms],
            )
            .map(|_| ())
        })
        .await
    }

    /// Events on items owned by `owner_id`, oldest first, strictly after `since_ms`.
    pub async fn events_since(&self, owner_id: String, since_ms: i64, limit: i64) -> Result<Vec<Event>, AppError> {
        self.run(move |c| {
            let mut st = c.prepare(&format!(
                "SELECT {EVENT_COLS} FROM events WHERE owner_id = ?1 AND created_ms > ?2 ORDER BY created_ms, id LIMIT ?3"
            ))?;
            st.query_map(params![owner_id, since_ms, limit], event_from_row)?.collect()
        })
        .await
    }

    /// Aggregate usage of the items a scope can see.
    pub async fn scope_usage(&self, scope: ItemScope) -> Result<Usage, AppError> {
        self.run(move |c| {
            let workspaces = serde_json::to_string(&scope.workspaces).unwrap_or_else(|_| "[]".into());
            c.query_row(
                "SELECT COUNT(*), COALESCE(SUM(bytes), 0), COALESCE(SUM(views), 0) FROM items i \
                 WHERE (?1 OR i.user_id = ?2 OR i.workspace_id IN (SELECT value FROM json_each(?3)))",
                params![scope.all, scope.user_id, workspaces],
                |r| Ok(Usage { items: r.get(0)?, bytes: r.get(1)?, views: r.get(2)? }),
            )
        })
        .await
    }

    // Short links

    /// Existing short code for this user and URL, if any.
    pub async fn link_for_url(&self, user_id: String, url: String) -> Result<Option<String>, AppError> {
        self.run(move |c| {
            c.query_row("SELECT code FROM links WHERE user_id = ?1 AND url = ?2 LIMIT 1", (user_id, url), |r| r.get(0)).optional()
        })
        .await
    }

    /// Insert a link; `false` when the code collides.
    pub async fn create_link(&self, code: String, url: String, user_id: String, workspace_id: String) -> Result<bool, AppError> {
        self.run(move |c| {
            match c.execute(
                "INSERT INTO links(code, url, user_id, workspace_id, created_ms) VALUES(?1, ?2, ?3, ?4, ?5)",
                params![code, url, user_id, workspace_id, now_ms()],
            ) {
                Err(e) if is_conflict(&e) => Ok(false),
                other => other.map(|_| true),
            }
        })
        .await
    }

    /// Resolve a short code and count the click.
    pub async fn follow_link(&self, code: String) -> Result<Option<String>, AppError> {
        self.run(move |c| {
            let url: Option<String> = c.query_row("SELECT url FROM links WHERE code = ?1", [&code], |r| r.get(0)).optional()?;
            if url.is_some() {
                c.execute("UPDATE links SET clicks = clicks + 1 WHERE code = ?1", [&code])?;
            }
            Ok(url)
        })
        .await
    }

    pub async fn list_links(&self, scope: ItemScope, limit: i64, offset: i64) -> Result<(Vec<Link>, i64), AppError> {
        self.run(move |c| {
            let workspaces = serde_json::to_string(&scope.workspaces).unwrap_or_else(|_| "[]".into());
            let filter = "(?1 OR l.user_id = ?2 OR l.workspace_id IN (SELECT value FROM json_each(?3)))";
            let total = c.query_row(&format!("SELECT COUNT(*) FROM links l WHERE {filter}"), params![scope.all, scope.user_id, workspaces], |r| r.get(0))?;
            let mut st = c.prepare(&format!(
                "SELECT l.code, l.url, u.username, l.created_ms, l.clicks FROM links l JOIN users u ON u.id = l.user_id WHERE {filter} ORDER BY l.created_ms DESC LIMIT ?4 OFFSET ?5"
            ))?;
            let links = st
                .query_map(params![scope.all, scope.user_id, workspaces, limit, offset], |r| {
                    Ok(Link { code: r.get(0)?, url: r.get(1)?, owner: r.get(2)?, created_ms: r.get(3)?, clicks: r.get(4)? })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((links, total))
        })
        .await
    }

    pub async fn delete_link(&self, code: String, scope: ItemScope) -> Result<bool, AppError> {
        self.run(move |c| {
            let workspaces = serde_json::to_string(&scope.workspaces).unwrap_or_else(|_| "[]".into());
            c.execute(
                "DELETE FROM links WHERE code = ?4 AND (?1 OR user_id = ?2 OR workspace_id IN (SELECT value FROM json_each(?3)))",
                params![scope.all, scope.user_id, workspaces, code],
            )
            .map(|n| n > 0)
        })
        .await
    }

    // Sweeper

    /// Items past their own expiry, or older than their workspace's retention window.
    pub async fn lapsed_items(&self, now: i64) -> Result<Vec<Item>, AppError> {
        self.run(move |c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM {} JOIN workspaces w ON w.id = i.workspace_id \
                 WHERE (i.expires_ms IS NOT NULL AND i.expires_ms <= ?1) \
                    OR (w.retention_days > 0 AND i.created_ms < ?1 - w.retention_days * {DAY_MS}) LIMIT 500",
                Item::COLS,
                Item::FROM
            ))?;
            st.query_map([now], Item::from_row)?.collect()
        })
        .await
    }

    /// Drop stale bookkeeping rows: dedupe buckets, old events, expired sessions.
    pub async fn prune(&self, now: i64) -> Result<(), AppError> {
        self.run(move |c| {
            c.execute("DELETE FROM views WHERE bucket < ?1", [now / crate::util::HOUR_MS - 48])?;
            c.execute("DELETE FROM events WHERE created_ms < ?1", [now - 30 * DAY_MS])?;
            c.execute("DELETE FROM sessions WHERE expires_ms < ?1", [now])?;
            Ok(())
        })
        .await
    }
}
