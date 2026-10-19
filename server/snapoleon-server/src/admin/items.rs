//! Admin: items (search, detail, delete) and short links.

use axum::{
    Router,
    extract::{Path, Query, State},
    response::Response,
    routing::{get, post},
};
use maud::{Markup, html};
use serde::Deserialize;
use crate::i18n::{t, tf};

use super::{Nav, NoticeQuery, page, redirect_with};
use crate::{
    auth::Me,
    db::Item,
    error::{AppError, PageResult},
    state::AppState,
    util::{format_ms, human_bytes, url_encode},
    viewer,
};

const PAGE_SIZE: i64 = 25;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/items", get(list))
        .route("/admin/items/{id}", get(detail))
        .route("/admin/items/{id}/delete", post(delete))
        .route(
            "/admin/items/{id}/comments/{cid}/delete",
            post(delete_comment),
        )
        .route("/admin/links", get(links))
        .route("/admin/links/{code}/delete", post(delete_link))
}

#[derive(Deserialize, Default)]
struct ListQuery {
    q: Option<String>,
    page: Option<i64>,
    #[serde(flatten)]
    notice: NoticeQuery,
}

/// Previous / next links for a result set.
pub fn pager(base: &str, q: &str, page: i64, total: i64) -> Markup {
    let pages = (total + PAGE_SIZE - 1) / PAGE_SIZE;
    let href = |p: i64| format!("{base}?page={p}&q={}", url_encode(q));
    html! {
        @if pages > 1 {
            nav.pager {
                @if page > 1 { a.button.secondary href=(href(page - 1)) { (t("server.admin.prev")) } }
                span.dim { (tf("server.admin.page_of", &[("page", &page.to_string()), ("pages", &pages.to_string())])) }
                @if page < pages { a.button.secondary href=(href(page + 1)) { (t("server.admin.next")) } }
            }
        }
    }
}

async fn list(
    State(state): State<AppState>,
    me: Me,
    Query(q): Query<ListQuery>,
) -> PageResult<Markup> {
    let page_no = q.page.unwrap_or(1).max(1);
    let query = q.q.unwrap_or_default().trim().to_string();
    let (items, total) = state
        .db
        .list_items(
            me.scope(),
            query.clone(),
            PAGE_SIZE,
            (page_no - 1) * PAGE_SIZE,
        )
        .await?;
    Ok(page(
        &me,
        Nav::Items,
        &t("server.admin.nav.items"),
        html! {
            (q.notice.render())
            form.search method="get" action="/admin/items" {
                input type="search" name="q" value=(query) placeholder=(t("server.admin.items.search"));
                button.button.secondary type="submit" { (t("server.admin.search")) }
            }
            @if items.is_empty() {
                p.dim { (t("server.admin.items.empty")) }
            } @else {
                div.table-wrap { table {
                    thead { tr { th {} th { (t("server.admin.items.title")) } th { (t("server.admin.items.uploader")) } th { (t("server.admin.items.size")) } th { (t("server.admin.items.views")) } th { (t("server.admin.items.comments")) } th { (t("server.admin.items.created")) } th { (t("server.admin.items.expires")) } } }
                    tbody {
                        @for i in &items {
                            tr {
                                td.thumb-cell { @if i.thumb_key.is_some() && !i.is_protected() { img.thumb src=(format!("/{}/thumb.jpg", i.code)) alt="" loading="lazy"; } }
                                td { a href=(format!("/admin/items/{}", i.id)) { (i.display_title()) } " " @if i.is_protected() { span.badge { (t("server.admin.items.protected")) } } br; span.mono.dim { (i.code) } }
                                td { (i.uploader) }
                                td.mono { (human_bytes(i.bytes.max(0) as u64)) }
                                td.mono { (i.views) }
                                td.mono { (i.comments) }
                                td { (format_ms(i.created_ms)) }
                                td { @if let Some(e) = i.expires_ms { (format_ms(e)) } @else { span.dim { "-" } } }
                            }
                        }
                    }
                } }
                (pager("/admin/items", &query, page_no, total))
            }
        },
    ))
}

async fn visible_item(state: &AppState, me: &Me, id: String) -> Result<Item, AppError> {
    let item = state.db.item_by_id(id).await?.ok_or(AppError::NotFound)?;
    if me.can_manage_item(&item) {
        Ok(item)
    } else {
        Err(AppError::Forbidden)
    }
}

async fn detail(
    State(state): State<AppState>,
    me: Me,
    Path(id): Path<String>,
    Query(notice): Query<NoticeQuery>,
) -> PageResult<Markup> {
    let item = visible_item(&state, &me, id).await?;
    let comments = state.db.comments(item.id.clone()).await?;
    let page_url = state.item_url(&item.code);
    let delete_url = state.url(&format!("/{}/delete/{}", item.code, item.delete_secret));
    Ok(page(
        &me,
        Nav::Items,
        item.display_title(),
        html! {
            (notice.render())
            div.detail {
                div.preview {
                    @if item.is_video() { video controls preload="metadata" src=(state.raw_url(&item.code)) {} }
                    @else if item.is_protected() { p.dim { (t("server.admin.items.protected_preview")) } }
                    @else { img src=(state.raw_url(&item.code)) alt=(item.display_title()); }
                }
                dl.facts {
                    dt { (t("server.admin.items.uploader")) } dd { (item.uploader) }
                    dt { (t("server.admin.items.created")) } dd { (format_ms(item.created_ms)) " UTC" }
                    dt { (t("server.admin.items.expires")) } dd { @if let Some(e) = item.expires_ms { (format_ms(e)) " UTC" } @else { (t("server.admin.never")) } }
                    dt { (t("server.admin.items.size")) } dd.mono { (human_bytes(item.bytes.max(0) as u64)) " · " (item.mime) @if item.width > 0 { " · " (item.width) " × " (item.height) } }
                    dt { (t("server.admin.items.views")) } dd.mono { (item.views) }
                    dt { (t("server.admin.items.link")) } dd { div.copy-row { input.mono readonly value=(page_url); button.button.secondary type="button" data-copy=(page_url) data-done=(t("server.viewer.copied")) { (t("server.common.copy")) } } }
                    dt { (t("server.admin.items.delete_link")) } dd { div.copy-row { input.mono readonly value=(delete_url); button.button.secondary type="button" data-copy=(delete_url) data-done=(t("server.viewer.copied")) { (t("server.common.copy")) } } }
                }
            }
            h2 { (t("server.viewer.comments")) }
            @if comments.is_empty() { p.dim { (t("server.viewer.no_comments")) } }
            @for c in &comments {
                article.comment {
                    header { strong { (c.name) } " " span.dim { (format_ms(c.created_ms)) } }
                    p { (c.text) }
                    form method="post" action=(format!("/admin/items/{}/comments/{}/delete", item.id, c.id)) { button.link.danger type="submit" { (t("server.common.delete")) } }
                }
            }
            h2 { (t("server.admin.items.danger")) }
            form method="post" action=(format!("/admin/items/{}/delete", item.id)) {
                p.dim { (t("server.admin.items.delete_hint")) }
                button.button.danger type="submit" { (t("server.admin.items.delete")) }
            }
        },
    ))
}

async fn delete(
    State(state): State<AppState>,
    me: Me,
    Path(id): Path<String>,
) -> PageResult<Response> {
    let item = visible_item(&state, &me, id).await?;
    viewer::remove_item(&state, &item).await?;
    Ok(redirect_with("/admin/items", "deleted"))
}

#[derive(Deserialize)]
struct CommentPath {
    id: String,
    cid: i64,
}

async fn delete_comment(
    State(state): State<AppState>,
    me: Me,
    Path(p): Path<CommentPath>,
) -> PageResult<Response> {
    let item = visible_item(&state, &me, p.id).await?;
    state.db.delete_comment(item.id.clone(), p.cid).await?;
    Ok(redirect_with(
        &format!("/admin/items/{}", item.id),
        "deleted",
    ))
}

async fn links(
    State(state): State<AppState>,
    me: Me,
    Query(q): Query<ListQuery>,
) -> PageResult<Markup> {
    let page_no = q.page.unwrap_or(1).max(1);
    let (links, total) = state
        .db
        .list_links(me.scope(), PAGE_SIZE, (page_no - 1) * PAGE_SIZE)
        .await?;
    Ok(page(
        &me,
        Nav::Links,
        &t("server.admin.nav.links"),
        html! {
            (q.notice.render())
            @if links.is_empty() {
                p.dim { (t("server.admin.links.empty")) }
            } @else {
                div.table-wrap { table {
                    thead { tr { th { (t("server.admin.links.short")) } th { (t("server.admin.links.target")) } th { (t("server.admin.items.uploader")) } th { (t("server.admin.links.clicks")) } th { (t("server.admin.items.created")) } th {} } }
                    tbody {
                        @for l in &links {
                            tr {
                                td.mono { a href=(format!("/s/{}", l.code)) { (state.url(&format!("/s/{}", l.code))) } }
                                td.truncate { (l.url) }
                                td { (l.owner) }
                                td.mono { (l.clicks) }
                                td { (format_ms(l.created_ms)) }
                                td { form method="post" action=(format!("/admin/links/{}/delete", l.code)) { button.link.danger type="submit" { (t("server.common.delete")) } } }
                            }
                        }
                    }
                } }
                (pager("/admin/links", "", page_no, total))
            }
        },
    ))
}

async fn delete_link(
    State(state): State<AppState>,
    me: Me,
    Path(code): Path<String>,
) -> PageResult<Response> {
    if !state.db.delete_link(code, me.scope()).await? {
        return Err(AppError::NotFound.into());
    }
    Ok(redirect_with("/admin/links", "deleted"))
}
