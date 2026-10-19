//! OAuth 2.0 authorization-code flow with PKCE (S256) and a loopback redirect
//! served from a tiny `std::net` listener, plus token refresh. Tokens
//! serialize to JSON so they can live in the [`snap_core::secrets::SecretStore`].

use std::{
    fmt,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use snap_core::{Error, Result, now_ms, secrets::SecretStore, t, tf};

use crate::{
    http::Call,
    util::{html_escape, pct},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Box,
    Dropbox,
    GoogleDrive,
    OneDrive,
    YandexDisk,
}

struct Endpoints {
    auth_url: &'static str,
    token_url: &'static str,
    scope: &'static str,
    /// Extra authorization-request parameters (e.g. offline access).
    auth_extra: &'static [(&'static str, &'static str)],
    /// Microsoft only accepts `localhost` loopback redirects for public clients.
    redirect_host: &'static str,
}

impl Provider {
    pub const ALL: [Provider; 5] = [Provider::Box, Provider::Dropbox, Provider::GoogleDrive, Provider::OneDrive, Provider::YandexDisk];

    pub fn name(self) -> &'static str {
        match self {
            Provider::Box => "Box",
            Provider::Dropbox => "Dropbox",
            Provider::GoogleDrive => "Google Drive",
            Provider::OneDrive => "OneDrive",
            Provider::YandexDisk => "Yandex Disk",
        }
    }

    fn endpoints(self) -> Endpoints {
        match self {
            Provider::Box => Endpoints {
                auth_url: "https://account.box.com/api/oauth2/authorize",
                token_url: "https://api.box.com/oauth2/token",
                scope: "",
                auth_extra: &[],
                redirect_host: "127.0.0.1",
            },
            Provider::Dropbox => Endpoints {
                auth_url: "https://www.dropbox.com/oauth2/authorize",
                token_url: "https://api.dropboxapi.com/oauth2/token",
                scope: "files.content.write files.content.read sharing.write sharing.read",
                auth_extra: &[("token_access_type", "offline")],
                redirect_host: "127.0.0.1",
            },
            Provider::GoogleDrive => Endpoints {
                auth_url: "https://accounts.google.com/o/oauth2/v2/auth",
                token_url: "https://oauth2.googleapis.com/token",
                scope: "https://www.googleapis.com/auth/drive.file",
                auth_extra: &[("access_type", "offline"), ("prompt", "consent")],
                redirect_host: "127.0.0.1",
            },
            Provider::OneDrive => Endpoints {
                auth_url: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
                token_url: "https://login.microsoftonline.com/common/oauth2/v2.0/token",
                scope: "Files.ReadWrite offline_access",
                auth_extra: &[],
                redirect_host: "localhost",
            },
            Provider::YandexDisk => Endpoints {
                auth_url: "https://oauth.yandex.ru/authorize",
                token_url: "https://oauth.yandex.ru/token",
                scope: "cloud_api:disk.write cloud_api:disk.read cloud_api:disk.info",
                auth_extra: &[],
                redirect_host: "127.0.0.1",
            },
        }
    }
}

/// Credentials for one account, stored as JSON in the secret store. The
/// client id/secret travel with the tokens so a destination can refresh on its own.
#[derive(Clone, Serialize, Deserialize)]
pub struct Tokens {
    pub provider: Provider,
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Absolute expiry of the access token, ms since the Unix epoch.
    #[serde(default)]
    pub expires_at_ms: Option<i64>,
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
}

impl fmt::Debug for Tokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tokens").field("provider", &self.provider).field("expires_at_ms", &self.expires_at_ms).finish_non_exhaustive()
    }
}

impl Tokens {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("tokens are plain data")
    }

    pub fn from_json(s: &str) -> Result<Self> {
        serde_json::from_str(s).map_err(|e| Error::invalid(format!("tokens: {e}")))
    }

    /// True when the access token is expired or about to expire.
    pub fn is_expired(&self) -> bool {
        self.expires_at_ms.is_some_and(|at| at - 60_000 <= now_ms())
    }

    /// Exchange the refresh token for a new access token, keeping the client credentials.
    pub fn refreshed(&self) -> Result<Tokens> {
        let rt = self.refresh_token.as_deref().ok_or_else(|| Error::permission(t("upload.error.no_refresh_token").into_owned()))?;
        refresh_with_secret(self.provider, &self.client_id, self.client_secret.as_deref(), rt)
    }
}

pub struct AuthorizeOptions<'a> {
    /// How long to wait for the user to finish in the browser.
    pub timeout: Duration,
    /// Called with the authorization URL instead of opening a browser.
    pub on_url: Option<&'a dyn Fn(&str)>,
    /// Fixed loopback port, for providers that need an exactly registered redirect URI.
    pub port: u16,
}

impl Default for AuthorizeOptions<'_> {
    fn default() -> Self {
        Self { timeout: Duration::from_secs(300), on_url: None, port: 0 }
    }
}

/// Interactive sign-in: opens the browser, waits for the loopback redirect and
/// exchanges the code. Blocks until done, denied or timed out.
pub fn authorize(p: Provider, client_id: &str, client_secret: Option<&str>) -> Result<Tokens> {
    authorize_with(p, client_id, client_secret, &AuthorizeOptions::default())
}

pub fn authorize_with(p: Provider, client_id: &str, client_secret: Option<&str>, opts: &AuthorizeOptions) -> Result<Tokens> {
    if client_id.trim().is_empty() {
        return Err(Error::invalid(t("upload.error.no_client_id").into_owned()));
    }
    let ep = p.endpoints();
    let listener = TcpListener::bind(("127.0.0.1", opts.port))?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://{}:{port}/", ep.redirect_host);

    let verifier = random_string(64);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_string(24);

    let mut query = vec![
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri.as_str()),
        ("state", state.as_str()),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
    ];
    if !ep.scope.is_empty() {
        query.push(("scope", ep.scope));
    }
    query.extend_from_slice(ep.auth_extra);
    let auth_url = format!("{}?{}", ep.auth_url, query.iter().map(|(k, v)| format!("{}={}", pct(k), pct(v))).collect::<Vec<_>>().join("&"));

    match opts.on_url {
        Some(f) => f(&auth_url),
        None => webbrowser::open(&auth_url).map_err(|e| std::io::Error::other(format!("{}: {e}", t("upload.error.open_browser"))))?,
    }

    let code = wait_for_code(&listener, &state, opts.timeout)?;
    let mut params = vec![
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("client_id", client_id),
        ("code_verifier", verifier.as_str()),
    ];
    if let Some(secret) = client_secret.filter(|s| !s.is_empty()) {
        params.push(("client_secret", secret));
    }
    token_request(p, &ep, &params, client_id, client_secret, None)
}

/// Refresh without a client secret (public clients: Dropbox, Microsoft, PKCE apps).
pub fn refresh(p: Provider, client_id: &str, refresh_token: &str) -> Result<Tokens> {
    refresh_with_secret(p, client_id, None, refresh_token)
}

/// Refresh for providers whose app registration carries a client secret (Google, Box).
pub fn refresh_with_secret(p: Provider, client_id: &str, client_secret: Option<&str>, refresh_token: &str) -> Result<Tokens> {
    let mut params = vec![("grant_type", "refresh_token"), ("refresh_token", refresh_token), ("client_id", client_id)];
    if let Some(secret) = client_secret.filter(|s| !s.is_empty()) {
        params.push(("client_secret", secret));
    }
    token_request(p, &p.endpoints(), &params, client_id, client_secret, Some(refresh_token))
}

/// Load tokens from the secret store; refresh and write them back when expired.
/// A secret that is not JSON is treated as a bare access token by the callers.
pub(crate) fn load_fresh(key: &str, secrets: &dyn SecretStore) -> Result<Option<Tokens>> {
    let Some(raw) = secrets.get(key) else { return Ok(None) };
    let Ok(tokens) = Tokens::from_json(&raw) else { return Ok(None) };
    if !tokens.is_expired() || tokens.refresh_token.is_none() {
        return Ok(Some(tokens));
    }
    tracing::debug!(provider = tokens.provider.name(), "refreshing expired access token");
    let fresh = tokens.refreshed()?;
    secrets.set(key, &fresh.to_json())?;
    Ok(Some(fresh))
}

fn token_request(
    p: Provider,
    ep: &Endpoints,
    params: &[(&str, &str)],
    client_id: &str,
    client_secret: Option<&str>,
    old_refresh: Option<&str>,
) -> Result<Tokens> {
    let resp = Call::post(ep.token_url).header("Accept", "application/json").form(params).send()?;
    let status = resp.status;
    let json: Value = resp.json().unwrap_or(Value::Null);
    if status >= 400 || json.get("error").is_some() {
        let detail = json["error_description"].as_str().or_else(|| json["error"].as_str()).unwrap_or("token request failed");
        let status = status.to_string();
        return Err(Error::Network(tf("upload.error.http", &[("provider", p.name()), ("status", &status), ("detail", detail)])));
    }
    let access_token = json["access_token"]
        .as_str()
        .ok_or_else(|| Error::Network(tf("upload.error.bad_response", &[("provider", p.name())])))?
        .to_string();
    let refresh_token = json["refresh_token"].as_str().map(String::from).or_else(|| old_refresh.map(String::from));
    let expires_at_ms =
        json["expires_in"].as_i64().or_else(|| json["expires_in"].as_str().and_then(|s| s.parse().ok())).map(|s| now_ms() + s * 1000);
    Ok(Tokens {
        provider: p,
        access_token,
        refresh_token,
        expires_at_ms,
        client_id: client_id.to_string(),
        client_secret: client_secret.filter(|s| !s.is_empty()).map(String::from),
    })
}

fn random_string(len: usize) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut rng = rand::rng();
    (0..len).map(|_| CHARS[rng.random_range(0..CHARS.len())] as char).collect()
}

enum Callback {
    Code(String),
    Failed(String),
}

fn wait_for_code(listener: &TcpListener, state: &str, timeout: Duration) -> Result<String> {
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Some(cb) = handle_connection(stream, state) {
                    return match cb {
                        Callback::Code(code) => Ok(code),
                        Callback::Failed(reason) if reason == "access_denied" => Err(Error::Cancelled),
                        Callback::Failed(reason) => Err(Error::Network(reason)),
                    };
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(Error::Network(t("upload.oauth.timeout").into_owned()));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// Serve one connection; returns the callback outcome if this was the redirect.
fn handle_connection(mut stream: TcpStream, state: &str) -> Option<Callback> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16 * 1024 {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let head = String::from_utf8_lossy(&buf);
    let target = head.lines().next()?.split_whitespace().nth(1)?;
    let url = url::Url::parse(&format!("http://localhost{target}")).ok()?;
    let param = |name: &str| url.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned());

    let outcome = if let Some(err) = param("error") {
        Callback::Failed(err)
    } else if let Some(code) = param("code") {
        if param("state").as_deref() != Some(state) {
            Callback::Failed(t("upload.oauth.state_mismatch").into_owned())
        } else {
            Callback::Code(code)
        }
    } else {
        // Favicon probes and the like.
        respond(&mut stream, 404, "Not Found", "");
        return None;
    };
    let (title, body) = match &outcome {
        Callback::Code(_) => (t("upload.oauth.done_title"), t("upload.oauth.done_body")),
        Callback::Failed(_) => (t("upload.oauth.failed_title"), t("upload.oauth.failed_body")),
    };
    respond(&mut stream, 200, "OK", &page(&title, &body, matches!(outcome, Callback::Code(_))));
    Some(outcome)
}

fn respond(stream: &mut TcpStream, status: u16, reason: &str, body: &str) {
    let msg = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(msg.as_bytes());
    let _ = stream.flush();
}

fn page(title: &str, body: &str, ok: bool) -> String {
    let accent = if ok { "#3D5AFE" } else { "#D7263D" };
    let mark = if ok { "&#10003;" } else { "!" };
    format!(
        r#"<!doctype html>
<html lang="{lang}"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>{title}</title>
<style>
:root{{--bg:#F5F6FA;--card:#fff;--text:#16181D;--muted:#5B6270;--accent:{accent}}}
@media(prefers-color-scheme:dark){{:root{{--bg:#0F1115;--card:#181B21;--text:#ECEEF3;--muted:#9AA1AE}}}}
*{{box-sizing:border-box}}
body{{margin:0;min-height:100vh;display:grid;place-items:center;background:var(--bg);color:var(--text);font:16px/1.5 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif}}
main{{background:var(--card);border-radius:16px;padding:40px 48px;max-width:420px;margin:16px;text-align:center;box-shadow:0 8px 32px rgba(0,0,0,.12)}}
.mark{{width:56px;height:56px;border-radius:50%;margin:0 auto 20px;display:grid;place-items:center;background:var(--accent);color:#fff;font-size:28px;font-weight:700}}
h1{{font-size:20px;margin:0 0 8px}}p{{margin:0;color:var(--muted)}}
</style></head><body><main><div class="mark">{mark}</div><h1>{title}</h1><p>{body}</p></main></body></html>"#,
        lang = html_escape(&snap_core::i18n::current_language()),
        title = html_escape(title),
        body = html_escape(body),
    )
}
