//! Item events (views, comments): stored for the events API and pushed to
//! workspace webhooks (generic JSON, Slack-compatible, Telegram bot).

use std::{net::IpAddr, time::Duration};

use crate::i18n::{t, tf};
use serde_json::json;
use snap_core::now_ms;

use crate::{
    db::{Event, Item, Webhook, WebhookKind},
    state::AppState,
    util::{hmac_hex, is_public_ip, truncate},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    View,
    Comment,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::View => "view",
            EventKind::Comment => "comment",
        }
    }
}

const TELEGRAM_API: &str = "https://api.telegram.org";
const ATTEMPTS: u32 = 3;
/// At most one view notification per item in this window; views keep being counted.
const VIEW_NOTIFY_EVERY: Duration = Duration::from_secs(600);

/// DNS resolver that drops every non-public address. The connection is made to exactly
/// the addresses returned here, so a name cannot be re-resolved to an internal host later.
struct PublicOnly;

impl reqwest::dns::Resolve for PublicOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let addrs: Vec<_> = tokio::net::lookup_host((name.as_str(), 0)).await?.filter(|a| is_public_ip(a.ip())).collect();
            if addrs.is_empty() {
                return Err(BLOCKED.into());
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

const BLOCKED: &str = "address not allowed";

/// HTTP client for user-supplied webhook targets: public addresses only, no redirects, no proxies.
pub fn client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(format!("snapoleon-server/{}", snap_core::VERSION))
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .dns_resolver(PublicOnly)
        .build()
}

/// Whether a webhook target is an acceptable http(s) URL that does not name an internal address.
/// Host names are checked again at connection time by [`PublicOnly`].
pub fn target_allowed(target: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(target) else { return false };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let Some(host) = url.host_str() else { return false };
    match host.trim_matches(['[', ']']).parse::<IpAddr>() {
        Ok(ip) => is_public_ip(ip),
        Err(_) => !host.eq_ignore_ascii_case("localhost") && !host.to_ascii_lowercase().ends_with(".localhost"),
    }
}

/// Record an event for the item's owner and notify the workspace's webhooks.
/// Views are debounced per item so anonymous traffic cannot flood the events table or the chat channels.
pub async fn emit(state: &AppState, item: &Item, kind: EventKind, actor: &str, text: &str) {
    if kind == EventKind::View && !state.limiter.allow(&format!("view-notify:{}", item.id), 1, VIEW_NOTIFY_EVERY) {
        return;
    }
    let event = Event {
        id: 0,
        owner_id: item.user_id.clone(),
        workspace_id: item.workspace_id.clone(),
        item_id: item.id.clone(),
        code: item.code.clone(),
        title: item.display_title().to_string(),
        kind: kind.as_str().to_string(),
        actor: truncate(actor, 80),
        text: truncate(text, 500),
        created_ms: now_ms(),
    };
    if let Err(e) = state.db.add_event(event.clone()).await {
        tracing::warn!("recording event failed: {:?}", e.message());
    }
    let hooks = match state.db.webhooks(item.workspace_id.clone()).await {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!("loading webhooks failed: {:?}", e.message());
            return;
        }
    };
    for hook in hooks.into_iter().filter(|h| h.enabled && if kind == EventKind::View { h.on_view } else { h.on_comment }) {
        let state = state.clone();
        let payload = Payload::new(&state, item, &event);
        tokio::spawn(async move {
            if let Err(e) = deliver_with_retry(&state, &hook, &payload).await {
                tracing::warn!("webhook {} ({}) failed: {e}", hook.id, hook.kind.as_str());
            }
        });
    }
}

/// Everything a webhook needs, resolved up front.
pub struct Payload {
    event: Event,
    url: String,
    raw_url: String,
    mime: String,
    views: i64,
    comments: i64,
}

impl Payload {
    pub fn new(state: &AppState, item: &Item, event: &Event) -> Self {
        Self {
            event: event.clone(),
            url: state.item_url(&item.code),
            raw_url: state.raw_url(&item.code),
            mime: item.mime.clone(),
            views: item.views,
            comments: item.comments,
        }
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "event": self.event.kind,
            "created_ms": self.event.created_ms,
            "workspace_id": self.event.workspace_id,
            "actor": self.event.actor,
            "text": self.event.text,
            "item": {
                "id": self.event.item_id,
                "code": self.event.code,
                "title": self.event.title,
                "url": self.url,
                "raw_url": self.raw_url,
                "mime": self.mime,
                "views": self.views,
                "comments": self.comments,
            },
        })
    }

    /// Human-readable message used by Slack and Telegram. `escape` neutralises user text for the target's markup.
    fn message(&self, escape: fn(&str) -> String) -> String {
        let actor = if self.event.actor.is_empty() { "?".to_string() } else { escape(&self.event.actor) };
        let title = escape(&self.event.title);
        let head = if self.event.kind == EventKind::Comment.as_str() {
            tf("server.hook.comment", &[("actor", &actor), ("title", &title), ("text", &escape(&self.event.text))])
        } else {
            tf("server.hook.view", &[("title", &title)])
        };
        format!("{head}\n{}", self.url)
    }
}

/// Slack parses `<...>` and `&` in message text (mentions, links); escape them so user text stays literal.
fn escape_slack(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// A fake comment used by the admin "send test" button.
pub fn sample_payload(state: &AppState, workspace_id: &str) -> Payload {
    let event = Event {
        id: 0,
        owner_id: String::new(),
        workspace_id: workspace_id.to_string(),
        item_id: "sample".into(),
        code: "sample".into(),
        title: t("server.hook.sample_title").into_owned(),
        kind: EventKind::Comment.as_str().into(),
        actor: "Snapoleon".into(),
        text: t("server.hook.sample_text").into_owned(),
        created_ms: now_ms(),
    };
    Payload { url: state.url("/sample"), raw_url: state.url("/sample/raw"), mime: "image/png".into(), views: 0, comments: 1, event }
}

async fn deliver_with_retry(state: &AppState, hook: &Webhook, payload: &Payload) -> Result<(), String> {
    let mut last = String::new();
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(2u64.pow(attempt) * 2)).await;
        }
        match deliver(state, hook, payload).await {
            Ok(()) => return Ok(()),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// One delivery attempt; the error string is safe to show to admins.
pub async fn deliver(state: &AppState, hook: &Webhook, payload: &Payload) -> Result<(), String> {
    if hook.kind != WebhookKind::Telegram && !target_allowed(&hook.target) {
        return Err(BLOCKED.to_string());
    }
    let req = match hook.kind {
        WebhookKind::Generic => {
            let body = payload.json().to_string();
            let mut req = state
                .hook_http
                .post(&hook.target)
                .header("content-type", "application/json")
                .header("x-snapoleon-event", &payload.event.kind);
            if !hook.secret.is_empty() {
                req = req.header("x-snapoleon-signature", format!("sha256={}", hmac_hex(hook.secret.as_bytes(), body.as_bytes())));
            }
            req.body(body)
        }
        WebhookKind::Slack => state.hook_http.post(&hook.target).json(&json!({ "text": payload.message(escape_slack) })),
        WebhookKind::Telegram => state
            .hook_http
            .post(format!("{TELEGRAM_API}/bot{}/sendMessage", hook.secret))
            .json(&json!({ "chat_id": hook.target, "text": payload.message(str::to_string), "disable_web_page_preview": false })),
    };
    let resp = req.timeout(Duration::from_secs(10)).send().await.map_err(|e| e.without_url().to_string())?;
    let status = resp.status();
    if status.is_success() { Ok(()) } else { Err(format!("HTTP {}", status.as_u16())) }
}
