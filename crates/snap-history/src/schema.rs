//! Schema migrations, applied in order. The position in this list (1-based)
//! is stored in `PRAGMA user_version`. Never edit a released migration;
//! append a new one instead.
//!
//! `captures_fts` is an external-content FTS5 table over `captures`; the
//! triggers keep it in sync, and `captures.seq` is the stable rowid.

pub(crate) const MIGRATIONS: &[&str] = &[r#"
    CREATE TABLE captures (
        seq          INTEGER PRIMARY KEY,
        id           TEXT NOT NULL UNIQUE,
        kind         TEXT NOT NULL,
        file         TEXT,
        project      TEXT,
        thumb        TEXT,
        width        INTEGER NOT NULL,
        height       INTEGER NOT NULL,
        bytes        INTEGER NOT NULL,
        created_ms   INTEGER NOT NULL,
        app_name     TEXT,
        app_id       TEXT,
        window_title TEXT,
        url          TEXT,
        element      TEXT,
        monitor_name TEXT,
        text         TEXT,
        tags         TEXT NOT NULL DEFAULT '[]',
        favorite     INTEGER NOT NULL DEFAULT 0,
        duration_ms  INTEGER
    );
    CREATE INDEX captures_created ON captures(created_ms);
    CREATE INDEX captures_app ON captures(app_name);

    CREATE TABLE links (
        seq         INTEGER PRIMARY KEY,
        capture_id  TEXT NOT NULL REFERENCES captures(id) ON DELETE CASCADE,
        url         TEXT NOT NULL,
        destination TEXT NOT NULL,
        delete_url  TEXT,
        created_ms  INTEGER NOT NULL
    );
    CREATE INDEX links_capture ON links(capture_id);

    CREATE VIRTUAL TABLE captures_fts USING fts5(
        text, window_title, app_name, url, tags,
        content = 'captures',
        content_rowid = 'seq',
        tokenize = 'unicode61 remove_diacritics 2'
    );

    CREATE TRIGGER captures_fts_ai AFTER INSERT ON captures BEGIN
        INSERT INTO captures_fts(rowid, text, window_title, app_name, url, tags)
        VALUES (new.seq, new.text, new.window_title, new.app_name, new.url, new.tags);
    END;
    CREATE TRIGGER captures_fts_ad AFTER DELETE ON captures BEGIN
        INSERT INTO captures_fts(captures_fts, rowid, text, window_title, app_name, url, tags)
        VALUES ('delete', old.seq, old.text, old.window_title, old.app_name, old.url, old.tags);
    END;
    CREATE TRIGGER captures_fts_au AFTER UPDATE OF text, window_title, app_name, url, tags ON captures BEGIN
        INSERT INTO captures_fts(captures_fts, rowid, text, window_title, app_name, url, tags)
        VALUES ('delete', old.seq, old.text, old.window_title, old.app_name, old.url, old.tags);
        INSERT INTO captures_fts(rowid, text, window_title, app_name, url, tags)
        VALUES (new.seq, new.text, new.window_title, new.app_name, new.url, new.tags);
    END;
"#];
