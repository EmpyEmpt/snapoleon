//! Server-rendered admin UI: sign-in, overview, items, links, account/tokens,
//! users and workspace settings.

mod items;
mod people;
mod workspaces;

use std::{sync::OnceLock, time::Duration};

use axum::{
    Form, Router,
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use maud::{Markup, html};
use serde::Deserialize;
use crate::i18n::{t, tf};

use crate::{
    auth::{self, MaybeMe, Me, SESSION_COOKIE, SESSION_TTL_MS, cookie_value, set_cookie},
    config::{Registration, StorageKind},
    db::NewUser,
    error::{AppError, PageResult},
    state::{AppState, Client},
    util::human_bytes,
    views::{self, brand_link},
};

pub fn routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/admin", get(overview))
        .route("/admin/login", get(login_page).post(login))
        .route("/admin/logout", post(logout))
        .route("/register", get(register_page).post(register))
        .merge(items::routes())
        .merge(people::routes())
        .merge(workspaces::routes())
        .layer(middleware::from_fn_with_state(state, same_origin))
}

/// Reject cross-site form posts: browsers send `Origin` (and `Sec-Fetch-Site`) on every POST.
async fn same_origin(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if req.method() != Method::GET
        && req.method() != Method::HEAD
        && req.method() != Method::OPTIONS
    {
        let headers = req.headers();
        let cross_site = headers
            .get("sec-fetch-site")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v == "cross-site");
        let bad_origin = headers
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|origin| {
                let host = origin.split_once("://").map_or(origin, |(_, h)| h);
                let request_host = headers
                    .get(header::HOST)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default();
                host != state.cfg.public_host() && host != request_host
            });
        if cross_site || bad_origin {
            return crate::error::PageError(AppError::Forbidden).into_response();
        }
    }
    next.run(req).await
}

/// One-shot banner selected by `?n=` after a redirect; only known keys render.
#[derive(Deserialize, Default)]
pub struct NoticeQuery {
    n: Option<String>,
    d: Option<String>,
}

const NOTICES: &[(&str, bool)] = &[
    ("saved", true),
    ("created", true),
    ("deleted", true),
    ("updated", true),
    ("test_ok", true),
    ("test_failed", false),
    ("error", false),
];

impl NoticeQuery {
    pub fn render(&self) -> Markup {
        let Some((key, ok)) = self
            .n
            .as_deref()
            .and_then(|n| NOTICES.iter().find(|(k, _)| *k == n))
        else {
            return html! {};
        };
        let detail = self
            .d
            .as_deref()
            .map(|d| d.chars().take(200).collect::<String>())
            .unwrap_or_default();
        html! {
            p.notice.ok[*ok].error[!*ok] role="status" { (t(&format!("server.admin.notice.{key}"))) @if !detail.is_empty() { ": " (detail) } }
        }
    }
}

pub fn redirect_with(path: &str, notice: &str) -> Response {
    Redirect::to(&format!("{path}?n={notice}")).into_response()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    Overview,
    Items,
    Links,
    Account,
    Users,
    Workspaces,
}

/// Admin page chrome: sidebar navigation plus the content area.
pub fn page(me: &Me, active: Nav, title: &str, body: Markup) -> Markup {
    let item = |nav: Nav, href: &str, key: &str| {
        html! { a href=(href) class=[(nav == active).then_some("active")] aria-current=[(nav == active).then_some("page")] { (t(key)) } }
    };
    views::shell(
        &format!("{title} - Snapoleon"),
        html! { meta name="robots" content="noindex"; },
        "admin",
        html! {
            div.layout {
                aside.sidebar {
                    (brand_link())
                    nav.nav {
                        (item(Nav::Overview, "/admin", "server.admin.nav.overview"))
                        (item(Nav::Items, "/admin/items", "server.admin.nav.items"))
                        (item(Nav::Links, "/admin/links", "server.admin.nav.links"))
                        @if me.manages_any() { (item(Nav::Workspaces, "/admin/workspaces", "server.admin.nav.workspaces")) }
                        @if me.is_admin() { (item(Nav::Users, "/admin/users", "server.admin.nav.users")) }
                        (item(Nav::Account, "/admin/account", "server.admin.nav.account"))
                    }
                    div.who {
                        span.dim { (me.user.label()) }
                        form method="post" action="/admin/logout" { button.link type="submit" { (t("server.admin.nav.signout")) } }
                    }
                }
                main.content {
                    h1 { (title) }
                    (body)
                }
            }
        },
    )
}

/// Labelled form control.
pub fn field(label: &str, input: Markup) -> Markup {
    html! { label.field { span { (label) } (input) } }
}

pub fn checked_box(name: &str, label: &str, on: bool) -> Markup {
    html! { label.check { input type="checkbox" name=(name) checked[on]; span { (label) } } }
}

// Overview

async fn overview(State(state): State<AppState>, me: Me) -> PageResult<Markup> {
    let usage = state.db.scope_usage(me.scope()).await?;
    let managed = if me.is_admin() {
        state
            .db
            .list_workspaces()
            .await?
            .into_iter()
            .map(|w| (w, None))
            .collect::<Vec<_>>()
    } else {
        me.managed_workspaces()
            .into_iter()
            .map(|m| (m.workspace, Some(m.role)))
            .collect()
    };
    let mut rows = Vec::new();
    for (ws, _) in &managed {
        rows.push((ws.clone(), state.db.workspace_usage(ws.id.clone()).await?));
    }
    let users = if me.is_admin() {
        Some(state.db.count_users().await?)
    } else {
        None
    };
    let backend = match state.cfg.storage.kind {
        StorageKind::Fs => t("server.admin.overview.backend_fs").into_owned(),
        StorageKind::S3 => tf(
            "server.admin.overview.backend_s3",
            &[("bucket", &state.cfg.storage.bucket)],
        ),
    };
    Ok(page(
        &me,
        Nav::Overview,
        &t("server.admin.nav.overview"),
        html! {
            section.cards {
                div.stat { span.dim { (t("server.admin.overview.items")) } strong.mono { (usage.items) } }
                div.stat { span.dim { (t("server.admin.overview.storage")) } strong.mono { (human_bytes(usage.bytes.max(0) as u64)) } }
                div.stat { span.dim { (t("server.admin.overview.views")) } strong.mono { (usage.views) } }
                @if let Some(n) = users { div.stat { span.dim { (t("server.admin.overview.users")) } strong.mono { (n) } } }
            }
            p.dim { (tf("server.admin.overview.backend", &[("backend", &backend), ("max", &state.cfg.max_upload_mb.to_string())])) }
            @if !rows.is_empty() {
                h2 { (t("server.admin.overview.workspaces")) }
                table {
                    thead { tr { th { (t("server.admin.workspaces.name")) } th { (t("server.admin.overview.items")) } th { (t("server.admin.overview.storage")) } th { (t("server.admin.workspaces.quota")) } th { (t("server.admin.workspaces.retention")) } } }
                    tbody {
                        @for (ws, u) in &rows {
                            tr {
                                td { a href=(format!("/admin/workspaces/{}", ws.id)) { (ws.name) } }
                                td.mono { (u.items) }
                                td.mono { (human_bytes(u.bytes.max(0) as u64)) }
                                td { @if ws.quota_mb > 0 { (format!("{} MB", ws.quota_mb)) } @else { span.dim { (t("server.admin.unlimited")) } } }
                                td { @if ws.retention_days > 0 { (tf("server.admin.days", &[("n", &ws.retention_days.to_string())])) } @else { span.dim { (t("server.admin.forever")) } } }
                            }
                        }
                    }
                }
            }
        },
    ))
}

// Sign-in

fn login_markup(state: &AppState, error: Option<&str>) -> Markup {
    views::shell(
        &format!("{} - Snapoleon", t("server.admin.login.title")),
        html! { meta name="robots" content="noindex"; },
        "centered",
        html! {
            main.card.narrow {
                (brand_link())
                h1 { (t("server.admin.login.title")) }
                @if let Some(e) = error { p.notice.error role="alert" { (e) } }
                form method="post" action="/admin/login" {
                    (field(&t("server.admin.login.username"), html! { input type="text" name="username" autocomplete="username" autofocus; }))
                    (field(&t("server.admin.login.password"), html! { input type="password" name="password" autocomplete="current-password"; }))
                    button.button type="submit" { (t("server.admin.login.submit")) }
                }
                @if state.cfg.oidc.enabled() {
                    p.sep { span { (t("server.admin.login.or")) } }
                    a.button.secondary href="/auth/oidc/login" { (t("server.admin.login.sso")) }
                }
                details.token-login {
                    summary { (t("server.admin.login.with_token")) }
                    form method="post" action="/admin/login" {
                        (field(&t("server.admin.login.token"), html! { input type="password" name="token" autocomplete="off" placeholder="snp_..."; }))
                        button.button.secondary type="submit" { (t("server.admin.login.submit")) }
                    }
                }
                @if state.cfg.registration == Registration::Open {
                    p.dim { (t("server.admin.login.no_account")) " " a href="/register" { (t("server.admin.login.register")) } }
                }
            }
        },
    )
}

async fn login_page(State(state): State<AppState>, MaybeMe(me): MaybeMe) -> PageResult<Response> {
    if me.is_some() {
        return Ok(Redirect::to("/admin").into_response());
    }
    Ok(login_markup(&state, None).into_response())
}

#[derive(Deserialize)]
struct LoginForm {
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    token: String,
}

/// Argon2 hash of nothing in particular, verified against when the user is unknown so timing does not reveal accounts.
fn dummy_hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| {
        use argon2::password_hash::PasswordHasher;
        argon2::Argon2::default()
            .hash_password(b"snapoleon-dummy")
            .map(|h| h.to_string())
            .unwrap_or_default()
    })
}

async fn start_session(state: &AppState, user_id: String, to: &str) -> PageResult<Response> {
    let sid = state.db.create_session(user_id, SESSION_TTL_MS).await?;
    let mut resp = Redirect::to(to).into_response();
    resp.headers_mut().append(
        header::SET_COOKIE,
        set_cookie(
            SESSION_COOKIE,
            &sid,
            SESSION_TTL_MS / 1000,
            "/",
            state.cfg.secure_cookies(),
        ),
    );
    Ok(resp)
}

async fn login(
    State(state): State<AppState>,
    client: Client,
    Form(form): Form<LoginForm>,
) -> PageResult<Response> {
    state.limiter.check(
        &format!("login:{}", client.ip),
        10,
        Duration::from_secs(600),
    )?;
    let bad = |state: &AppState| {
        Ok((
            StatusCode::UNAUTHORIZED,
            login_markup(state, Some(&t("server.admin.login.failed"))),
        )
            .into_response())
    };

    if !form.token.trim().is_empty() {
        return match state.db.user_by_token(form.token.trim()).await? {
            Some(user) => start_session(&state, user.id, "/admin").await,
            None => bad(&state),
        };
    }
    let username = form.username.trim().to_string();
    state.limiter.check(
        &format!("login-user:{}", username.to_lowercase()),
        10,
        Duration::from_secs(600),
    )?;
    let user = state.db.user_by_username(username).await?;
    let hash = user
        .as_ref()
        .and_then(|u| u.password_hash.clone())
        .unwrap_or_else(|| dummy_hash().to_string());
    let verified = auth::verify_password(hash, form.password).await;
    match user {
        Some(u) if verified && !u.disabled && u.password_hash.is_some() => {
            start_session(&state, u.id, "/admin").await
        }
        _ => bad(&state),
    }
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> PageResult<Response> {
    if let Some(sid) = cookie_value(&headers, SESSION_COOKIE) {
        state.db.delete_session(&sid).await?;
    }
    let mut resp = Redirect::to("/admin/login").into_response();
    resp.headers_mut().append(
        header::SET_COOKIE,
        set_cookie(SESSION_COOKIE, "", 0, "/", state.cfg.secure_cookies()),
    );
    Ok(resp)
}

// Self-registration

fn register_markup(error: Option<&str>) -> Markup {
    views::shell(
        &format!("{} - Snapoleon", t("server.admin.register.title")),
        html! { meta name="robots" content="noindex"; },
        "centered",
        html! {
            main.card.narrow {
                (brand_link())
                h1 { (t("server.admin.register.title")) }
                @if let Some(e) = error { p.notice.error role="alert" { (e) } }
                form method="post" action="/register" {
                    (field(&t("server.admin.users.username"), html! { input type="text" name="username" required minlength="3" maxlength="40" autocomplete="username"; }))
                    (field(&t("server.admin.users.display_name"), html! { input type="text" name="display_name" maxlength="80" autocomplete="name"; }))
                    (field(&t("server.admin.login.password"), html! { input type="password" name="password" required minlength="8" autocomplete="new-password"; }))
                    button.button type="submit" { (t("server.admin.register.submit")) }
                }
                p.dim { a href="/admin/login" { (t("server.admin.register.back")) } }
            }
        },
    )
}

async fn register_page(State(state): State<AppState>) -> PageResult<Markup> {
    if state.cfg.registration != Registration::Open {
        return Err(AppError::NotFound.into());
    }
    Ok(register_markup(None))
}

#[derive(Deserialize)]
struct RegisterForm {
    username: String,
    #[serde(default)]
    display_name: String,
    password: String,
}

pub fn valid_username(name: &str) -> bool {
    (3..=40).contains(&name.chars().count())
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

async fn register(
    State(state): State<AppState>,
    client: Client,
    Form(form): Form<RegisterForm>,
) -> PageResult<Response> {
    if state.cfg.registration != Registration::Open {
        return Err(AppError::NotFound.into());
    }
    state.limiter.check(
        &format!("register:{}", client.ip),
        5,
        Duration::from_secs(3600),
    )?;
    let username = form.username.trim().to_string();
    let invalid =
        |msg: String| Ok((StatusCode::BAD_REQUEST, register_markup(Some(&msg))).into_response());
    if !valid_username(&username) {
        return invalid(t("server.error.invalid_username").into_owned());
    }
    if form.password.chars().count() < 8 {
        return invalid(t("server.error.weak_password").into_owned());
    }
    let hash = auth::hash_password(form.password).await?;
    let created = state
        .db
        .create_user(NewUser {
            username,
            display_name: form.display_name.trim().chars().take(80).collect(),
            email: String::new(),
            password_hash: Some(hash),
            oidc_sub: None,
            is_admin: false,
        })
        .await?;
    let Some(user) = created else {
        return invalid(t("server.error.username_taken").into_owned());
    };
    let workspace = state.db.default_workspace().await?;
    state
        .db
        .set_member(workspace.id, user.id.clone(), crate::db::Role::Member)
        .await?;
    start_session(&state, user.id, "/admin").await
}
