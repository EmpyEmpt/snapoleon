//! `snapoleon-server.toml` plus `SNAPOLEON_*` environment overrides.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Registration {
    Open,
    #[default]
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageKind {
    #[default]
    Fs,
    S3,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    pub kind: StorageKind,
    /// S3-compatible endpoint (R2, MinIO, Yandex, ...). Empty = AWS.
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub key: String,
    pub secret: String,
    /// Optional key prefix inside the bucket.
    pub prefix: String,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            kind: StorageKind::Fs,
            endpoint: String::new(),
            bucket: String::new(),
            region: "auto".into(),
            key: String::new(),
            secret: String::new(),
            prefix: String::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct OidcConfig {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    /// Members of this group (per `groups_claim`) become server admins.
    pub admin_group: String,
    pub groups_claim: String,
    pub scopes: String,
    /// Create a local account on first SSO login.
    pub auto_provision: bool,
}

impl Default for OidcConfig {
    fn default() -> Self {
        Self {
            issuer: String::new(),
            client_id: String::new(),
            client_secret: String::new(),
            admin_group: String::new(),
            groups_claim: "groups".into(),
            scopes: "openid profile email".into(),
            auto_provision: true,
        }
    }
}

impl OidcConfig {
    pub fn enabled(&self) -> bool {
        !self.issuer.is_empty() && !self.client_id.is_empty()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub bind: String,
    /// Externally visible base URL used in links and OG tags.
    pub public_url: String,
    pub data_dir: PathBuf,
    pub max_upload_mb: u64,
    pub registration: Registration,
    /// UI language for pages the server renders (empty = `en`).
    pub language: String,
    /// Trust `CF-Connecting-IP` / `X-Real-IP` / `X-Forwarded-For` from a reverse proxy.
    pub trust_proxy: bool,
    pub storage: StorageConfig,
    pub oidc: OidcConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8787".into(),
            public_url: String::new(),
            data_dir: PathBuf::from("data"),
            max_upload_mb: 100,
            registration: Registration::Closed,
            language: "en".into(),
            trust_proxy: false,
            storage: StorageConfig::default(),
            oidc: OidcConfig::default(),
        }
    }
}

impl Config {
    /// Load from `path` (or `SNAPOLEON_CONFIG`, or `./snapoleon-server.toml` when present), then apply env overrides.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let path = path
            .map(Path::to_path_buf)
            .or_else(|| std::env::var_os("SNAPOLEON_CONFIG").map(PathBuf::from))
            .or_else(|| Some(PathBuf::from("snapoleon-server.toml")).filter(|p| p.exists()));
        let mut cfg = match &path {
            Some(p) => {
                let text = std::fs::read_to_string(p)
                    .with_context(|| format!("reading {}", p.display()))?;
                toml::from_str(&text).with_context(|| format!("parsing {}", p.display()))?
            }
            None => Config::default(),
        };
        cfg.apply_env()?;
        cfg.finish()?;
        Ok(cfg)
    }

    fn apply_env(&mut self) -> Result<()> {
        fn var(name: &str) -> Option<String> {
            std::env::var(name).ok().filter(|v| !v.is_empty())
        }
        fn set(target: &mut String, name: &str) {
            if let Some(v) = var(name) {
                *target = v;
            }
        }
        fn flag(v: &str) -> bool {
            matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
        }

        set(&mut self.bind, "SNAPOLEON_BIND");
        set(&mut self.public_url, "SNAPOLEON_PUBLIC_URL");
        set(&mut self.language, "SNAPOLEON_LANGUAGE");
        if let Some(v) = var("SNAPOLEON_DATA_DIR") {
            self.data_dir = PathBuf::from(v);
        }
        if let Some(v) = var("SNAPOLEON_MAX_UPLOAD_MB") {
            self.max_upload_mb = v.parse().context("SNAPOLEON_MAX_UPLOAD_MB")?;
        }
        if let Some(v) = var("SNAPOLEON_REGISTRATION") {
            self.registration = match v.to_ascii_lowercase().as_str() {
                "open" => Registration::Open,
                "closed" => Registration::Closed,
                other => bail!("SNAPOLEON_REGISTRATION must be open or closed, got {other}"),
            };
        }
        if let Some(v) = var("SNAPOLEON_TRUST_PROXY") {
            self.trust_proxy = flag(&v);
        }
        if let Some(v) = var("SNAPOLEON_STORAGE") {
            self.storage.kind = match v.to_ascii_lowercase().as_str() {
                "fs" => StorageKind::Fs,
                "s3" => StorageKind::S3,
                other => bail!("SNAPOLEON_STORAGE must be fs or s3, got {other}"),
            };
        }
        set(&mut self.storage.endpoint, "SNAPOLEON_S3_ENDPOINT");
        set(&mut self.storage.bucket, "SNAPOLEON_S3_BUCKET");
        set(&mut self.storage.region, "SNAPOLEON_S3_REGION");
        set(&mut self.storage.key, "SNAPOLEON_S3_KEY");
        set(&mut self.storage.secret, "SNAPOLEON_S3_SECRET");
        set(&mut self.storage.prefix, "SNAPOLEON_S3_PREFIX");
        set(&mut self.oidc.issuer, "SNAPOLEON_OIDC_ISSUER");
        set(&mut self.oidc.client_id, "SNAPOLEON_OIDC_CLIENT_ID");
        set(&mut self.oidc.client_secret, "SNAPOLEON_OIDC_CLIENT_SECRET");
        set(&mut self.oidc.admin_group, "SNAPOLEON_OIDC_ADMIN_GROUP");
        set(&mut self.oidc.groups_claim, "SNAPOLEON_OIDC_GROUPS_CLAIM");
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        if self.public_url.trim().is_empty() {
            self.public_url = format!("http://{}", self.bind);
        }
        self.public_url = self.public_url.trim().trim_end_matches('/').to_string();
        if !self.public_url.starts_with("http://") && !self.public_url.starts_with("https://") {
            bail!("public_url must start with http:// or https://");
        }
        if self.storage.kind == StorageKind::S3 && self.storage.bucket.is_empty() {
            bail!("storage.bucket is required for S3 storage");
        }
        if self.oidc.issuer.ends_with('/') {
            self.oidc.issuer.pop();
        }
        self.max_upload_mb = self.max_upload_mb.clamp(1, 20_480);
        Ok(())
    }

    pub fn max_upload_bytes(&self) -> u64 {
        self.max_upload_mb * 1024 * 1024
    }

    pub fn secure_cookies(&self) -> bool {
        self.public_url.starts_with("https://")
    }

    /// `host[:port]` of the public URL.
    pub fn public_host(&self) -> &str {
        self.public_url
            .split_once("://")
            .map_or("", |(_, rest)| rest.split('/').next().unwrap_or(""))
    }
}
