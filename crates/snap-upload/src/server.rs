//! Client for the self-hosted `snapoleon-server` (team mode).
//!
//! Wire format (JSON, bearer token): `GET /api/v1/me`, `GET /api/v1/items?page=N`,
//! `DELETE /api/v1/items/{id}`, `GET /api/v1/events?since={ms}`,
//! `POST /api/v1/upload` (multipart), `POST /api/v1/shorten`.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use snap_core::{Error, Result, t};

use crate::{
    UploadRequest, UploadResult, Uploader,
    http::{Call, Progress, Response},
    multipart::Multipart,
    util::{join_url, mime_or_octet},
};

const PROVIDER: &str = "Snapoleon server";

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Me {
    pub id: String,
    pub name: String,
    pub email: Option<String>,
    pub workspace: Option<String>,
    pub role: Option<String>,
    pub quota_bytes: Option<u64>,
    pub used_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteItem {
    pub id: String,
    pub title: Option<String>,
    /// Public viewer page.
    pub url: String,
    pub raw_url: Option<String>,
    pub mime: Option<String>,
    pub size_bytes: u64,
    pub views: u64,
    pub created_ms: i64,
    pub expires_ms: Option<i64>,
    pub has_password: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteEventKind {
    View,
    Comment,
    #[default]
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteEvent {
    pub id: String,
    pub kind: RemoteEventKind,
    pub item_id: String,
    pub at_ms: i64,
    pub actor: Option<String>,
    /// Comment text, when `kind` is a comment.
    pub text: Option<String>,
}

/// What the server returns for a finished upload.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct Uploaded {
    pub id: String,
    pub url: String,
    pub raw_url: Option<String>,
    pub delete_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Client {
    base: String,
    token: String,
}

impl Client {
    pub fn new(url: &str, token: &str) -> Self {
        Self { base: url.trim().trim_end_matches('/').to_string(), token: token.trim().to_string() }
    }

    fn url(&self, path: &str) -> String {
        join_url(&self.base, path)
    }

    fn check(&self, resp: Response) -> Result<Response> {
        match resp.status {
            401 | 403 => Err(Error::permission(t("upload.error.server_auth").into_owned())),
            404 => Err(Error::NotFound(resp.url.clone())),
            _ => resp.check(PROVIDER),
        }
    }

    pub fn me(&self) -> Result<Me> {
        let resp = self.check(Call::get(self.url("api/v1/me")).bearer(&self.token).send()?)?;
        serde_json::from_slice(&resp.body).map_err(|e| Error::Network(e.to_string()))
    }

    /// One page (newest first) of the caller's uploads; empty past the last page.
    pub fn list(&self, page: u32) -> Result<Vec<RemoteItem>> {
        let resp = self.check(Call::get(self.url(&format!("api/v1/items?page={page}"))).bearer(&self.token).send()?)?;
        list_from(resp.json()?, "items")
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        self.check(Call::new("DELETE", self.url(&format!("api/v1/items/{}", crate::util::pct(id)))).bearer(&self.token).send()?).map(|_| ())
    }

    /// View / comment notifications newer than `since_ms`.
    pub fn events(&self, since_ms: i64) -> Result<Vec<RemoteEvent>> {
        let resp = self.check(Call::get(self.url(&format!("api/v1/events?since={since_ms}"))).bearer(&self.token).send()?)?;
        list_from(resp.json()?, "events")
    }

    pub fn upload(&self, req: &UploadRequest, progress: &dyn Fn(u64, u64)) -> Result<Uploaded> {
        let mut form = Multipart::form();
        if let Some(h) = req.expiry_hours.filter(|h| *h > 0) {
            form = form.text("expiry_hours", &h.to_string());
        }
        if let Some(p) = req.password.filter(|p| !p.is_empty()) {
            form = form.text("password", p);
        }
        if let Some(title) = req.title.filter(|t| !t.is_empty()) {
            form = form.text("title", title);
        }
        let form = form.file("file", req.file_name, mime_or_octet(req.mime), req.data);
        let resp = self.check(
            Call::post(self.url("api/v1/upload"))
                .bearer(&self.token)
                .multipart(form)
                .progress(Progress::new(progress, req.data.len() as u64))
                .send()?,
        )?;
        let mut uploaded: Uploaded = serde_json::from_slice(&resp.body).map_err(|e| Error::Network(e.to_string()))?;
        if uploaded.url.is_empty() {
            return Err(Error::Network(t("upload.error.no_link").into_owned()));
        }
        // The server may answer with paths relative to its base.
        uploaded.delete_url = uploaded.delete_url.filter(|u| !u.is_empty()).map(|u| self.absolute(&u));
        uploaded.raw_url = uploaded.raw_url.filter(|u| !u.is_empty()).map(|u| self.absolute(&u));
        uploaded.url = self.absolute(&uploaded.url);
        Ok(uploaded)
    }

    /// Create a short link for `url` on the server.
    pub fn shorten(&self, url: &str) -> Result<String> {
        let resp = self.check(Call::post(self.url("api/v1/shorten")).bearer(&self.token).json(&json!({ "url": url })).send()?)?;
        let body = resp.json()?;
        ["short_url", "shortUrl", "url", "short"]
            .iter()
            .find_map(|k| body.get(*k).and_then(Value::as_str))
            .map(|u| self.absolute(u))
            .ok_or_else(|| Error::Network(t("upload.error.no_link").into_owned()))
    }

    fn absolute(&self, u: &str) -> String {
        if u.contains("://") { u.to_string() } else { join_url(&self.base, u) }
    }
}

/// Accept either a bare array or an object wrapping it under `key`.
fn list_from<T: for<'de> Deserialize<'de>>(body: Value, key: &str) -> Result<Vec<T>> {
    let array = match body {
        Value::Array(_) => body,
        Value::Object(mut o) => o.remove(key).unwrap_or(Value::Array(Vec::new())),
        _ => Value::Array(Vec::new()),
    };
    serde_json::from_value(array).map_err(|e| Error::Network(e.to_string()))
}

/// Upload destination for the team server.
pub(crate) struct SnapServer {
    pub client: Client,
}

impl Uploader for SnapServer {
    fn upload(&self, req: &UploadRequest, progress: &dyn Fn(u64, u64)) -> Result<UploadResult> {
        let up = self.client.upload(req, progress)?;
        let total = req.data.len() as u64;
        progress(total, total);
        Ok(UploadResult { url: up.url, thumbnail_url: up.raw_url, delete_url: up.delete_url, provider: PROVIDER.into() })
    }

    fn test(&self) -> Result<()> {
        self.client.me().map(|_| ())
    }
}
