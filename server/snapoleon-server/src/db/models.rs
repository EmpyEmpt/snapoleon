//! Row types and their mappers.

use rusqlite::Row;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    Member,
    Admin,
    Owner,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Member => "member",
            Role::Admin => "admin",
            Role::Owner => "owner",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "member" => Some(Role::Member),
            "admin" => Some(Role::Admin),
            "owner" => Some(Role::Owner),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct User {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub email: String,
    pub password_hash: Option<String>,
    pub is_admin: bool,
    pub disabled: bool,
    pub created_ms: i64,
}

impl User {
    pub const COLS: &'static str = "u.id, u.username, u.display_name, u.email, u.password_hash, u.is_admin, u.disabled, u.created_ms";

    pub fn from_row(r: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: r.get(0)?,
            username: r.get(1)?,
            display_name: r.get(2)?,
            email: r.get(3)?,
            password_hash: r.get(4)?,
            is_admin: r.get(5)?,
            disabled: r.get(6)?,
            created_ms: r.get(7)?,
        })
    }

    pub fn label(&self) -> &str {
        if self.display_name.is_empty() { &self.username } else { &self.display_name }
    }
}

#[derive(Debug, Clone)]
pub struct Workspace {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub retention_days: i64,
    pub quota_mb: i64,
    pub default_expiry_hours: i64,
    pub created_ms: i64,
}

impl Workspace {
    pub const COLS: &'static str = "w.id, w.slug, w.name, w.retention_days, w.quota_mb, w.default_expiry_hours, w.created_ms";

    pub fn from_row(r: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: r.get(0)?,
            slug: r.get(1)?,
            name: r.get(2)?,
            retention_days: r.get(3)?,
            quota_mb: r.get(4)?,
            default_expiry_hours: r.get(5)?,
            created_ms: r.get(6)?,
        })
    }
}

/// A workspace together with the caller's role in it.
#[derive(Debug, Clone)]
pub struct Membership {
    pub workspace: Workspace,
    pub role: Role,
}

#[derive(Debug, Clone)]
pub struct Member {
    pub user: User,
    pub role: Role,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub id: String,
    pub owner: String,
    pub name: String,
    pub prefix: String,
    pub created_ms: i64,
    pub last_used_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct Item {
    pub id: String,
    pub code: String,
    pub workspace_id: String,
    pub user_id: String,
    pub uploader: String,
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
    pub created_ms: i64,
    pub expires_ms: Option<i64>,
    pub views: i64,
    pub comments: i64,
}

impl Item {
    pub const COLS: &'static str = "i.id, i.code, i.workspace_id, i.user_id, CASE WHEN u.display_name = '' THEN u.username ELSE u.display_name END, \
         i.title, i.file_name, i.mime, i.bytes, i.width, i.height, i.blob_key, i.thumb_key, i.password_hash, i.delete_secret, \
         i.created_ms, i.expires_ms, i.views, i.comments";
    pub const FROM: &'static str = "items i JOIN users u ON u.id = i.user_id";

    pub fn from_row(r: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: r.get(0)?,
            code: r.get(1)?,
            workspace_id: r.get(2)?,
            user_id: r.get(3)?,
            uploader: r.get(4)?,
            title: r.get(5)?,
            file_name: r.get(6)?,
            mime: r.get(7)?,
            bytes: r.get(8)?,
            width: r.get(9)?,
            height: r.get(10)?,
            blob_key: r.get(11)?,
            thumb_key: r.get(12)?,
            password_hash: r.get(13)?,
            delete_secret: r.get(14)?,
            created_ms: r.get(15)?,
            expires_ms: r.get(16)?,
            views: r.get(17)?,
            comments: r.get(18)?,
        })
    }

    pub fn is_video(&self) -> bool {
        self.mime.starts_with("video/")
    }

    pub fn is_protected(&self) -> bool {
        self.password_hash.is_some()
    }

    pub fn is_expired(&self, now_ms: i64) -> bool {
        self.expires_ms.is_some_and(|e| e <= now_ms)
    }

    /// Title for display: explicit title, else file name, else the code.
    pub fn display_title(&self) -> &str {
        if !self.title.is_empty() {
            &self.title
        } else if !self.file_name.is_empty() {
            &self.file_name
        } else {
            &self.code
        }
    }

    /// All storage keys that belong to this item.
    pub fn blob_keys(&self) -> Vec<String> {
        let mut keys = vec![self.blob_key.clone(), crate::storage::og_key(self)];
        keys.extend(self.thumb_key.clone());
        keys
    }
}

#[derive(Debug, Clone)]
pub struct Comment {
    pub id: i64,
    pub name: String,
    pub text: String,
    pub created_ms: i64,
}

#[derive(Debug, Clone)]
pub struct Event {
    pub id: i64,
    pub owner_id: String,
    pub workspace_id: String,
    pub item_id: String,
    pub code: String,
    pub title: String,
    pub kind: String,
    pub actor: String,
    pub text: String,
    pub created_ms: i64,
}

#[derive(Debug, Clone)]
pub struct Link {
    pub code: String,
    pub url: String,
    pub owner: String,
    pub created_ms: i64,
    pub clicks: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebhookKind {
    Generic,
    Slack,
    Telegram,
}

impl WebhookKind {
    pub fn as_str(self) -> &'static str {
        match self {
            WebhookKind::Generic => "generic",
            WebhookKind::Slack => "slack",
            WebhookKind::Telegram => "telegram",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "generic" => Some(WebhookKind::Generic),
            "slack" => Some(WebhookKind::Slack),
            "telegram" => Some(WebhookKind::Telegram),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Webhook {
    pub id: String,
    pub workspace_id: String,
    pub kind: WebhookKind,
    /// URL for generic / Slack hooks, chat id for Telegram.
    pub target: String,
    /// HMAC signing secret (generic) or bot token (Telegram).
    pub secret: String,
    pub on_view: bool,
    pub on_comment: bool,
    pub enabled: bool,
}

impl Webhook {
    pub fn from_row(r: &Row) -> rusqlite::Result<Self> {
        let kind: String = r.get(2)?;
        Ok(Self {
            id: r.get(0)?,
            workspace_id: r.get(1)?,
            kind: WebhookKind::parse(&kind).unwrap_or(WebhookKind::Generic),
            target: r.get(3)?,
            secret: r.get(4)?,
            on_view: r.get(5)?,
            on_comment: r.get(6)?,
            enabled: r.get(7)?,
        })
    }
}

/// Aggregate usage numbers for dashboards and quota checks.
#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub items: i64,
    pub bytes: i64,
    pub views: i64,
}
