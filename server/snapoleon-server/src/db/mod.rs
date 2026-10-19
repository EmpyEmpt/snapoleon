//! SQLite persistence. One WAL-mode connection behind a mutex, driven from
//! `spawn_blocking`; the workload (small metadata rows) does not need a pool.

mod accounts;
mod content;
mod models;

use std::{path::Path, sync::Arc};

use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::Connection;

pub use accounts::*;
pub use content::*;
pub use models::*;

use crate::error::AppError;

const SCHEMA_V1: &str = "
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE TABLE users (
    id TEXT PRIMARY KEY,
    username TEXT NOT NULL UNIQUE COLLATE NOCASE,
    display_name TEXT NOT NULL DEFAULT '',
    email TEXT NOT NULL DEFAULT '',
    password_hash TEXT,
    oidc_sub TEXT UNIQUE,
    is_admin INTEGER NOT NULL DEFAULT 0,
    disabled INTEGER NOT NULL DEFAULT 0,
    created_ms INTEGER NOT NULL
);

CREATE TABLE workspaces (
    id TEXT PRIMARY KEY,
    slug TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    retention_days INTEGER NOT NULL DEFAULT 0,
    quota_mb INTEGER NOT NULL DEFAULT 0,
    default_expiry_hours INTEGER NOT NULL DEFAULT 0,
    created_ms INTEGER NOT NULL
);

CREATE TABLE members (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'member')),
    PRIMARY KEY (workspace_id, user_id)
);

CREATE TABLE tokens (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    hash TEXT NOT NULL UNIQUE,
    prefix TEXT NOT NULL,
    created_ms INTEGER NOT NULL,
    last_used_ms INTEGER
);

CREATE TABLE sessions (
    hash TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_ms INTEGER NOT NULL,
    expires_ms INTEGER NOT NULL
);

CREATE TABLE items (
    id TEXT PRIMARY KEY,
    code TEXT NOT NULL UNIQUE,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(id),
    title TEXT NOT NULL DEFAULT '',
    file_name TEXT NOT NULL DEFAULT '',
    mime TEXT NOT NULL,
    bytes INTEGER NOT NULL,
    width INTEGER NOT NULL DEFAULT 0,
    height INTEGER NOT NULL DEFAULT 0,
    blob_key TEXT NOT NULL,
    thumb_key TEXT,
    password_hash TEXT,
    delete_secret TEXT NOT NULL,
    created_ms INTEGER NOT NULL,
    expires_ms INTEGER,
    views INTEGER NOT NULL DEFAULT 0,
    comments INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX items_user ON items(user_id, created_ms DESC);
CREATE INDEX items_ws ON items(workspace_id, created_ms DESC);
CREATE INDEX items_expiry ON items(expires_ms) WHERE expires_ms IS NOT NULL;

CREATE TABLE comments (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    text TEXT NOT NULL,
    created_ms INTEGER NOT NULL
);
CREATE INDEX comments_item ON comments(item_id, id);

CREATE TABLE views (
    item_id TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    visitor TEXT NOT NULL,
    bucket INTEGER NOT NULL,
    PRIMARY KEY (item_id, visitor, bucket)
) WITHOUT ROWID;

CREATE TABLE events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    owner_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    item_id TEXT NOT NULL,
    code TEXT NOT NULL,
    title TEXT NOT NULL,
    kind TEXT NOT NULL,
    actor TEXT NOT NULL DEFAULT '',
    text TEXT NOT NULL DEFAULT '',
    created_ms INTEGER NOT NULL
);
CREATE INDEX events_owner ON events(owner_id, created_ms);

CREATE TABLE links (
    code TEXT PRIMARY KEY,
    url TEXT NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    created_ms INTEGER NOT NULL,
    clicks INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX links_user_url ON links(user_id, url);

CREATE TABLE webhooks (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('generic', 'slack', 'telegram')),
    target TEXT NOT NULL,
    secret TEXT NOT NULL DEFAULT '',
    on_view INTEGER NOT NULL DEFAULT 1,
    on_comment INTEGER NOT NULL DEFAULT 1,
    enabled INTEGER NOT NULL DEFAULT 1
);
";

const MIGRATIONS: &[&str] = &[SCHEMA_V1];

#[derive(Clone)]
pub struct Db(Arc<Mutex<Connection>>);

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let mut conn =
            Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        migrate(&mut conn)?;
        Ok(Self(Arc::new(Mutex::new(conn))))
    }

    /// Run `f` on the blocking pool with exclusive access to the connection.
    pub async fn run<T, F>(&self, f: F) -> Result<T, AppError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> rusqlite::Result<T> + Send + 'static,
    {
        let conn = self.0.clone();
        let out = tokio::task::spawn_blocking(move || f(&mut conn.lock())).await?;
        Ok(out?)
    }

    pub async fn meta_get(&self, key: &'static str) -> Result<Option<String>, AppError> {
        self.run(move |c| {
            use rusqlite::OptionalExtension;
            c.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
                .optional()
        })
        .await
    }

    pub async fn meta_set(&self, key: &'static str, value: String) -> Result<(), AppError> {
        self.run(move |c| c.execute("INSERT INTO meta(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", (key, value)).map(|_| ()))
            .await
    }
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", i as i64 + 1)?;
        tx.commit()?;
    }
    Ok(())
}
