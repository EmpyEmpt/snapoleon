//! Shared application state, client identification and rate limiting.

use std::{
    collections::{HashMap, VecDeque},
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::{HeaderMap, header, request::Parts},
};
use parking_lot::Mutex;
use tokio::sync::Semaphore;

use crate::{
    config::Config,
    db::Db,
    error::AppError,
    oidc::Oidc,
    storage::Blobs,
    util::{self, Cidr},
};

pub struct Shared {
    pub cfg: Config,
    pub db: Db,
    pub blobs: Blobs,
    /// Per-installation secret for cookies and visitor hashing.
    pub secret: Vec<u8>,
    pub limiter: RateLimiter,
    /// Bounds how many images are buffered in memory at once (see `api::Receiver`).
    pub image_slots: Arc<Semaphore>,
    pub http: reqwest::Client,
    /// Client for user-supplied webhook targets (see `webhooks::client`).
    pub hook_http: reqwest::Client,
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

/// Most distinct keys the limiter tracks; beyond this new keys are refused (fail closed).
const MAX_KEYS: usize = 100_000;
const SWEEP_EVERY: Duration = Duration::from_secs(30);

struct Window {
    span: Duration,
    hits: VecDeque<Instant>,
}

impl Window {
    fn trim(&mut self, now: Instant) {
        while self.hits.front().is_some_and(|t| now.duration_since(*t) >= self.span) {
            self.hits.pop_front();
        }
    }
}

struct Windows {
    map: HashMap<String, Window>,
    swept: Instant,
}

/// Sliding-window rate limiter keyed by an arbitrary string.
pub struct RateLimiter {
    windows: Mutex<Windows>,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self { windows: Mutex::new(Windows { map: HashMap::new(), swept: Instant::now() }) }
    }
}

impl RateLimiter {
    /// Record a hit and report whether it is within `max` per `window`.
    pub fn allow(&self, key: &str, max: usize, window: Duration) -> bool {
        let now = Instant::now();
        let mut w = self.windows.lock();
        if now.duration_since(w.swept) >= SWEEP_EVERY {
            w.map.retain(|_, e| {
                e.trim(now);
                !e.hits.is_empty()
            });
            w.swept = now;
        }
        if w.map.len() >= MAX_KEYS && !w.map.contains_key(key) {
            return false;
        }
        let e = w.map.entry(key.to_string()).or_insert_with(|| Window { span: window, hits: VecDeque::new() });
        e.span = window;
        e.trim(now);
        if e.hits.len() >= max {
            return false;
        }
        e.hits.push_back(now);
        true
    }

    /// Whether `key` already reached `max` hits within `window`, without recording a hit.
    pub fn exceeded(&self, key: &str, max: usize, window: Duration) -> bool {
        let now = Instant::now();
        self.windows.lock().map.get(key).is_some_and(|e| e.hits.iter().filter(|t| now.duration_since(**t) < window).count() >= max)
    }

    pub fn check(&self, key: &str, max: usize, window: Duration) -> Result<(), AppError> {
        if self.allow(key, max, window) { Ok(()) } else { Err(AppError::TooManyRequests) }
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

/// The client address a trusted proxy reported in `header`. For `X-Forwarded-For` this is the
/// rightmost entry that is not itself a trusted proxy; other headers carry a single address.
fn forwarded_ip(headers: &HeaderMap, header: &str, trusted: &[Cidr]) -> Option<IpAddr> {
    let value = headers.get(header)?.to_str().ok()?;
    if header == "x-forwarded-for" {
        value.rsplit(',').filter_map(|v| v.trim().parse::<IpAddr>().ok()).find(|ip| !trusted.iter().any(|n| n.contains(*ip)))
    } else {
        value.trim().parse().ok()
    }
}

impl FromRequestParts<AppState> for Client {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip());
        let trusted = &state.cfg.trusted_proxies;
        let forwarded = peer
            .filter(|p| trusted.iter().any(|n| n.contains(*p)))
            .and_then(|_| forwarded_ip(&parts.headers, &state.cfg.proxy_header, trusted));
        let ip = forwarded.or(peer).map(|ip| ip.to_canonical().to_string()).unwrap_or_default();
        let user_agent = parts.headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        Ok(Client { ip, user_agent })
    }
}
