//! Blob storage on top of `object_store` (local filesystem or S3-compatible).
//!
//! Public objects live at predictable keys derived from the short code
//! (`raw/<code>`, `thumb/<code>.jpg`, `og/<code>.png`) so a CDN worker can serve
//! them directly; password-protected items use unguessable `private/<id>` keys
//! that the worker never maps.

use std::{ops::Range, sync::Arc};

use anyhow::{Context, Result};
use bytes::Bytes;
use object_store::{
    Attribute, AttributeValue, Attributes, GetOptions, GetRange, GetResult, MultipartUpload, ObjectStore, ObjectStoreExt,
    PutMultipartOptions, PutOptions, PutPayload, WriteMultipart, aws::AmazonS3Builder, local::LocalFileSystem, path::Path,
    prefix::PrefixStore,
};

use crate::{
    config::{Config, StorageKind},
    db::Item,
    error::AppError,
};

/// Public objects are re-validated hourly so deletions reach caches and CDNs quickly.
pub const PUBLIC_CACHE: &str = "public, max-age=3600";
pub const SHORT_CACHE: &str = "public, max-age=300";

/// `secret` is a random per-item name that appears nowhere else (not in URLs, ETags or the row id),
/// so a publicly readable bucket still cannot be used to reach protected files.
pub fn blob_key(secret: &str, code: &str, protected: bool) -> String {
    if protected { format!("private/{secret}") } else { format!("raw/{code}") }
}

pub fn thumb_key(secret: &str, code: &str, protected: bool) -> String {
    if protected { format!("private/{secret}.thumb") } else { format!("thumb/{code}.jpg") }
}

pub fn og_key(item: &Item) -> String {
    format!("og/{}.png", item.code)
}

/// Headers describing a stored object; only honoured by remote backends.
pub struct PutMeta<'a> {
    pub mime: &'a str,
    pub cache_control: &'a str,
    pub expires_ms: Option<i64>,
}

impl PutMeta<'_> {
    fn attributes(&self) -> Attributes {
        let mut a = Attributes::new();
        a.insert(Attribute::ContentType, AttributeValue::from(self.mime.to_string()));
        a.insert(Attribute::CacheControl, AttributeValue::from(self.cache_control.to_string()));
        if let Some(exp) = self.expires_ms {
            a.insert(Attribute::Metadata("expires-ms".into()), AttributeValue::from(exp.to_string()));
        }
        a
    }
}

#[derive(Clone)]
pub struct Blobs {
    store: Arc<dyn ObjectStore>,
    remote: bool,
}

pub struct StreamUpload {
    inner: WriteMultipart,
}

impl StreamUpload {
    pub async fn push(&mut self, chunk: &[u8]) -> Result<(), AppError> {
        self.inner.wait_for_capacity(4).await?;
        self.inner.write(chunk);
        Ok(())
    }

    pub async fn finish(self) -> Result<(), AppError> {
        self.inner.finish().await?;
        Ok(())
    }

    pub async fn abort(self) {
        if let Err(e) = self.inner.abort().await {
            tracing::warn!("aborting multipart upload failed: {e}");
        }
    }
}

impl Blobs {
    pub fn open(cfg: &Config) -> Result<Self> {
        match cfg.storage.kind {
            StorageKind::Fs => {
                let dir = cfg.data_dir.join("blobs");
                std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
                let store = LocalFileSystem::new_with_prefix(&dir)?.with_automatic_cleanup(true);
                Ok(Self { store: Arc::new(store), remote: false })
            }
            StorageKind::S3 => {
                let s = &cfg.storage;
                let mut b = AmazonS3Builder::new().with_bucket_name(&s.bucket).with_region(&s.region);
                if !s.endpoint.is_empty() {
                    b = b.with_endpoint(&s.endpoint).with_allow_http(s.endpoint.starts_with("http://"));
                }
                if !s.key.is_empty() {
                    b = b.with_access_key_id(&s.key).with_secret_access_key(&s.secret);
                }
                let s3 = b.build().context("configuring S3 storage")?;
                let store: Arc<dyn ObjectStore> = if s.prefix.trim_matches('/').is_empty() {
                    Arc::new(s3)
                } else {
                    Arc::new(PrefixStore::new(s3, s.prefix.trim_matches('/')))
                };
                Ok(Self { store, remote: true })
            }
        }
    }

    pub async fn put(&self, key: &str, data: Bytes, meta: PutMeta<'_>) -> Result<(), AppError> {
        let opts = if self.remote { PutOptions::from(meta.attributes()) } else { PutOptions::default() };
        self.store.put_opts(&Path::from(key), PutPayload::from(data), opts).await?;
        Ok(())
    }

    /// Start a streaming upload for objects too large to buffer.
    pub async fn begin(&self, key: &str, meta: PutMeta<'_>) -> Result<StreamUpload, AppError> {
        let opts = if self.remote { PutMultipartOptions::from(meta.attributes()) } else { PutMultipartOptions::default() };
        let upload: Box<dyn MultipartUpload> = self.store.put_multipart_opts(&Path::from(key), opts).await?;
        Ok(StreamUpload { inner: WriteMultipart::new(upload) })
    }

    pub async fn get(&self, key: &str, range: Option<Range<u64>>) -> Result<GetResult, AppError> {
        let options = GetOptions { range: range.map(GetRange::Bounded), ..Default::default() };
        match self.store.get_opts(&Path::from(key), options).await {
            Ok(r) => Ok(r),
            Err(object_store::Error::NotFound { .. }) => Err(AppError::NotFound),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn read(&self, key: &str) -> Result<Bytes, AppError> {
        Ok(self.get(key, None).await?.bytes().await?)
    }

    pub async fn size(&self, key: &str) -> Result<u64, AppError> {
        match self.store.head(&Path::from(key)).await {
            Ok(m) => Ok(m.size),
            Err(object_store::Error::NotFound { .. }) => Err(AppError::NotFound),
            Err(e) => Err(e.into()),
        }
    }

    /// Move a finished upload into place. A remote rename keeps the old object's attributes,
    /// so objects that need new ones (expiry) are re-written under `meta` instead.
    pub async fn promote(&self, from: &str, to: &str, meta: PutMeta<'_>) -> Result<(), AppError> {
        if !(self.remote && meta.expires_ms.is_some()) {
            self.store.rename(&Path::from(from), &Path::from(to)).await?;
            return Ok(());
        }
        const CHUNK: u64 = 8 * 1024 * 1024;
        let len = self.size(from).await?;
        let mut upload = self.begin(to, meta).await?;
        let mut offset = 0;
        while offset < len {
            let end = (offset + CHUNK).min(len);
            let pushed = match self.get(from, Some(offset..end)).await {
                Ok(part) => match part.bytes().await {
                    Ok(bytes) => upload.push(&bytes).await,
                    Err(e) => Err(e.into()),
                },
                Err(e) => Err(e),
            };
            if let Err(e) = pushed {
                upload.abort().await;
                return Err(e);
            }
            offset = end;
        }
        upload.finish().await
    }

    pub async fn exists(&self, key: &str) -> bool {
        self.store.head(&Path::from(key)).await.is_ok()
    }

    /// Delete an object; a missing object is not an error.
    async fn try_delete(&self, key: &str) -> Result<(), AppError> {
        match self.store.delete(&Path::from(key)).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Best-effort delete of scratch or rolled-back objects.
    pub async fn delete(&self, key: &str) {
        if let Err(e) = self.try_delete(key).await {
            tracing::warn!("deleting {key} failed: {}", e.message());
        }
    }

    /// Delete every object of an item; fails (after trying them all) so the caller can keep the row and retry.
    pub async fn delete_item(&self, item: &Item) -> Result<(), AppError> {
        let mut first = Ok(());
        for key in item.blob_keys() {
            if let Err(e) = self.try_delete(&key).await {
                tracing::warn!("deleting {key} failed: {}", e.message());
                first = first.and(Err(e));
            }
        }
        first
    }
}
