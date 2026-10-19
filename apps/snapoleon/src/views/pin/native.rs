//! The two OS facilities a pin needs that the shell does not expose: whole-window opacity (the shell has
//! no transparent surfaces, so the compositor blends the window as a whole) and a global "is Esc held"
//! probe for leaving click-through mode, when the pin itself no longer receives input.
//!
//! Both are supported on Windows and macOS. Linux has neither a stable window-opacity API for every
//! compositor nor a global key state, so the pin hides the matching controls there ([`SUPPORTED`]).

/// Whether window opacity and click-through (which depends on the Esc probe) are available.
pub const SUPPORTED: bool = cfg!(any(windows, target_os = "macos"));

/// Make the window with this native id (`Shell::native_id`) `opacity` (0..1) opaque.
///
/// winit rewrites the window style whenever a flag such as visibility or hit-testing changes, which drops
/// the layered bit on Windows, so callers repeat this until the value sticks (see `PinView::sync_opacity`).
pub fn set_opacity(native_id: u64, opacity: f32) {
    imp::set_opacity(native_id, opacity.clamp(0.0, 1.0));
}

/// Whether Esc is physically down right now, in any application.
pub fn escape_down() -> bool {
    imp::escape_down()
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    const GWL_EXSTYLE: i32 = -20;
    const WS_EX_LAYERED: i32 = 0x0008_0000;
    const LWA_ALPHA: u32 = 0x2;
    const VK_ESCAPE: i32 = 0x1B;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetWindowLongW(hwnd: *mut c_void, index: i32) -> i32;
        fn SetWindowLongW(hwnd: *mut c_void, index: i32, value: i32) -> i32;
        fn SetLayeredWindowAttributes(hwnd: *mut c_void, key: u32, alpha: u8, flags: u32) -> i32;
        fn GetAsyncKeyState(key: i32) -> i16;
    }

    pub fn set_opacity(native_id: u64, opacity: f32) {
        let hwnd = native_id as usize as *mut c_void;
        let alpha = (opacity * 255.0).round() as u8;
        // SAFETY: plain Win32 calls on a window owned by this process; the handle comes from the shell.
        unsafe {
            let ex = GetWindowLongW(hwnd, GWL_EXSTYLE);
            if ex & WS_EX_LAYERED == 0 {
                SetWindowLongW(hwnd, GWL_EXSTYLE, ex | WS_EX_LAYERED);
            }
            SetLayeredWindowAttributes(hwnd, 0, alpha, LWA_ALPHA);
        }
    }

    pub fn escape_down() -> bool {
        // SAFETY: GetAsyncKeyState has no preconditions; the high bit means "down now".
        unsafe { GetAsyncKeyState(VK_ESCAPE) < 0 }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::{
        ffi::{c_char, c_void},
        mem::transmute,
    };

    const KEY_CODE_ESCAPE: u16 = 53;
    /// `kCGEventSourceStateCombinedSessionState`.
    const COMBINED_SESSION_STATE: i32 = 0;

    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventSourceKeyState(state: i32, key: u16) -> bool;
    }

    type Msg0 = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
    type MsgInt = unsafe extern "C" fn(*mut c_void, *mut c_void, i64) -> *mut c_void;
    type MsgFloat = unsafe extern "C" fn(*mut c_void, *mut c_void, f64);

    /// `native_id` is the NSWindow number, so the window is looked up through `NSApp`.
    pub fn set_opacity(native_id: u64, opacity: f32) {
        // SAFETY: Objective-C message sends with the selectors' documented signatures, on the main thread
        // (views run on the UI thread). A missing window yields a nil receiver, which is a no-op.
        unsafe {
            let send0: Msg0 = transmute(objc_msgSend as *const ());
            let send_int: MsgInt = transmute(objc_msgSend as *const ());
            let send_float: MsgFloat = transmute(objc_msgSend as *const ());
            let app = send0(objc_getClass(c"NSApplication".as_ptr()), sel_registerName(c"sharedApplication".as_ptr()));
            let window = send_int(app, sel_registerName(c"windowWithWindowNumber:".as_ptr()), native_id as i64);
            send_float(window, sel_registerName(c"setAlphaValue:".as_ptr()), f64::from(opacity));
        }
    }

    pub fn escape_down() -> bool {
        // SAFETY: CGEventSourceKeyState only reads the session's key state.
        unsafe { CGEventSourceKeyState(COMBINED_SESSION_STATE, KEY_CODE_ESCAPE) }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod imp {
    pub fn set_opacity(_native_id: u64, _opacity: f32) {}

    pub fn escape_down() -> bool {
        false
    }
}
