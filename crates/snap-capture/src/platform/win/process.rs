//! Process → executable path and human-readable application name.

use std::{collections::HashMap, ffi::c_void, path::Path, sync::OnceLock};

use parking_lot::Mutex;
use windows::{
    Win32::{
        Foundation::CloseHandle,
        Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW},
        System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW},
    },
    core::{PCWSTR, PWSTR},
};

pub struct AppInfo {
    pub exe: Option<String>,
    /// FileDescription of the executable, else its file stem, else empty.
    pub name: String,
}

pub fn app_info(pid: u32) -> AppInfo {
    let Some(exe) = exe_path(pid) else { return AppInfo { exe: None, name: String::new() } };
    let name = description_cached(&exe);
    AppInfo { exe: Some(exe), name }
}

/// Lower-case executable file stem, e.g. `chrome`.
pub fn exe_stem(exe: &str) -> String {
    Path::new(exe).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn exe_path(pid: u32) -> Option<String> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    let r = unsafe { QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len) };
    let _ = unsafe { CloseHandle(handle) };
    r.ok()?;
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

fn description_cached(exe: &str) -> String {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let key = exe.to_lowercase();
    if let Some(hit) = cache.lock().get(&key) {
        return hit.clone();
    }
    let name = file_description(exe)
        .filter(|d| !d.trim().is_empty())
        .or_else(|| Path::new(exe).file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_default();
    cache.lock().insert(key, name.clone());
    name
}

fn file_description(exe: &str) -> Option<String> {
    let path: Vec<u16> = exe.encode_utf16().chain(Some(0)).collect();
    let path = PCWSTR(path.as_ptr());
    let size = unsafe { GetFileVersionInfoSizeW(path, None) };
    if size == 0 {
        return None;
    }
    let mut block = vec![0u8; size as usize];
    unsafe { GetFileVersionInfoW(path, None, size, block.as_mut_ptr() as *mut c_void) }.ok()?;

    let query = |sub: &str| -> Option<(*const u8, u32)> {
        let sub: Vec<u16> = sub.encode_utf16().chain(Some(0)).collect();
        let (mut ptr, mut len) = (std::ptr::null_mut::<c_void>(), 0u32);
        let ok = unsafe { VerQueryValueW(block.as_ptr() as *const c_void, PCWSTR(sub.as_ptr()), &mut ptr, &mut len) };
        (ok.as_bool() && !ptr.is_null() && len > 0).then_some((ptr as *const u8, len))
    };

    let mut langs: Vec<String> = Vec::new();
    if let Some((ptr, len)) = query("\\VarFileInfo\\Translation") {
        let pairs = unsafe { std::slice::from_raw_parts(ptr as *const u16, len as usize / 2) };
        langs.extend(pairs.as_chunks::<2>().0.iter().map(|p| format!("{:04x}{:04x}", p[0], p[1])));
    }
    langs.extend(["040904b0".into(), "040904e4".into(), "000004b0".into()]);

    langs.iter().find_map(|l| {
        let (ptr, len) = query(&format!("\\StringFileInfo\\{l}\\FileDescription"))?;
        let s = unsafe { std::slice::from_raw_parts(ptr as *const u16, len as usize) };
        let s = super::wide_to_string(s);
        (!s.trim().is_empty()).then_some(s)
    })
}
