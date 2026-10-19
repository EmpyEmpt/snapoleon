//! Admin: own account (password, API tokens) and user management.

use crate::i18n::t;
use axum::{
    Form, Router,
    extract::{Path, Query, State},
    response::Response,
    routing::{get, post},
};
use maud::{Markup, html};
use serde::Deserialize;

use super::{Nav, NoticeQuery, checked_box, field, page, redirect_with, valid_username};
use crate::{
    auth::{self, Me},
    db::{NewUser, Role, Token},
    error::{AppError, PageResult},
    state::AppState,
    util::format_ms,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/account", get(account))
        .route("/admin/account/password", post(change_password))
        .route("/admin/tokens", post(create_token))
        .route("/admin/tokens/{id}/revoke", post(revoke_token))
        .route("/admin/users", get(users).post(create_user))
        .route("/admin/users/{id}/disabled", post(set_disabled))
        .route("/admin/users/{id}/admin", post(set_admin))
        .route("/admin/users/{id}/password", post(reset_password))
}

fn token_table(tokens: &[Token], show_owner: bool) -> Markup {
    html! {
        @if tokens.is_empty() {
            p.dim { (t("server.admin.tokens.empty")) }
        } @else {
            div.table-wrap { table {
                thead { tr { th { (t("server.admin.tokens.name")) } @if show_owner { th { (t("server.admin.items.uploader")) } } th { (t("server.admin.tokens.prefix")) } th { (t("server.admin.items.created")) } th { (t("server.admin.tokens.last_used")) } th {} } }
                tbody {
                    @for tk in tokens {
                        tr {
                            td { (tk.name) }
                            @if show_owner { td { (tk.owner) } }
                            td.mono { (tk.prefix) "…" }
                            td { (format_ms(tk.created_ms)) }
                            td { @if let Some(ms) = tk.last_used_ms { (format_ms(ms)) } @else { span.dim { (t("server.admin.never")) } } }
                            td { form method="post" action=(format!("/admin/tokens/{}/revoke", tk.id)) { button.link.danger type="submit" { (t("server.admin.tokens.revoke")) } } }
                        }
                    }
                }
            } }
        }
    }
}

async fn account_page(state: &AppState, me: &Me, notice: Markup, new_token: Option<String>) -> PageResult<Markup> {
    let tokens = state.db.list_tokens(Some(me.user.id.clone())).await?;
    let local = me.user.password_hash.is_some();
    Ok(page(
        me,
        Nav::Account,
        &t("server.admin.nav.account"),
        html! {
            (notice)
            section {
                h2 { (t("server.admin.tokens.title")) }
                p.dim { (t("server.admin.tokens.hint")) }
                @if let Some(token) = new_token {
                    div.notice.ok {
                        p { strong { (t("server.admin.tokens.created")) } }
                        div.copy-row { input.mono readonly value=(token); button.button.secondary type="button" data-copy=(token) data-done=(t("server.viewer.copied")) { (t("server.common.copy")) } }
                        p.dim { (t("server.admin.tokens.once")) }
                    }
                }
                (token_table(&tokens, false))
                form.inline method="post" action="/admin/tokens" {
                    (field(&t("server.admin.tokens.name"), html! { input type="text" name="name" required maxlength="60" placeholder=(t("server.admin.tokens.name_hint")); }))
                    button.button type="submit" { (t("server.admin.tokens.create")) }
                }
                p.dim { (t("server.admin.tokens.url_hint")) " " code.mono { (state.cfg.public_url) } }
            }
            @if local {
                section {
                    h2 { (t("server.admin.account.password")) }
                    form method="post" action="/admin/account/password" {
                        (field(&t("server.admin.account.current_password"), html! { input type="password" name="current" required autocomplete="current-password"; }))
                        (field(&t("server.admin.account.new_password"), html! { input type="password" name="password" required minlength="8" autocomplete="new-password"; }))
                        button.button type="submit" { (t("server.common.save")) }
                    }
                }
            }
        },
    ))
}

async fn account(State(state): State<AppState>, me: Me, Query(n): Query<NoticeQuery>) -> PageResult<Markup> {
    account_page(&state, &me, n.render(), None).await
}

#[derive(Deserialize)]
struct TokenForm {
    name: String,
}

async fn create_token(State(state): State<AppState>, me: Me, Form(form): Form<TokenForm>) -> PageResult<Markup> {
    let name = form.name.trim().chars().take(60).collect::<String>();
    if name.is_empty() {
        return Err(AppError::bad_request(t("server.error.name_required").into_owned()).into());
    }
    let token = state.db.create_token(me.user.id.clone(), name).await?;
    account_page(&state, &me, html! {}, Some(token)).await
}

async fn revoke_token(State(state): State<AppState>, me: Me, Path(id): Path<String>) -> PageResult<Response> {
    // Admins may revoke anyone's token; everyone else only their own.
    let owner = if me.is_admin() { None } else { Some(me.user.id.clone()) };
    state.db.revoke_token(id, owner).await?;
    Ok(redirect_with("/admin/account", "deleted"))
}

#[derive(Deserialize)]
struct PasswordForm {
    current: String,
    password: String,
}

async fn change_password(State(state): State<AppState>, me: Me, Form(form): Form<PasswordForm>) -> PageResult<Response> {
    let Some(hash) = me.user.password_hash.clone() else {
        return Err(AppError::Forbidden.into());
    };
    if !auth::verify_password(hash, form.current).await {
        return Ok(redirect_with("/admin/account", "error"));
    }
    if form.password.chars().count() < 8 {
        return Err(AppError::bad_request(t("server.error.weak_password").into_owned()).into());
    }
    let new_hash = auth::hash_password(form.password).await?;
    state.db.set_user_password(me.user.id.clone(), new_hash).await?;
    // Every session was revoked; keep this browser signed in with a fresh one.
    super::start_session(&state, me.user.id.clone(), "/admin/account?n=saved").await
}

// User management (instance admins only)

fn require_admin(me: &Me) -> Result<(), AppError> {
    if me.is_admin() { Ok(()) } else { Err(AppError::Forbidden) }
}

async fn users(State(state): State<AppState>, me: Me, Query(n): Query<NoticeQuery>) -> PageResult<Markup> {
    require_admin(&me)?;
    let users = state.db.list_users().await?;
    let workspaces = state.db.list_workspaces().await?;
    let tokens = state.db.list_tokens(None).await?;
    Ok(page(
        &me,
        Nav::Users,
        &t("server.admin.nav.users"),
        html! {
            (n.render())
            div.table-wrap { table {
                thead { tr { th { (t("server.admin.users.username")) } th { (t("server.admin.users.email")) } th { (t("server.admin.users.kind")) } th { (t("server.admin.users.status")) } th { (t("server.admin.items.created")) } th { (t("server.admin.users.actions")) } } }
                tbody {
                    @for u in &users {
                        @let own = u.id == me.user.id;
                        tr {
                            td { strong { (u.username) } @if !u.display_name.is_empty() { br; span.dim { (u.display_name) } } }
                            td { (u.email) }
                            td { @if u.password_hash.is_some() { (t("server.admin.users.local")) } @else { (t("server.admin.users.sso")) } }
                            td { @if u.is_admin { span.badge { (t("server.admin.users.admin")) } " " } @if u.disabled { span.badge.danger { (t("server.admin.users.disabled")) } } }
                            td { (format_ms(u.created_ms)) }
                            td.actions {
                                @if !own {
                                    form.inline method="post" action=(format!("/admin/users/{}/admin", u.id)) {
                                        input type="hidden" name="value" value=(if u.is_admin { "0" } else { "1" });
                                        button.link type="submit" { @if u.is_admin { (t("server.admin.users.revoke_admin")) } @else { (t("server.admin.users.make_admin")) } }
                                    }
                                    form.inline method="post" action=(format!("/admin/users/{}/disabled", u.id)) {
                                        input type="hidden" name="value" value=(if u.disabled { "0" } else { "1" });
                                        button.link.danger[!u.disabled] type="submit" { @if u.disabled { (t("server.admin.users.enable")) } @else { (t("server.admin.users.disable")) } }
                                    }
                                }
                                @if u.password_hash.is_some() {
                                    form.inline method="post" action=(format!("/admin/users/{}/password", u.id)) {
                                        input type="password" name="password" required minlength="8" placeholder=(t("server.admin.users.new_password")) autocomplete="new-password";
                                        button.link type="submit" { (t("server.admin.users.set_password")) }
                                    }
                                }
                            }
                        }
                    }
                }
            } }
            h2 { (t("server.admin.users.create")) }
            form method="post" action="/admin/users" {
                (field(&t("server.admin.users.username"), html! { input type="text" name="username" required minlength="3" maxlength="40" autocomplete="off"; }))
                (field(&t("server.admin.users.display_name"), html! { input type="text" name="display_name" maxlength="80"; }))
                (field(&t("server.admin.users.email"), html! { input type="email" name="email" maxlength="120"; }))
                (field(&t("server.admin.login.password"), html! { input type="password" name="password" required minlength="8" autocomplete="new-password"; }))
                (field(&t("server.admin.users.workspace"), html! { select name="workspace" { @for w in &workspaces { option value=(w.id) { (w.name) } } } }))
                (checked_box("is_admin", &t("server.admin.users.admin"), false))
                button.button type="submit" { (t("server.admin.users.create")) }
            }
            h2 { (t("server.admin.tokens.all")) }
            (token_table(&tokens, true))
        },
    ))
}

#[derive(Deserialize)]
struct CreateUserForm {
    username: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    email: String,
    password: String,
    workspace: String,
    is_admin: Option<String>,
}

async fn create_user(State(state): State<AppState>, me: Me, Form(form): Form<CreateUserForm>) -> PageResult<Response> {
    require_admin(&me)?;
    let username = form.username.trim().to_string();
    if !valid_username(&username) {
        return Err(AppError::bad_request(t("server.error.invalid_username").into_owned()).into());
    }
    if form.password.chars().count() < 8 {
        return Err(AppError::bad_request(t("server.error.weak_password").into_owned()).into());
    }
    let workspace = state.db.workspace(form.workspace).await?.ok_or(AppError::NotFound)?;
    let hash = auth::hash_password(form.password).await?;
    let created = state
        .db
        .create_user(NewUser {
            username,
            display_name: form.display_name.trim().chars().take(80).collect(),
            email: form.email.trim().chars().take(120).collect(),
            password_hash: Some(hash),
            oidc_sub: None,
            is_admin: form.is_admin.is_some(),
        })
        .await?;
    let user = created.ok_or_else(|| AppError::bad_request(t("server.error.username_taken").into_owned()))?;
    state.db.set_member(workspace.id, user.id, Role::Member).await?;
    Ok(redirect_with("/admin/users", "created"))
}

#[derive(Deserialize)]
struct ToggleForm {
    value: String,
}

async fn set_disabled(State(state): State<AppState>, me: Me, Path(id): Path<String>, Form(f): Form<ToggleForm>) -> PageResult<Response> {
    require_admin(&me)?;
    if id == me.user.id {
        return Err(AppError::Forbidden.into());
    }
    state.db.set_user_disabled(id, f.value == "1").await?;
    Ok(redirect_with("/admin/users", "updated"))
}

async fn set_admin(State(state): State<AppState>, me: Me, Path(id): Path<String>, Form(f): Form<ToggleForm>) -> PageResult<Response> {
    require_admin(&me)?;
    if id == me.user.id {
        return Err(AppError::Forbidden.into());
    }
    state.db.set_user_admin(id, f.value == "1").await?;
    Ok(redirect_with("/admin/users", "updated"))
}

#[derive(Deserialize)]
struct NewPasswordForm {
    password: String,
}

async fn reset_password(
    State(state): State<AppState>,
    me: Me,
    Path(id): Path<String>,
    Form(f): Form<NewPasswordForm>,
) -> PageResult<Response> {
    require_admin(&me)?;
    if f.password.chars().count() < 8 {
        return Err(AppError::bad_request(t("server.error.weak_password").into_owned()).into());
    }
    let user = state.db.user_by_id(id).await?.ok_or(AppError::NotFound)?;
    if user.password_hash.is_none() {
        return Err(AppError::Forbidden.into());
    }
    let hash = auth::hash_password(f.password).await?;
    state.db.set_user_password(user.id, hash).await?;
    Ok(redirect_with("/admin/users", "saved"))
}
