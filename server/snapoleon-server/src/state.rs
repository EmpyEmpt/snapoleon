//! Shared application state, client identification and rate limiting.

use std::{
    collections::{HashMap, VecDeque},
    net::{IpAddr, Ipv6Addr, SocketAddr},
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

/// Most distinct keys each limiter table tracks; when full, the stalest sixteenth is evicted.
const MAX_KEYS: usize = 100_000;
const SWEEP_EVERY: Duration = Duration::from_secs(30);
/// A backoff entry untouched for this long starts over.
const BACKOFF_FORGET: Duration = Duration::from_secs(3600);

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

/// Exponential delay schedule for guessable secrets: the first `free` failures cost nothing, each
/// further one doubles the spacing between attempts from `base` up to `cap`.
pub struct Backoff {
    pub free: u32,
    pub base: Duration,
    pub cap: Duration,
    /// Longest an attempt may be queued behind earlier ones.
    pub horizon: Duration,
}

impl Backoff {
    fn step(&self, strikes: u32) -> Duration {
        match strikes.saturating_sub(self.free) {
            0 => Duration::ZERO,
            over => self.base.checked_mul(1 << (over - 1).min(20)).map_or(self.cap, |d| d.min(self.cap)),
        }
    }
}

struct Delay {
    strikes: u32,
    /// When the next attempt may run; attempts queue one `step` apart.
    next: Instant,
    touched: Instant,
}

#[derive(Default)]
struct Tables {
    windows: HashMap<String, Window>,
    delays: HashMap<String, Delay>,
}

/// Drop the stalest sixteenth of `map` (by `last`; entries without one go first) to make room.
fn evict_stalest<V>(map: &mut HashMap<String, V>, last: impl Fn(&V) -> Option<Instant>) {
    let mut times: Vec<Option<Instant>> = map.values().map(&last).collect();
    let nth = (MAX_KEYS / 16).min(times.len() - 1);
    let cutoff = *times.select_nth_unstable(nth).1;
    map.retain(|_, v| last(v) > cutoff);
}

struct Windows {
    tables: Tables,
    swept: Instant,
}

/// Sliding-window rate limiter keyed by an arbitrary string.
pub struct RateLimiter {
    windows: Mutex<Windows>,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self { windows: Mutex::new(Windows { tables: Tables::default(), swept: Instant::now() }) }
    }
}

impl Windows {
    fn sweep(&mut self, now: Instant) {
        if now.duration_since(self.swept) < SWEEP_EVERY {
            return;
        }
        self.tables.windows.retain(|_, e| {
            e.trim(now);
            !e.hits.is_empty()
        });
        self.tables.delays.retain(|_, d| d.next > now || now.duration_since(d.touched) < BACKOFF_FORGET);
        self.swept = now;
    }

    fn delay(&mut self, key: &str, now: Instant) -> &mut Delay {
        let delays = &mut self.tables.delays;
        if delays.len() >= MAX_KEYS && !delays.contains_key(key) {
            evict_stalest(delays, |d| Some(d.touched));
        }
        let d = delays.entry(key.to_string()).or_insert(Delay { strikes: 0, next: now, touched: now });
        if now.duration_since(d.touched) >= BACKOFF_FORGET {
            d.strikes = 0;
        }
        d.touched = now;
        d
    }
}

impl RateLimiter {
    /// Record a hit and report whether it is within `max` per `window`.
    pub fn allow(&self, key: &str, max: usize, window: Duration) -> bool {
        let now = Instant::now();
        let mut w = self.windows.lock();
        w.sweep(now);
        let map = &mut w.tables.windows;
        if map.len() >= MAX_KEYS && !map.contains_key(key) {
            evict_stalest(map, |e| e.hits.back().copied());
        }
        let e = map.entry(key.to_string()).or_insert_with(|| Window { span: window, hits: VecDeque::new() });
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
        self.windows
            .lock()
            .tables
            .windows
            .get(key)
            .is_some_and(|e| e.hits.iter().filter(|t| now.duration_since(**t) < window).count() >= max)
    }

    pub fn check(&self, key: &str, max: usize, window: Duration) -> Result<(), AppError> {
        if self.allow(key, max, window) { Ok(()) } else { Err(AppError::TooManyRequests) }
    }

    /// Queue one attempt on `key` and return how long it must wait before running. Attempts are
    /// spaced by the current backoff step, so parallel guesses cannot beat the schedule. `None`
    /// means the queue already extends past the horizon and nothing was queued.
    pub fn reserve(&self, key: &str, backoff: &Backoff) -> Option<Duration> {
        let now = Instant::now();
        let mut w = self.windows.lock();
        w.sweep(now);
        let d = w.delay(key, now);
        let wait = d.next.saturating_duration_since(now);
        if wait > backoff.horizon {
            return None;
        }
        d.next = now + wait + backoff.step(d.strikes);
        Some(wait)
    }

    /// Count a failed attempt on `key`, lengthening the spacing of later ones.
    pub fn penalize(&self, key: &str) {
        let now = Instant::now();
        let mut w = self.windows.lock();
        let d = w.delay(key, now);
        d.strikes = d.strikes.saturating_add(1);
    }

    /// Clear the backoff on `key` after a correct secret.
    pub fn forgive(&self, key: &str) {
        self.windows.lock().tables.delays.remove(key);
    }
}

/// The calling client as far as the server can tell.
pub struct Client {
    /// Canonical address; IPv6 is reduced to its /64 network.
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

/// The address as a limiter key: IPv6 clients share their /64, since one subscriber holds the whole
/// prefix and could otherwise mint a fresh key per request.
fn limiter_ip(ip: IpAddr) -> String {
    match ip.to_canonical() {
        IpAddr::V6(v6) => Ipv6Addr::from(u128::from(v6) & (u128::MAX << 64)).to_string(),
        v4 => v4.to_string(),
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
        let ip = forwarded.or(peer).map(limiter_ip).unwrap_or_default();
        let user_agent = parts.headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        Ok(Client { ip, user_agent })
    }
}
