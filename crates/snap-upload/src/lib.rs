//! Upload destinations, link shortening and link formatting.
//!
//! Everything is blocking (`ureq` + rustls); run uploads on a worker thread.
//! Credentials never live in settings: each [`Destination`] names a
//! `secret_ref` that is resolved through the [`SecretStore`] when the
//! uploader is built.

mod boxcom;
mod dropbox;
mod format;
mod ftp;
mod gdrive;
mod http;
mod imgur;
mod local;
mod multipart;
mod onedrive;
mod s3;
mod session;
mod shorten;
mod telegram;
mod util;
mod webdav;
mod yandex;

mod custom;
pub mod oauth;
pub mod presets;
pub mod server;
pub mod sigv4;

pub use custom::sxcu;
pub use format::format_link;
pub use shorten::{shorten, snap_server_secret_key};

use snap_core::{
    Error, Result,
    secrets::SecretStore,
    settings::{Destination, DestinationKind},
    tf,
};

use std::sync::Arc;

use oauth::Provider;
use session::Session;

/// One file to upload.
pub struct UploadRequest<'a> {
    pub data: &'a [u8],
    /// Suggested remote name (may contain `/` for sub-folders on providers that support it).
    pub file_name: &'a str,
    pub mime: &'a str,
    pub title: Option<&'a str>,
    /// Honoured by the team server only.
    pub expiry_hours: Option<u32>,
    /// Honoured by the team server only.
    pub password: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UploadResult {
    /// The link to share.
    pub url: String,
    /// A direct, embeddable image URL when the provider exposes one (thumbnail
    /// or raw file); `None` otherwise.
    pub thumbnail_url: Option<String>,
    pub delete_url: Option<String>,
    pub provider: String,
}

pub trait Uploader: Send + Sync {
    /// Upload `req`. `progress` receives `(bytes_sent, total_bytes)`.
    fn upload(&self, req: &UploadRequest, progress: &dyn Fn(u64, u64)) -> Result<UploadResult>;
    /// Verify that the destination is reachable and the credentials work.
    fn test(&self) -> Result<()>;
}

/// Build the uploader for a configured destination, resolving its secret.
///
/// OAuth-backed destinations expect the secret to be [`oauth::Tokens`] JSON
/// (or a bare access token); expired tokens are refreshed and written back.
pub fn uploader_for(d: &Destination, secrets: &Arc<dyn SecretStore>) -> Result<Box<dyn Uploader>> {
    let secret = || d.secret_ref.as_deref().filter(|k| !k.is_empty()).and_then(|k| secrets.get(k));
    let required =
        || secret().filter(|s| !s.is_empty()).ok_or_else(|| Error::permission(tf("upload.error.missing_secret", &[("name", &d.name)])));
    let oauth = |provider: Provider, client_id: &str| Session::load(provider, d.secret_ref.as_deref(), secrets, client_id);

    Ok(match &d.kind {
        DestinationKind::S3 {
            endpoint,
            region,
            bucket,
            access_key_id,
            path_style,
            prefix,
            public_base_url,
            random_code_len,
            acl_public_read,
        } => Box::new(s3::S3Uploader::new(s3::S3Config {
            endpoint: endpoint.clone(),
            region: if region.is_empty() { "us-east-1".into() } else { region.clone() },
            bucket: bucket.clone(),
            access_key_id: access_key_id.clone(),
            secret_key: required()?.trim().to_string(),
            path_style: *path_style,
            prefix: prefix.clone(),
            public_base_url: public_base_url.clone(),
            random_code_len: *random_code_len,
            acl_public_read: *acl_public_read,
        })?),
        DestinationKind::Ftp { host, port, user, secure, dir, public_base_url } => Box::new(ftp::Ftp {
            host: host.trim().to_string(),
            port: if *port == 0 { 21 } else { *port },
            user: user.clone(),
            password: secret().unwrap_or_default(),
            secure: *secure,
            dir: dir.clone(),
            public_base_url: public_base_url.clone(),
        }),
        DestinationKind::WebDav { url, user, public_base_url } => Box::new(webdav::WebDav {
            url: url.clone(),
            user: user.clone(),
            password: secret().unwrap_or_default(),
            public_base_url: public_base_url.clone(),
        }),
        DestinationKind::LocalFolder { dir, public_base_url } => {
            if dir.trim().is_empty() {
                return Err(Error::invalid(tf("upload.error.local_dir", &[("name", &d.name)])));
            }
            Box::new(local::LocalFolder { dir: dir.trim().into(), public_base_url: public_base_url.clone() })
        }
        DestinationKind::Imgur { client_id } => {
            if client_id.trim().is_empty() {
                return Err(Error::invalid(tf("upload.error.imgur_client", &[("name", &d.name)])));
            }
            Box::new(imgur::Imgur { client_id: client_id.clone() })
        }
        DestinationKind::Telegram { chat_id } => {
            Box::new(telegram::Telegram { token: required()?.trim().to_string(), chat_id: chat_id.clone() })
        }
        DestinationKind::YandexDisk { folder, publish } => {
            Box::new(yandex::YandexDisk { session: oauth(Provider::YandexDisk, "")?, folder: folder.clone(), publish: *publish })
        }
        DestinationKind::Dropbox { folder } => {
            Box::new(dropbox::Dropbox { session: oauth(Provider::Dropbox, "")?, folder: folder.clone() })
        }
        DestinationKind::GoogleDrive { folder_id } => {
            Box::new(gdrive::GoogleDrive { session: oauth(Provider::GoogleDrive, "")?, folder_id: folder_id.clone() })
        }
        DestinationKind::OneDrive { folder } => {
            Box::new(onedrive::OneDrive { session: oauth(Provider::OneDrive, "")?, folder: folder.clone() })
        }
        DestinationKind::Box { folder_id, client_id } => {
            Box::new(boxcom::BoxCom { session: oauth(Provider::Box, client_id)?, folder_id: folder_id.clone() })
        }
        DestinationKind::CustomHttp(cfg) => Box::new(custom::Custom { cfg: cfg.clone(), secret: secret().unwrap_or_default() }),
        DestinationKind::SnapServer { url } => Box::new(server::SnapServer { client: server::Client::new(url, &required()?) }),
    })
}
