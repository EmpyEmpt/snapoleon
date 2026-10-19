//! Shared application state, client identification and rate limiting.

use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::{HeaderMap, header, request::Parts},
};
use parking_lot::Mutex;

use crate::{config::Config, db::Db, error::AppError, oidc::Oidc, storage::Blobs, util};

pub struct Shared {
    pub cfg: Config,
    pub db: Db,
    pub blobs: Blobs,
    /// Per-installation secret for cookies and visitor hashing.
    pub secret: Vec<u8>,
    pub limiter: RateLimiter,
    pub http: reqwest::Client,
    pub oidc: Oidc,
}

pub type AppState = Arc<Shared>;

impl Shared {
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.cfg.public_url)
    }

    pub fn item_url(&self, code: &str) -> String {
        self.url(&format!("/{code}"))
    }

    pub fn raw_url(&self, code: &str) -> String {
        self.url(&format!("/{code}/raw"))
    }

    /// HMAC under the installation secret, domain-separated by `purpose`.
    pub fn sign(&self, purpose: &str, msg: &str) -> String {
        util::hmac_hex(&self.secret, format!("{purpose}:{msg}").as_bytes())
    }
}

/// Sliding-window rate limiter keyed by an arbitrary string.
#[derive(Default)]
pub struct RateLimiter {
    hits: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl RateLimiter {
    /// Record a hit and report whether it is within `max` per `window`.
    pub fn allow(&self, key: &str, max: usize, window: Duration) -> bool {
        let now = Instant::now();
        let mut hits = self.hits.lock();
        if hits.len() > 20_000 {
            hits.retain(|_, q| {
                q.back()
                    .is_some_and(|t| now.duration_since(*t) < Duration::from_secs(3600))
            });
        }
        let q = hits.entry(key.to_string()).or_default();
        while q.front().is_some_and(|t| now.duration_since(*t) >= window) {
            q.pop_front();
        }
        if q.len() >= max {
            return false;
        }
        q.push_back(now);
        true
    }

    pub fn check(&self, key: &str, max: usize, window: Duration) -> Result<(), AppError> {
        if self.allow(key, max, window) {
            Ok(())
        } else {
            Err(AppError::TooManyRequests)
        }
    }
}

/// The calling client as far as the server can tell.
pub struct Client {
    pub ip: String,
    pub user_agent: String,
}

impl Client {
    /// Stable, salted hash of IP and user agent for dedupe and rate limits.
    pub fn visitor(&self, state: &Shared) -> String {
        state.sign("visitor", &format!("{}|{}", self.ip, self.user_agent))[..32].to_string()
    }

    /// Link-preview crawlers and other automated fetchers; they should not count as views.
    pub fn is_bot(&self) -> bool {
        let ua = self.user_agent.to_ascii_lowercase();
        ua.is_empty()
            || [
                "bot",
                "crawler",
                "spider",
                "slack",
                "telegram",
                "discord",
                "whatsapp",
                "facebookexternalhit",
                "skype",
                "preview",
                "curl",
                "wget",
                "python-requests",
                "go-http-client",
                "embed",
                "vkshare",
                "viber",
            ]
            .iter()
            .any(|needle| ua.contains(needle))
    }
}

fn forwarded_ip(headers: &HeaderMap) -> Option<String> {
    let get = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    get("cf-connecting-ip")
        .or_else(|| get("x-real-ip"))
        .or_else(|| {
            get("x-forwarded-for")
                .and_then(|v| v.rsplit(',').next())
                .map(str::trim)
        })
        .map(str::to_string)
}

impl FromRequestParts<AppState> for Client {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip().to_string())
            .unwrap_or_default();
        let ip = if state.cfg.trust_proxy {
            forwarded_ip(&parts.headers).unwrap_or(peer)
        } else {
            peer
        };
        let user_agent = parts
            .headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        Ok(Client { ip, user_agent })
    }
}
