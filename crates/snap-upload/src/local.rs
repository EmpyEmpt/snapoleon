//! Copy into a (possibly synced) folder and derive a URL from a base.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use snap_core::{Error, Result, tf};

use crate::{
    UploadRequest, UploadResult, Uploader,
    util::{join_url, pct_path, safe_relative_path, split_ext},
};

pub(crate) struct LocalFolder {
    pub dir: PathBuf,
    pub public_base_url: String,
}

impl LocalFolder {
    /// First free path for `rel` inside the folder: `name.png`, `name (2).png`, ...
    fn free_path(&self, rel: &str) -> (PathBuf, String) {
        let full = |r: &str| r.split('/').fold(self.dir.clone(), |p, seg| p.join(seg));
        if !full(rel).exists() {
            return (full(rel), rel.to_string());
        }
        let (dir_part, leaf) = rel.rsplit_once('/').map_or(("", rel), |(d, l)| (d, l));
        let (stem, ext) = split_ext(leaf);
        for n in 2..10_000 {
            let candidate = if dir_part.is_empty() { format!("{stem} ({n}){ext}") } else { format!("{dir_part}/{stem} ({n}){ext}") };
            if !full(&candidate).exists() {
                return (full(&candidate), candidate);
            }
        }
        (full(rel), rel.to_string())
    }
}

impl Uploader for LocalFolder {
    fn upload(&self, req: &UploadRequest, progress: &dyn Fn(u64, u64)) -> Result<UploadResult> {
        let (path, rel) = self.free_path(&safe_relative_path(req.file_name));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let total = req.data.len() as u64;
        let mut file = fs::File::create_new(&path)?;
        let mut written = 0u64;
        for chunk in req.data.chunks(256 * 1024) {
            file.write_all(chunk)?;
            written += chunk.len() as u64;
            progress(written, total);
        }
        file.sync_all()?;

        let url =
            if self.public_base_url.trim().is_empty() { file_url(&path)? } else { join_url(self.public_base_url.trim(), &pct_path(&rel)) };
        Ok(UploadResult { url, thumbnail_url: None, delete_url: None, provider: "Local folder".into() })
    }

    fn test(&self) -> Result<()> {
        fs::create_dir_all(&self.dir)?;
        let probe = self.dir.join(format!(".snapoleon-test-{}", snap_core::naming::random_code(6)));
        fs::write(&probe, b"snapoleon")?;
        fs::remove_file(&probe)?;
        Ok(())
    }
}

fn file_url(path: &Path) -> Result<String> {
    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    url::Url::from_file_path(&abs)
        .map(|u| u.to_string())
        .map_err(|_| Error::invalid(tf("upload.error.bad_url", &[("url", &abs.display().to_string())])))
}
