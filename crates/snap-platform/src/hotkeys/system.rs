//! Shortcuts the operating system keeps for itself. The OS does not refuse a global hotkey that clashes with one of
//! them (Carbon's `RegisterEventHotKey` succeeds for Shift+Cmd+4, `RegisterHotKey` for Print Screen), so the clash has
//! to be looked up to be explained.

#[cfg(any(target_os = "macos", windows))]
use std::time::{Duration, Instant};

#[cfg(any(target_os = "macos", windows))]
use parking_lot::Mutex;

use super::SystemConflict;
use super::accel::Accel;

/// The OS state is read again after this long: the user may turn a system shortcut off while Snapoleon runs.
#[cfg(any(target_os = "macos", windows))]
const REFRESH: Duration = Duration::from_secs(3);

/// `read()` through a short-lived cache, because the settings page asks for every binding on every frame.
#[cfg(any(target_os = "macos", windows))]
fn cached<T: Clone>(slot: &Mutex<Option<(Instant, T)>>, read: impl FnOnce() -> T) -> T {
    let mut slot = slot.lock();
    match &*slot {
        Some((at, value)) if at.elapsed() < REFRESH => value.clone(),
        _ => {
            let value = read();
            *slot = Some((Instant::now(), value.clone()));
            value
        }
    }
}

#[cfg(target_os = "macos")]
pub(super) fn conflict(a: Accel) -> Option<SystemConflict> {
    mac::conflict(a)
}

#[cfg(windows)]
pub(super) fn conflict(a: Accel) -> Option<SystemConflict> {
    win::conflict(a)
}

#[cfg(target_os = "linux")]
pub(super) fn conflict(_a: Accel) -> Option<SystemConflict> {
    // Desktop environments keep their shortcuts in their own settings stores; X11 grabs of a taken combination
    // already fail and are reported by `HotkeyManager::register`.
    None
}

#[cfg(target_os = "macos")]
mod mac {
    use std::sync::Arc;

    use global_hotkey::hotkey::{Code, Modifiers};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};
    use parking_lot::Mutex;
    use snap_core::{t, tf};

    use super::{Accel, SystemConflict, cached};

    // Carbon modifier bits (Events.h). Entries also carry other bits, such as 0x20000 (the Fn key) on arrows and
    // F-keys, which a hotkey registration does not distinguish.
    const CMD: u32 = 0x100;
    const SHIFT: u32 = 0x200;
    const OPTION: u32 = 0x800;
    const CONTROL: u32 = 0x1000;
    const MODIFIER_MASK: u32 = CMD | SHIFT | OPTION | CONTROL;

    /// Keyboard Shortcuts in System Settings. No URL anchor opens a category directly.
    const SETTINGS_URL: &str = "x-apple.systempreferences:com.apple.Keyboard-Settings.extension";

    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        /// HIToolbox: every system ("symbolic") hotkey, defaults included, as dictionaries with
        /// `kHISymbolicHotKeyCode` (virtual key code), `kHISymbolicHotKeyModifiers` (Carbon bits) and
        /// `kHISymbolicHotKeyEnabled`. Exported (HIToolbox.tbd) but declared in no public header; Copy rule.
        fn CopySymbolicHotKeys(out: *mut *mut NSArray<NSDictionary<NSString, AnyObject>>) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        /// Drops this process' cached copy of a preferences domain, so a change made in System Settings is seen.
        fn CFPreferencesAppSynchronize(application_id: *const NSString) -> u8;
    }

    /// (virtual key code, Carbon modifiers) of every enabled system hotkey.
    type Taken = Arc<Vec<(u32, u32)>>;

    static TAKEN: Mutex<Option<(std::time::Instant, Taken)>> = Mutex::new(None);

    pub fn conflict(a: Accel) -> Option<SystemConflict> {
        let code = virtual_key(a.key)?;
        let mods = carbon_modifiers(a.mods);
        let taken = cached(&TAKEN, read_taken);
        if !taken.contains(&(code, mods)) {
            return None;
        }
        let accel = a.label();
        let key = if is_screenshot(code, mods) { "platform.hotkey.system.mac_screenshots" } else { "platform.hotkey.system.mac_other" };
        Some(SystemConflict {
            message: tf(key, &[("accel", &accel)]),
            open_label: t("platform.hotkey.system.mac_open").into_owned(),
            settings_url: SETTINGS_URL,
        })
    }

    fn read_taken() -> Taken {
        let domain = NSString::from_str("com.apple.symbolichotkeys");
        let mut array = std::ptr::null_mut();
        // SAFETY: both functions take/return plain CF objects; NSString and NSArray are toll-free bridged with their
        // CF counterparts. The array is returned at +1 and released by the `Retained`.
        let array = unsafe {
            CFPreferencesAppSynchronize(Retained::as_ptr(&domain));
            let status = CopySymbolicHotKeys(&mut array);
            if status != 0 {
                tracing::debug!("CopySymbolicHotKeys failed: {status}");
            }
            Retained::from_raw(array)
        };
        let Some(array) = array else { return Taken::default() };
        let number = |entry: &NSDictionary<NSString, AnyObject>, key: &str| {
            entry.objectForKey(&NSString::from_str(key)).and_then(|v| v.downcast_ref::<NSNumber>().map(NSNumber::integerValue))
        };
        let taken = array
            .iter()
            .filter(|entry| number(entry, "kHISymbolicHotKeyEnabled").is_some_and(|on| on != 0))
            .filter_map(|entry| {
                let code = number(&entry, "kHISymbolicHotKeyCode")?;
                let mods = number(&entry, "kHISymbolicHotKeyModifiers")?;
                // 0xFFFF marks a shortcut defined by character rather than by key.
                (0..0xFFFF).contains(&code).then_some((code as u32, mods as u32 & MODIFIER_MASK))
            })
            .collect();
        Arc::new(taken)
    }

    /// The screenshot shortcuts (Shift+Cmd+3/4/5/6, with Ctrl to copy instead of save).
    fn is_screenshot(code: u32, mods: u32) -> bool {
        const KEYS_3_4_5_6: [u32; 4] = [0x14, 0x15, 0x17, 0x16];
        KEYS_3_4_5_6.contains(&code) && (mods == CMD | SHIFT || mods == CONTROL | CMD | SHIFT)
    }

    fn carbon_modifiers(m: Modifiers) -> u32 {
        [(Modifiers::META, CMD), (Modifiers::SHIFT, SHIFT), (Modifiers::ALT, OPTION), (Modifiers::CONTROL, CONTROL)]
            .into_iter()
            .filter(|(m2, _)| m.contains(*m2))
            .map(|(_, bit)| bit)
            .sum()
    }

    /// ANSI virtual key codes (Events.h `kVK_*`), the same table `global-hotkey` registers with.
    fn virtual_key(code: Code) -> Option<u32> {
        Some(match code {
            Code::KeyA => 0x00,
            Code::KeyS => 0x01,
            Code::KeyD => 0x02,
            Code::KeyF => 0x03,
            Code::KeyH => 0x04,
            Code::KeyG => 0x05,
            Code::KeyZ => 0x06,
            Code::KeyX => 0x07,
            Code::KeyC => 0x08,
            Code::KeyV => 0x09,
            Code::KeyB => 0x0b,
            Code::KeyQ => 0x0c,
            Code::KeyW => 0x0d,
            Code::KeyE => 0x0e,
            Code::KeyR => 0x0f,
            Code::KeyY => 0x10,
            Code::KeyT => 0x11,
            Code::Digit1 => 0x12,
            Code::Digit2 => 0x13,
            Code::Digit3 => 0x14,
            Code::Digit4 => 0x15,
            Code::Digit6 => 0x16,
            Code::Digit5 => 0x17,
            Code::Equal => 0x18,
            Code::Digit9 => 0x19,
            Code::Digit7 => 0x1a,
            Code::Minus => 0x1b,
            Code::Digit8 => 0x1c,
            Code::Digit0 => 0x1d,
            Code::BracketRight => 0x1e,
            Code::KeyO => 0x1f,
            Code::KeyU => 0x20,
            Code::BracketLeft => 0x21,
            Code::KeyI => 0x22,
            Code::KeyP => 0x23,
            Code::Enter => 0x24,
            Code::KeyL => 0x25,
            Code::KeyJ => 0x26,
            Code::Quote => 0x27,
            Code::KeyK => 0x28,
            Code::Semicolon => 0x29,
            Code::Backslash => 0x2a,
            Code::Comma => 0x2b,
            Code::Slash => 0x2c,
            Code::KeyN => 0x2d,
            Code::KeyM => 0x2e,
            Code::Period => 0x2f,
            Code::Tab => 0x30,
            Code::Space => 0x31,
            Code::Backquote => 0x32,
            Code::Backspace => 0x33,
            Code::Escape => 0x35,
            Code::F17 => 0x40,
            Code::NumpadDecimal => 0x41,
            Code::NumpadMultiply => 0x43,
            Code::NumpadAdd => 0x45,
            Code::NumpadDivide => 0x4b,
            Code::NumpadEnter => 0x4c,
            Code::NumpadSubtract => 0x4e,
            Code::F18 => 0x4f,
            Code::F19 => 0x50,
            Code::Numpad0 => 0x52,
            Code::Numpad1 => 0x53,
            Code::Numpad2 => 0x54,
            Code::Numpad3 => 0x55,
            Code::Numpad4 => 0x56,
            Code::Numpad5 => 0x57,
            Code::Numpad6 => 0x58,
            Code::Numpad7 => 0x59,
            Code::F20 => 0x5a,
            Code::Numpad8 => 0x5b,
            Code::Numpad9 => 0x5c,
            Code::F5 => 0x60,
            Code::F6 => 0x61,
            Code::F7 => 0x62,
            Code::F3 => 0x63,
            Code::F8 => 0x64,
            Code::F9 => 0x65,
            Code::F11 => 0x67,
            Code::F13 => 0x69,
            Code::F16 => 0x6a,
            Code::F14 => 0x6b,
            Code::F10 => 0x6d,
            Code::F12 => 0x6f,
            Code::F15 => 0x71,
            Code::Insert => 0x72,
            Code::Home => 0x73,
            Code::PageUp => 0x74,
            Code::Delete => 0x75,
            Code::F4 => 0x76,
            Code::End => 0x77,
            Code::F2 => 0x78,
            Code::PageDown => 0x79,
            Code::F1 => 0x7a,
            Code::ArrowLeft => 0x7b,
            Code::ArrowRight => 0x7c,
            Code::ArrowDown => 0x7d,
            Code::ArrowUp => 0x7e,
            _ => return None,
        })
    }
}

#[cfg(windows)]
mod win {
    use global_hotkey::hotkey::Code;
    use parking_lot::Mutex;
    use snap_core::t;
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    use super::{Accel, SystemConflict, cached};

    /// Settings > Accessibility > Keyboard, where "Use the Print screen key to open screen capture" lives.
    const SETTINGS_URL: &str = "ms-settings:easeofaccess-keyboard";

    static SNIPPING: Mutex<Option<(std::time::Instant, bool)>> = Mutex::new(None);

    pub fn conflict(a: Accel) -> Option<SystemConflict> {
        if a.key != Code::PrintScreen || !a.mods.is_empty() || !cached(&SNIPPING, print_screen_opens_snipping) {
            return None;
        }
        Some(SystemConflict {
            message: t("platform.hotkey.system.print_screen").into_owned(),
            open_label: t("platform.hotkey.system.windows_open").into_owned(),
            settings_url: SETTINGS_URL,
        })
    }

    /// The toggle is stored once the user flips it. Until then the build decides: Windows 11 22H2 and later turned
    /// it on by default in 2023 updates, earlier versions leave it off.
    fn print_screen_opens_snipping() -> bool {
        let stored: Option<u32> = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(r"Control Panel\Keyboard")
            .and_then(|k| k.get_value("PrintScreenKeyForSnippingEnabled"))
            .ok();
        stored.map_or_else(|| windows_build() >= 22621, |v| v != 0)
    }

    fn windows_build() -> u32 {
        RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
            .and_then(|k| k.get_value::<String, _>("CurrentBuildNumber"))
            .ok()
            .and_then(|b| b.trim().parse().ok())
            .unwrap_or(0)
    }
}
