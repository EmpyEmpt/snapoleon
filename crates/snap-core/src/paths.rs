//! Filesystem locations. Portable mode: if a file named `portable` (or a
//! `data` dir) sits next to the executable, everything lives beside it.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

struct Dirs {
    config: PathBuf,
    data: PathBuf,
    cache: PathBuf,
    pictures: PathBuf,
    videos: PathBuf,
    portable: bool,
}

fn dirs() -> &'static Dirs {
    static DIRS: OnceLock<Dirs> = OnceLock::new();
    DIRS.get_or_init(|| {
        if let Some(root) = portable_root() {
            let data = root.join("data");
            return Dirs {
                config: data.clone(),
                cache: data.join("cache"),
                pictures: data.join("captures"),
                videos: data.join("captures"),
                data,
                portable: true,
            };
        }
        let proj = directories::ProjectDirs::from("app", "Snapoleon", "Snapoleon");
        let user = directories::UserDirs::new();
        let home = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
        let pictures = user.as_ref().and_then(|u| u.picture_dir().map(Path::to_path_buf)).unwrap_or_else(|| home.join("Pictures"));
        let videos = user.as_ref().and_then(|u| u.video_dir().map(Path::to_path_buf)).unwrap_or_else(|| home.join("Videos"));
        match proj {
            Some(p) => Dirs {
                config: p.config_dir().to_path_buf(),
                data: p.data_dir().to_path_buf(),
                cache: p.cache_dir().to_path_buf(),
                pictures: pictures.join("Snapoleon"),
                videos: videos.join("Snapoleon"),
                portable: false,
            },
            None => {
                let base = home.join(".snapoleon");
                Dirs {
                    config: base.clone(),
                    data: base.clone(),
                    cache: base.join("cache"),
                    pictures: pictures.join("Snapoleon"),
                    videos: videos.join("Snapoleon"),
                    portable: false,
                }
            }
        }
    })
}

fn portable_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    (dir.join("portable").exists() || dir.join("data").is_dir()).then(|| dir.to_path_buf())
}

pub fn is_portable() -> bool {
    dirs().portable
}
pub fn config_dir() -> PathBuf {
    dirs().config.clone()
}
pub fn data_dir() -> PathBuf {
    dirs().data.clone()
}
pub fn cache_dir() -> PathBuf {
    dirs().cache.clone()
}
pub fn settings_file() -> PathBuf {
    config_dir().join("settings.toml")
}
pub fn history_db() -> PathBuf {
    data_dir().join("history.sqlite")
}
pub fn thumbnails_dir() -> PathBuf {
    cache_dir().join("thumbs")
}
/// Downloaded models (OCR fallback etc.).
pub fn models_dir() -> PathBuf {
    data_dir().join("models")
}
/// Community translations that override/extend embedded ones.
pub fn user_locales_dir() -> PathBuf {
    data_dir().join("locales")
}
pub fn default_save_dir() -> PathBuf {
    dirs().pictures.clone()
}
pub fn default_video_dir() -> PathBuf {
    dirs().videos.clone()
}
pub fn logs_dir() -> PathBuf {
    data_dir().join("logs")
}

/// Resolve the configured save dir (empty = default).
pub fn save_dir(configured: &str) -> PathBuf {
    if configured.trim().is_empty() {
        default_save_dir()
    } else {
        PathBuf::from(shellexpand_home(configured))
    }
}

fn shellexpand_home(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\"))
        && let Some(b) = directories::BaseDirs::new()
    {
        return b.home_dir().join(rest).display().to_string();
    }
    p.to_string()
}
