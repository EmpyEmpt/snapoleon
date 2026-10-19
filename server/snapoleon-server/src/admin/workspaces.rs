//! Admin: workspaces, members, retention/quota/expiry settings and webhooks.

use crate::i18n::t;
use axum::{
    Form, Router,
    extract::{Path, Query, State},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use maud::{Markup, html};
use serde::Deserialize;

use super::{Nav, NoticeQuery, checked_box, field, page, redirect_with, valid_username};
use crate::{
    auth::{self, Me},
    db::{NewUser, Role, Webhook, WebhookKind, Workspace},
    error::{AppError, PageResult},
    state::AppState,
    util::{format_ms, human_bytes, url_encode},
    webhooks,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/workspaces", get(list).post(create))
        .route("/admin/workspaces/{id}", get(detail))
        .route("/admin/workspaces/{id}/settings", post(save_settings))
        .route("/admin/workspaces/{id}/members", post(add_member))
        .route("/admin/workspaces/{id}/members/{uid}/role", post(set_role))
        .route("/admin/workspaces/{id}/members/{uid}/remove", post(remove_member))
        .route("/admin/workspaces/{id}/webhooks", post(add_webhook))
        .route("/admin/workspaces/{id}/webhooks/{wid}/delete", post(delete_webhook))
        .route("/admin/workspaces/{id}/webhooks/{wid}/test", post(test_webhook))
}

async fn managed(state: &AppState, me: &Me, id: &str) -> Result<Workspace, AppError> {
    let ws = state.db.workspace(id.to_string()).await?.ok_or(AppError::NotFound)?;
    if me.manages(&ws.id) { Ok(ws) } else { Err(AppError::Forbidden) }
}

/// Owners (and instance admins) may change any role; admins only deal with plain members.
fn may_change_role(me: &Me, workspace_id: &str, current: Option<Role>, new: Option<Role>) -> bool {
    if me.is_admin() {
        return true;
    }
    match me.role_in(workspace_id) {
        Some(Role::Owner) => true,
        Some(Role::Admin) => current.is_none_or(|r| r == Role::Member) && new.is_none_or(|r| r == Role::Member),
        _ => false,
    }
}

async fn list(State(state): State<AppState>, me: Me, Query(n): Query<NoticeQuery>) -> PageResult<Markup> {
    if !me.manages_any() {
        return Err(AppError::Forbidden.into());
    }
    let spaces: Vec<Workspace> =
        if me.is_admin() { state.db.list_workspaces().await? } else { me.managed_workspaces().into_iter().map(|m| m.workspace).collect() };
    let mut rows = Vec::new();
    for w in spaces {
        let usage = state.db.workspace_usage(w.id.clone()).await?;
        rows.push((w, usage));
    }
    Ok(page(
        &me,
        Nav::Workspaces,
        &t("server.admin.nav.workspaces"),
        html! {
            (n.render())
            div.table-wrap { table {
                thead { tr { th { (t("server.admin.workspaces.name")) } th { (t("server.admin.overview.items")) } th { (t("server.admin.overview.storage")) } th { (t("server.admin.workspaces.quota")) } th { (t("server.admin.workspaces.retention")) } } }
                tbody {
                    @for (w, u) in &rows {
                        tr {
                            td { a href=(format!("/admin/workspaces/{}", w.id)) { (w.name) } br; span.mono.dim { (w.slug) } }
                            td.mono { (u.items) }
                            td.mono { (human_bytes(u.bytes.max(0) as u64)) }
                            td { @if w.quota_mb > 0 { (format!("{} MB", w.quota_mb)) } @else { span.dim { (t("server.admin.unlimited")) } } }
                            td { @if w.retention_days > 0 { (w.retention_days) " d" } @else { span.dim { (t("server.admin.forever")) } } }
                        }
                    }
                }
            } }
            @if me.is_admin() {
                h2 { (t("server.admin.workspaces.create")) }
                form.inline method="post" action="/admin/workspaces" {
                    (field(&t("server.admin.workspaces.name"), html! { input type="text" name="name" required maxlength="80"; }))
                    button.button type="submit" { (t("server.admin.workspaces.create")) }
                }
            }
        },
    ))
}

#[derive(Deserialize)]
struct CreateForm {
    name: String,
}

async fn create(State(state): State<AppState>, me: Me, Form(form): Form<CreateForm>) -> PageResult<Response> {
    if !me.is_admin() {
        return Err(AppError::Forbidden.into());
    }
    let name = form.name.trim().chars().take(80).collect::<String>();
    if name.is_empty() {
        return Err(AppError::bad_request(t("server.error.name_required").into_owned()).into());
    }
    let ws = state.db.create_workspace(name, Some(me.user.id.clone())).await?;
    Ok(redirect_with(&format!("/admin/workspaces/{}", ws.id), "created"))
}

fn role_label(r: Role) -> String {
    t(&format!("server.admin.role.{}", r.as_str())).into_owned()
}

async fn detail(State(state): State<AppState>, me: Me, Path(id): Path<String>, Query(n): Query<NoticeQuery>) -> PageResult<Markup> {
    let ws = managed(&state, &me, &id).await?;
    let members = state.db.members(ws.id.clone()).await?;
    let hooks = state.db.webhooks(ws.id.clone()).await?;
    let usage = state.db.workspace_usage(ws.id.clone()).await?;
    let base = format!("/admin/workspaces/{}", ws.id);
    Ok(page(
        &me,
        Nav::Workspaces,
        &ws.name,
        html! {
            (n.render())
            p.dim { (usage.items) " · " (human_bytes(usage.bytes.max(0) as u64)) }

            section {
                h2 { (t("server.admin.workspaces.settings")) }
                form method="post" action=(format!("{base}/settings")) {
                    (field(&t("server.admin.workspaces.name"), html! { input type="text" name="name" value=(ws.name) required maxlength="80"; }))
                    (field(&t("server.admin.workspaces.retention_label"), html! { input type="number" name="retention_days" min="0" value=(ws.retention_days); }))
                    p.hint { (t("server.admin.workspaces.retention_hint")) }
                    (field(&t("server.admin.workspaces.quota_label"), html! { input type="number" name="quota_mb" min="0" value=(ws.quota_mb); }))
                    p.hint { (t("server.admin.workspaces.quota_hint")) }
                    (field(&t("server.admin.workspaces.expiry_label"), html! { input type="number" name="default_expiry_hours" min="0" value=(ws.default_expiry_hours); }))
                    p.hint { (t("server.admin.workspaces.expiry_hint")) }
                    button.button type="submit" { (t("server.common.save")) }
                }
            }

            section {
                h2 { (t("server.admin.workspaces.members")) }
                div.table-wrap { table {
                    thead { tr { th { (t("server.admin.users.username")) } th { (t("server.admin.workspaces.role")) } th {} } }
                    tbody {
                        @for m in &members {
                            tr {
                                td { strong { (m.user.username) } @if !m.user.display_name.is_empty() { " " span.dim { (m.user.display_name) } } }
                                td {
                                    @if may_change_role(&me, &ws.id, Some(m.role), Some(m.role)) {
                                        form.inline method="post" action=(format!("{base}/members/{}/role", m.user.id)) {
                                            select name="role" { @for r in [Role::Member, Role::Admin, Role::Owner] { option value=(r.as_str()) selected[r == m.role] { (role_label(r)) } } }
                                            button.link type="submit" { (t("server.common.save")) }
                                        }
                                    } @else { (role_label(m.role)) }
                                }
                                td {
                                    @if may_change_role(&me, &ws.id, Some(m.role), None) {
                                        form method="post" action=(format!("{base}/members/{}/remove", m.user.id)) { button.link.danger type="submit" { (t("server.admin.workspaces.remove")) } }
                                    }
                                }
                            }
                        }
                    }
                } }
                h3 { (t("server.admin.workspaces.add_member")) }
                form method="post" action=(format!("{base}/members")) {
                    (field(&t("server.admin.users.username"), html! { input type="text" name="username" required maxlength="40"; }))
                    (field(&t("server.admin.workspaces.role"), html! { select name="role" { @for r in [Role::Member, Role::Admin, Role::Owner] { option value=(r.as_str()) { (role_label(r)) } } } }))
                    (field(&t("server.admin.workspaces.new_user_password"), html! { input type="password" name="password" minlength="8" autocomplete="new-password"; }))
                    p.hint { (t("server.admin.workspaces.new_user_hint")) }
                    button.button type="submit" { (t("server.admin.workspaces.add_member")) }
                }
            }

            section {
                h2 { (t("server.admin.webhooks.title")) }
                p.dim { (t("server.admin.webhooks.hint")) }
                @if hooks.is_empty() {
                    p.dim { (t("server.admin.webhooks.empty")) }
                } @else {
                    div.table-wrap { table {
                        thead { tr { th { (t("server.admin.webhooks.kind")) } th { (t("server.admin.webhooks.target")) } th { (t("server.admin.webhooks.events")) } th {} } }
                        tbody {
                            @for h in &hooks {
                                tr {
                                    td { (t(&format!("server.admin.webhooks.kind_{}", h.kind.as_str()))) }
                                    td.truncate { (hook_target(h)) }
                                    td { @if h.on_view { span.badge { (t("server.admin.webhooks.on_view")) } " " } @if h.on_comment { span.badge { (t("server.admin.webhooks.on_comment")) } } }
                                    td.actions {
                                        form.inline method="post" action=(format!("{base}/webhooks/{}/test", h.id)) { button.link type="submit" { (t("server.admin.webhooks.test")) } }
                                        form.inline method="post" action=(format!("{base}/webhooks/{}/delete", h.id)) { button.link.danger type="submit" { (t("server.common.delete")) } }
                                    }
                                }
                            }
                        }
                    } }
                }
                h3 { (t("server.admin.webhooks.add")) }
                form method="post" action=(format!("{base}/webhooks")) {
                    (field(&t("server.admin.webhooks.kind"), html! { select name="kind" {
                        option value="generic" { (t("server.admin.webhooks.kind_generic")) }
                        option value="slack" { (t("server.admin.webhooks.kind_slack")) }
                        option value="telegram" { (t("server.admin.webhooks.kind_telegram")) }
                    } }))
                    (field(&t("server.admin.webhooks.target"), html! { input type="text" name="target" required maxlength="500"; }))
                    p.hint { (t("server.admin.webhooks.target_hint")) }
                    (field(&t("server.admin.webhooks.secret"), html! { input type="password" name="secret" maxlength="200" autocomplete="off"; }))
                    p.hint { (t("server.admin.webhooks.secret_hint")) }
                    (checked_box("on_view", &t("server.admin.webhooks.on_view"), true))
                    (checked_box("on_comment", &t("server.admin.webhooks.on_comment"), true))
                    button.button type="submit" { (t("server.admin.webhooks.add")) }
                }
            }
            p.dim { (t("server.admin.workspaces.created")) " " (format_ms(ws.created_ms)) }
        },
    ))
}

/// Webhook target as shown to admins; URLs may embed tokens, so only scheme and host are listed.
fn hook_target(h: &Webhook) -> String {
    match h.kind {
        WebhookKind::Telegram => h.target.clone(),
        _ => {
            let (scheme, rest) = h.target.split_once("://").unwrap_or(("", &h.target));
            let host = rest.split('/').next().unwrap_or_default();
            if scheme.is_empty() { host.to_string() } else { format!("{scheme}://{host}/…") }
        }
    }
}

#[derive(Deserialize)]
struct SettingsForm {
    name: String,
    #[serde(default)]
    retention_days: String,
    #[serde(default)]
    quota_mb: String,
    #[serde(default)]
    default_expiry_hours: String,
}

fn number(s: &str) -> i64 {
    s.trim().parse::<i64>().unwrap_or(0).clamp(0, 1_000_000)
}

async fn save_settings(State(state): State<AppState>, me: Me, Path(id): Path<String>, Form(f): Form<SettingsForm>) -> PageResult<Response> {
    let ws = managed(&state, &me, &id).await?;
    let name = f.name.trim().chars().take(80).collect::<String>();
    if name.is_empty() {
        return Err(AppError::bad_request(t("server.error.name_required").into_owned()).into());
    }
    state.db.update_workspace(ws.id.clone(), name, number(&f.retention_days), number(&f.quota_mb), number(&f.default_expiry_hours)).await?;
    Ok(redirect_with(&format!("/admin/workspaces/{}", ws.id), "saved"))
}

#[derive(Deserialize)]
struct AddMemberForm {
    username: String,
    role: String,
    #[serde(default)]
    password: String,
}

async fn add_member(State(state): State<AppState>, me: Me, Path(id): Path<String>, Form(f): Form<AddMemberForm>) -> PageResult<Response> {
    let ws = managed(&state, &me, &id).await?;
    let role = Role::parse(&f.role).ok_or_else(|| AppError::bad_request(t("server.error.bad_role").into_owned()))?;
    if !may_change_role(&me, &ws.id, None, Some(role)) {
        return Err(AppError::Forbidden.into());
    }
    let username = f.username.trim().to_string();
    let user = match state.db.user_by_username(username.clone()).await? {
        Some(u) => u,
        None => {
            // Unknown name: create a local account when a password was provided.
            if !valid_username(&username) || f.password.chars().count() < 8 {
                return Err(AppError::bad_request(t("server.error.unknown_user").into_owned()).into());
            }
            let hash = auth::hash_password(f.password).await?;
            state
                .db
                .create_user(NewUser {
                    username,
                    display_name: String::new(),
                    email: String::new(),
                    password_hash: Some(hash),
                    oidc_sub: None,
                    is_admin: false,
                })
                .await?
                .ok_or_else(|| AppError::bad_request(t("server.error.username_taken").into_owned()))?
        }
    };
    state.db.set_member(ws.id.clone(), user.id, role).await?;
    Ok(redirect_with(&format!("/admin/workspaces/{}", ws.id), "saved"))
}

#[derive(Deserialize)]
struct RoleForm {
    role: String,
}

async fn set_role(
    State(state): State<AppState>,
    me: Me,
    Path((id, uid)): Path<(String, String)>,
    Form(f): Form<RoleForm>,
) -> PageResult<Response> {
    let ws = managed(&state, &me, &id).await?;
    let new = Role::parse(&f.role).ok_or_else(|| AppError::bad_request(t("server.error.bad_role").into_owned()))?;
    let current =
        state.db.members(ws.id.clone()).await?.into_iter().find(|m| m.user.id == uid).map(|m| m.role).ok_or(AppError::NotFound)?;
    if !may_change_role(&me, &ws.id, Some(current), Some(new)) {
        return Err(AppError::Forbidden.into());
    }
    state.db.change_member(ws.id.clone(), uid, Some(new)).await?;
    Ok(redirect_with(&format!("/admin/workspaces/{}", ws.id), "saved"))
}

async fn remove_member(State(state): State<AppState>, me: Me, Path((id, uid)): Path<(String, String)>) -> PageResult<Response> {
    let ws = managed(&state, &me, &id).await?;
    let current =
        state.db.members(ws.id.clone()).await?.into_iter().find(|m| m.user.id == uid).map(|m| m.role).ok_or(AppError::NotFound)?;
    if !may_change_role(&me, &ws.id, Some(current), None) {
        return Err(AppError::Forbidden.into());
    }
    state.db.change_member(ws.id.clone(), uid, None).await?;
    Ok(redirect_with(&format!("/admin/workspaces/{}", ws.id), "deleted"))
}

#[derive(Deserialize)]
struct WebhookForm {
    kind: String,
    target: String,
    #[serde(default)]
    secret: String,
    on_view: Option<String>,
    on_comment: Option<String>,
}

async fn add_webhook(State(state): State<AppState>, me: Me, Path(id): Path<String>, Form(f): Form<WebhookForm>) -> PageResult<Response> {
    let ws = managed(&state, &me, &id).await?;
    let kind = WebhookKind::parse(&f.kind).ok_or_else(|| AppError::bad_request(t("server.error.bad_webhook").into_owned()))?;
    let target = f.target.trim().to_string();
    let secret = f.secret.trim().to_string();
    let valid = match kind {
        WebhookKind::Telegram => !target.is_empty() && !secret.is_empty(),
        _ => target.starts_with("https://") || target.starts_with("http://"),
    };
    if !valid {
        return Err(AppError::bad_request(t("server.error.bad_webhook").into_owned()).into());
    }
    state
        .db
        .add_webhook(Webhook {
            id: String::new(),
            workspace_id: ws.id.clone(),
            kind,
            target,
            secret,
            on_view: f.on_view.is_some(),
            on_comment: f.on_comment.is_some(),
            enabled: true,
        })
        .await?;
    Ok(redirect_with(&format!("/admin/workspaces/{}", ws.id), "created"))
}

async fn delete_webhook(State(state): State<AppState>, me: Me, Path((id, wid)): Path<(String, String)>) -> PageResult<Response> {
    let ws = managed(&state, &me, &id).await?;
    state.db.delete_webhook(ws.id.clone(), wid).await?;
    Ok(redirect_with(&format!("/admin/workspaces/{}", ws.id), "deleted"))
}

async fn test_webhook(State(state): State<AppState>, me: Me, Path((id, wid)): Path<(String, String)>) -> PageResult<Response> {
    let ws = managed(&state, &me, &id).await?;
    let hook = state.db.webhook(ws.id.clone(), wid).await?.ok_or(AppError::NotFound)?;
    let base = format!("/admin/workspaces/{}", ws.id);
    let payload = webhooks::sample_payload(&state, &ws.id);
    Ok(match webhooks::deliver(&state, &hook, &payload).await {
        Ok(()) => redirect_with(&base, "test_ok"),
        Err(e) => Redirect::to(&format!("{base}?n=test_failed&d={}", url_encode(&e))).into_response(),
    })
}
