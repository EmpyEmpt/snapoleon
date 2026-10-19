//! Yandex Disk REST API: request an upload href, PUT the bytes, publish the
//! resource and read back its public URL.

use snap_core::{Error, Result, naming, t};

use crate::{
    UploadRequest, UploadResult, Uploader,
    http::{Call, Progress, Response},
    session::Session,
    util::{pct, safe_relative_path, split_ext},
};

const PROVIDER: &str = "Yandex Disk";
const API: &str = "https://cloud-api.yandex.net/v1/disk";

pub(crate) struct YandexDisk {
    pub session: Session,
    pub folder: String,
    pub publish: bool,
}

impl YandexDisk {
    /// `disk:/Snapoleon` style root for uploads (`app:/` roots are kept as given).
    fn root(&self) -> String {
        let folder = self.folder.trim();
        if folder.starts_with("disk:") || folder.starts_with("app:") {
            return folder.trim_end_matches('/').to_string();
        }
        let folder = folder.trim_matches('/');
        format!("disk:/{}", if folder.is_empty() { "Snapoleon" } else { folder })
    }

    fn api(&self, method: &str, endpoint: &str, path: &str, extra: &str) -> Result<Response> {
        let url = format!("{API}/{endpoint}?path={}{extra}", pct(path));
        self.session.send(|tok| Call::new(method, url.as_str()).header("Authorization", format!("OAuth {tok}")).send())
    }

    fn make_dirs(&self, full_path: &str) -> Result<()> {
        let (scheme, rest) = full_path.split_once(":/").unwrap_or(("disk", full_path));
        let mut path = format!("{scheme}:");
        for seg in rest.split('/').filter(|s| !s.is_empty()) {
            path = format!("{path}/{seg}");
            let resp = self.api("PUT", "resources", &path, "")?;
            // 409 = already exists.
            if !resp.is_success() && resp.status != 409 {
                return Err(resp.error(PROVIDER));
            }
        }
        Ok(())
    }

    /// Ask for an upload href, creating missing folders and renaming on name clashes.
    fn upload_href(&self, rel: &str) -> Result<(String, String)> {
        let root = self.root();
        let mut rel = rel.to_string();
        let mut dirs_made = false;
        for _ in 0..4 {
            let path = format!("{root}/{rel}");
            let resp = self.api("GET", "resources/upload", &path, "&overwrite=false")?;
            if resp.is_success() {
                let href = resp.json()?["href"]
                    .as_str()
                    .map(String::from)
                    .ok_or_else(|| Error::Network(t("upload.error.no_link").into_owned()))?;
                return Ok((href, path));
            }
            let kind = resp.json().ok().and_then(|j| j["error"].as_str().map(String::from)).unwrap_or_default();
            match (resp.status, kind.as_str()) {
                (409, "DiskPathDoesntExistsError") if !dirs_made => {
                    let parent = path.rsplit_once('/').map_or(root.as_str(), |(p, _)| p);
                    self.make_dirs(parent)?;
                    dirs_made = true;
                }
                (409, "DiskResourceAlreadyExistsError" | "DiskPathPointsToExistentDirectoryError") => {
                    let (stem, ext) = split_ext(&rel);
                    rel = format!("{stem}-{}{ext}", naming::random_code(4));
                }
                _ => return Err(resp.error(PROVIDER)),
            }
        }
        Err(Error::Network(t("upload.error.conflict").into_owned()))
    }
}

impl Uploader for YandexDisk {
    fn upload(&self, req: &UploadRequest, progress: &dyn Fn(u64, u64)) -> Result<UploadResult> {
        let (href, path) = self.upload_href(&safe_relative_path(req.file_name))?;
        let total = req.body.size();
        Call::put(href)
            .body(req.body, crate::util::mime_or_octet(req.mime))
            .progress(Progress::new(progress, total))
            .send()?
            .check(PROVIDER)?;
        progress(total, total);

        let url = if self.publish {
            self.api("PUT", "resources/publish", &path, "")?.check(PROVIDER)?;
            let meta = self.api("GET", "resources", &path, "&fields=public_url")?.check(PROVIDER)?.json()?;
            meta["public_url"].as_str().map(String::from).ok_or_else(|| Error::Network(t("upload.error.no_link").into_owned()))?
        } else {
            format!("https://disk.yandex.ru/client/disk/{}", pct(path.trim_start_matches("disk:/")))
        };
        Ok(UploadResult { url, thumbnail_url: None, delete_url: None, provider: PROVIDER.into() })
    }

    fn test(&self) -> Result<()> {
        self.session.send(|tok| Call::get(API).header("Authorization", format!("OAuth {tok}")).send())?.check(PROVIDER).map(|_| ())
    }
}
