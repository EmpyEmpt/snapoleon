//! OS queries the design system needs: system theme, reduce-motion, and keys egui cannot report.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux as imp;
#[cfg(target_os = "macos")]
use macos as imp;
#[cfg(windows)]
use windows as imp;

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod imp {
    use super::ExtraKeys;

    pub fn system_dark() -> Option<bool> {
        None
    }
    pub fn reduce_motion() -> bool {
        false
    }
    pub fn extra_keys_down() -> ExtraKeys {
        ExtraKeys::default()
    }
}

/// Whether the OS is set to a dark app theme; `None` when it cannot tell.
pub fn system_dark() -> Option<bool> {
    imp::system_dark()
}

/// Whether the user asked the OS to minimise animations.
pub fn reduce_motion() -> bool {
    imp::reduce_motion()
}

/// Keys that egui's event model does not expose, sampled from the OS.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExtraKeys {
    /// The Windows / Super key is held.
    pub super_key: bool,
    /// A non-modifier key that egui cannot report, as an accelerator name.
    pub key: Option<&'static str>,
}

/// Polls the keys in [`ExtraKeys`]. Only meaningful while the app window is focused.
pub fn extra_keys_down() -> ExtraKeys {
    imp::extra_keys_down()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn run(program: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(program)
        .args(args)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}
