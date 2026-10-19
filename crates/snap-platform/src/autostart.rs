//! Start at login. The app is launched with `--background` so it goes straight
//! to the tray.
//!
//! - Windows: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
//! - macOS: `~/Library/LaunchAgents/app.snapoleon.plist`
//! - Linux: `~/.config/autostart/snapoleon.desktop`

use snap_core::Result;

pub const BACKGROUND_ARG: &str = "--background";

pub fn set(enabled: bool) -> Result<()> {
    imp::set(enabled)
}

/// Whether the app is currently set to start at login.
pub fn get() -> bool {
    imp::get()
}

#[cfg(windows)]
mod imp {
    use snap_core::{APP_NAME, Result};
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};

    use super::BACKGROUND_ARG;
    use crate::util::exe_path;

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    /// Task Manager's "Startup apps" toggle lives here: a leading odd byte means disabled.
    const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

    pub fn set(enabled: bool) -> Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if enabled {
            let (run, _) = hkcu.create_subkey(RUN)?;
            run.set_value(APP_NAME, &format!("\"{}\" {BACKGROUND_ARG}", exe_path()?.display()))?;
            // Re-enable the entry if the user had switched it off in Task Manager.
            if let Ok(approved) = hkcu.open_subkey_with_flags(APPROVED, KEY_SET_VALUE) {
                let _ = approved.delete_value(APP_NAME);
            }
        } else if let Ok(run) = hkcu.open_subkey_with_flags(RUN, KEY_SET_VALUE) {
            match run.delete_value(APP_NAME) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    pub fn get() -> bool {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let present = hkcu.open_subkey_with_flags(RUN, KEY_READ).and_then(|k| k.get_value::<String, _>(APP_NAME)).is_ok();
        if !present {
            return false;
        }
        let disabled = hkcu
            .open_subkey_with_flags(APPROVED, KEY_READ)
            .and_then(|k| k.get_raw_value(APP_NAME))
            .is_ok_and(|v| v.bytes.first().is_some_and(|b| b & 1 == 1));
        !disabled
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::path::PathBuf;

    use snap_core::{APP_ID, Error, Result};

    use super::BACKGROUND_ARG;
    use crate::util::exe_path;

    fn plist_path() -> Result<PathBuf> {
        let home = directories::BaseDirs::new().ok_or_else(|| Error::NotFound("home directory".into()))?;
        Ok(home.home_dir().join("Library/LaunchAgents").join(format!("{APP_ID}.plist")))
    }

    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    }

    pub fn set(enabled: bool) -> Result<()> {
        let path = plist_path()?;
        if !enabled {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
                _ => Ok(()),
            };
        }
        let exe = exe_path()?;
        let plist = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n<dict>\n\
             \t<key>Label</key>\n\t<string>{APP_ID}</string>\n\
             \t<key>ProgramArguments</key>\n\t<array>\n\t\t<string>{}</string>\n\t\t<string>{BACKGROUND_ARG}</string>\n\t</array>\n\
             \t<key>RunAtLoad</key>\n\t<true/>\n\
             \t<key>ProcessType</key>\n\t<string>Interactive</string>\n\
             </dict>\n</plist>\n",
            xml_escape(&exe.to_string_lossy())
        );
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, plist)?;
        Ok(())
    }

    pub fn get() -> bool {
        plist_path().is_ok_and(|p| p.is_file())
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::path::PathBuf;

    use snap_core::{APP_NAME, Error, Result};

    use super::BACKGROUND_ARG;
    use crate::util::{desktop_quote, exe_path};

    fn entry_path() -> Result<PathBuf> {
        let dirs = directories::BaseDirs::new().ok_or_else(|| Error::NotFound("home directory".into()))?;
        Ok(dirs.config_dir().join("autostart").join("snapoleon.desktop"))
    }

    pub fn set(enabled: bool) -> Result<()> {
        let path = entry_path()?;
        if !enabled {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
                _ => Ok(()),
            };
        }
        let exe = exe_path()?;
        let entry = format!(
            "[Desktop Entry]\nType=Application\nName={APP_NAME}\nExec={} {BACKGROUND_ARG}\n\
             Icon=snapoleon\nTerminal=false\nX-GNOME-Autostart-enabled=true\nStartupNotify=false\n",
            desktop_quote(&exe.to_string_lossy())
        );
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, entry)?;
        Ok(())
    }

    pub fn get() -> bool {
        let Ok(text) = entry_path().and_then(|p| std::fs::read_to_string(p).map_err(Error::from)) else {
            return false;
        };
        !text.lines().any(|l| {
            let l = l.trim();
            l.eq_ignore_ascii_case("Hidden=true") || l.eq_ignore_ascii_case("X-GNOME-Autostart-enabled=false")
        })
    }
}
