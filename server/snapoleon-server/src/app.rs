//! Router assembly: routes, static assets, security headers and middleware.

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderName, HeaderValue, StatusCode, Uri, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use maud::Markup;
use tower_http::{
    compression::{CompressionLayer, DefaultPredicate, Predicate, predicate::NotForContentType},
    limit::RequestBodyLimitLayer,
    trace::TraceLayer,
};

use crate::{
    admin, api,
    error::{ApiError, AppError, PageError},
    oidc,
    state::AppState,
    viewer, views,
};

const STYLE: &str = include_str!("../static/style.css");
const SCRIPT: &str = include_str!("../static/app.js");
const FAVICON: &str = include_str!("../static/favicon.svg");

const CSP: &str = "default-src 'none'; img-src 'self' data:; media-src 'self'; style-src 'self'; script-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'";

pub fn router(state: AppState) -> Router {
    let max_body = state.cfg.max_upload_bytes() as usize + 1024 * 1024;
    // Video is range-served and already compressed; only compress text-like responses.
    let compression = CompressionLayer::new().compress_when(
        DefaultPredicate::new().and(NotForContentType::new("video/")).and(NotForContentType::new("application/octet-stream")),
    );
    Router::new()
        .route("/", get(home))
        .route("/healthz", get(healthz))
        .route("/og.png", get(viewer::site_card))
        .route("/static/style.css", get(|| asset("text/css; charset=utf-8", STYLE)))
        .route("/static/app.js", get(|| asset("text/javascript; charset=utf-8", SCRIPT)))
        .route("/static/favicon.svg", get(|| asset("image/svg+xml", FAVICON)))
        .merge(api::routes(max_body))
        .merge(admin::routes(state.clone()))
        .merge(oidc::routes())
        .merge(viewer::routes())
        .fallback(not_found)
        .layer(middleware::from_fn(security_headers))
        .layer(middleware::from_fn(crate::i18n::negotiate))
        .layer(compression)
        .layer(RequestBodyLimitLayer::new(max_body))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn asset(mime: &'static str, body: &'static str) -> Response {
    ([(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, "public, max-age=86400")], body).into_response()
}

async fn home(State(state): State<AppState>) -> Markup {
    views::home_page(&state.cfg.public_url)
}

async fn healthz(State(state): State<AppState>) -> Response {
    match state.db.run(|c| c.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))).await {
        Ok(_) => Json(serde_json::json!({ "status": "ok", "version": snap_core::VERSION })).into_response(),
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({ "status": "error" }))).into_response(),
    }
}

async fn not_found(uri: Uri) -> Response {
    if uri.path().starts_with("/api/") {
        ApiError(AppError::NotFound).into_response()
    } else {
        PageError(AppError::NotFound).into_response()
    }
}

async fn security_headers(req: axum::extract::Request, next: Next) -> Response {
    let mut resp = next.run(req).await;
    let is_html = resp.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|v| v.starts_with("text/html"));
    let h = resp.headers_mut();
    h.entry(HeaderName::from_static("x-content-type-options")).or_insert(HeaderValue::from_static("nosniff"));
    h.entry(header::REFERRER_POLICY).or_insert(HeaderValue::from_static("strict-origin-when-cross-origin"));
    if is_html {
        h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
        h.insert(HeaderName::from_static("x-frame-options"), HeaderValue::from_static("DENY"));
    }
    resp
}
