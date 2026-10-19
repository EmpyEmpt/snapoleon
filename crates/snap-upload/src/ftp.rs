//! FTP and explicit FTPS (`AUTH TLS`, rustls) uploads.

use std::{
    io::Write,
    net::{TcpStream, ToSocketAddrs},
    sync::Arc,
    time::Duration,
};

use snap_core::{Error, Result, tf};
use suppaftp::{
    FtpError, FtpStream, ImplFtpStream, RustlsConnector, RustlsFtpStream, TlsStream,
    rustls::{ClientConfig, RootCertStore, crypto::ring},
    types::FileType,
};

use crate::{
    Uploader, UploadRequest, UploadResult,
    util::{join_url, pct_path, safe_relative_path},
};

const PROVIDER: &str = "FTP";
const CHUNK: usize = 64 * 1024;

pub(crate) struct Ftp {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub secure: bool,
    /// Remote folder (created on demand). Not template-expanded: `public_base_url`
    /// is expected to be the public address of this very folder.
    pub dir: String,
    pub public_base_url: String,
}

fn ftp_err(host: &str, e: FtpError) -> Error {
    let detail = match &e {
        FtpError::UnexpectedResponse(r) => String::from_utf8_lossy(&r.body).trim().to_string(),
        other => other.to_string(),
    };
    Error::Network(tf("upload.error.request_failed", &[("provider", &format!("{PROVIDER} {host}")), ("detail", &detail)]))
}

impl Ftp {
    fn tcp(&self) -> Result<TcpStream> {
        let addrs = (self.host.as_str(), self.port).to_socket_addrs().map_err(|e| Error::Network(format!("{}: {e}", self.host)))?;
        let mut last = None;
        for addr in addrs {
            match TcpStream::connect_timeout(&addr, Duration::from_secs(15)) {
                Ok(s) => {
                    s.set_read_timeout(Some(Duration::from_secs(60)))?;
                    s.set_write_timeout(Some(Duration::from_secs(60)))?;
                    s.set_nodelay(true)?;
                    return Ok(s);
                }
                Err(e) => last = Some(e),
            }
        }
        Err(Error::Network(format!("{}: {}", self.host, last.map(|e| e.to_string()).unwrap_or_default())))
    }

    fn tls_connector() -> Result<RustlsConnector> {
        let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let config = ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| Error::Network(e.to_string()))?
            .with_root_certificates(roots)
            .with_no_client_auth();
        Ok(RustlsConnector::from(Arc::new(config)))
    }

    fn with_session<R>(&self, f: impl FnOnce(&mut dyn Session) -> Result<R>) -> Result<R> {
        let tcp = self.tcp()?;
        let host = self.host.as_str();
        if self.secure {
            let mut ftp = RustlsFtpStream::connect_with_stream(tcp)
                .and_then(|s| s.into_secure(Self::tls_connector().map_err(|e| FtpError::SecureError(e.to_string()))?, host))
                .map_err(|e| ftp_err(host, e))?;
            self.login(&mut ftp)?;
            let out = f(&mut ftp);
            let _ = ftp.quit();
            out
        } else {
            let mut ftp = FtpStream::connect_with_stream(tcp).map_err(|e| ftp_err(host, e))?;
            self.login(&mut ftp)?;
            let out = f(&mut ftp);
            let _ = ftp.quit();
            out
        }
    }

    fn login<T: TlsStream>(&self, ftp: &mut ImplFtpStream<T>) -> Result<()> {
        let (user, pass) = if self.user.is_empty() { ("anonymous", "anonymous@snapoleon") } else { (self.user.as_str(), self.password.as_str()) };
        ftp.login(user, pass).map_err(|e| ftp_err(&self.host, e))?;
        ftp.transfer_type(FileType::Binary).map_err(|e| ftp_err(&self.host, e))
    }

    /// `cwd` into every segment, creating the ones that do not exist yet.
    fn enter_dir(&self, s: &mut dyn Session, path: &str) -> Result<()> {
        if path.starts_with('/') {
            s.cwd("/").map_err(|e| ftp_err(&self.host, e))?;
        }
        for seg in path.split('/').filter(|s| !s.is_empty() && *s != ".") {
            if s.cwd(seg).is_err() {
                s.mkdir(seg).map_err(|e| ftp_err(&self.host, e))?;
                s.cwd(seg).map_err(|e| ftp_err(&self.host, e))?;
            }
        }
        Ok(())
    }
}

/// The handful of FTP operations we need, object-safe across TLS flavours.
trait Session {
    fn cwd(&mut self, path: &str) -> std::result::Result<(), FtpError>;
    fn mkdir(&mut self, path: &str) -> std::result::Result<(), FtpError>;
    fn store(&mut self, name: &str, data: &[u8], progress: &dyn Fn(u64, u64)) -> std::result::Result<(), FtpError>;
}

impl<T: TlsStream> Session for ImplFtpStream<T> {
    fn cwd(&mut self, path: &str) -> std::result::Result<(), FtpError> {
        ImplFtpStream::cwd(self, path)
    }
    fn mkdir(&mut self, path: &str) -> std::result::Result<(), FtpError> {
        ImplFtpStream::mkdir(self, path)
    }
    fn store(&mut self, name: &str, data: &[u8], progress: &dyn Fn(u64, u64)) -> std::result::Result<(), FtpError> {
        let mut stream = self.put_with_stream(name)?;
        let total = data.len() as u64;
        let mut sent = 0u64;
        for chunk in data.chunks(CHUNK) {
            stream.write_all(chunk).map_err(FtpError::ConnectionError)?;
            sent += chunk.len() as u64;
            progress(sent, total);
        }
        stream.finish()
    }
}

impl Uploader for Ftp {
    fn upload(&self, req: &UploadRequest, progress: &dyn Fn(u64, u64)) -> Result<UploadResult> {
        let rel = safe_relative_path(req.file_name);
        self.with_session(|s| {
            self.enter_dir(s, &self.dir)?;
            let (sub, leaf) = rel.rsplit_once('/').unwrap_or(("", &rel));
            self.enter_dir(s, sub)?;
            s.store(leaf, req.data, progress).map_err(|e| ftp_err(&self.host, e))
        })?;

        let url = if self.public_base_url.trim().is_empty() {
            let port = if self.port == 21 { String::new() } else { format!(":{}", self.port) };
            let dir = self.dir.trim_matches('/');
            let path = if dir.is_empty() { rel.clone() } else { format!("{dir}/{rel}") };
            format!("ftp://{}{port}/{}", self.host, pct_path(&path))
        } else {
            join_url(self.public_base_url.trim(), &pct_path(&rel))
        };
        Ok(UploadResult { url, thumbnail_url: None, delete_url: None, provider: if self.secure { "FTPS" } else { PROVIDER }.into() })
    }

    fn test(&self) -> Result<()> {
        self.with_session(|s| self.enter_dir(s, &self.dir))
    }
}
