//! Thin blocking HTTP layer over `ureq` shared by every destination: explicit
//! bodies with progress reporting, status codes returned instead of raised,
//! and a response type with the helpers the providers need.

use std::{
    borrow::Cow,
    io::{self, Read},
    sync::OnceLock,
    time::Duration,
};

use base64::Engine as _;
use serde_json::Value;
use snap_core::{Error, Result, tf};
use ureq::{ResponseExt, SendBody, http};

use crate::{multipart::Multipart, util::pct};

const MAX_RESPONSE: u64 = 32 * 1024 * 1024;

fn build_agent(follow_redirects: bool) -> ureq::Agent {
    let mut b = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .allow_non_standard_methods(true)
        .user_agent(format!("Snapoleon/{}", snap_core::VERSION))
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(300)));
    if !follow_redirects {
        b = b.max_redirects(0).max_redirects_will_error(false);
    }
    b.build().into()
}

/// Shared agent (connection pool). The `manual` variant hands 3xx responses
/// back untouched, which resumable-upload protocols (HTTP 308) rely on.
fn agent(follow_redirects: bool) -> ureq::Agent {
    static FOLLOW: OnceLock<ureq::Agent> = OnceLock::new();
    static MANUAL: OnceLock<ureq::Agent> = OnceLock::new();
    let cell = if follow_redirects { &FOLLOW } else { &MANUAL };
    cell.get_or_init(|| build_agent(follow_redirects)).clone()
}

/// Reports `(sent, total)` for one request body, offset by `base` so that
/// chunked uploads can present a single overall progress bar.
#[derive(Clone, Copy)]
pub(crate) struct Progress<'a> {
    pub cb: &'a dyn Fn(u64, u64),
    pub base: u64,
    pub total: u64,
}

impl<'a> Progress<'a> {
    pub fn new(cb: &'a dyn Fn(u64, u64), total: u64) -> Self {
        Self { cb, base: 0, total }
    }
    pub fn at(self, base: u64) -> Self {
        Self { base, ..self }
    }
}

struct ChunkReader<'r, 'a> {
    chunks: &'r [Cow<'a, [u8]>],
    idx: usize,
    off: usize,
    sent: u64,
    progress: Option<Progress<'r>>,
}

impl Read for ChunkReader<'_, '_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while let Some(chunk) = self.chunks.get(self.idx) {
            let rest = &chunk[self.off..];
            if rest.is_empty() {
                self.idx += 1;
                self.off = 0;
                continue;
            }
            let n = rest.len().min(buf.len());
            buf[..n].copy_from_slice(&rest[..n]);
            self.off += n;
            self.sent += n as u64;
            if let Some(p) = &self.progress {
                (p.cb)((p.base + self.sent).min(p.total), p.total);
            }
            return Ok(n);
        }
        Ok(0)
    }
}

pub(crate) struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Final URL after redirects.
    pub url: String,
}

impl Response {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json(&self) -> Result<Value> {
        serde_json::from_slice(&self.body).map_err(|_| Error::Network(self.text().chars().take(300).collect()))
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// Succeed on 2xx, otherwise produce a readable error naming the provider.
    pub fn check(self, provider: &str) -> Result<Self> {
        if self.is_success() { Ok(self) } else { Err(self.error(provider)) }
    }

    pub fn error(&self, provider: &str) -> Error {
        let status = self.status.to_string();
        match error_detail(&self.body) {
            Some(detail) => Error::Network(tf("upload.error.http", &[("provider", provider), ("status", &status), ("detail", &detail)])),
            None => Error::Network(tf("upload.error.http_nodetail", &[("provider", provider), ("status", &status)])),
        }
    }
}

/// Best-effort human message out of an error body (JSON `message`/`error`
/// fields, S3 XML `<Message>` or plain text).
fn error_detail(body: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(body);
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        let found = ["message", "error_description", "error_summary", "description", "error", "errors"].iter().find_map(|k| match v.get(*k)? {
            Value::String(s) => Some(s.clone()),
            Value::Object(o) => ["message", "description", "error"].iter().find_map(|k| o.get(*k)?.as_str().map(String::from)),
            _ => None,
        });
        return found.map(|m| m.chars().take(300).collect());
    }
    if text.starts_with('<') {
        return xml_tag(text, "Message").map(|m| m.chars().take(300).collect());
    }
    Some(text.chars().take(300).collect())
}

fn xml_tag(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&format!("</{tag}>"))?;
    Some(text[start..start + end].to_string())
}

pub(crate) struct Call<'a> {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Vec<Cow<'a, [u8]>>,
    progress: Option<Progress<'a>>,
    follow_redirects: bool,
}

impl<'a> Call<'a> {
    pub fn new(method: &str, url: impl Into<String>) -> Self {
        Self { method: method.to_ascii_uppercase(), url: url.into(), headers: Vec::new(), body: Vec::new(), progress: None, follow_redirects: true }
    }

    pub fn get(url: impl Into<String>) -> Self {
        Self::new("GET", url)
    }
    pub fn post(url: impl Into<String>) -> Self {
        Self::new("POST", url)
    }
    pub fn put(url: impl Into<String>) -> Self {
        Self::new("PUT", url)
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.to_string(), value.into()));
        self
    }

    pub fn bearer(self, token: &str) -> Self {
        self.header("Authorization", format!("Bearer {token}"))
    }

    pub fn basic(self, user: &str, password: &str) -> Self {
        let cred = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"));
        self.header("Authorization", format!("Basic {cred}"))
    }

    pub fn bytes(mut self, data: &'a [u8], content_type: &str) -> Self {
        self.body = vec![Cow::Borrowed(data)];
        self.header("Content-Type", content_type)
    }

    pub fn owned(mut self, data: Vec<u8>, content_type: &str) -> Self {
        self.body = vec![Cow::Owned(data)];
        self.header("Content-Type", content_type)
    }

    /// Body bytes without touching `Content-Type` (the caller set it already).
    pub fn raw_body(mut self, data: &'a [u8]) -> Self {
        self.body = vec![Cow::Borrowed(data)];
        self
    }

    pub fn multipart(mut self, m: Multipart<'a>) -> Self {
        let (content_type, parts) = m.finish();
        self.body = parts;
        self.header("Content-Type", content_type)
    }

    pub fn json(self, v: &Value) -> Self {
        self.owned(v.to_string().into_bytes(), "application/json")
    }

    pub fn form(self, pairs: &[(&str, &str)]) -> Self {
        let body = pairs.iter().map(|(k, v)| format!("{}={}", pct(k), pct(v))).collect::<Vec<_>>().join("&");
        self.owned(body.into_bytes(), "application/x-www-form-urlencoded")
    }

    pub fn progress(mut self, p: Progress<'a>) -> Self {
        self.progress = Some(p);
        self
    }

    /// Return 3xx responses instead of following them.
    pub fn manual_redirects(mut self) -> Self {
        self.follow_redirects = false;
        self
    }

    pub fn send(self) -> Result<Response> {
        let len: u64 = self.body.iter().map(|c| c.len() as u64).sum();
        let mut builder = http::Request::builder().method(self.method.as_str()).uri(self.url.as_str());
        for (k, v) in &self.headers {
            builder = builder.header(k, v);
        }
        let has_body = !self.body.is_empty() || matches!(self.method.as_str(), "POST" | "PUT" | "PATCH");
        if has_body {
            builder = builder.header("Content-Length", len.to_string());
        }
        let mut reader = ChunkReader { chunks: &self.body, idx: 0, off: 0, sent: 0, progress: self.progress };
        let (parts, ()) = builder.body(()).map_err(|e| Error::invalid(format!("{}: {e}", self.url)))?.into_parts();
        let agent = agent(self.follow_redirects);
        let result = if has_body {
            agent.run(http::Request::from_parts(parts, SendBody::from_reader(&mut reader)))
        } else {
            agent.run(http::Request::from_parts(parts, SendBody::none()))
        };
        let mut response = result.map_err(|e| Error::Network(format!("{}: {e}", host_of(&self.url))))?;
        let status = response.status().as_u16();
        let url = response.get_uri().to_string();
        let headers = response
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned()))
            .collect();
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE)
            .read_to_vec()
            .map_err(|e| Error::Network(format!("{}: {e}", host_of(&self.url))))?;
        Ok(Response { status, headers, body, url })
    }
}

fn host_of(url: &str) -> String {
    url::Url::parse(url).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or_else(|| url.to_string())
}
