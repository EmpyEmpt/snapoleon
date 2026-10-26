//! Privacy permissions. Real checks on macOS (TCC); on Windows and Linux no
//! per-app permission exists for these capabilities, so everything is granted.
//!
//! The system request calls only show UI while the permission is undecided: after the first prompt (answered,
//! dismissed or ignored) they return silently. [`status`] tells the states apart so the UI can offer the action that
//! still does something: [`request`] while undecided, [`open_settings`] after that.

/// A permission the app may need.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Perm {
    ScreenRecording,
    Accessibility,
    InputMonitoring,
    Microphone,
}

impl Perm {
    pub const ALL: [Perm; 4] = [Perm::ScreenRecording, Perm::Accessibility, Perm::InputMonitoring, Perm::Microphone];
}

/// Where a permission stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Granted,
    /// Never asked: the system prompt can still be shown.
    NotDetermined,
    /// Asked before (or refused): only System Settings can grant it now.
    Denied,
}

/// Whether the permission is currently granted.
///
/// macOS answers for Screen Recording once per process: a grant made in System Settings shows here only after the
/// app restarts (see [`relaunch`]).
pub fn granted(p: Perm) -> bool {
    imp::granted(p)
}

/// The current state of the permission. Screen Recording and Accessibility have no "undecided" query, so they count as
/// [`Status::Denied`] once this app asked for them (remembered across launches).
pub fn status(p: Perm) -> Status {
    if imp::granted(p) {
        return Status::Granted;
    }
    imp::undecided_status(p)
}

/// Show the system prompt, but only while the permission is undecided (the call would do nothing otherwise).
pub fn request(p: Perm) {
    if status(p) != Status::NotDetermined {
        tracing::info!("permission {p:?} already decided; not prompting");
        return;
    }
    tracing::info!("requesting permission {p:?}");
    imp::request(p);
    imp::set_asked(p, true);
}

/// Open the OS settings page where the user can grant the permission.
pub fn open_settings(p: Perm) {
    tracing::info!("opening system settings for {p:?}");
    imp::open_settings(p);
}

/// Whether [`reset`] can work: macOS, running from an app bundle with a bundle identifier.
pub fn can_reset() -> bool {
    imp::bundle_id().is_some()
}

/// Forget the user's answer (`tccutil reset`), so the next [`request`] prompts again. This repairs a stale entry, for
/// example one left by a build signed differently, whose switch looks on but no longer matches the app.
/// Blocks while `tccutil` runs; call it off the UI thread.
pub fn reset(p: Perm) -> snap_core::Result<()> {
    tracing::info!("resetting permission {p:?}");
    imp::reset(p)?;
    imp::set_asked(p, false);
    Ok(())
}

/// Start a new instance of the app once this process has exited, with `args`, then leave the quitting to the caller.
/// macOS applies a Screen Recording grant only to processes started after it.
pub fn relaunch(args: &[&str]) -> snap_core::Result<()> {
    tracing::info!("relaunching with {args:?}");
    imp::relaunch(args)
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::{Perm, Status};

    pub fn granted(_p: Perm) -> bool {
        true
    }

    pub fn undecided_status(_p: Perm) -> Status {
        Status::Granted
    }

    pub fn request(_p: Perm) {}

    pub fn set_asked(_p: Perm, _asked: bool) {}

    pub fn open_settings(_p: Perm) {}

    pub fn bundle_id() -> Option<String> {
        None
    }

    pub fn reset(_p: Perm) -> snap_core::Result<()> {
        Err(snap_core::Error::unsupported("permission reset"))
    }

    pub fn relaunch(_args: &[&str]) -> snap_core::Result<()> {
        Err(snap_core::Error::unsupported("relaunch"))
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::c_void;
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    use block2::RcBlock;
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use objc2_foundation::{NSBundle, NSDictionary, NSNumber, NSProcessInfo, NSString, NSURL, NSUserDefaults};

    use super::{Perm, Status};
    use crate::util::{other, spawn_detached};

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
    }

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOHIDCheckAccess(request_type: u32) -> u32;
        fn IOHIDRequestAccess(request_type: u32) -> bool;
    }

    // Provides the AVCaptureDevice class.
    #[link(name = "AVFoundation", kind = "framework")]
    unsafe extern "C" {}

    // Provides the NSWorkspace class.
    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {}

    const IOHID_REQUEST_TYPE_LISTEN_EVENT: u32 = 1;
    const IOHID_ACCESS_GRANTED: u32 = 0;
    const IOHID_ACCESS_DENIED: u32 = 1;
    const AV_NOT_DETERMINED: isize = 0;
    const AV_AUTHORIZED: isize = 3;

    fn audio_media_type() -> Retained<NSString> {
        // AVMediaTypeAudio
        NSString::from_str("soun")
    }

    fn microphone_status() -> Option<isize> {
        let cls = AnyClass::get(c"AVCaptureDevice")?;
        let media = audio_media_type();
        // SAFETY: a class method taking an NSString and returning an NSInteger.
        Some(unsafe { msg_send![cls, authorizationStatusForMediaType: &*media] })
    }

    pub fn granted(p: Perm) -> bool {
        // SAFETY: argument-free (or plain integer) C queries.
        unsafe {
            match p {
                Perm::ScreenRecording => CGPreflightScreenCaptureAccess(),
                Perm::Accessibility => AXIsProcessTrusted(),
                Perm::InputMonitoring => IOHIDCheckAccess(IOHID_REQUEST_TYPE_LISTEN_EVENT) == IOHID_ACCESS_GRANTED,
                Perm::Microphone => microphone_status() == Some(AV_AUTHORIZED),
            }
        }
    }

    /// The status of a permission that is not granted.
    pub fn undecided_status(p: Perm) -> Status {
        let undecided = match p {
            Perm::ScreenRecording | Perm::Accessibility => !asked(p),
            // SAFETY: a plain integer C query.
            Perm::InputMonitoring => (unsafe { IOHIDCheckAccess(IOHID_REQUEST_TYPE_LISTEN_EVENT) }) != IOHID_ACCESS_DENIED,
            Perm::Microphone => microphone_status() == Some(AV_NOT_DETERMINED),
        };
        if undecided { Status::NotDetermined } else { Status::Denied }
    }

    pub fn request(p: Perm) {
        // SAFETY: see `granted`; the completion block outlives the call because AVFoundation copies it.
        unsafe {
            match p {
                Perm::ScreenRecording => {
                    CGRequestScreenCaptureAccess();
                }
                Perm::Accessibility => {
                    let key = NSString::from_str("AXTrustedCheckOptionPrompt");
                    let yes = NSNumber::new_bool(true);
                    let options = NSDictionary::from_slices(&[&*key], &[&*yes]);
                    AXIsProcessTrustedWithOptions(Retained::as_ptr(&options).cast());
                }
                Perm::InputMonitoring => {
                    IOHIDRequestAccess(IOHID_REQUEST_TYPE_LISTEN_EVENT);
                }
                Perm::Microphone => {
                    if let Some(cls) = AnyClass::get(c"AVCaptureDevice") {
                        let media = audio_media_type();
                        let done = RcBlock::new(|granted: Bool| tracing::info!("microphone access answered: {}", granted.as_bool()));
                        let _: () = msg_send![cls, requestAccessForMediaType: &*media, completionHandler: &*done];
                    }
                }
            }
        }
    }

    /// The defaults key remembering that this app asked for `p` (the per-app defaults domain survives relaunches).
    fn asked_key(p: Perm) -> Retained<NSString> {
        NSString::from_str(&format!("SnapoleonAsked{p:?}"))
    }

    fn asked(p: Perm) -> bool {
        NSUserDefaults::standardUserDefaults().boolForKey(&asked_key(p))
    }

    pub fn set_asked(p: Perm, asked: bool) {
        let defaults = NSUserDefaults::standardUserDefaults();
        if asked {
            defaults.setBool_forKey(true, &asked_key(p));
        } else {
            defaults.removeObjectForKey(&asked_key(p));
        }
    }

    pub fn open_settings(p: Perm) {
        let anchor = match p {
            Perm::ScreenRecording => "Privacy_ScreenCapture",
            Perm::Accessibility => "Privacy_Accessibility",
            Perm::InputMonitoring => "Privacy_ListenEvent",
            Perm::Microphone => "Privacy_Microphone",
        };
        // System Settings (macOS 13+) names the pane by its extension; the old preference pane id still resolves
        // there only through a legacy alias, and is the only one macOS 12 knows.
        let legacy = format!("x-apple.systempreferences:com.apple.preference.security?{anchor}");
        let mut urls = vec![legacy];
        if NSProcessInfo::processInfo().operatingSystemVersion().majorVersion >= 13 {
            urls.insert(0, format!("x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?{anchor}"));
        }
        if !urls.iter().any(|url| open_url(url)) {
            tracing::warn!("could not open system settings for {p:?}");
        }
    }

    fn open_url(url: &str) -> bool {
        let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) else { return false };
        let Some(cls) = AnyClass::get(c"NSWorkspace") else { return false };
        // SAFETY: `sharedWorkspace` returns the process-wide instance; `openURL:` takes an NSURL and returns a BOOL.
        unsafe {
            let workspace: Option<Retained<AnyObject>> = msg_send![cls, sharedWorkspace];
            workspace.is_some_and(|w| msg_send![&*w, openURL: &*url])
        }
    }

    /// The identifier TCC files grants under; `None` outside an app bundle (a bare `cargo run`).
    pub fn bundle_id() -> Option<String> {
        let bundle = NSBundle::mainBundle();
        if !bundle.bundlePath().to_string().ends_with(".app") {
            return None;
        }
        bundle.bundleIdentifier().map(|id| id.to_string())
    }

    pub fn reset(p: Perm) -> snap_core::Result<()> {
        let id = bundle_id().ok_or_else(|| snap_core::Error::unsupported("permission reset outside an app bundle"))?;
        let service = match p {
            Perm::ScreenRecording => "ScreenCapture",
            Perm::Accessibility => "Accessibility",
            Perm::InputMonitoring => "ListenEvent",
            Perm::Microphone => "Microphone",
        };
        let out = Command::new("/usr/bin/tccutil").args(["reset", service, &id]).output()?;
        if out.status.success() {
            Ok(())
        } else {
            Err(other(format!("tccutil reset {service} {id}: {}", String::from_utf8_lossy(&out.stderr).trim())))
        }
    }

    pub fn relaunch(args: &[&str]) -> snap_core::Result<()> {
        // The new instance must start after this one is gone: while it runs, the single-instance check would hand the
        // launch to it. The wait gives up after 30 s so a hung quit cannot leave the helper behind for good.
        const WAIT: &str = r#"pid=$1; target=$2; shift 2; i=0
while kill -0 "$pid" 2>/dev/null && [ $i -lt 300 ]; do sleep 0.1; i=$((i+1)); done
"#;
        let bundle = NSBundle::mainBundle().bundlePath().to_string();
        let (target, start) = if bundle.ends_with(".app") {
            (bundle, r#"exec /usr/bin/open -n "$target" --args "$@""#)
        } else {
            (crate::util::exe_path()?.display().to_string(), r#"exec "$target" "$@""#)
        };
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg(format!("{WAIT}{start}")).arg("snapoleon-relaunch").arg(std::process::id().to_string()).arg(target);
        cmd.args(args);
        // Its own process group, so nothing aimed at the quitting app's group reaches the helper.
        cmd.process_group(0);
        spawn_detached(&mut cmd)?;
        Ok(())
    }
}
