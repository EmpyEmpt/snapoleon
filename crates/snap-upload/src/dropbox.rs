//! Dropbox: `files/upload` (upload sessions for large files) and a shared link
//! converted to a direct (`raw=1`) URL.

use serde_json::{Value, json};
use snap_core::{Error, Result, t};

use crate::{
    Uploader, UploadRequest, UploadResult,
    http::{Call, Progress},
    session::Session,
    util::safe_relative_path,
};

const PROVIDER: &str = "Dropbox";
const CONTENT: &str = "https://content.dropboxapi.com/2";
const API: &str = "https://api.dropboxapi.com/2";
const SINGLE_LIMIT: usize = 140 * 1024 * 1024;
const CHUNK: usize = 8 * 1024 * 1024;

pub(crate) struct Dropbox {
    pub session: Session,
    pub folder: String,
}

/// JSON for the `Dropbox-API-Arg` header, which must be pure ASCII.
fn api_arg(v: &Value) -> String {
    let mut out = String::new();
    for c in v.to_string().chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut units = [0u16; 2];
            for u in c.encode_utf16(&mut units) {
                out.push_str(&format!("\\u{u:04x}"));
            }
        }
    }
    out
}

impl Dropbox {
    fn remote_path(&self, rel: &str) -> String {
        let folder = self.folder.trim().trim_matches('/');
        if folder.is_empty() { format!("/{rel}") } else { format!("/{folder}/{rel}") }
    }

    fn content_call<'a>(&self, endpoint: &str, token: &str, arg: &Value, body: &'a [u8], progress: Option<Progress<'a>>) -> Result<crate::http::Response> {
        let mut call = Call::post(format!("{CONTENT}/{endpoint}")).bearer(token).header("Dropbox-API-Arg", api_arg(arg)).bytes(body, "application/octet-stream");
        if let Some(p) = progress {
            call = call.progress(p);
        }
        call.send()
    }

    /// Uploads and returns the stored path (it may differ because of `autorename`).
    fn upload_file(&self, path: &str, data: &[u8], progress: &dyn Fn(u64, u64)) -> Result<String> {
        let total = data.len() as u64;
        let commit = json!({ "path": path, "mode": "add", "autorename": true, "mute": true });
        if data.len() <= SINGLE_LIMIT {
            let meta = self.session.send(|tok| self.content_call("files/upload", tok, &commit, data, Some(Progress::new(progress, total))))?.check(PROVIDER)?.json()?;
            return Ok(stored_path(&meta, path));
        }

        let mut chunks = data.chunks(CHUNK);
        let first = chunks.next().unwrap_or_default();
        let start = self
            .session
            .send(|tok| self.content_call("files/upload_session/start", tok, &json!({ "close": false }), first, Some(Progress::new(progress, total))))?
            .check(PROVIDER)?
            .json()?;
        let session_id = start["session_id"].as_str().ok_or_else(|| Error::Network(t("upload.error.no_link").into_owned()))?.to_string();
        let mut offset = first.len() as u64;
        let mut chunks = chunks.peekable();
        while let Some(chunk) = chunks.next() {
            let cursor = json!({ "session_id": session_id, "offset": offset });
            let progress_here = Progress::new(progress, total).at(offset);
            if chunks.peek().is_some() {
                let arg = json!({ "cursor": cursor, "close": false });
                self.session.send(|tok| self.content_call("files/upload_session/append_v2", tok, &arg, chunk, Some(progress_here)))?.check(PROVIDER)?;
            } else {
                let arg = json!({ "cursor": cursor, "commit": commit });
                let meta = self.session.send(|tok| self.content_call("files/upload_session/finish", tok, &arg, chunk, Some(progress_here)))?.check(PROVIDER)?.json()?;
                return Ok(stored_path(&meta, path));
            }
            offset += chunk.len() as u64;
        }
        Ok(path.to_string())
    }

    fn api_json(&self, endpoint: &str, body: &Value) -> Result<crate::http::Response> {
        self.session.send(|tok| Call::post(format!("{API}/{endpoint}")).bearer(tok).json(body).send())
    }

    fn shared_link(&self, path: &str) -> Result<String> {
        let created = self.api_json(
            "sharing/create_shared_link_with_settings",
            &json!({ "path": path, "settings": { "requested_visibility": "public", "audience": "public", "access": "viewer" } }),
        )?;
        if created.is_success() {
            return created.json()?["url"].as_str().map(String::from).ok_or_else(|| Error::Network(t("upload.error.no_link").into_owned()));
        }
        // 409 `shared_link_already_exists`: reuse the existing link.
        let existing = self.api_json("sharing/list_shared_links", &json!({ "path": path, "direct_only": true }))?.check(PROVIDER)?.json()?;
        existing["links"][0]["url"].as_str().map(String::from).ok_or_else(|| created.error(PROVIDER))
    }
}

/// Turn a Dropbox preview link into one that serves the file itself.
fn raw_link(url: &str) -> String {
    if url.contains("dl=0") {
        url.replace("dl=0", "raw=1")
    } else if url.contains("raw=1") {
        url.to_string()
    } else if url.contains('?') {
        format!("{url}&raw=1")
    } else {
        format!("{url}?raw=1")
    }
}

impl Uploader for Dropbox {
    fn upload(&self, req: &UploadRequest, progress: &dyn Fn(u64, u64)) -> Result<UploadResult> {
        let path = self.remote_path(&safe_relative_path(req.file_name));
        let stored = self.upload_file(&path, req.data, progress)?;
        let link = self.shared_link(&stored)?;
        let total = req.data.len() as u64;
        progress(total, total);
        Ok(UploadResult { url: raw_link(&link), thumbnail_url: None, delete_url: None, provider: PROVIDER.into() })
    }

    fn test(&self) -> Result<()> {
        self.session.send(|tok| Call::post(format!("{API}/users/get_current_account")).bearer(tok).send())?.check(PROVIDER).map(|_| ())
    }
}

fn stored_path(meta: &Value, fallback: &str) -> String {
    meta["path_lower"].as_str().unwrap_or(fallback).to_string()
}
