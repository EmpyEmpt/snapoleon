//! Hide and restore the desktop icon list view.

use std::ffi::c_void;

use windows::{
    Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{FindWindowExW, FindWindowW, IsWindow, IsWindowVisible, SW_HIDE, SW_SHOW, ShowWindow},
    },
    core::{PCWSTR, w},
};

/// Restores the icons on drop.
pub struct IconsGuard {
    list_view: isize,
}

fn to_hwnd(v: isize) -> HWND {
    HWND(v as *mut c_void)
}

/// `SysListView32` inside `SHELLDLL_DefView`, which lives under `Progman` or (after the wallpaper
/// slideshow/Win+D reparenting) under one of the `WorkerW` windows.
fn find_list_view() -> Option<HWND> {
    let progman = unsafe { FindWindowW(w!("Progman"), PCWSTR::null()) }.ok()?;
    let mut def_view = unsafe { FindWindowExW(Some(progman), None, w!("SHELLDLL_DefView"), PCWSTR::null()) }.ok();
    let mut worker: Option<HWND> = None;
    while def_view.is_none() {
        let next = unsafe { FindWindowExW(None, worker, w!("WorkerW"), PCWSTR::null()) }.ok()?;
        def_view = unsafe { FindWindowExW(Some(next), None, w!("SHELLDLL_DefView"), PCWSTR::null()) }.ok();
        worker = Some(next);
    }
    unsafe { FindWindowExW(def_view, None, w!("SysListView32"), PCWSTR::null()) }.ok()
}

pub fn hide() -> Option<IconsGuard> {
    let list_view = find_list_view()?;
    if !unsafe { IsWindowVisible(list_view) }.as_bool() {
        return None;
    }
    let _ = unsafe { ShowWindow(list_view, SW_HIDE) };
    Some(IconsGuard { list_view: list_view.0 as isize })
}

impl Drop for IconsGuard {
    fn drop(&mut self) {
        let h = to_hwnd(self.list_view);
        if unsafe { IsWindow(Some(h)) }.as_bool() {
            let _ = unsafe { ShowWindow(h, SW_SHOW) };
        }
    }
}
