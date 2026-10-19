//! Google Drive v3: multipart upload (resumable for large files), then an
//! `anyone`/`reader` permission and the view + direct thumbnail links.

use serde_json::{Value, json};
use snap_core::{Error, Result, t};

use crate::{
    UploadRequest, UploadResult, Uploader,
    http::{Call, Progress, Response},
    multipart::Multipart,
    session::Session,
    util::{base_name, mime_or_octet, safe_relative_path},
};

const PROVIDER: &str = "Google Drive";
const UPLOAD: &str = "https://www.googleapis.com/upload/drive/v3/files";
const FILES: &str = "https://www.googleapis.com/drive/v3/files";
const FIELDS: &str = "id,name,webViewLink";
/// Google allows multipart uploads up to 5 MB; larger files use the resumable protocol.
const MULTIPART_LIMIT: usize = 5 * 1024 * 1024;
/// Resumable chunks must be multiples of 256 KiB.
const CHUNK: usize = 32 * 256 * 1024;

pub(crate) struct GoogleDrive {
    pub session: Session,
    pub folder_id: String,
}

impl GoogleDrive {
    fn metadata(&self, name: &str) -> Value {
        let mut meta = json!({ "name": name });
        if !self.folder_id.trim().is_empty() {
            meta["parents"] = json!([self.folder_id.trim()]);
        }
        meta
    }

    fn upload_multipart(&self, meta: &Value, req: &UploadRequest, progress: &dyn Fn(u64, u64)) -> Result<Response> {
        let meta = meta.to_string();
        let total = req.data.len() as u64;
        self.session.send(|tok| {
            let body = Multipart::related().inline("application/json; charset=UTF-8", &meta).part(mime_or_octet(req.mime), req.data);
            Call::post(format!("{UPLOAD}?uploadType=multipart&supportsAllDrives=true&fields={FIELDS}"))
                .bearer(tok)
                .multipart(body)
                .progress(Progress::new(progress, total))
                .send()
        })
    }

    fn upload_resumable(&self, meta: &Value, req: &UploadRequest, progress: &dyn Fn(u64, u64)) -> Result<Response> {
        let total = req.data.len() as u64;
        let init = self
            .session
            .send(|tok| {
                Call::post(format!("{UPLOAD}?uploadType=resumable&supportsAllDrives=true&fields={FIELDS}"))
                    .bearer(tok)
                    .header("X-Upload-Content-Type", mime_or_octet(req.mime))
                    .header("X-Upload-Content-Length", total.to_string())
                    .json(meta)
                    .send()
            })?
            .check(PROVIDER)?;
        let location = init.header("location").ok_or_else(|| Error::Network(t("upload.error.no_link").into_owned()))?.to_string();

        let mut offset = 0usize;
        loop {
            let end = (offset + CHUNK).min(req.data.len());
            let chunk = &req.data[offset..end];
            let resp = Call::put(&location)
                .header("Content-Range", format!("bytes {offset}-{}/{total}", end.saturating_sub(1)))
                .bytes(chunk, mime_or_octet(req.mime))
                .progress(Progress::new(progress, total).at(offset as u64))
                .manual_redirects()
                .send()?;
            if resp.status == 308 {
                offset = end;
                continue;
            }
            return Ok(resp);
        }
    }
}

impl Uploader for GoogleDrive {
    fn upload(&self, req: &UploadRequest, progress: &dyn Fn(u64, u64)) -> Result<UploadResult> {
        let name = base_name(&safe_relative_path(req.file_name)).to_string();
        let meta = self.metadata(&name);
        let resp = if req.data.len() <= MULTIPART_LIMIT {
            self.upload_multipart(&meta, req, progress)?
        } else {
            self.upload_resumable(&meta, req, progress)?
        };
        let file = resp.check(PROVIDER)?.json()?;
        let id = file["id"].as_str().ok_or_else(|| Error::Network(t("upload.error.no_link").into_owned()))?.to_string();

        self.session
            .send(|tok| {
                Call::post(format!("{FILES}/{id}/permissions?supportsAllDrives=true"))
                    .bearer(tok)
                    .json(&json!({ "role": "reader", "type": "anyone" }))
                    .send()
            })?
            .check(PROVIDER)?;

        let total = req.data.len() as u64;
        progress(total, total);
        let url = file["webViewLink"].as_str().map(String::from).unwrap_or_else(|| format!("https://drive.google.com/file/d/{id}/view"));
        let thumbnail_url = Some(format!("https://drive.google.com/thumbnail?id={id}&sz=w1600"));
        Ok(UploadResult { url, thumbnail_url, delete_url: None, provider: PROVIDER.into() })
    }

    fn test(&self) -> Result<()> {
        self.session
            .send(|tok| Call::get("https://www.googleapis.com/drive/v3/about?fields=user").bearer(tok).send())?
            .check(PROVIDER)
            .map(|_| ())
    }
}
