use std::mem::size_of;

use snap_core::{Error, Rect, Result};
use windows_sys::Win32::{
    Foundation::HWND,
    Graphics::{
        Dwm::{DWMNCRP_DISABLED, DWMWA_NCRENDERING_POLICY, DwmSetWindowAttribute},
        Gdi::{CreateRoundRectRgn, SetWindowRgn},
    },
    System::Threading::{AttachThreadInput, GetCurrentThreadId},
    UI::WindowsAndMessaging::{
        BringWindowToTop, GWL_EXSTYLE, GetForegroundWindow, GetWindowLongW, GetWindowThreadProcessId, HWND_TOPMOST, SWP_FRAMECHANGED,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow, SetWindowDisplayAffinity, SetWindowLongW, SetWindowPos,
        WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
    },
};
use winit::{
    event_loop::ActiveEventLoop,
    platform::windows::{CornerPreference, WindowAttributesExtWindows},
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::{Window, WindowAttributes},
};

use crate::WindowSpec;

/// The OS honours exact physical rects, so any deviation is a real deviation.
pub(crate) const COVER_TOLERANCE: i32 = 0;

fn hwnd(window: &Window) -> Option<HWND> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get() as HWND),
        _ => None,
    }
}

pub(crate) fn window_attributes(_el: &ActiveEventLoop, attrs: WindowAttributes, spec: &WindowSpec, _app_id: &str) -> WindowAttributes {
    let corners = if spec.cover.is_some() {
        CornerPreference::DoNotRound
    } else if spec.rounded {
        CornerPreference::Round
    } else {
        CornerPreference::Default
    };
    attrs.with_corner_preference(corners).with_undecorated_shadow(false)
}

/// Native tweaks applied while the window is still hidden.
pub(crate) fn configure_window(window: &Window, spec: &WindowSpec) {
    let Some(h) = hwnd(window) else { return };
    if spec.is_tool() {
        set_tool_style(h);
    }
    if spec.exclude_from_capture
        && let Err(e) = set_exclude_from_capture(window, true)
    {
        tracing::warn!("exclude_from_capture failed: {e}");
    }
    if spec.cover.is_some() {
        let policy = DWMNCRP_DISABLED;
        // SAFETY: `policy` outlives the call and the size matches DWMNCRENDERINGPOLICY (i32).
        unsafe { DwmSetWindowAttribute(h, DWMWA_NCRENDERING_POLICY as u32, (&raw const policy).cast(), size_of::<i32>() as u32) };
    }
}

/// Clip the window to a stadium (corner diameter = height), or lift the clip. The region is not
/// anti-aliased, but it is the only way to get round ends without a transparent swap chain.
pub(crate) fn set_pill(window: &Window, on: bool) {
    let Some(h) = hwnd(window) else { return };
    let size = window.inner_size();
    // SAFETY: the region is handed to the window, which owns and frees it; null removes the clip.
    unsafe {
        let region = if on {
            CreateRoundRectRgn(0, 0, size.width as i32 + 1, size.height as i32 + 1, size.height as i32, size.height as i32)
        } else {
            std::ptr::null_mut()
        };
        SetWindowRgn(h, region, 1);
    }
}

/// winit rewrites the extended style whenever any window flag changes
/// (visibility, level, hit-testing), which drops `WS_EX_TOOLWINDOW`. Call this
/// after every such change on a tool window.
pub(crate) fn refresh(window: &Window, tool_style: bool) {
    if tool_style && let Some(h) = hwnd(window) {
        set_tool_style(h);
    }
}

fn set_tool_style(h: HWND) {
    // SAFETY: plain Win32 calls on a window owned by this process.
    unsafe {
        let ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
        let want = (ex | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW;
        if want != ex {
            SetWindowLongW(h, GWL_EXSTYLE, want as i32);
            SetWindowPos(h, std::ptr::null_mut(), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
        }
    }
}

pub(crate) fn set_exclude_from_capture(window: &Window, on: bool) -> Result<()> {
    let h = hwnd(window).ok_or_else(|| Error::unsupported("window has no Win32 handle"))?;
    let affinity = if on { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE };
    // SAFETY: plain Win32 call on a window owned by this process.
    if unsafe { SetWindowDisplayAffinity(h, affinity) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

/// Exact physical placement, bypassing winit's DPI conversions.
pub(crate) fn place_cover(window: &Window, r: Rect, _monitors: &[(Rect, f32)]) {
    if let Some(h) = hwnd(window) {
        // SAFETY: plain Win32 call on a window owned by this process.
        unsafe { SetWindowPos(h, HWND_TOPMOST, r.x, r.y, r.w as i32, r.h as i32, SWP_NOACTIVATE) };
    }
}

pub(crate) fn set_outer_rect(window: &Window, r: Rect, _monitors: &[(Rect, f32)]) {
    if let Some(h) = hwnd(window) {
        // SAFETY: plain Win32 call on a window owned by this process.
        unsafe { SetWindowPos(h, std::ptr::null_mut(), r.x, r.y, r.w as i32, r.h as i32, SWP_NOZORDER | SWP_NOACTIVATE) };
    }
}

pub(crate) fn native_id(window: &Window) -> Option<u64> {
    hwnd(window).map(|h| h as usize as u64)
}

/// Bring the window to the foreground. Windows only lets the process that last received input do that; a window
/// opened by a global hotkey or a second launch usually is not, so the request is retried while borrowing the
/// input state of the current foreground thread (no synthetic key presses, which would be seen by that app).
pub(crate) fn activate(window: &Window) {
    let Some(h) = hwnd(window) else { return };
    // SAFETY: plain Win32 calls on a window owned by this process; the foreground window is only queried.
    unsafe {
        if GetForegroundWindow() == h || SetForegroundWindow(h) != 0 {
            return;
        }
        let foreground = GetForegroundWindow();
        let (theirs, ours) = (GetWindowThreadProcessId(foreground, std::ptr::null_mut()), GetCurrentThreadId());
        if foreground.is_null() || theirs == 0 || theirs == ours {
            return;
        }
        AttachThreadInput(ours, theirs, 1);
        BringWindowToTop(h);
        SetForegroundWindow(h);
        AttachThreadInput(ours, theirs, 0);
    }
}

pub(crate) fn show(window: &Window, _key: bool) {
    window.set_visible(true);
}

pub(crate) fn app_active(windows: &mut dyn Iterator<Item = &Window>) -> bool {
    windows.any(Window::has_focus)
}

/// The OS gives the focus back to the window below when ours goes away.
pub(crate) fn remember_frontmost() {}

pub(crate) fn restore_frontmost() {}

pub(crate) fn safe_area_top(_window: &Window) -> f32 {
    0.0
}
