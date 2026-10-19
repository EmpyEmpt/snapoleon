use std::ffi::c_void;

use super::ExtraKeys;

#[link(name = "advapi32")]
unsafe extern "system" {
    fn RegGetValueW(
        hkey: isize,
        subkey: *const u16,
        value: *const u16,
        flags: u32,
        kind: *mut u32,
        data: *mut c_void,
        len: *mut u32,
    ) -> i32;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn SystemParametersInfoW(action: u32, param: u32, pv: *mut c_void, win_ini: u32) -> i32;
    fn GetAsyncKeyState(vkey: i32) -> i16;
}

const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
const RRF_RT_REG_DWORD: u32 = 0x10;
const SPI_GETCLIENTAREAANIMATION: u32 = 0x1042;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn read_dword(subkey: &str, value: &str) -> Option<u32> {
    let (subkey, value) = (wide(subkey), wide(value));
    let mut data = 0u32;
    let mut len = size_of::<u32>() as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&raw mut data).cast(),
            &mut len,
        )
    };
    (rc == 0).then_some(data)
}

pub fn system_dark() -> Option<bool> {
    read_dword(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "AppsUseLightTheme").map(|v| v == 0)
}

pub fn reduce_motion() -> bool {
    let mut enabled = 1i32;
    let ok = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut enabled).cast(), 0) };
    ok != 0 && enabled == 0
}

fn down(vkey: i32) -> bool {
    (unsafe { GetAsyncKeyState(vkey) }) < 0
}

pub fn extra_keys_down() -> ExtraKeys {
    const VK_PAUSE: i32 = 0x13;
    const VK_SNAPSHOT: i32 = 0x2C;
    const VK_LWIN: i32 = 0x5B;
    const VK_RWIN: i32 = 0x5C;
    const VK_SCROLL: i32 = 0x91;
    let key = if down(VK_SNAPSHOT) {
        Some("PrintScreen")
    } else if down(VK_SCROLL) {
        Some("ScrollLock")
    } else if down(VK_PAUSE) {
        Some("Pause")
    } else {
        None
    };
    ExtraKeys { super_key: down(VK_LWIN) || down(VK_RWIN), key }
}
