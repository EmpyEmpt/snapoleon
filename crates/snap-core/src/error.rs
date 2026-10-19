use std::borrow::Cow;

/// Error type shared across crates. Library crates may define richer local
/// errors but convert into this at their public boundary.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cancelled")]
    Cancelled,
    #[error("not supported on this platform: {0}")]
    Unsupported(Cow<'static, str>),
    #[error("permission required: {0}")]
    Permission(Cow<'static, str>),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("network: {0}")]
    Network(String),
    #[error("blocked by policy: {0}")]
    Policy(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Image(#[from] image::ImageError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl Error {
    pub fn unsupported(what: impl Into<Cow<'static, str>>) -> Self {
        Self::Unsupported(what.into())
    }
    pub fn permission(what: impl Into<Cow<'static, str>>) -> Self {
        Self::Permission(what.into())
    }
    pub fn invalid(what: impl Into<String>) -> Self {
        Self::Invalid(what.into())
    }
    pub fn network(what: impl std::fmt::Display) -> Self {
        Self::Network(what.to_string())
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
