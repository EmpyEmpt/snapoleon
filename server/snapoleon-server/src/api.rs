//! `/api/v1`: the JSON API used by the Snapoleon client (Bearer token auth).

use std::time::Duration;

use axum::{
    Json, Router,
    extract::{
        DefaultBodyLimit, FromRequest, Multipart, Path, Query, Request, State, multipart::Field,
    },
    http::{HeaderName, HeaderValue},
    response::IntoResponse,
    routing::{delete, get, post},
};
use bytes::Bytes;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use snap_core::{now_ms, t};

use crate::{
    auth::{self, ApiUser},
    db::{Event, Item, ItemScope, Membership, NewItem, Role, User, Workspace},
    error::{ApiError, ApiResult, AppError},
    og,
    state::AppState,
    storage::{self, PUBLIC_CACHE, PutMeta, StreamUpload},
    util::{
        self, HOUR_MS, MediaKind, Sniffed, clean_file_name, new_code, new_id, new_secret, truncate,
    },
    viewer,
};

const CODE_LEN: usize = 8;
const SHORT_CODE_LEN: usize = 6;
const MAX_EXPIRY_HOURS: i64 = 24 * 365 * 10;

pub fn routes(max_body: usize) -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/upload",
            post(upload).layer(DefaultBodyLimit::max(max_body)),
        )
        .route("/api/v1/me", get(me))
        .route("/api/v1/items", get(list_items))
        .route("/api/v1/items/{id}", delete(delete_item))
        .route("/api/v1/events", get(events))
        .route("/api/v1/shorten", post(shorten))
}

/// JSON body extractor whose rejections use the API error format.
struct ApiJson<T>(T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for ApiJson<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(v)) => Ok(Self(v)),
            Err(e) => Err(AppError::bad_request(e.body_text()).into()),
        }
    }
}

#[derive(Serialize)]
struct WorkspaceJson {
    id: String,
    slug: String,
    name: String,
    role: &'static str,
}

fn workspace_json(m: &Membership) -> WorkspaceJson {
    WorkspaceJson {
        id: m.workspace.id.clone(),
        slug: m.workspace.slug.clone(),
        name: m.workspace.name.clone(),
        role: m.role.as_str(),
    }
}

#[derive(Serialize)]
struct MeJson {
    id: String,
    username: String,
    display_name: String,
    email: String,
    is_admin: bool,
    workspace: Option<WorkspaceJson>,
    workspaces: Vec<WorkspaceJson>,
    max_upload_mb: u64,
    quota_mb: i64,
    used_bytes: i64,
    server: &'static str,
    version: &'static str,
}

async fn me(State(state): State<AppState>, ApiUser(user): ApiUser) -> ApiResult<Json<MeJson>> {
    let memberships = state.db.memberships(user.id.clone()).await?;
    let current = memberships.first();
    let usage = match current {
        Some(m) => state.db.workspace_usage(m.workspace.id.clone()).await?,
        None => Default::default(),
    };
    Ok(Json(MeJson {
        id: user.id.clone(),
        username: user.username.clone(),
        display_name: user.display_name.clone(),
        email: user.email.clone(),
        is_admin: user.is_admin,
        workspace: current.map(workspace_json),
        workspaces: memberships.iter().map(workspace_json).collect(),
        max_upload_mb: state.cfg.max_upload_mb,
        quota_mb: current.map_or(0, |m| m.workspace.quota_mb),
        used_bytes: usage.bytes,
        server: "snapoleon-server",
        version: snap_core::VERSION,
    }))
}

#[derive(Serialize)]
struct ItemJson {
    id: String,
    code: String,
    url: String,
    raw_url: String,
    thumbnail_url: Option<String>,
    delete_url: String,
    title: String,
    file_name: String,
    mime: String,
    bytes: i64,
    width: i64,
    height: i64,
    views: i64,
    comments: i64,
    has_password: bool,
    created_ms: i64,
    expires_ms: Option<i64>,
    workspace_id: String,
    uploader: String,
}

fn thumb_url(state: &AppState, item: &Item) -> Option<String> {
    item.thumb_key
        .as_ref()
        .map(|_| state.url(&format!("/{}/thumb.jpg", item.code)))
}

fn delete_url(state: &AppState, item: &Item) -> String {
    state.url(&format!("/{}/delete/{}", item.code, item.delete_secret))
}

fn item_json(state: &AppState, item: &Item) -> ItemJson {
    ItemJson {
        id: item.id.clone(),
        code: item.code.clone(),
        url: state.item_url(&item.code),
        raw_url: state.raw_url(&item.code),
        thumbnail_url: thumb_url(state, item),
        delete_url: delete_url(state, item),
        title: item.title.clone(),
        file_name: item.file_name.clone(),
        mime: item.mime.clone(),
        bytes: item.bytes,
        width: item.width,
        height: item.height,
        views: item.views,
        comments: item.comments,
        has_password: item.is_protected(),
        created_ms: item.created_ms,
        expires_ms: item.expires_ms,
        workspace_id: item.workspace_id.clone(),
        uploader: item.uploader.clone(),
    }
}

#[derive(Deserialize)]
struct ListQuery {
    page: Option<i64>,
    per_page: Option<i64>,
    q: Option<String>,
}

/// Newest-first page of the caller's own items. `page` is 1-based (0 is treated as 1);
/// the total match count is returned in `X-Total-Count`.
async fn list_items(
    State(state): State<AppState>,
    ApiUser(user): ApiUser,
    Query(q): Query<ListQuery>,
) -> ApiResult<impl IntoResponse> {
    let per_page = q.per_page.unwrap_or(50).clamp(1, 200);
    let page = q.page.unwrap_or(1).max(1);
    let (items, total) = state
        .db
        .list_items(
            ItemScope::own(&user.id),
            q.q.unwrap_or_default().trim().to_string(),
            per_page,
            (page - 1) * per_page,
        )
        .await?;
    let body: Vec<ItemJson> = items.iter().map(|i| item_json(&state, i)).collect();
    Ok((
        [(
            HeaderName::from_static("x-total-count"),
            HeaderValue::from(total),
        )],
        Json(body),
    ))
}

async fn delete_item(
    State(state): State<AppState>,
    ApiUser(user): ApiUser,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let item = match state.db.item_by_id(id.clone()).await? {
        Some(i) => i,
        None => state.db.item_by_code(id).await?.ok_or(AppError::NotFound)?,
    };
    if item.user_id != user.id && !user.is_admin {
        let memberships = state.db.memberships(user.id.clone()).await?;
        let admin_here = memberships
            .iter()
            .any(|m| m.workspace.id == item.workspace_id && m.role >= Role::Admin);
        if !admin_here {
            return Err(AppError::Forbidden.into());
        }
    }
    viewer::remove_item(&state, &item).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
struct EventsQuery {
    since: Option<i64>,
    limit: Option<i64>,
}

#[derive(Serialize)]
struct EventJson {
    id: i64,
    kind: String,
    item_id: String,
    code: String,
    url: String,
    title: String,
    actor: String,
    text: String,
    created_ms: i64,
}

fn event_json(state: &AppState, e: Event) -> EventJson {
    EventJson {
        id: e.id,
        kind: e.kind,
        item_id: e.item_id,
        url: state.item_url(&e.code),
        code: e.code,
        title: e.title,
        actor: e.actor,
        text: e.text,
        created_ms: e.created_ms,
    }
}

/// Views and comments on the caller's items strictly after `since` (epoch ms), oldest first.
async fn events(
    State(state): State<AppState>,
    ApiUser(user): ApiUser,
    Query(q): Query<EventsQuery>,
) -> ApiResult<Json<Vec<EventJson>>> {
    let events = state
        .db
        .events_since(
            user.id,
            q.since.unwrap_or(0),
            q.limit.unwrap_or(200).clamp(1, 500),
        )
        .await?;
    Ok(Json(
        events.into_iter().map(|e| event_json(&state, e)).collect(),
    ))
}

#[derive(Deserialize)]
struct ShortenRequest {
    url: String,
}

fn valid_target(url: &str) -> bool {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    url.len() <= 4096
        && rest.is_some_and(|r| !r.is_empty() && !r.starts_with('/'))
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

async fn shorten(
    State(state): State<AppState>,
    ApiUser(user): ApiUser,
    ApiJson(req): ApiJson<ShortenRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    state
        .limiter
        .check(&format!("shorten:{}", user.id), 60, Duration::from_secs(60))?;
    let url = req.url.trim().to_string();
    if !valid_target(&url) {
        return Err(AppError::bad_request(t("server.error.invalid_url").into_owned()).into());
    }
    if let Some(code) = state.db.link_for_url(user.id.clone(), url.clone()).await? {
        return Ok(Json(
            serde_json::json!({ "url": state.url(&format!("/s/{code}")), "code": code }),
        ));
    }
    let workspace = resolve_workspace(&state, &user, "").await?;
    for attempt in 0..8 {
        let code = new_code(SHORT_CODE_LEN + attempt / 4);
        if state
            .db
            .create_link(
                code.clone(),
                url.clone(),
                user.id.clone(),
                workspace.id.clone(),
            )
            .await?
        {
            return Ok(Json(
                serde_json::json!({ "url": state.url(&format!("/s/{code}")), "code": code }),
            ));
        }
    }
    Err(AppError::Internal(anyhow::anyhow!("could not allocate a short code")).into())
}

// Upload

/// Where an incoming file is being collected.
enum Collect {
    Memory(Vec<u8>),
    Stream(StreamUpload),
}

/// A fully received file, either in memory (images) or already stored under a temporary key (video).
enum Stored {
    Memory(Bytes),
    Temp(String),
}

struct Received {
    stored: Stored,
    media: Sniffed,
    size: u64,
    file_name: String,
}

struct Receiver<'a> {
    state: &'a AppState,
    limit: u64,
    head: Vec<u8>,
    media: Option<Sniffed>,
    sink: Option<Collect>,
    size: u64,
    tmp_key: String,
}

impl Receiver<'_> {
    async fn push(&mut self, chunk: Bytes) -> Result<(), AppError> {
        self.size += chunk.len() as u64;
        if self.size > self.limit {
            return Err(AppError::PayloadTooLarge);
        }
        match &mut self.sink {
            Some(Collect::Memory(buf)) => buf.extend_from_slice(&chunk),
            Some(Collect::Stream(upload)) => upload.push(&chunk).await?,
            None => {
                self.head.extend_from_slice(&chunk);
                if self.head.len() >= 16 {
                    self.start().await?;
                }
            }
        }
        Ok(())
    }

    /// Decide how to store the file once enough bytes arrived to identify it.
    async fn start(&mut self) -> Result<(), AppError> {
        let media = util::sniff(&self.head).ok_or(AppError::UnsupportedMedia)?;
        let head = std::mem::take(&mut self.head);
        self.media = Some(media);
        self.sink = Some(if media.kind == MediaKind::Video {
            let mut upload = self
                .state
                .blobs
                .begin(
                    &self.tmp_key,
                    PutMeta {
                        mime: media.mime,
                        cache_control: PUBLIC_CACHE,
                        expires_ms: None,
                    },
                )
                .await?;
            upload.push(&head).await?;
            Collect::Stream(upload)
        } else {
            Collect::Memory(head)
        });
        Ok(())
    }

    async fn finish(mut self, file_name: String) -> Result<Received, AppError> {
        if self.sink.is_none() {
            self.start().await?;
        }
        let media = self.media.ok_or(AppError::UnsupportedMedia)?;
        let stored = match self.sink {
            Some(Collect::Memory(buf)) => Stored::Memory(Bytes::from(buf)),
            Some(Collect::Stream(upload)) => {
                upload.finish().await?;
                Stored::Temp(self.tmp_key)
            }
            None => return Err(AppError::UnsupportedMedia),
        };
        Ok(Received {
            stored,
            media,
            size: self.size,
            file_name,
        })
    }

    async fn abort(self) {
        if let Some(Collect::Stream(upload)) = self.sink {
            upload.abort().await;
        }
    }
}

async fn receive_file(
    state: &AppState,
    mut field: Field<'_>,
    id: &str,
) -> Result<Received, AppError> {
    let file_name = clean_file_name(field.file_name().unwrap_or_default());
    let mut rx = Receiver {
        state,
        limit: state.cfg.max_upload_bytes(),
        head: Vec::new(),
        media: None,
        sink: None,
        size: 0,
        tmp_key: format!("tmp/{id}"),
    };
    loop {
        match field.chunk().await {
            Ok(Some(chunk)) => {
                if let Err(e) = rx.push(chunk).await {
                    rx.abort().await;
                    return Err(e);
                }
            }
            Ok(None) => break,
            Err(e) => {
                rx.abort().await;
                return Err(e.into());
            }
        }
    }
    rx.finish(file_name).await
}

#[derive(Default)]
struct Fields {
    title: String,
    password: String,
    expiry_hours: Option<i64>,
    workspace: String,
    file_name: String,
}

#[derive(Serialize)]
struct UploadJson {
    id: String,
    code: String,
    url: String,
    raw_url: String,
    delete_url: String,
    thumbnail_url: Option<String>,
    expires_ms: Option<i64>,
    bytes: i64,
    mime: String,
}

/// Image facts and the thumbnail JPEG, produced off the async runtime.
struct Analysed {
    width: u32,
    height: u32,
    thumb: Bytes,
}

async fn analyse(bytes: Bytes) -> Result<Analysed, AppError> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<Analysed> {
        let img = og::decode(&bytes)?;
        Ok(Analysed {
            width: img.width(),
            height: img.height(),
            thumb: Bytes::from(og::thumbnail_jpeg(&img)?),
        })
    })
    .await?
    .map_err(|_| AppError::UnsupportedMedia)
}

async fn upload(
    State(state): State<AppState>,
    ApiUser(user): ApiUser,
    mut multipart: Multipart,
) -> ApiResult<Json<UploadJson>> {
    state
        .limiter
        .check(&format!("upload:{}", user.id), 120, Duration::from_secs(60))?;
    let id = new_id();
    let mut fields = Fields::default();
    let mut received: Option<Received> = None;

    let parsed: Result<(), AppError> = async {
        while let Some(field) = multipart.next_field().await? {
            match field.name().unwrap_or_default() {
                "file" if received.is_none() => {
                    received = Some(receive_file(&state, field, &id).await?)
                }
                "title" => fields.title = field.text().await?,
                "password" => fields.password = field.text().await?,
                "expiry_hours" => fields.expiry_hours = field.text().await?.trim().parse().ok(),
                "workspace" => fields.workspace = field.text().await?.trim().to_string(),
                "file_name" => fields.file_name = field.text().await?,
                _ => {}
            }
        }
        Ok(())
    }
    .await;
    let received = match (parsed, received) {
        (Ok(()), Some(r)) => r,
        (Ok(()), None) => {
            return Err(AppError::bad_request(t("server.error.no_file").into_owned()).into());
        }
        (Err(e), r) => {
            discard(&state, r.as_ref()).await;
            return Err(e.into());
        }
    };
    let result = finalize(&state, &user, &id, fields, &received).await;
    discard(&state, Some(&received)).await;
    Ok(Json(result?))
}

/// Remove a streamed upload's temporary object (a no-op once it was renamed into place).
async fn discard(state: &AppState, received: Option<&Received>) {
    if let Some(Received {
        stored: Stored::Temp(key),
        ..
    }) = received
    {
        state.blobs.delete(key).await;
    }
}

async fn resolve_workspace(
    state: &AppState,
    user: &User,
    wanted: &str,
) -> Result<Workspace, AppError> {
    let memberships = state.db.memberships(user.id.clone()).await?;
    if wanted.is_empty() {
        return match memberships.into_iter().next() {
            Some(m) => Ok(m.workspace),
            None => state.db.default_workspace().await,
        };
    }
    let ws = state
        .db
        .workspace(wanted.to_string())
        .await?
        .ok_or(AppError::NotFound)?;
    if user.is_admin || memberships.iter().any(|m| m.workspace.id == ws.id) {
        Ok(ws)
    } else {
        Err(AppError::Forbidden)
    }
}

async fn finalize(
    state: &AppState,
    user: &User,
    id: &str,
    fields: Fields,
    rx: &Received,
) -> Result<UploadJson, AppError> {
    let ws = resolve_workspace(state, user, &fields.workspace).await?;
    if ws.quota_mb > 0 {
        let used = state.db.workspace_usage(ws.id.clone()).await?.bytes;
        if used + rx.size as i64 > ws.quota_mb * 1024 * 1024 {
            return Err(AppError::InsufficientStorage);
        }
    }
    let hours = fields
        .expiry_hours
        .unwrap_or(ws.default_expiry_hours)
        .clamp(0, MAX_EXPIRY_HOURS);
    let expires_ms = (hours > 0).then(|| now_ms() + hours * HOUR_MS);
    let password_hash = if fields.password.is_empty() {
        None
    } else {
        Some(auth::hash_password(fields.password).await?)
    };
    let protected = password_hash.is_some();

    // Images are decoded (which validates them) and thumbnailed; videos are stored as sent.
    let analysed = match &rx.stored {
        Stored::Memory(bytes) => Some(analyse(bytes.clone()).await?),
        Stored::Temp(_) => None,
    };
    let meta = |mime| PutMeta {
        mime,
        cache_control: PUBLIC_CACHE,
        expires_ms,
    };
    let file_name = if fields.file_name.trim().is_empty() {
        rx.file_name.clone()
    } else {
        clean_file_name(&fields.file_name)
    };
    let delete_secret = new_secret(32);

    for attempt in 0..8 {
        let code = new_code(CODE_LEN + attempt / 4);
        let blob_key = storage::blob_key(id, &code, protected);
        let thumb_key = analysed
            .as_ref()
            .map(|_| storage::thumb_key(id, &code, protected));
        // Blobs go in before the row so a visible item always has its data.
        let written = async {
            if let Stored::Memory(bytes) = &rx.stored {
                state
                    .blobs
                    .put(&blob_key, bytes.clone(), meta(rx.media.mime))
                    .await?;
            }
            if let (Some(key), Some(a)) = (&thumb_key, &analysed) {
                state
                    .blobs
                    .put(key, a.thumb.clone(), meta("image/jpeg"))
                    .await?;
            }
            Ok::<_, AppError>(())
        }
        .await;
        let inserted = match written {
            Ok(()) => {
                state
                    .db
                    .insert_item(NewItem {
                        id: id.to_string(),
                        code: code.clone(),
                        workspace_id: ws.id.clone(),
                        user_id: user.id.clone(),
                        title: truncate(fields.title.trim(), 200),
                        file_name: file_name.clone(),
                        mime: rx.media.mime.to_string(),
                        bytes: rx.size as i64,
                        width: analysed.as_ref().map_or(0, |a| i64::from(a.width)),
                        height: analysed.as_ref().map_or(0, |a| i64::from(a.height)),
                        blob_key: blob_key.clone(),
                        thumb_key: thumb_key.clone(),
                        password_hash: password_hash.clone(),
                        delete_secret: delete_secret.clone(),
                        expires_ms,
                    })
                    .await
            }
            Err(e) => Err(e),
        };
        match inserted {
            Ok(Some(item)) => {
                if let Stored::Temp(tmp) = &rx.stored
                    && let Err(e) = state.blobs.rename(tmp, &blob_key).await
                {
                    let _ = state.db.delete_item(item.id.clone()).await;
                    return Err(e);
                }
                spawn_preview(state, &item);
                return Ok(UploadJson {
                    id: item.id.clone(),
                    code: item.code.clone(),
                    url: state.item_url(&item.code),
                    raw_url: state.raw_url(&item.code),
                    delete_url: delete_url(state, &item),
                    thumbnail_url: thumb_url(state, &item),
                    expires_ms: item.expires_ms,
                    bytes: item.bytes,
                    mime: item.mime.clone(),
                });
            }
            other => {
                // Code collision or a failure: drop what this attempt wrote.
                if matches!(rx.stored, Stored::Memory(_)) {
                    state.blobs.delete(&blob_key).await;
                }
                if let Some(k) = &thumb_key {
                    state.blobs.delete(k).await;
                }
                other?;
            }
        }
    }
    Err(AppError::Internal(anyhow::anyhow!(
        "could not allocate a short code"
    )))
}

/// Pre-render the link-preview card so the first unfurl is instant (and a CDN can serve it).
fn spawn_preview(state: &AppState, item: &Item) {
    if item.is_protected() {
        return;
    }
    let (state, item) = (state.clone(), item.clone());
    tokio::spawn(async move {
        if let Err(e) = viewer::ensure_preview(&state, &item).await {
            tracing::warn!("preview for {} failed: {}", item.code, e.message());
        }
    });
}
