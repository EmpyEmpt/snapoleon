//! Logging: stderr, plus a size-capped file in `paths::logs_dir()` for the running app.

use std::{fs::File, io::Write, path::PathBuf, sync::Mutex};

use snap_core::paths;
use tracing_subscriber::{EnvFilter, Layer, fmt, layer::SubscriberExt, util::SubscriberInitExt};

const MAX_LOG_BYTES: u64 = 4 * 1024 * 1024;
const APP_FILTER: &str = "info,wgpu_core=warn,wgpu_hal=warn,naga=warn,egui_wgpu=warn,glutin=warn";
const CONSOLE_FILTER: &str = "warn";

/// Where a process's log goes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Sink {
    /// The running tray app: stderr and the log file.
    Application,
    /// A command line invocation (a headless command, or a launch that hands over to the running app): warnings on
    /// stderr only. Nothing is written to standard output and no log file is touched.
    Console,
}

pub fn init(sink: Sink) {
    let default = if sink == Sink::Application { APP_FILTER } else { CONSOLE_FILTER };
    let filter = || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    let stderr = fmt::layer().with_writer(std::io::stderr).with_target(false).with_filter(filter());
    let file = (sink == Sink::Application)
        .then(open_log_file)
        .flatten()
        .map(|file| fmt::layer().with_writer(Mutex::new(file)).with_ansi(false).with_filter(filter()));
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
