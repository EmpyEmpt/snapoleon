//! Privacy permissions. Real checks on macOS (TCC); on Windows and Linux no
//! per-app permission exists for these capabilities, so everything is granted.

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

/// Whether the permission is currently granted.
pub fn granted(p: Perm) -> bool {
    imp::granted(p)
}

/// Ask the OS for the permission. On macOS this shows the system prompt the
/// first time; if the user already denied it, use [`open_settings`] instead.
pub fn request(p: Perm) {
    imp::request(p);
}

/// Open the OS settings page where the user can grant the permission.
pub fn open_settings(p: Perm) {
    imp::open_settings(p);
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::Perm;

    pub fn granted(_p: Perm) -> bool {
        true
    }

    pub fn request(_p: Perm) {}

    pub fn open_settings(_p: Perm) {}
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::c_void;
    use std::process::Command;

    use block2::RcBlock;
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, Bool};
    use objc2_foundation::{NSDictionary, NSNumber, NSString};

    use super::Perm;

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

    const IOHID_REQUEST_TYPE_LISTEN_EVENT: u32 = 1;
    const IOHID_ACCESS_GRANTED: u32 = 0;
    const AV_AUTHORIZED: isize = 3;

    fn audio_media_type() -> Retained<NSString> {
        // AVMediaTypeAudio
        NSString::from_str("soun")
    }

    pub fn granted(p: Perm) -> bool {
        // SAFETY: argument-free (or plain integer) C queries and a class message with an NSString.
        unsafe {
            match p {
                Perm::ScreenRecording => CGPreflightScreenCaptureAccess(),
                Perm::Accessibility => AXIsProcessTrusted(),
                Perm::InputMonitoring => IOHIDCheckAccess(IOHID_REQUEST_TYPE_LISTEN_EVENT) == IOHID_ACCESS_GRANTED,
                Perm::Microphone => match AnyClass::get(c"AVCaptureDevice") {
                    Some(cls) => {
                        let media = audio_media_type();
                        let status: isize = msg_send![cls, authorizationStatusForMediaType: &*media];
                        status == AV_AUTHORIZED
                    }
                    None => false,
                },
            }
        }
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
                        let done = RcBlock::new(|_granted: Bool| {});
                        let _: () = msg_send![cls, requestAccessForMediaType: &*media, completionHandler: &*done];
                    }
                }
            }
        }
    }

    pub fn open_settings(p: Perm) {
        let pane = match p {
            Perm::ScreenRecording => "Privacy_ScreenCapture",
            Perm::Accessibility => "Privacy_Accessibility",
            Perm::InputMonitoring => "Privacy_ListenEvent",
            Perm::Microphone => "Privacy_Microphone",
        };
        let url = format!("x-apple.systempreferences:com.apple.preference.security?{pane}");
        if let Err(e) = crate::util::spawn_detached(Command::new("open").arg(url)) {
            tracing::warn!("open system settings: {e}");
        }
    }
}
