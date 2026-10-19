//! Open files, URLs and reveal files in the system file manager.

use std::path::Path;

use snap_core::{Error, Result};

/// Open a file or folder with its default application.
pub fn path(p: &Path) -> Result<()> {
    if !p.exists() {
        return Err(Error::NotFound(p.display().to_string()));
    }
    imp::open_path(&std::path::absolute(p)?)
}

/// Open a web (`http`, `https`, `ftp`) or `mailto:` link in the default handler.
/// Other schemes are refused: this is called with strings that can originate from
/// captured text and server responses.
pub fn url(u: &str) -> Result<()> {
    let u = u.trim();
    let allowed = ["http://", "https://", "ftp://", "mailto:"];
    let has_allowed_scheme = allowed.iter().any(|s| u.len() > s.len() && u.get(..s.len()).is_some_and(|head| head.eq_ignore_ascii_case(s)));
    if !has_allowed_scheme || u.chars().any(char::is_control) {
        return Err(Error::invalid(format!("refusing to open {u:?}")));
    }
    imp::open_url(u)
}

/// Show a file selected in its folder (Explorer, Finder, or the default file manager).
pub fn reveal(p: &Path) -> Result<()> {
    if !p.exists() {
        return Err(Error::NotFound(p.display().to_string()));
    }
    imp::reveal(&std::path::absolute(p)?)
}

#[cfg(windows)]
mod imp {
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use std::process::Command;

    use snap_core::{Error, Result};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{HSTRING, w};

    fn shell_open(target: &str) -> Result<()> {
        // SAFETY: all arguments are valid, NUL-terminated wide strings or null.
        let result = unsafe { ShellExecuteW(None, w!("open"), &HSTRING::from(target), None, None, SW_SHOWNORMAL) };
        // Values greater than 32 mean success.
        if result.0 as isize > 32 { Ok(()) } else { Err(Error::Io(std::io::Error::last_os_error())) }
    }

    pub fn open_path(p: &Path) -> Result<()> {
        shell_open(&p.to_string_lossy())
    }

    pub fn open_url(u: &str) -> Result<()> {
        shell_open(u)
    }

    pub fn reveal(p: &Path) -> Result<()> {
        // `explorer /select,"<path>"` needs the quoting passed through verbatim.
        let arg = format!("/select,\"{}\"", p.display());
        Command::new("explorer.exe").raw_arg(arg).spawn()?;
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::path::Path;
    use std::process::Command;

    use snap_core::Result;

    use crate::util::spawn_detached;

    pub fn open_path(p: &Path) -> Result<()> {
        Ok(spawn_detached(Command::new("open").arg(p))?)
    }

    pub fn open_url(u: &str) -> Result<()> {
        Ok(spawn_detached(Command::new("open").arg(u))?)
    }

    pub fn reveal(p: &Path) -> Result<()> {
        Ok(spawn_detached(Command::new("open").arg("-R").arg(p))?)
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::path::Path;
    use std::process::Command;

    use snap_core::Result;

    use crate::util::{file_uri, spawn_detached};

    pub fn open_path(p: &Path) -> Result<()> {
        Ok(spawn_detached(Command::new("xdg-open").arg(p))?)
    }

    pub fn open_url(u: &str) -> Result<()> {
        Ok(spawn_detached(Command::new("xdg-open").arg(u))?)
    }

    /// Ask the file manager to select the file (`FileManager1.ShowItems`);
    /// fall back to opening the containing folder.
    pub fn reveal(p: &Path) -> Result<()> {
        if show_items(p).is_ok() {
            return Ok(());
        }
        let dir = if p.is_dir() { p } else { p.parent().unwrap_or(p) };
        open_path(dir)
    }

    fn show_items(p: &Path) -> zbus::Result<()> {
        let conn = zbus::blocking::Connection::session()?;
        conn.call_method(
            Some("org.freedesktop.FileManager1"),
            "/org/freedesktop/FileManager1",
            Some("org.freedesktop.FileManager1"),
            "ShowItems",
            &(vec![file_uri(p)], ""),
        )?;
        Ok(())
    }
}
