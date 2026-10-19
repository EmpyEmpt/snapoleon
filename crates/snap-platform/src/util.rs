//! Small helpers shared by the platform modules.

use std::fmt::Display;
#[cfg(target_os = "linux")]
use std::path::Path;
use std::path::PathBuf;
#[cfg(unix)]
use std::process::{Command, Stdio};

use snap_core::Error;

pub(crate) fn other(msg: impl Display) -> Error {
    Error::Other(anyhow::anyhow!("{msg}"))
}

pub(crate) fn exe_path() -> snap_core::Result<PathBuf> {
    Ok(std::env::current_exe()?)
}

/// Spawn a helper process without waiting for it. A reaper thread collects the
/// exit status so no zombie is left behind.
#[cfg(unix)]
pub(crate) fn spawn_detached(cmd: &mut Command) -> std::io::Result<()> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Run a helper to completion and report whether it exited successfully.
#[cfg(target_os = "linux")]
pub(crate) fn run_quiet(cmd: &mut Command) -> bool {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

/// `file://` URI for an absolute path (RFC 3986 percent-encoding).
#[cfg(target_os = "linux")]
pub(crate) fn file_uri(path: &Path) -> String {
    let s = path.to_string_lossy();
    let mut out = String::from("file://");
    if !s.starts_with('/') {
        out.push('/');
    }
    for b in s.replace('\\', "/").bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Quote a path/argument for the `Exec=` key of a desktop entry.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn desktop_quote(arg: &str) -> String {
    let mut out = String::from("\"");
    for c in arg.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out.replace('%', "%%")
}
