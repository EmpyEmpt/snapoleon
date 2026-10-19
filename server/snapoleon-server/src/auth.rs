//! Passwords, cookies, API-token and session authentication.

use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier},
};
use axum::{
    extract::FromRequestParts,
    http::{HeaderMap, HeaderValue, header, request::Parts},
    response::{IntoResponse, Redirect, Response},
};

use crate::{
    db::{Item, ItemScope, Membership, Role, User},
    error::{ApiError, AppError},
    state::AppState,
};

pub const SESSION_COOKIE: &str = "snap_sid";
pub const SESSION_TTL_MS: i64 = 14 * 24 * 3_600_000;

pub async fn hash_password(password: String) -> Result<String, AppError> {
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(password.as_bytes())
            .map(|h| h.to_string())
            .map_err(|e| anyhow::anyhow!("hashing password: {e}"))
    })
    .await?
    .map_err(AppError::Internal)
}

pub async fn verify_password(hash: String, password: String) -> bool {
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .verify_password(password.as_bytes(), hash.as_str())
            .is_ok()
    })
    .await
    .unwrap_or(false)
}

pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| {
            pair.trim()
                .strip_prefix(name)?
                .strip_prefix('=')
                .map(str::to_string)
        })
}

pub fn set_cookie(
    name: &str,
    value: &str,
    max_age_secs: i64,
    path: &str,
    secure: bool,
) -> HeaderValue {
    let secure = if secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{name}={value}; Path={path}; Max-Age={max_age_secs}; HttpOnly; SameSite=Lax{secure}"
    ))
    .expect("cookie values are ASCII")
}

/// The signed-in user of an admin-UI session plus their workspace roles.
pub struct Me {
    pub user: User,
    pub memberships: Vec<Membership>,
}

impl Me {
    pub fn is_admin(&self) -> bool {
        self.user.is_admin
    }

    pub fn role_in(&self, workspace_id: &str) -> Option<Role> {
        self.memberships
            .iter()
            .find(|m| m.workspace.id == workspace_id)
            .map(|m| m.role)
    }

    /// Instance admins and workspace owners/admins manage a workspace.
    pub fn manages(&self, workspace_id: &str) -> bool {
        self.is_admin() || self.role_in(workspace_id).is_some_and(|r| r >= Role::Admin)
    }

    pub fn managed_workspaces(&self) -> Vec<Membership> {
        self.memberships
            .iter()
            .filter(|m| m.role >= Role::Admin)
            .cloned()
            .collect()
    }

    pub fn manages_any(&self) -> bool {
        self.is_admin() || self.memberships.iter().any(|m| m.role >= Role::Admin)
    }

    pub fn can_manage_item(&self, item: &Item) -> bool {
        item.user_id == self.user.id || self.manages(&item.workspace_id)
    }

    /// Items and links this user may browse in the admin UI.
    pub fn scope(&self) -> ItemScope {
        ItemScope {
            all: self.is_admin(),
            user_id: self.user.id.clone(),
            workspaces: self
                .managed_workspaces()
                .into_iter()
                .map(|m| m.workspace.id)
                .collect(),
        }
    }
}

/// Session lookup that redirects anonymous visitors to the login page.
pub struct SessionRejection(Response);

impl IntoResponse for SessionRejection {
    fn into_response(self) -> Response {
        self.0
    }
}

pub async fn session_user(parts: &Parts, state: &AppState) -> Result<Option<Me>, AppError> {
    let Some(sid) = cookie_value(&parts.headers, SESSION_COOKIE) else {
        return Ok(None);
    };
    let Some(user) = state.db.user_by_session(&sid).await? else {
        return Ok(None);
    };
    let memberships = state.db.memberships(user.id.clone()).await?;
    Ok(Some(Me { user, memberships }))
}

impl FromRequestParts<AppState> for Me {
    type Rejection = SessionRejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match session_user(parts, state).await {
            Ok(Some(me)) => Ok(me),
            Ok(None) => Err(SessionRejection(
                Redirect::to("/admin/login").into_response(),
            )),
            Err(e) => Err(SessionRejection(crate::error::PageError(e).into_response())),
        }
    }
}

/// Optional session, for pages that behave differently when signed in.
pub struct MaybeMe(pub Option<Me>);

impl FromRequestParts<AppState> for MaybeMe {
    type Rejection = crate::error::PageError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(MaybeMe(session_user(parts, state).await?))
    }
}

/// Bearer-token authenticated API caller.
pub struct ApiUser(pub User);

pub fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let v = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = v.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then_some(token.trim())
}

impl FromRequestParts<AppState> for ApiUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer_token(&parts.headers).ok_or(AppError::Unauthorized)?;
        let user = state
            .db
            .user_by_token(token)
            .await?
            .ok_or(AppError::Unauthorized)?;
        Ok(ApiUser(user))
    }
}

/// Optional API caller; used by endpoints that also serve anonymous viewers.
pub async fn optional_api_user(headers: &HeaderMap, state: &AppState) -> Option<User> {
    let token = bearer_token(headers)?;
    state.db.user_by_token(token).await.ok().flatten()
}
