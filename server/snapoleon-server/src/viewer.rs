//! Public pages: viewer, raw/thumb/preview delivery, comments, unlock,
//! delete-by-secret and short-link redirects.

use std::{ops::Range, time::Duration};

use axum::{
    Form, Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use bytes::Bytes;
use serde::Deserialize;
use crate::i18n::t;
use snap_core::now_ms;
use tokio::sync::OnceCell;

use crate::{
    auth::{self, cookie_value, set_cookie},
    db::Item,
    error::{AppError, PageError, PageResult},
    og::{self, Card, Hero},
    state::{AppState, Client},
    storage::{self, PUBLIC_CACHE, PutMeta, SHORT_CACHE},
    util::{self, HOUR_MS, human_bytes, truncate},
    views::{self, Links, ViewerState},
    webhooks::{self, EventKind},
};

const UNLOCK_TTL_SECS: i64 = 7 * 24 * 3600;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/{code}", get(view))
        .route("/{code}/raw", get(raw))
        .route("/{code}/thumb.jpg", get(thumb))
        .route("/{code}/og.png", get(preview))
        .route("/{code}/unlock", axum::routing::post(unlock))
        .route("/{code}/comment", axum::routing::post(comment))
        .route(
            "/{code}/delete/{secret}",
            get(delete_confirm).post(delete_now),
        )
        .route("/s/{code}", get(short_link))
}

fn valid_code(code: &str) -> bool {
    (4..=24).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_alphanumeric())
}

async fn load(state: &AppState, code: &str) -> Result<Item, AppError> {
    if !valid_code(code) {
        return Err(AppError::NotFound);
    }
    let item = state
        .db
        .item_by_code(code.to_string())
        .await?
        .ok_or(AppError::NotFound)?;
    if item.is_expired(now_ms()) {
        Err(AppError::Gone)
    } else {
        Ok(item)
    }
}

fn unlock_cookie_name(code: &str) -> String {
    format!("snap_u_{code}")
}

fn unlock_value(state: &AppState, item: &Item) -> String {
    state.sign(
        "unlock",
        &format!(
            "{}:{}",
            item.id,
            item.password_hash.as_deref().unwrap_or_default()
        ),
    )
}

/// Open items, or protected ones the visitor has unlocked (cookie) or owns (Bearer token).
async fn can_access(state: &AppState, headers: &HeaderMap, item: &Item) -> bool {
    if !item.is_protected() {
        return true;
    }
    if cookie_value(headers, &unlock_cookie_name(&item.code))
        .is_some_and(|c| util::ct_eq(&c, &unlock_value(state, item)))
    {
        return true;
    }
    auth::optional_api_user(headers, state)
        .await
        .is_some_and(|u| u.id == item.user_id || u.is_admin)
}

pub fn links_for(state: &AppState, item: &Item) -> Links {
    Links {
        page: state.item_url(&item.code),
        raw: state.raw_url(&item.code),
        og: state.url(&format!("/{}/og.png", item.code)),
        thumb: item
            .thumb_key
            .as_ref()
            .map(|_| state.url(&format!("/{}/thumb.jpg", item.code))),
    }
}

struct Flash {
    wrong_password: bool,
    comment_error: Option<String>,
    comment_sent: bool,
}

async fn render(
    state: &AppState,
    headers: &HeaderMap,
    item: &Item,
    flash: Flash,
    status: StatusCode,
) -> PageResult<Response> {
    let unlocked = can_access(state, headers, item).await;
    let comments = if unlocked {
        state.db.comments(item.id.clone()).await?
    } else {
        Vec::new()
    };
    let page = views::viewer_page(&ViewerState {
        item,
        links: &links_for(state, item),
        comments: &comments,
        unlocked,
        wrong_password: flash.wrong_password,
        comment_error: flash.comment_error,
        comment_sent: flash.comment_sent,
    });
    Ok((status, [(header::CACHE_CONTROL, "no-cache")], page).into_response())
}

#[derive(Deserialize, Default)]
struct ViewQuery {
    sent: Option<String>,
}

async fn view(
    State(state): State<AppState>,
    Path(code): Path<String>,
    client: Client,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<ViewQuery>,
) -> PageResult<Response> {
    let item = load(&state, &code).await?;
    let unlocked = can_access(&state, &headers, &item).await;
    if unlocked && !client.is_bot() {
        let visitor = client.visitor(&state);
        let bucket = now_ms() / HOUR_MS;
        if state
            .db
            .register_view(item.id.clone(), visitor, bucket)
            .await?
        {
            let mut viewed = item.clone();
            viewed.views += 1;
            webhooks::emit(&state, &viewed, EventKind::View, "", "").await;
        }
    }
    let flash = Flash {
        wrong_password: false,
        comment_error: None,
        comment_sent: q.sent.is_some(),
    };
    render(&state, &headers, &item, flash, StatusCode::OK).await
}

#[derive(Deserialize)]
struct UnlockForm {
    password: String,
}

async fn unlock(
    State(state): State<AppState>,
    Path(code): Path<String>,
    client: Client,
    headers: HeaderMap,
    Form(form): Form<UnlockForm>,
) -> PageResult<Response> {
    let item = load(&state, &code).await?;
    let Some(hash) = item.password_hash.clone() else {
        return Ok(Redirect::to(&format!("/{code}")).into_response());
    };
    state.limiter.check(
        &format!("unlock:{}:{}", client.ip, item.id),
        8,
        Duration::from_secs(600),
    )?;
    if !auth::verify_password(hash, form.password).await {
        let flash = Flash {
            wrong_password: true,
            comment_error: None,
            comment_sent: false,
        };
        return render(&state, &headers, &item, flash, StatusCode::FORBIDDEN).await;
    }
    let mut resp = Redirect::to(&format!("/{code}")).into_response();
    resp.headers_mut().append(
        header::SET_COOKIE,
        set_cookie(
            &unlock_cookie_name(&code),
            &unlock_value(&state, &item),
            UNLOCK_TTL_SECS,
            &format!("/{code}"),
            state.cfg.secure_cookies(),
        ),
    );
    Ok(resp)
}

#[derive(Deserialize)]
struct CommentForm {
    name: String,
    text: String,
    #[serde(default)]
    website: String,
}

fn clean_text(s: &str, max: usize) -> String {
    truncate(
        s.chars()
            .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
            .collect::<String>()
            .trim(),
        max,
    )
}

async fn comment(
    State(state): State<AppState>,
    Path(code): Path<String>,
    client: Client,
    headers: HeaderMap,
    Form(form): Form<CommentForm>,
) -> PageResult<Response> {
    let item = load(&state, &code).await?;
    if !can_access(&state, &headers, &item).await {
        return Err(AppError::Forbidden.into());
    }
    // Bots fill the hidden field; act as if it worked.
    if !form.website.is_empty() {
        return Ok(Redirect::to(&format!("/{code}?sent=1#comments")).into_response());
    }
    let visitor = client.visitor(&state);
    let limited = !state
        .limiter
        .allow(&format!("comment:{visitor}"), 4, Duration::from_secs(60))
        || !state.limiter.allow(
            &format!("comment-item:{}", item.id),
            40,
            Duration::from_secs(3600),
        );
    if limited {
        let flash = Flash {
            wrong_password: false,
            comment_error: Some(AppError::TooManyRequests.message()),
            comment_sent: false,
        };
        return render(
            &state,
            &headers,
            &item,
            flash,
            StatusCode::TOO_MANY_REQUESTS,
        )
        .await;
    }
    let name = clean_text(&form.name, 60).replace('\n', " ");
    let text = clean_text(&form.text, 2000);
    if name.is_empty() || text.is_empty() {
        let flash = Flash {
            wrong_password: false,
            comment_error: Some(t("server.viewer.comment_empty").into_owned()),
            comment_sent: false,
        };
        return render(&state, &headers, &item, flash, StatusCode::BAD_REQUEST).await;
    }
    state
        .db
        .add_comment(item.id.clone(), name.clone(), text.clone())
        .await?;
    let mut commented = item.clone();
    commented.comments += 1;
    webhooks::emit(&state, &commented, EventKind::Comment, &name, &text).await;
    Ok(Redirect::to(&format!("/{code}?sent=1#comments")).into_response())
}

// Blob delivery

enum RangeRequest {
    Full,
    Partial(Range<u64>),
    Unsatisfiable,
}

/// Parse a single-range `Range: bytes=` header against an object of `len` bytes.
fn parse_range(value: Option<&str>, len: u64) -> RangeRequest {
    let Some(spec) = value.and_then(|v| v.trim().strip_prefix("bytes=")) else {
        return RangeRequest::Full;
    };
    if spec.contains(',') {
        return RangeRequest::Full;
    }
    let Some((start, end)) = spec.split_once('-') else {
        return RangeRequest::Full;
    };
    let (start, end) = (start.trim(), end.trim());
    let range = if start.is_empty() {
        // Suffix range: the last N bytes.
        match end.parse::<u64>() {
            Ok(n) if n > 0 && len > 0 => Some(len.saturating_sub(n)..len),
            _ => None,
        }
    } else {
        match (start.parse::<u64>(), end.parse::<u64>(), end.is_empty()) {
            (Ok(s), _, true) if s < len => Some(s..len),
            (Ok(s), Ok(e), false) if s <= e && s < len => Some(s..(e + 1).min(len)),
            _ => None,
        }
    };
    range.map_or(RangeRequest::Unsatisfiable, RangeRequest::Partial)
}

fn header_value(s: &str) -> HeaderValue {
    HeaderValue::from_str(s).unwrap_or_else(|_| HeaderValue::from_static(""))
}

fn content_disposition(item: &Item, download: bool) -> HeaderValue {
    let ext = util::sniff_ext(&item.mime);
    let base = if item.file_name.is_empty() {
        format!("{}.{ext}", item.code)
    } else {
        item.file_name.clone()
    };
    let ascii: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .filter(|c| *c != '"' && *c != '\\')
        .collect();
    let kind = if download { "attachment" } else { "inline" };
    header_value(&format!(
        "{kind}; filename=\"{ascii}\"; filename*=UTF-8''{}",
        util::url_encode(&base)
    ))
}

struct BlobResponse<'a> {
    key: &'a str,
    mime: &'a str,
    len: u64,
    etag: String,
    cache: &'a str,
    disposition: Option<HeaderValue>,
}

async fn serve_blob(
    state: &AppState,
    headers: &HeaderMap,
    b: BlobResponse<'_>,
) -> Result<Response, AppError> {
    let etag = format!("\"{}\"", b.etag);
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',')
                .any(|t| t.trim().trim_start_matches("W/") == etag)
        })
    {
        return Ok((
            StatusCode::NOT_MODIFIED,
            [
                (header::ETAG, header_value(&etag)),
                (header::CACHE_CONTROL, header_value(b.cache)),
            ],
        )
            .into_response());
    }
    let (status, range) = match parse_range(
        headers.get(header::RANGE).and_then(|v| v.to_str().ok()),
        b.len,
    ) {
        RangeRequest::Full => (StatusCode::OK, 0..b.len),
        RangeRequest::Partial(r) => (StatusCode::PARTIAL_CONTENT, r),
        RangeRequest::Unsatisfiable => return Err(AppError::RangeNotSatisfiable(b.len)),
    };
    let result = state
        .blobs
        .get(
            b.key,
            (status == StatusCode::PARTIAL_CONTENT).then(|| range.clone()),
        )
        .await?;
    let mut resp = Response::new(Body::from_stream(result.into_stream()));
    *resp.status_mut() = status;
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, header_value(b.mime));
    h.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from(range.end - range.start),
    );
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(header::ETAG, header_value(&etag));
    h.insert(header::CACHE_CONTROL, header_value(b.cache));
    h.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    if status == StatusCode::PARTIAL_CONTENT {
        h.insert(
            header::CONTENT_RANGE,
            header_value(&format!(
                "bytes {}-{}/{}",
                range.start,
                range.end - 1,
                b.len
            )),
        );
    }
    if let Some(d) = b.disposition {
        h.insert(header::CONTENT_DISPOSITION, d);
    }
    Ok(resp)
}

fn cache_for(item: &Item) -> &'static str {
    if item.is_protected() {
        "private, max-age=600"
    } else if item.expires_ms.is_some() {
        SHORT_CACHE
    } else {
        PUBLIC_CACHE
    }
}

#[derive(Deserialize, Default)]
struct RawQuery {
    download: Option<String>,
}

async fn raw(
    State(state): State<AppState>,
    Path(code): Path<String>,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<RawQuery>,
) -> PageResult<Response> {
    let item = load(&state, &code).await?;
    if !can_access(&state, &headers, &item).await {
        return Err(AppError::Forbidden.into());
    }
    let resp = serve_blob(
        &state,
        &headers,
        BlobResponse {
            key: &item.blob_key,
            mime: &item.mime,
            len: item.bytes.max(0) as u64,
            etag: item.id.clone(),
            cache: cache_for(&item),
            disposition: Some(content_disposition(&item, q.download.is_some())),
        },
    )
    .await?;
    Ok(resp)
}

async fn thumb(
    State(state): State<AppState>,
    Path(code): Path<String>,
    headers: HeaderMap,
) -> PageResult<Response> {
    let item = load(&state, &code).await?;
    if !can_access(&state, &headers, &item).await {
        return Err(AppError::Forbidden.into());
    }
    let key = item.thumb_key.as_deref().ok_or(AppError::NotFound)?;
    let len = state.blobs.size(key).await?;
    Ok(serve_blob(
        &state,
        &headers,
        BlobResponse {
            key,
            mime: "image/jpeg",
            len,
            etag: format!("{}-t", item.id),
            cache: cache_for(&item),
            disposition: None,
        },
    )
    .await?)
}

/// Render a text-only card (no item image) on a blocking thread.
async fn text_card(
    hero: Hero,
    title_key: &'static str,
    subtitle_key: &'static str,
) -> Result<Bytes, AppError> {
    let png = tokio::task::spawn_blocking(move || {
        let (title, subtitle) = (t(title_key), t(subtitle_key));
        og::render_card(&Card {
            hero,
            image: None,
            title: &title,
            subtitle: &subtitle,
        })
        .and_then(|img| og::encode_png(&img))
    })
    .await?
    .map_err(AppError::Internal)?;
    Ok(Bytes::from(png))
}

/// Placeholder card for protected items; rendered once per process.
async fn locked_card() -> Result<Bytes, AppError> {
    static CARD: OnceCell<Bytes> = OnceCell::const_new();
    CARD.get_or_try_init(|| {
        text_card(
            Hero::Locked,
            "server.og.protected",
            "server.og.protected_hint",
        )
    })
    .await
    .cloned()
}

/// Preview card for the server's start page; rendered once per process.
pub async fn site_card() -> PageResult<Response> {
    static CARD: OnceCell<Bytes> = OnceCell::const_new();
    let png = CARD
        .get_or_try_init(|| text_card(Hero::Brand, "server.home.title", "server.home.tagline"))
        .await?
        .clone();
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        png,
    )
        .into_response())
}

/// Render and store the preview card for a public item.
pub async fn ensure_preview(state: &AppState, item: &Item) -> Result<Bytes, AppError> {
    let key = storage::og_key(item);
    if state.blobs.exists(&key).await {
        return state.blobs.read(&key).await;
    }
    let source = if item.is_video() {
        None
    } else if item.bytes > 40 * 1024 * 1024 {
        match &item.thumb_key {
            Some(k) => Some(state.blobs.read(k).await?),
            None => None,
        }
    } else {
        Some(state.blobs.read(&item.blob_key).await?)
    };
    let title = item.display_title().to_string();
    let ext = util::sniff_ext(&item.mime).to_ascii_uppercase();
    let mut facts = vec![ext];
    if item.width > 0 {
        facts.push(format!("{} \u{d7} {}", item.width, item.height));
    }
    facts.push(human_bytes(item.bytes.max(0) as u64));
    facts.push(item.uploader.clone());
    let subtitle = facts.join("  \u{b7}  ");
    let video = item.is_video();
    let png = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
        let decoded = match &source {
            Some(bytes) => Some(og::decode(bytes)?),
            None => None,
        };
        let hero = if video { Hero::Video } else { Hero::Image };
        let card = Card {
            hero,
            image: decoded.as_ref(),
            title: &title,
            subtitle: &subtitle,
        };
        og::encode_png(&og::render_card(&card)?)
    })
    .await?
    .map_err(AppError::Internal)?;
    let png = Bytes::from(png);
    state
        .blobs
        .put(
            &key,
            png.clone(),
            PutMeta {
                mime: "image/png",
                cache_control: SHORT_CACHE,
                expires_ms: item.expires_ms,
            },
        )
        .await?;
    Ok(png)
}

async fn preview(
    State(state): State<AppState>,
    Path(code): Path<String>,
    headers: HeaderMap,
) -> PageResult<Response> {
    let item = load(&state, &code).await?;
    let png = if item.is_protected() {
        locked_card().await?
    } else {
        ensure_preview(&state, &item).await?
    };
    let etag = format!("\"{}-og{}\"", item.id, png.len());
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        == Some(etag.as_str())
    {
        return Ok(StatusCode::NOT_MODIFIED.into_response());
    }
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
            (header::ETAG, etag.as_str()),
        ],
        png,
    )
        .into_response())
}

// Delete by secret

async fn load_for_delete(
    state: &AppState,
    client: &Client,
    code: &str,
    secret: &str,
) -> Result<Item, AppError> {
    state.limiter.check(
        &format!("delete:{}", client.ip),
        20,
        Duration::from_secs(600),
    )?;
    let item = state
        .db
        .item_by_code(code.to_string())
        .await?
        .ok_or(AppError::NotFound)?;
    if util::ct_eq(&item.delete_secret, secret) {
        Ok(item)
    } else {
        Err(AppError::NotFound)
    }
}

async fn delete_confirm(
    State(state): State<AppState>,
    Path((code, secret)): Path<(String, String)>,
    client: Client,
) -> PageResult<Response> {
    let item = load_for_delete(&state, &client, &code, &secret).await?;
    Ok((
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        views::delete_confirm_page(&item),
    )
        .into_response())
}

async fn delete_now(
    State(state): State<AppState>,
    Path((code, secret)): Path<(String, String)>,
    client: Client,
) -> PageResult<Response> {
    let item = load_for_delete(&state, &client, &code, &secret).await?;
    remove_item(&state, &item).await?;
    Ok((
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        views::deleted_page(),
    )
        .into_response())
}

/// Delete an item's row and every stored object that belongs to it.
pub async fn remove_item(state: &AppState, item: &Item) -> Result<(), AppError> {
    if let Some(row) = state.db.delete_item(item.id.clone()).await? {
        state.blobs.delete_item(&row).await;
    }
    Ok(())
}

async fn short_link(
    State(state): State<AppState>,
    Path(code): Path<String>,
) -> Result<Response, PageError> {
    if !valid_code(&code) {
        return Err(AppError::NotFound.into());
    }
    let url = state
        .db
        .follow_link(code)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok((
        [
            (header::CACHE_CONTROL, "private, max-age=0"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        Redirect::temporary(&url),
    )
        .into_response())
}
