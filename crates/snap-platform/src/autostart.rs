//! Start at login. The app is launched with `--background` so it goes straight
//! to the tray.
//!
//! - Windows: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
//! - macOS: `SMAppService.mainAppService` (macOS 13+): an "Open at Login" item that shows the app's name and icon
//!   and follows the bundle wherever it moves. Login items get no arguments, so a launch at login is recognised
//!   by [`launched_at_login`] instead of `--background`. Earlier versions wrote a LaunchAgent plist; it is removed
//!   on the first start.
//! - Linux: `~/.config/autostart/snapoleon.desktop`

use snap_core::Result;

pub const BACKGROUND_ARG: &str = "--background";

/// Where starting at login stands, as the OS sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Off,
    On,
    /// macOS: registered, but switched off in System Settings, Login Items; only the user can allow it there.
    NeedsApproval,
}

pub fn set(enabled: bool) -> Result<()> {
    imp::set(enabled)
}

/// Whether the app is currently set to start at login.
pub fn get() -> bool {
    imp::get()
}

pub fn status() -> Status {
    imp::status()
}

/// Bring the OS registration in line with the stored preference `want` at startup, and return the value the
/// preference should have now. On macOS the preference is applied once; after that the OS is the source of truth,
/// because the user can switch the login item off in System Settings, which the app must not undo.
pub fn reconcile(want: bool) -> Result<bool> {
    imp::reconcile(want)
}

/// This process was started by the OS as a login item (macOS; elsewhere autostart passes [`BACKGROUND_ARG`]).
/// Valid only while the app finishes launching.
pub fn launched_at_login() -> bool {
    imp::launched_at_login()
}

/// Open the OS page that lists login items (macOS: System Settings, General, Login Items).
pub fn open_login_items() -> Result<()> {
    imp::open_login_items()
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

    pub fn status() -> super::Status {
        if get() { super::Status::On } else { super::Status::Off }
    }

    pub fn reconcile(want: bool) -> Result<bool> {
        if get() != want {
            set(want)?;
        }
        Ok(want)
    }

    pub fn launched_at_login() -> bool {
        false
    }

    pub fn open_login_items() -> Result<()> {
        Err(snap_core::Error::unsupported("login items settings"))
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::path::PathBuf;

    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager, NSBundle};
    use objc2_service_management::{SMAppService, SMAppServiceStatus};
    use snap_core::{APP_ID, Error, Result, paths, t};

    use super::Status;
    use crate::util::other;

    /// `kAEOpenApplication`, `keyAEPropData` and `keyAELaunchedAsLogInItem`.
    const OPEN_APPLICATION: u32 = u32::from_be_bytes(*b"oapp");
    const PROP_DATA: u32 = u32::from_be_bytes(*b"prdt");
    const LAUNCHED_AS_LOGIN_ITEM: u32 = u32::from_be_bytes(*b"lgit");

    /// The LaunchAgent older versions wrote.
    fn legacy_plist() -> Option<PathBuf> {
        let home = directories::BaseDirs::new()?;
        Some(home.home_dir().join("Library/LaunchAgents").join(format!("{APP_ID}.plist")))
    }

    /// Remove the old LaunchAgent; `true` when there was one. It pointed at whatever path the executable had then,
    /// which goes stale when the app moves, and showed up as a bare executable in System Settings.
    fn remove_legacy() -> bool {
        legacy_plist().is_some_and(|p| std::fs::remove_file(p).is_ok())
    }

    /// Written once the preference has been applied through SMAppService on this machine.
    fn applied_marker() -> PathBuf {
        paths::data_dir().join("login-item-applied")
    }

    /// The `.app` this process runs from. SMAppService registers the main bundle, so without one there is nothing to
    /// register (`cargo run`).
    fn bundle_path() -> Option<String> {
        let bundle = NSBundle::mainBundle();
        let path = bundle.bundlePath().to_string();
        (bundle.bundleIdentifier().is_some() && path.ends_with(".app")).then_some(path)
    }

    fn service() -> Retained<SMAppService> {
        // SAFETY: a class method without arguments.
        unsafe { SMAppService::mainAppService() }
    }

    pub fn status() -> Status {
        if bundle_path().is_none() {
            return Status::Off;
        }
        // SAFETY: a plain property read.
        match unsafe { service().status() } {
            SMAppServiceStatus::Enabled => Status::On,
            SMAppServiceStatus::RequiresApproval => Status::NeedsApproval,
            _ => Status::Off,
        }
    }

    pub fn get() -> bool {
        status() == Status::On
    }

    pub fn set(enabled: bool) -> Result<()> {
        remove_legacy();
        let bundle = bundle_path().ok_or_else(|| Error::unsupported("launch at login outside Snapoleon.app"))?;
        // A copy started from Downloads or a disk image runs from a random read-only location (App Translocation)
        // that is gone after a restart.
        if enabled && bundle.contains("/AppTranslocation/") {
            return Err(Error::invalid(t("platform.autostart.translocated")));
        }
        let service = service();
        // SAFETY: plain calls on the main app service.
        let result = unsafe { if enabled { service.registerAndReturnError() } else { service.unregisterAndReturnError() } };
        match result {
            Ok(()) => Ok(()),
            // Unregistering what is not registered, or registering what the user switched off in System Settings
            // (it stays "needs approval", which the settings page explains), is not a failure.
            Err(_) if (status() == Status::Off) != enabled => Ok(()),
            Err(e) => Err(other(e.localizedDescription())),
        }
    }

    pub fn reconcile(want: bool) -> Result<bool> {
        if bundle_path().is_none() {
            return Ok(want);
        }
        let marker = applied_marker();
        if remove_legacy() || !marker.exists() {
            set(want)?;
            if let Some(dir) = marker.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&marker, b"")?;
            return Ok(want);
        }
        Ok(status() != Status::Off)
    }

    pub fn launched_at_login() -> bool {
        let Some(event) = NSAppleEventManager::sharedAppleEventManager().currentAppleEvent() else { return false };
        // SAFETY: `eventID` and `paramDescriptorForKeyword:` take and return plain values on a valid descriptor.
        unsafe {
            let id: u32 = msg_send![&*event, eventID];
            let prop: Option<Retained<NSAppleEventDescriptor>> = msg_send![&*event, paramDescriptorForKeyword: PROP_DATA];
            id == OPEN_APPLICATION && prop.is_some_and(|p| p.enumCodeValue() == LAUNCHED_AS_LOGIN_ITEM)
        }
    }

    pub fn open_login_items() -> Result<()> {
        // SAFETY: a class method without arguments.
        unsafe { SMAppService::openSystemSettingsLoginItems() };
        Ok(())
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

    pub fn status() -> super::Status {
        if get() { super::Status::On } else { super::Status::Off }
    }

    pub fn reconcile(want: bool) -> Result<bool> {
        if get() != want {
            set(want)?;
        }
        Ok(want)
    }

    pub fn launched_at_login() -> bool {
        false
    }

    pub fn open_login_items() -> Result<()> {
        Err(snap_core::Error::unsupported("login items settings"))
    }
}
