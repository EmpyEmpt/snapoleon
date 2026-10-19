//! Logging: stderr plus a size-capped file in `paths::logs_dir()`.

use std::{fs::File, io::Write, path::PathBuf, sync::Mutex};

use snap_core::paths;
use tracing_subscriber::{EnvFilter, Layer, fmt, layer::SubscriberExt, util::SubscriberInitExt};

const MAX_LOG_BYTES: u64 = 4 * 1024 * 1024;
const DEFAULT_FILTER: &str = "info,wgpu_core=warn,wgpu_hal=warn,naga=warn,egui_wgpu=warn";

pub fn init() {
    let filter = || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    let stderr = fmt::layer().with_writer(std::io::stderr).with_target(false).with_filter(filter());
    let file = open_log_file().map(|file| fmt::layer().with_writer(Mutex::new(file)).with_ansi(false).with_filter(filter()));
    tracing_subscriber::registry().with(stderr).with(file).init();
    std::panic::set_hook(Box::new(|info| tracing::error!("panic: {info}")));
}

fn log_path() -> PathBuf {
    paths::logs_dir().join("snapoleon.log")
}

/// Opens the log for appending, moving a log that grew too large aside first.
fn open_log_file() -> Option<File> {
    let path = log_path();
    std::fs::create_dir_all(path.parent()?).ok()?;
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        let _ = std::fs::rename(&path, path.with_extension("old.log"));
    }
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).ok()?;
    let _ = writeln!(file, "--- snapoleon {} started ---", snap_core::VERSION);
    Some(file)
}
