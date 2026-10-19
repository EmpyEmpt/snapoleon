//! Top-level window enumeration.

use std::{ffi::c_void, mem};

use snap_core::{Rect, Result};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, RECT},
        Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute},
        UI::WindowsAndMessaging::{
            EnumChildWindows, EnumWindows, GWL_EXSTYLE, GetClassNameW, GetForegroundWindow, GetWindowLongPtrW, GetWindowPlacement,
            GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, WINDOWPLACEMENT, WS_EX_APPWINDOW,
            WS_EX_TOOLWINDOW,
        },
    },
    core::BOOL,
};

use super::{monitors::virtual_bounds, os_err, process, rect_of};
use crate::WindowInfo;

/// Shell and desktop windows that are visible but never meaningful capture targets.
const SHELL_CLASSES: &[&str] = &["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd", "NotifyIconOverflowWindow"];

pub fn hwnd_from_id(id: u64) -> HWND {
    HWND(id as usize as *mut c_void)
}

pub fn id_of(h: HWND) -> u64 {
    h.0 as usize as u64
}

pub fn is_minimized(h: HWND) -> bool {
    unsafe { IsIconic(h) }.as_bool()
}

/// The visible frame (DWM extended frame bounds, i.e. without the invisible resize borders).
pub fn visible_frame(h: HWND) -> Option<Rect> {
    if !unsafe { IsWindow(Some(h)) }.as_bool() {
        return None;
    }
    if is_minimized(h) {
        let mut wp = WINDOWPLACEMENT { length: mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
        return unsafe { GetWindowPlacement(h, &mut wp) }.ok().map(|_| rect_of(wp.rcNormalPosition));
    }
    let mut r = RECT::default();
    let ok =
        unsafe { DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut _ as *mut c_void, mem::size_of::<RECT>() as u32) }
            .is_ok();
    if !ok {
        unsafe { GetWindowRect(h, &mut r) }.ok()?;
    }
    Some(rect_of(r))
}

pub fn class_name(h: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn title(h: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub fn pid_of(h: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
    pid
}

fn is_cloaked(h: HWND) -> bool {
    let mut cloaked = 0u32;
    let ok = unsafe { DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut _ as *mut c_void, mem::size_of::<u32>() as u32) };
    ok.is_ok() && cloaked != 0
}

/// UWP windows are hosted by `ApplicationFrameHost`; the real app owns the `CoreWindow` child.
fn hosted_app_pid(h: HWND, host_pid: u32) -> u32 {
    struct Search {
        host_pid: u32,
        found: u32,
    }
    unsafe extern "system" fn visit(child: HWND, data: LPARAM) -> BOOL {
        let s = unsafe { &mut *(data.0 as *mut Search) };
        let pid = pid_of(child);
        if pid != s.host_pid && class_name(child) == "Windows.UI.Core.CoreWindow" {
            s.found = pid;
            return false.into();
        }
        true.into()
    }
    let mut s = Search { host_pid, found: 0 };
    let _ = unsafe { EnumChildWindows(Some(h), Some(visit), LPARAM(&mut s as *mut _ as isize)) };
    if s.found != 0 { s.found } else { host_pid }
}

unsafe extern "system" fn collect(h: HWND, data: LPARAM) -> BOOL {
    unsafe { &mut *(data.0 as *mut Vec<HWND>) }.push(h);
    true.into()
}

/// Describe `h` if it is a window worth offering as a capture target.
fn describe(h: HWND, own_pid: u32, desktop: Rect) -> Option<WindowInfo> {
    if !unsafe { IsWindowVisible(h) }.as_bool() {
        return None;
    }
    let ex = unsafe { GetWindowLongPtrW(h, GWL_EXSTYLE) } as u32;
    let app_window = ex & WS_EX_APPWINDOW.0 != 0;
    if ex & WS_EX_TOOLWINDOW.0 != 0 && !app_window {
        return None;
    }
    let mut pid = pid_of(h);
    if pid == own_pid || is_cloaked(h) {
        return None;
    }
    let class = class_name(h);
    if SHELL_CLASSES.contains(&class.as_str()) {
        return None;
    }
    let title = title(h);
    if title.is_empty() && !app_window {
        return None;
    }
    let minimized = is_minimized(h);
    let rect = visible_frame(h)?;
    if rect.is_empty() || (!minimized && rect.intersect(desktop).is_none()) {
        return None;
    }
    if class == "ApplicationFrameWindow" {
        pid = hosted_app_pid(h, pid);
        if pid == own_pid {
            return None;
        }
    }
    let app = process::app_info(pid);
    let app_name = if app.name.is_empty() { class } else { app.name };
    Some(WindowInfo { id: id_of(h), pid, title, app_name, app_id: app.exe, rect, z: 0, minimized })
}

pub fn list() -> Result<Vec<WindowInfo>> {
    let mut handles: Vec<HWND> = Vec::new();
    unsafe { EnumWindows(Some(collect), LPARAM(&mut handles as *mut _ as isize)) }.map_err(|e| os_err("EnumWindows", e))?;
    let (own_pid, desktop) = (std::process::id(), virtual_bounds());
    let mut out: Vec<WindowInfo> = handles.into_iter().filter_map(|h| describe(h, own_pid, desktop)).collect();
    for (z, w) in out.iter_mut().enumerate() {
        w.z = z as u32;
    }
    Ok(out)
}

pub fn foreground() -> Option<WindowInfo> {
    let fg = id_of(unsafe { GetForegroundWindow() });
    list().ok()?.into_iter().find(|w| w.id == fg)
}
