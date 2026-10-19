//! Users, workspaces, memberships, API tokens, sessions and webhooks.

use rusqlite::{Connection, OptionalExtension, params};
use crate::i18n::t;
use snap_core::now_ms;

use super::{Db, Member, Membership, Role, Token, Usage, User, Webhook, Workspace};
use crate::{
    error::AppError,
    util::{new_id, new_secret, sha256_hex},
};

pub struct NewUser {
    pub username: String,
    pub display_name: String,
    pub email: String,
    pub password_hash: Option<String>,
    pub oidc_sub: Option<String>,
    pub is_admin: bool,
}

const TOKEN_PREFIX: &str = "snp_";

pub fn hash_secret(secret: &str) -> String {
    sha256_hex(secret.as_bytes())
}

fn slugify(name: &str) -> String {
    let mut slug = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        format!("ws-{}", new_secret(5).to_ascii_lowercase())
    } else {
        slug
    }
}

fn user_query<P: rusqlite::Params>(
    c: &Connection,
    where_clause: &str,
    params: P,
) -> rusqlite::Result<Option<User>> {
    c.query_row(
        &format!("SELECT {} FROM users u WHERE {where_clause}", User::COLS),
        params,
        User::from_row,
    )
    .optional()
}

impl Db {
    pub async fn count_users(&self) -> Result<i64, AppError> {
        self.run(|c| c.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0)))
            .await
    }

    /// Insert a user; `None` when the username is taken.
    pub async fn create_user(&self, new: NewUser) -> Result<Option<User>, AppError> {
        self.run(move |c| {
            if c.query_row("SELECT EXISTS(SELECT 1 FROM users WHERE username = ?1)", [&new.username], |r| r.get::<_, bool>(0))? {
                return Ok(None);
            }
            let id = new_id();
            c.execute(
                "INSERT INTO users(id, username, display_name, email, password_hash, oidc_sub, is_admin, disabled, created_ms) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8)",
                params![id, new.username, new.display_name, new.email, new.password_hash, new.oidc_sub, new.is_admin, now_ms()],
            )?;
            user_query(c, "u.id = ?1", [&id])
        })
        .await
    }

    pub async fn user_by_id(&self, id: String) -> Result<Option<User>, AppError> {
        self.run(move |c| user_query(c, "u.id = ?1", [&id])).await
    }

    pub async fn user_by_username(&self, username: String) -> Result<Option<User>, AppError> {
        self.run(move |c| user_query(c, "u.username = ?1", [&username]))
            .await
    }

    pub async fn user_by_oidc_sub(&self, sub: String) -> Result<Option<User>, AppError> {
        self.run(move |c| user_query(c, "u.oidc_sub = ?1", [&sub]))
            .await
    }

    pub async fn list_users(&self) -> Result<Vec<User>, AppError> {
        self.run(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM users u ORDER BY u.username",
                User::COLS
            ))?;
            st.query_map([], User::from_row)?.collect()
        })
        .await
    }

    pub async fn set_user_password(&self, id: String, hash: String) -> Result<(), AppError> {
        self.run(move |c| {
            c.execute(
                "UPDATE users SET password_hash = ?2 WHERE id = ?1",
                (id, hash),
            )
            .map(|_| ())
        })
        .await
    }

    pub async fn set_user_disabled(&self, id: String, disabled: bool) -> Result<(), AppError> {
        self.run(move |c| {
            c.execute(
                "UPDATE users SET disabled = ?2 WHERE id = ?1",
                (&id, disabled),
            )?;
            if disabled {
                c.execute("DELETE FROM sessions WHERE user_id = ?1", [&id])?;
            }
            Ok(())
        })
        .await
    }

    pub async fn set_user_admin(&self, id: String, admin: bool) -> Result<(), AppError> {
        self.run(move |c| {
            c.execute("UPDATE users SET is_admin = ?2 WHERE id = ?1", (id, admin))
                .map(|_| ())
        })
        .await
    }

    /// Refresh profile data coming from the identity provider.
    pub async fn sync_oidc_profile(
        &self,
        id: String,
        display_name: String,
        email: String,
        admin: bool,
    ) -> Result<(), AppError> {
        self.run(move |c| {
            c.execute(
                "UPDATE users SET display_name = ?2, email = ?3, is_admin = ?4 WHERE id = ?1",
                params![id, display_name, email, admin],
            )
            .map(|_| ())
        })
        .await
    }

    // Workspaces

    pub async fn create_workspace(
        &self,
        name: String,
        owner_id: Option<String>,
    ) -> Result<Workspace, AppError> {
        self.run(move |c| {
            let base = slugify(&name);
            let mut slug = base.clone();
            let mut n = 1;
            while c.query_row(
                "SELECT EXISTS(SELECT 1 FROM workspaces WHERE slug = ?1)",
                [&slug],
                |r| r.get::<_, bool>(0),
            )? {
                n += 1;
                slug = format!("{base}-{n}");
            }
            let id = new_id();
            c.execute(
                "INSERT INTO workspaces(id, slug, name, created_ms) VALUES(?1, ?2, ?3, ?4)",
                params![id, slug, name, now_ms()],
            )?;
            if let Some(owner) = owner_id {
                c.execute(
                    "INSERT INTO members(workspace_id, user_id, role) VALUES(?1, ?2, 'owner')",
                    (&id, owner),
                )?;
            }
            c.query_row(
                &format!(
                    "SELECT {} FROM workspaces w WHERE w.id = ?1",
                    Workspace::COLS
                ),
                [&id],
                Workspace::from_row,
            )
        })
        .await
    }

    pub async fn workspace(&self, id_or_slug: String) -> Result<Option<Workspace>, AppError> {
        self.run(move |c| {
            c.query_row(
                &format!(
                    "SELECT {} FROM workspaces w WHERE w.id = ?1 OR w.slug = ?1",
                    Workspace::COLS
                ),
                [&id_or_slug],
                Workspace::from_row,
            )
            .optional()
        })
        .await
    }

    /// The oldest workspace; created at first start.
    pub async fn default_workspace(&self) -> Result<Workspace, AppError> {
        self.run(|c| {
            c.query_row(
                &format!(
                    "SELECT {} FROM workspaces w ORDER BY w.created_ms, w.id LIMIT 1",
                    Workspace::COLS
                ),
                [],
                Workspace::from_row,
            )
        })
        .await
    }

    pub async fn list_workspaces(&self) -> Result<Vec<Workspace>, AppError> {
        self.run(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM workspaces w ORDER BY w.created_ms, w.name",
                Workspace::COLS
            ))?;
            st.query_map([], Workspace::from_row)?.collect()
        })
        .await
    }

    pub async fn update_workspace(
        &self,
        id: String,
        name: String,
        retention_days: i64,
        quota_mb: i64,
        default_expiry_hours: i64,
    ) -> Result<(), AppError> {
        self.run(move |c| {
            c.execute(
                "UPDATE workspaces SET name = ?2, retention_days = ?3, quota_mb = ?4, default_expiry_hours = ?5 WHERE id = ?1",
                params![id, name, retention_days.max(0), quota_mb.max(0), default_expiry_hours.max(0)],
            )
            .map(|_| ())
        })
        .await
    }

    pub async fn memberships(&self, user_id: String) -> Result<Vec<Membership>, AppError> {
        self.run(move |c| {
            let mut st = c.prepare(&format!(
                "SELECT {}, m.role FROM members m JOIN workspaces w ON w.id = m.workspace_id WHERE m.user_id = ?1 ORDER BY w.created_ms, w.name",
                Workspace::COLS
            ))?;
            st.query_map([&user_id], |r| {
                let role: String = r.get(7)?;
                Ok(Membership { workspace: Workspace::from_row(r)?, role: Role::parse(&role).unwrap_or(Role::Member) })
            })?
            .collect()
        })
        .await
    }

    pub async fn members(&self, workspace_id: String) -> Result<Vec<Member>, AppError> {
        self.run(move |c| {
            let mut st = c.prepare(&format!("SELECT {}, m.role FROM members m JOIN users u ON u.id = m.user_id WHERE m.workspace_id = ?1 ORDER BY u.username", User::COLS))?;
            st.query_map([&workspace_id], |r| {
                let role: String = r.get(8)?;
                Ok(Member { user: User::from_row(r)?, role: Role::parse(&role).unwrap_or(Role::Member) })
            })?
            .collect()
        })
        .await
    }

    pub async fn set_member(
        &self,
        workspace_id: String,
        user_id: String,
        role: Role,
    ) -> Result<(), AppError> {
        self.run(move |c| {
            c.execute(
                "INSERT INTO members(workspace_id, user_id, role) VALUES(?1, ?2, ?3) ON CONFLICT(workspace_id, user_id) DO UPDATE SET role = excluded.role",
                (workspace_id, user_id, role.as_str()),
            )
            .map(|_| ())
        })
        .await
    }

    /// Change a role or remove a member (`role = None`), refusing to orphan a workspace.
    pub async fn change_member(
        &self,
        workspace_id: String,
        user_id: String,
        role: Option<Role>,
    ) -> Result<(), AppError> {
        let kept = self
            .run(move |c| {
                let tx = c.transaction()?;
                let is_owner: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM members WHERE workspace_id = ?1 AND user_id = ?2 AND role = 'owner')", (&workspace_id, &user_id), |r| r.get(0))?;
                let owners: i64 = tx.query_row("SELECT COUNT(*) FROM members WHERE workspace_id = ?1 AND role = 'owner'", [&workspace_id], |r| r.get(0))?;
                if is_owner && owners <= 1 && role != Some(Role::Owner) {
                    return Ok(false);
                }
                match role {
                    Some(r) => tx.execute("UPDATE members SET role = ?3 WHERE workspace_id = ?1 AND user_id = ?2", (&workspace_id, &user_id, r.as_str()))?,
                    None => tx.execute("DELETE FROM members WHERE workspace_id = ?1 AND user_id = ?2", (&workspace_id, &user_id))?,
                };
                tx.commit()?;
                Ok(true)
            })
            .await?;
        if kept {
            Ok(())
        } else {
            Err(AppError::bad_request(
                t("server.error.last_owner").into_owned(),
            ))
        }
    }

    pub async fn workspace_usage(&self, workspace_id: String) -> Result<Usage, AppError> {
        self.run(move |c| {
            c.query_row("SELECT COUNT(*), COALESCE(SUM(bytes), 0), COALESCE(SUM(views), 0) FROM items WHERE workspace_id = ?1", [&workspace_id], |r| {
                Ok(Usage { items: r.get(0)?, bytes: r.get(1)?, views: r.get(2)? })
            })
        })
        .await
    }

    // Tokens

    /// Create an API token; the plaintext is returned once and only its hash is stored.
    pub async fn create_token(&self, user_id: String, name: String) -> Result<String, AppError> {
        let plain = format!("{TOKEN_PREFIX}{}", new_secret(40));
        let hash = hash_secret(&plain);
        let prefix: String = plain.chars().take(10).collect();
        self.run(move |c| {
            c.execute("INSERT INTO tokens(id, user_id, name, hash, prefix, created_ms) VALUES(?1, ?2, ?3, ?4, ?5, ?6)", params![new_id(), user_id, name, hash, prefix, now_ms()])
                .map(|_| ())
        })
        .await?;
        Ok(plain)
    }

    pub async fn user_by_token(&self, plain: &str) -> Result<Option<User>, AppError> {
        if !plain.starts_with(TOKEN_PREFIX) {
            return Ok(None);
        }
        let hash = hash_secret(plain);
        self.run(move |c| {
            let user = user_query(c, "u.disabled = 0 AND u.id = (SELECT user_id FROM tokens WHERE hash = ?1)", [&hash])?;
            if user.is_some() {
                let now = now_ms();
                c.execute("UPDATE tokens SET last_used_ms = ?2 WHERE hash = ?1 AND (last_used_ms IS NULL OR last_used_ms < ?2 - 300000)", (&hash, now))?;
            }
            Ok(user)
        })
        .await
    }

    /// Tokens of one user, or of everyone when `user_id` is `None`.
    pub async fn list_tokens(&self, user_id: Option<String>) -> Result<Vec<Token>, AppError> {
        self.run(move |c| {
            let mut st = c.prepare(
                "SELECT t.id, t.user_id, u.username, t.name, t.prefix, t.created_ms, t.last_used_ms FROM tokens t JOIN users u ON u.id = t.user_id \
                 WHERE (?1 IS NULL OR t.user_id = ?1) ORDER BY t.created_ms DESC",
            )?;
            st.query_map([user_id], |r| {
                Ok(Token { id: r.get(0)?, owner: r.get(2)?, name: r.get(3)?, prefix: r.get(4)?, created_ms: r.get(5)?, last_used_ms: r.get(6)? })
            })?
            .collect()
        })
        .await
    }

    /// Revoke a token; `user_id` restricts the revocation to that owner.
    pub async fn revoke_token(
        &self,
        id: String,
        user_id: Option<String>,
    ) -> Result<bool, AppError> {
        self.run(move |c| {
            c.execute(
                "DELETE FROM tokens WHERE id = ?1 AND (?2 IS NULL OR user_id = ?2)",
                (id, user_id),
            )
            .map(|n| n > 0)
        })
        .await
    }

    // Sessions

    pub async fn create_session(&self, user_id: String, ttl_ms: i64) -> Result<String, AppError> {
        let plain = new_secret(48);
        let hash = hash_secret(&plain);
        self.run(move |c| {
            let now = now_ms();
            c.execute("INSERT INTO sessions(hash, user_id, created_ms, expires_ms) VALUES(?1, ?2, ?3, ?4)", (hash, user_id, now, now + ttl_ms)).map(|_| ())
        })
        .await?;
        Ok(plain)
    }

    pub async fn user_by_session(&self, plain: &str) -> Result<Option<User>, AppError> {
        let hash = hash_secret(plain);
        self.run(move |c| user_query(c, "u.disabled = 0 AND u.id = (SELECT user_id FROM sessions WHERE hash = ?1 AND expires_ms > ?2)", (&hash, now_ms()))).await
    }

    pub async fn delete_session(&self, plain: &str) -> Result<(), AppError> {
        let hash = hash_secret(plain);
        self.run(move |c| {
            c.execute("DELETE FROM sessions WHERE hash = ?1", [hash])
                .map(|_| ())
        })
        .await
    }

    // Webhooks

    pub async fn webhooks(&self, workspace_id: String) -> Result<Vec<Webhook>, AppError> {
        self.run(move |c| {
            let mut st = c.prepare("SELECT id, workspace_id, kind, target, secret, on_view, on_comment, enabled FROM webhooks WHERE workspace_id = ?1 ORDER BY rowid")?;
            st.query_map([&workspace_id], Webhook::from_row)?.collect()
        })
        .await
    }

    pub async fn add_webhook(&self, hook: Webhook) -> Result<(), AppError> {
        self.run(move |c| {
            c.execute(
                "INSERT INTO webhooks(id, workspace_id, kind, target, secret, on_view, on_comment, enabled) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![new_id(), hook.workspace_id, hook.kind.as_str(), hook.target, hook.secret, hook.on_view, hook.on_comment, hook.enabled],
            )
            .map(|_| ())
        })
        .await
    }

    pub async fn delete_webhook(&self, workspace_id: String, id: String) -> Result<(), AppError> {
        self.run(move |c| {
            c.execute(
                "DELETE FROM webhooks WHERE id = ?1 AND workspace_id = ?2",
                (id, workspace_id),
            )
            .map(|_| ())
        })
        .await
    }

    pub async fn webhook(
        &self,
        workspace_id: String,
        id: String,
    ) -> Result<Option<Webhook>, AppError> {
        self.run(move |c| {
            c.query_row("SELECT id, workspace_id, kind, target, secret, on_view, on_comment, enabled FROM webhooks WHERE id = ?1 AND workspace_id = ?2", (id, workspace_id), Webhook::from_row).optional()
        })
        .await
    }
}
