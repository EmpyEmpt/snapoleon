//! Application errors and their JSON / HTML renderings.

use crate::i18n::t;
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};

use crate::views;

#[derive(Debug)]
pub enum AppError {
    BadRequest(String),
    Unauthorized,
    Forbidden,
    NotFound,
    Gone,
    PayloadTooLarge,
    InsufficientStorage,
    TooManyRequests,
    UnsupportedMedia,
    RangeNotSatisfiable(u64),
    Internal(anyhow::Error),
}

impl AppError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self::BadRequest(msg.into())
    }

    pub fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Gone => StatusCode::GONE,
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::InsufficientStorage => StatusCode::INSUFFICIENT_STORAGE,
            Self::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
            Self::UnsupportedMedia => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::RangeNotSatisfiable(_) => StatusCode::RANGE_NOT_SATISFIABLE,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Localised, user-facing description. Internal details are logged, never shown.
    pub fn message(&self) -> String {
        let key = match self {
            Self::BadRequest(msg) => return msg.clone(),
            Self::Unauthorized => "server.error.unauthorized",
            Self::Forbidden => "server.error.forbidden",
            Self::NotFound => "server.error.not_found",
            Self::Gone => "server.error.gone",
            Self::PayloadTooLarge => "server.error.too_large",
            Self::InsufficientStorage => "server.error.quota",
            Self::TooManyRequests => "server.error.rate_limited",
            Self::UnsupportedMedia => "server.error.unsupported_media",
            Self::RangeNotSatisfiable(_) => "server.error.range",
            Self::Internal(_) => "server.error.internal",
        };
        t(key).into_owned()
    }

    fn log(&self) {
        if let Self::Internal(e) = self {
            tracing::error!("internal error: {e:#}");
        }
    }
}

macro_rules! internal_from {
    ($($ty:ty),* $(,)?) => {
        $(impl From<$ty> for AppError {
            fn from(e: $ty) -> Self {
                Self::Internal(anyhow::Error::new(e))
            }
        })*
    };
}

internal_from!(rusqlite::Error, std::io::Error, object_store::Error, tokio::task::JoinError, image::ImageError);

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        Self::Internal(e)
    }
}

impl From<axum::extract::multipart::MultipartError> for AppError {
    fn from(e: axum::extract::multipart::MultipartError) -> Self {
        if e.status() == StatusCode::PAYLOAD_TOO_LARGE { Self::PayloadTooLarge } else { Self::bad_request(e.body_text()) }
    }
}

/// Error type for `/api/*` handlers: JSON body `{"error": "..."}`.
pub struct ApiError(pub AppError);

impl<E: Into<AppError>> From<E> for ApiError {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        self.0.log();
        let mut resp = (self.0.status(), Json(serde_json::json!({ "error": self.0.message() }))).into_response();
        if matches!(self.0, AppError::Unauthorized) {
            resp.headers_mut().insert(axum::http::header::WWW_AUTHENTICATE, "Bearer".parse().expect("static header"));
        }
        resp
    }
}

/// Error type for browser-facing handlers: a small HTML error page.
pub struct PageError(pub AppError);

impl<E: Into<AppError>> From<E> for PageError {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}

impl IntoResponse for PageError {
    fn into_response(self) -> Response {
        self.0.log();
        let status = self.0.status();
        let mut resp = (status, views::error_page(status, &self.0.message())).into_response();
        if let AppError::RangeNotSatisfiable(len) = self.0 {
            resp.headers_mut().insert(axum::http::header::CONTENT_RANGE, format!("bytes */{len}").parse().expect("ascii header"));
        }
        resp
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
pub type PageResult<T> = Result<T, PageError>;
