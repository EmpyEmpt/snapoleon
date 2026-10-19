//! WebDAV (Nextcloud, ownCloud, Yandex WebDAV, Apache/nginx dav): MKCOL the
//! missing parent collections, then PUT with basic auth.

use snap_core::{Error, Result, tf};

use crate::{
    UploadRequest, UploadResult, Uploader,
    http::{Call, Progress},
    util::{join_url, mime_or_octet, pct_path, safe_relative_path},
};

const PROVIDER: &str = "WebDAV";

pub(crate) struct WebDav {
    pub url: String,
    pub user: String,
    pub password: String,
    pub public_base_url: String,
}

impl WebDav {
    fn authed<'a>(&self, call: Call<'a>) -> Call<'a> {
        if self.user.is_empty() { call } else { call.basic(&self.user, &self.password) }
    }

    fn base(&self) -> &str {
        self.url.trim().trim_end_matches('/')
    }

    /// MKCOL the base collection and every sub-collection leading to `rel`.
    /// Existing collections answer 405 (or 301/409 on some servers); that is fine.
    fn make_parents(&self, rel: &str) -> Result<()> {
        let mut path = self.base().to_string();
        self.authed(Call::new("MKCOL", path.clone())).send()?;
        let segments: Vec<&str> = rel.split('/').collect();
        for seg in &segments[..segments.len().saturating_sub(1)] {
            path = join_url(&path, &pct_path(seg));
            self.authed(Call::new("MKCOL", path.clone())).send()?;
        }
        Ok(())
    }

    fn put(&self, target: &str, req: &UploadRequest, progress: &dyn Fn(u64, u64)) -> Result<crate::http::Response> {
        let total = req.data.len() as u64;
        self.authed(Call::put(target).bytes(req.data, mime_or_octet(req.mime)).progress(Progress::new(progress, total))).send()
    }
}

impl Uploader for WebDav {
    fn upload(&self, req: &UploadRequest, progress: &dyn Fn(u64, u64)) -> Result<UploadResult> {
        if self.base().is_empty() {
            return Err(Error::invalid(tf("upload.error.bad_url", &[("url", &self.url)])));
        }
        let rel = safe_relative_path(req.file_name);
        let target = join_url(self.base(), &pct_path(&rel));
        let mut resp = self.put(&target, req, progress)?;
        if matches!(resp.status, 404 | 409) {
            self.make_parents(&rel)?;
            resp = self.put(&target, req, progress)?;
        }
        resp.check(PROVIDER)?;
        let total = req.data.len() as u64;
        progress(total, total);

        let url = if self.public_base_url.trim().is_empty() { target } else { join_url(self.public_base_url.trim(), &pct_path(&rel)) };
        Ok(UploadResult { url, thumbnail_url: None, delete_url: None, provider: PROVIDER.into() })
    }

    fn test(&self) -> Result<()> {
        let propfind = || self.authed(Call::new("PROPFIND", self.base().to_string()).header("Depth", "0")).send();
        let resp = propfind()?;
        if resp.status == 404 {
            self.make_parents("x")?;
            return propfind()?.check(PROVIDER).map(|_| ());
        }
        resp.check(PROVIDER).map(|_| ())
    }
}
