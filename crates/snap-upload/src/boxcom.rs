//! Box: upload API (chunked upload sessions above 50 MB) and an `open` shared link.

use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use snap_core::{Error, Result, naming, t};

use crate::{
    UploadRequest, UploadResult, Uploader,
    http::{Call, Progress, Response},
    multipart::Multipart,
    session::Session,
    util::{base_name, mime_or_octet, safe_relative_path, split_ext},
};

const PROVIDER: &str = "Box";
const API: &str = "https://api.box.com/2.0";
const UPLOAD: &str = "https://upload.box.com/api/2.0";
const DIRECT_LIMIT: usize = 50 * 1024 * 1024;

pub(crate) struct BoxCom {
    pub session: Session,
    pub folder_id: String,
}

fn digest(data: &[u8]) -> String {
    format!("sha={}", STANDARD.encode(Sha1::digest(data)))
}

impl BoxCom {
    fn folder(&self) -> &str {
        let id = self.folder_id.trim();
        if id.is_empty() { "0" } else { id }
    }

    fn upload_direct(&self, name: &str, req: &UploadRequest, progress: &dyn Fn(u64, u64)) -> Result<Response> {
        let attributes = json!({ "name": name, "parent": { "id": self.folder() } }).to_string();
        let total = req.data.len() as u64;
        self.session.send(|tok| {
            let form = Multipart::form().text("attributes", &attributes).file("file", name, mime_or_octet(req.mime), req.data);
            Call::post(format!("{UPLOAD}/files/content")).bearer(tok).multipart(form).progress(Progress::new(progress, total)).send()
        })
    }

    fn upload_chunked(&self, name: &str, req: &UploadRequest, progress: &dyn Fn(u64, u64)) -> Result<Response> {
        let total = req.data.len() as u64;
        let created = self.session.send(|tok| {
            Call::post(format!("{UPLOAD}/files/upload_sessions"))
                .bearer(tok)
                .json(&json!({ "folder_id": self.folder(), "file_size": total, "file_name": name }))
                .send()
        })?;
        if !created.is_success() {
            return Ok(created);
        }
        let session = created.json()?;
        let part_url = session["session_endpoints"]["upload_part"].as_str();
        let commit_url = session["session_endpoints"]["commit"].as_str();
        let part_size = session["part_size"].as_u64().filter(|s| *s > 0);
        let (Some(part_url), Some(commit_url), Some(part_size)) = (part_url, commit_url, part_size) else {
            return Err(Error::Network(t("upload.error.no_link").into_owned()));
        };

        let mut parts = Vec::new();
        let mut offset = 0usize;
        while offset < req.data.len() {
            let end = (offset + part_size as usize).min(req.data.len());
            let chunk = &req.data[offset..end];
            let resp = self
                .session
                .send(|tok| {
                    Call::put(part_url)
                        .bearer(tok)
                        .header("Digest", digest(chunk))
                        .header("Content-Range", format!("bytes {offset}-{}/{total}", end - 1))
                        .bytes(chunk, "application/octet-stream")
                        .progress(Progress::new(progress, total).at(offset as u64))
                        .send()
                })?
                .check(PROVIDER)?
                .json()?;
            parts.push(resp["part"].clone());
            offset = end;
        }

        let whole = digest(req.data);
        for _ in 0..12 {
            let resp = self
                .session
                .send(|tok| Call::post(commit_url).bearer(tok).header("Digest", whole.clone()).json(&json!({ "parts": parts })).send())?;
            if resp.status != 202 {
                return Ok(resp);
            }
            let wait = resp.header("retry-after").and_then(|s| s.parse::<u64>().ok()).unwrap_or(1);
            std::thread::sleep(Duration::from_secs(wait.min(10)));
        }
        Err(Error::Network(t("upload.error.timeout").into_owned()))
    }

    fn upload_named(&self, name: &str, req: &UploadRequest, progress: &dyn Fn(u64, u64)) -> Result<Response> {
        if req.data.len() > DIRECT_LIMIT { self.upload_chunked(name, req, progress) } else { self.upload_direct(name, req, progress) }
    }
}

impl Uploader for BoxCom {
    fn upload(&self, req: &UploadRequest, progress: &dyn Fn(u64, u64)) -> Result<UploadResult> {
        let name = base_name(&safe_relative_path(req.file_name)).to_string();
        let mut resp = self.upload_named(&name, req, progress)?;
        if resp.status == 409 {
            // Name already taken in the folder: retry once with a random suffix.
            let (stem, ext) = split_ext(&name);
            resp = self.upload_named(&format!("{stem}-{}{ext}", naming::random_code(4)), req, progress)?;
        }
        let entries: Value = resp.check(PROVIDER)?.json()?;
        let id = entries["entries"][0]["id"].as_str().ok_or_else(|| Error::Network(t("upload.error.no_link").into_owned()))?.to_string();

        let shared = self
            .session
            .send(|tok| {
                Call::put(format!("{API}/files/{id}?fields=shared_link"))
                    .bearer(tok)
                    .json(&json!({ "shared_link": { "access": "open" } }))
                    .send()
            })?
            .check(PROVIDER)?
            .json()?;
        let url = shared["shared_link"]["url"].as_str().ok_or_else(|| Error::Network(t("upload.error.no_link").into_owned()))?.to_string();
        let total = req.data.len() as u64;
        progress(total, total);
        Ok(UploadResult { url, thumbnail_url: None, delete_url: None, provider: PROVIDER.into() })
    }

    fn test(&self) -> Result<()> {
        self.session.send(|tok| Call::get(format!("{API}/users/me")).bearer(tok).send())?.check(PROVIDER).map(|_| ())
    }
}
