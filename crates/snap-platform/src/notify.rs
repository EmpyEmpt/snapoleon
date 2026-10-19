//! Desktop notifications.
//!
//! - Windows: WinRT toast. Unpackaged executables need an AppUserModelID; it is
//!   registered per user under `HKCU\Software\Classes\AppUserModelId\app.snapoleon`.
//! - macOS: `osascript display notification` (works without an app bundle; the
//!   banner is attributed to Script Editor and cannot show an image).
//! - Linux: freedesktop notifications through `notify-rust`.

use std::path::Path;

use image::RgbaImage;
use snap_core::Result;

/// Show a notification. `image` is a preview picture where the platform supports one.
/// Returns once the notification has been handed to the OS.
pub fn show(title: &str, body: &str, image: Option<&Path>) -> Result<()> {
    imp::show(title, body, image)
}

/// Use `icon` as the application icon in toasts (Windows only; no-op elsewhere,
/// where the icon comes from the installed desktop entry / app bundle).
pub fn set_app_icon(icon: &RgbaImage) -> Result<()> {
    imp::set_app_icon(icon)
}

#[cfg(windows)]
mod imp {
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    use image::RgbaImage;
    use snap_core::{APP_ID, APP_NAME, Result, paths};
    use tauri_winrt_notification::Toast;
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    use crate::util::other;

    fn aumid_key() -> String {
        format!(r"Software\Classes\AppUserModelId\{APP_ID}")
    }

    /// Make sure the AUMID exists so toasts from this unpackaged exe are accepted
    /// and show the right name.
    fn register_aumid() -> Result<()> {
        static DONE: OnceLock<()> = OnceLock::new();
        if DONE.get().is_some() {
            return Ok(());
        }
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(aumid_key())?;
        key.set_value("DisplayName", &APP_NAME)?;
        let _ = DONE.set(());
        Ok(())
    }

    pub fn set_app_icon(icon: &RgbaImage) -> Result<()> {
        register_aumid()?;
        let dir = paths::cache_dir();
        std::fs::create_dir_all(&dir)?;
        let png: PathBuf = dir.join("toast-icon.png");
        icon.save_with_format(&png, image::ImageFormat::Png)?;
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(aumid_key())?;
        key.set_value("IconUri", &png.to_string_lossy().as_ref())?;
        key.set_value("IconBackgroundColor", &"00000000")?;
        Ok(())
    }

    pub fn show(title: &str, body: &str, image: Option<&Path>) -> Result<()> {
        register_aumid()?;
        let mut toast = Toast::new(APP_ID).title(title).text1(body);
        let picture = image.filter(|p| p.is_file()).and_then(|p| std::path::absolute(p).ok());
        if let Some(p) = &picture {
            // The toast XML wants a plain absolute path without the `\\?\` prefix.
            let plain = p.to_string_lossy().trim_start_matches(r"\\?\").to_string();
            toast = toast.image(Path::new(&plain), "");
        }
        toast.show().map_err(other)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::path::Path;
    use std::process::Command;

    use image::RgbaImage;
    use snap_core::Result;

    pub fn set_app_icon(_icon: &RgbaImage) -> Result<()> {
        Ok(())
    }

    fn applescript_string(s: &str) -> String {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    }

    pub fn show(title: &str, body: &str, _image: Option<&Path>) -> Result<()> {
        let script = format!("display notification {} with title {}", applescript_string(body), applescript_string(title));
        crate::util::spawn_detached(Command::new("osascript").arg("-e").arg(script))?;
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::path::Path;

    use image::RgbaImage;
    use snap_core::{APP_NAME, Result};

    use crate::util::other;

    pub fn set_app_icon(_icon: &RgbaImage) -> Result<()> {
        Ok(())
    }

    pub fn show(title: &str, body: &str, image: Option<&Path>) -> Result<()> {
        let mut n = notify_rust::Notification::new();
        n.appname(APP_NAME).summary(title).body(body).icon("snapoleon");
        if let Some(p) = image.filter(|p| p.is_file()).and_then(|p| std::path::absolute(p).ok()) {
            n.image_path(&p.to_string_lossy());
        }
        n.show().map(|_| ()).map_err(other)
    }
}
