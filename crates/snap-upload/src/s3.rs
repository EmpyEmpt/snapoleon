//! S3-compatible object storage over plain HTTP with our own SigV4.

use snap_core::{
    Error, Result,
    naming::{self, NameContext},
    t, tf,
};
use url::Url;

use crate::{
    UploadRequest, UploadResult, Uploader,
    http::{Call, Progress},
    sigv4::{self, Credentials},
    util::{join_url, mime_or_octet, pct_path, safe_relative_path, split_ext},
};

const PROVIDER: &str = "S3";

pub(crate) struct S3Config {
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    pub access_key_id: String,
    pub secret_key: String,
    pub path_style: bool,
    pub prefix: String,
    pub public_base_url: String,
    pub random_code_len: u8,
    pub acl_public_read: bool,
}

pub(crate) struct S3Uploader {
    cfg: S3Config,
    endpoint: Url,
    path_style: bool,
}

impl S3Uploader {
    pub fn new(cfg: S3Config) -> Result<Self> {
        if cfg.bucket.trim().is_empty() {
            return Err(Error::invalid(t("upload.error.s3_bucket")));
        }
        let raw = if cfg.endpoint.trim().is_empty() { "https://s3.{region}.amazonaws.com" } else { cfg.endpoint.trim() };
        let raw = raw.replace("{region}", &cfg.region);
        if raw.contains('<') || raw.contains('>') {
            return Err(Error::invalid(t("upload.error.s3_placeholder_endpoint")));
        }
        let raw = if raw.contains("://") { raw } else { format!("https://{raw}") };
        let endpoint = Url::parse(&raw).map_err(|_| Error::invalid(tf("upload.error.bad_url", &[("url", &raw)])))?;
        let host = endpoint.host_str().ok_or_else(|| Error::invalid(tf("upload.error.bad_url", &[("url", &raw)])))?;
        // Dotted bucket names break wildcard TLS certificates; IP hosts have no subdomains.
        let host_is_ip = host.parse::<std::net::IpAddr>().is_ok() || host == "localhost";
        let dotted_tls = endpoint.scheme() == "https" && cfg.bucket.contains('.');
        let path_style = cfg.path_style || host_is_ip || dotted_tls;
        Ok(Self { cfg, endpoint, path_style })
    }

    fn host_header(&self) -> String {
        let host = self.endpoint.host_str().unwrap_or_default();
        let host = if self.path_style { host.to_string() } else { format!("{}.{host}", self.cfg.bucket) };
        match self.endpoint.port() {
            Some(p) => format!("{host}:{p}"),
            None => host,
        }
    }

    /// `(url, canonical URI path)` for an object key.
    fn locate(&self, key: &str) -> (String, String) {
        let raw = if self.path_style { format!("/{}/{key}", self.cfg.bucket) } else { format!("/{key}") };
        let canonical = sigv4::canonical_uri(&raw);
        let url = format!("{}://{}{}", self.endpoint.scheme(), self.host_header(), canonical);
        (url, canonical)
    }

    fn signed<'a>(&self, method: &str, key: &str, body: &'a [u8], content_type: Option<&str>) -> Call<'a> {
        let (url, canonical) = self.locate(key);
        let payload_hash = sigv4::sha256_hex(body);
        let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();

        let mut headers = vec![
            ("host".to_string(), self.host_header()),
            ("x-amz-content-sha256".to_string(), payload_hash.clone()),
            ("x-amz-date".to_string(), now.clone()),
        ];
        if let Some(ct) = content_type {
            headers.push(("content-type".into(), ct.to_string()));
            headers.push(("cache-control".into(), self.cache_control()));
            if self.cfg.acl_public_read {
                headers.push(("x-amz-acl".into(), "public-read".into()));
            }
        }
        let creds = Credentials { access_key: &self.cfg.access_key_id, secret_key: &self.cfg.secret_key };
        let signed = sigv4::sign(method, &canonical, "", &headers, &payload_hash, &creds, &self.cfg.region, "s3", &now);

        let mut call = Call::new(method, url);
        for (k, v) in headers.into_iter().filter(|(k, _)| k != "host") {
            call = call.header(&k, v);
        }
        call.header("Authorization", signed.authorization).raw_body(body)
    }

    fn cache_control(&self) -> String {
        // Random names never change content, so they can be cached for a year.
        if self.cfg.random_code_len > 0 { "public, max-age=31536000, immutable".into() } else { "public, max-age=3600".into() }
    }

    fn object_key(&self, req: &UploadRequest) -> String {
        let mut cx = NameContext::now();
        cx.title = req.title;
        let prefix = naming::expand(&self.cfg.prefix, &cx);
        let name = safe_relative_path(req.file_name);
        let (stem, ext) = split_ext(&name);
        let leaf = if self.cfg.random_code_len > 0 {
            format!("{}{ext}", naming::random_code(self.cfg.random_code_len as usize))
        } else {
            format!("{stem}{ext}")
        };
        format!("{prefix}{leaf}").trim_start_matches('/').to_string()
    }

    fn public_url(&self, key: &str) -> String {
        let key_path = pct_path(key);
        if !self.cfg.public_base_url.trim().is_empty() {
            return join_url(self.cfg.public_base_url.trim(), &key_path);
        }
        let origin = format!("{}://{}", self.endpoint.scheme(), self.host_header());
        if self.path_style { join_url(&join_url(&origin, &self.cfg.bucket), &key_path) } else { join_url(&origin, &key_path) }
    }
}

impl Uploader for S3Uploader {
    fn upload(&self, req: &UploadRequest, progress: &dyn Fn(u64, u64)) -> Result<UploadResult> {
        let key = self.object_key(req);
        let total = req.data.len() as u64;
        let call = self.signed("PUT", &key, req.data, Some(mime_or_octet(req.mime))).progress(Progress::new(progress, total));
        call.send()?.check(PROVIDER)?;
        progress(total, total);
        Ok(UploadResult { url: self.public_url(&key), thumbnail_url: None, delete_url: None, provider: PROVIDER.into() })
    }

    /// Writes and removes a tiny probe object, which proves the credentials
    /// have the permissions an upload needs.
    fn test(&self) -> Result<()> {
        let key = format!("{}snapoleon-test-{}.txt", naming::expand(&self.cfg.prefix, &NameContext::now()), naming::random_code(8));
        let key = key.trim_start_matches('/');
        self.signed("PUT", key, b"snapoleon", Some("text/plain")).send()?.check(PROVIDER)?;
        // Best effort: a missing delete permission must not fail the test.
        let _ = self.signed("DELETE", key, b"", None).send();
        Ok(())
    }
}
