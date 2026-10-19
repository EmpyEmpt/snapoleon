//! OneDrive through Microsoft Graph: simple PUT up to 4 MB, upload session
//! above that, then an anonymous view link.

use serde_json::{Value, json};
use snap_core::{Error, Result, t};

use crate::{
    Uploader, UploadRequest, UploadResult,
    http::{Call, Progress, Response},
    session::Session,
    util::{pct_path, safe_relative_path},
};

const PROVIDER: &str = "OneDrive";
const GRAPH: &str = "https://graph.microsoft.com/v1.0";
const SIMPLE_LIMIT: usize = 4 * 1024 * 1024;
/// Upload-session fragments must be multiples of 320 KiB.
const CHUNK: usize = 32 * 320 * 1024;

pub(crate) struct OneDrive {
    pub session: Session,
    pub folder: String,
}

impl OneDrive {
    fn item_path(&self, rel: &str) -> String {
        let folder = self.folder.trim().trim_matches('/');
        let full = if folder.is_empty() { rel.to_string() } else { format!("{folder}/{rel}") };
        format!("{GRAPH}/me/drive/root:/{}", pct_path(&full))
    }

    fn put_simple(&self, path: &str, req: &UploadRequest, progress: &dyn Fn(u64, u64)) -> Result<Response> {
        let total = req.data.len() as u64;
        self.session.send(|tok| {
            Call::put(format!("{path}:/content?@microsoft.graph.conflictBehavior=rename"))
                .bearer(tok)
                .bytes(req.data, "application/octet-stream")
                .progress(Progress::new(progress, total))
                .send()
        })
    }

    fn put_session(&self, path: &str, req: &UploadRequest, progress: &dyn Fn(u64, u64)) -> Result<Response> {
        let total = req.data.len() as u64;
        let created = self
            .session
            .send(|tok| {
                Call::post(format!("{path}:/createUploadSession"))
                    .bearer(tok)
                    .json(&json!({ "item": { "@microsoft.graph.conflictBehavior": "rename" } }))
                    .send()
            })?
            .check(PROVIDER)?
            .json()?;
        let upload_url = created["uploadUrl"].as_str().ok_or_else(|| Error::Network(t("upload.error.no_link").into_owned()))?.to_string();

        let mut offset = 0usize;
        loop {
            let end = (offset + CHUNK).min(req.data.len());
            let resp = Call::put(&upload_url)
                .header("Content-Range", format!("bytes {offset}-{}/{total}", end.saturating_sub(1)))
                .bytes(&req.data[offset..end], "application/octet-stream")
                .progress(Progress::new(progress, total).at(offset as u64))
                .send()?;
            if resp.status == 202 {
                offset = end;
                continue;
            }
            return Ok(resp);
        }
    }

    fn create_link(&self, item_id: &str) -> Result<Option<String>> {
        // Organisation tenants often forbid anonymous links; fall back to org-wide.
        for scope in ["anonymous", "organization"] {
            let resp = self
                .session
                .send(|tok| Call::post(format!("{GRAPH}/me/drive/items/{item_id}/createLink")).bearer(tok).json(&json!({ "type": "view", "scope": scope })).send())?;
            if resp.is_success() {
                return Ok(resp.json()?["link"]["webUrl"].as_str().map(String::from));
            }
        }
        Ok(None)
    }
}

impl Uploader for OneDrive {
    fn upload(&self, req: &UploadRequest, progress: &dyn Fn(u64, u64)) -> Result<UploadResult> {
        let path = self.item_path(&safe_relative_path(req.file_name));
        let resp = if req.data.len() <= SIMPLE_LIMIT { self.put_simple(&path, req, progress)? } else { self.put_session(&path, req, progress)? };
        let item: Value = resp.check(PROVIDER)?.json()?;
        let id = item["id"].as_str().ok_or_else(|| Error::Network(t("upload.error.no_link").into_owned()))?;

        let url = match self.create_link(id)? {
            Some(link) => link,
            None => item["webUrl"].as_str().map(String::from).ok_or_else(|| Error::Network(t("upload.error.no_link").into_owned()))?,
        };
        let total = req.data.len() as u64;
        progress(total, total);
        Ok(UploadResult { url, thumbnail_url: None, delete_url: None, provider: PROVIDER.into() })
    }

    fn test(&self) -> Result<()> {
        self.session.send(|tok| Call::get(format!("{GRAPH}/me/drive")).bearer(tok).send())?.check(PROVIDER).map(|_| ())
    }
}
