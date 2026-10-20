//! macOS extras next to the xcap backend: permissions, Accessibility (AXUIElement) elements and browser
//! URLs, pointer position, event injection and Finder desktop icons.

use std::{
    ffi::c_void,
    process::Command,
    ptr,
    sync::{Mutex, MutexGuard, PoisonError},
    thread,
    time::{Duration, Instant},
};

use core_foundation::{
    array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef},
    base::{CFType, CFTypeRef, TCFType},
    string::{CFString, CFStringRef},
    url::CFURL,
};
use core_graphics::{
    event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton, ScrollEventUnit},
    event_source::{CGEventSource, CGEventSourceStateID},
    geometry::{CGPoint, CGSize},
};
use snap_core::{Error, Point, Rect, Result, RgbaImage};

use super::xcap_backend::{native_rect_to_physical, to_native, windows};
use crate::{ElementInfo, WindowInfo, labels::permission_error};

type AxElementRef = *const c_void;

const AX_VALUE_CGPOINT: u32 = 1;
const AX_VALUE_CGSIZE: u32 = 2;
const MAX_DEPTH: usize = 48;
const QUERY_TIMEOUT: Duration = Duration::from_millis(150);
const URL_TIMEOUT: Duration = Duration::from_millis(300);
const BROWSERS: &[&str] = &["chrome", "safari", "firefox", "brave", "edge", "opera", "vivaldi", "arc", "yandex", "chromium", "orion"];

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXUIElementCreateSystemWide() -> AxElementRef;
    fn AXUIElementCreateApplication(pid: i32) -> AxElementRef;
    fn AXUIElementCopyElementAtPosition(app: AxElementRef, x: f32, y: f32, out: *mut AxElementRef) -> i32;
    fn AXUIElementCopyAttributeValue(el: AxElementRef, attr: CFStringRef, out: *mut CFTypeRef) -> i32;
    fn AXUIElementGetPid(el: AxElementRef, pid: *mut i32) -> i32;
    fn AXUIElementSetMessagingTimeout(el: AxElementRef, seconds: f32) -> i32;
    fn AXValueGetValue(value: CFTypeRef, kind: u32, out: *mut c_void) -> bool;
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn proc_pidpath(pid: i32, buffer: *mut c_void, size: u32) -> i32;
}

/// Pointer position in points (global, top-left origin).
pub fn pointer() -> Option<(f64, f64)> {
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState).ok()?;
    let p = CGEvent::new(source).ok()?.location();
    Some((p.x, p.y))
}

pub fn exe_path(pid: u32) -> Option<String> {
    let mut buf = vec![0u8; 4096];
    let n = unsafe { proc_pidpath(pid as i32, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
    (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
}

/// Screen Recording permission, required for any pixel (and window title) access.
pub fn check_capture_access() -> Result<()> {
    if unsafe { CGPreflightScreenCaptureAccess() } { Ok(()) } else { Err(permission_error("capture.permission.screen_recording")) }
}

/// `CGWindowListCreateImage` includes the window shadow; cut the image back to the opaque window body.
pub fn trim_window_frame(img: RgbaImage, expected: Rect) -> RgbaImage {
    if img.width() <= expected.w + 2 && img.height() <= expected.h + 2 {
        return img;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, px) in img.enumerate_pixels() {
        if px[3] >= 200 {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
    }
    if x0 == u32::MAX {
        return img;
    }
    let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
    let plausible = w * 10 >= expected.w * 9 && h * 10 >= expected.h * 9 && (w < img.width() || h < img.height());
    if plausible { image::imageops::crop_imm(&img, x0, y0, w, h).to_image() } else { img }
}

fn owned(raw: CFTypeRef) -> Option<CFType> {
    (!raw.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(raw) })
}

fn attr(el: &CFType, name: &str) -> Option<CFType> {
    let key = CFString::new(name);
    let mut out: CFTypeRef = ptr::null();
    let err = unsafe { AXUIElementCopyAttributeValue(el.as_CFTypeRef(), key.as_concrete_TypeRef(), &mut out) };
    if err == 0 { owned(out) } else { None }
}

fn string_attr(el: &CFType, name: &str) -> Option<String> {
    attr(el, name)?.downcast::<CFString>().map(|s| s.to_string()).filter(|s| !s.is_empty())
}

fn children(el: &CFType, name: &str) -> Vec<CFType> {
    let Some(list) = attr(el, name) else { return Vec::new() };
    let raw = list.as_CFTypeRef() as CFArrayRef;
    let n = unsafe { CFArrayGetCount(raw) };
    (0..n)
        .filter_map(|i| {
            let item = unsafe { CFArrayGetValueAtIndex(raw, i) } as CFTypeRef;
            // The array owns its items; take our own reference.
            (!item.is_null()).then(|| unsafe { CFType::wrap_under_get_rule(item) })
        })
        .collect()
}

/// Frame in points: x, y, width, height.
fn frame(el: &CFType) -> Option<[f64; 4]> {
    let (pos, size) = (attr(el, "AXPosition")?, attr(el, "AXSize")?);
    let (mut p, mut s) = (CGPoint::new(0.0, 0.0), CGSize::new(0.0, 0.0));
    let ok = unsafe {
        AXValueGetValue(pos.as_CFTypeRef(), AX_VALUE_CGPOINT, &mut p as *mut _ as *mut c_void)
            && AXValueGetValue(size.as_CFTypeRef(), AX_VALUE_CGSIZE, &mut s as *mut _ as *mut c_void)
    };
    ok.then_some([p.x, p.y, s.width, s.height])
}

/// "AXStaticText" becomes "static text".
fn role_name(role: &str) -> String {
    let mut out = String::new();
    for (i, c) in role.strip_prefix("AX").unwrap_or(role).chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push(' ');
        }
        out.extend(c.to_lowercase());
    }
    if out.is_empty() { "element".into() } else { out }
}

fn require_trust() -> Result<()> {
    if unsafe { AXIsProcessTrusted() } { Ok(()) } else { Err(permission_error("capture.permission.accessibility")) }
}

pub fn elements_at(p: Point) -> Result<Vec<ElementInfo>> {
    require_trust()?;
    let (x, y) = to_native(p);
    let own_pid = std::process::id();
    let windows = windows()?;
    let under = windows.iter().find(|w| !w.minimized && w.rect.contains(p));

    let system =
        owned(unsafe { AXUIElementCreateSystemWide() } as CFTypeRef).ok_or_else(|| Error::Other(anyhow::anyhow!("AX system element")))?;
    unsafe { AXUIElementSetMessagingTimeout(system.as_CFTypeRef(), QUERY_TIMEOUT.as_secs_f32()) };
    let mut hit = element_at(&system, x, y);
    // Our overlay covers the screen: ask the application underneath instead.
    let is_ours = hit.as_ref().is_some_and(|h| pid_of(h) == Some(own_pid as i32));
    if (hit.is_none() || is_ours)
        && let Some(w) = under
        && let Some(app) = owned(unsafe { AXUIElementCreateApplication(w.pid as i32) } as CFTypeRef)
    {
        unsafe { AXUIElementSetMessagingTimeout(app.as_CFTypeRef(), QUERY_TIMEOUT.as_secs_f32()) };
        hit = element_at(&app, x, y);
    }
    let leaf = hit.ok_or_else(|| Error::NotFound("no accessible element at point".into()))?;

    let mut out: Vec<ElementInfo> = Vec::new();
    let mut cur = Some(leaf);
    for _ in 0..MAX_DEPTH {
        let Some(el) = cur else { break };
        let role = string_attr(&el, "AXRole").unwrap_or_default();
        if role == "AXApplication" {
            break;
        }
        if let Some([fx, fy, fw, fh]) = frame(&el) {
            let rect = native_rect_to_physical(fx, fy, fw, fh);
            let name = string_attr(&el, "AXTitle")
                .or_else(|| string_attr(&el, "AXDescription"))
                .or_else(|| string_attr(&el, "AXValue"))
                .unwrap_or_default();
            let owner = pid_of(&el).and_then(|pid| windows.iter().find(|w| w.pid as i32 == pid));
            let duplicate = out.last_mut().is_some_and(|prev| {
                let same = prev.rect == rect;
                if same && prev.name.is_empty() {
                    prev.name = name.clone();
                }
                same
            });
            if !rect.is_empty() && !duplicate {
                out.push(ElementInfo {
                    rect,
                    name,
                    role: role_name(&role),
                    app_name: owner.map(|w| w.app_name.clone()).filter(|n| !n.is_empty()),
                    window_id: under.filter(|w| owner.is_none_or(|o| o.pid == w.pid)).map(|w| w.id),
                });
            }
        }
        cur = attr(&el, "AXParent");
    }
    Ok(out)
}

fn element_at(parent: &CFType, x: f64, y: f64) -> Option<CFType> {
    let mut out: AxElementRef = ptr::null();
    let err = unsafe { AXUIElementCopyElementAtPosition(parent.as_CFTypeRef() as AxElementRef, x as f32, y as f32, &mut out) };
    if err == 0 { owned(out as CFTypeRef) } else { None }
}

fn pid_of(el: &CFType) -> Option<i32> {
    let mut pid = 0;
    (unsafe { AXUIElementGetPid(el.as_CFTypeRef() as AxElementRef, &mut pid) } == 0).then_some(pid)
}

/// Address-bar URL of a browser window: the `AXURL` of its web area.
pub fn browser_url(w: &WindowInfo) -> Option<String> {
    let name = w.app_name.to_lowercase();
    if !BROWSERS.iter().any(|b| name.contains(b)) || !unsafe { AXIsProcessTrusted() } {
        return None;
    }
    let app = owned(unsafe { AXUIElementCreateApplication(w.pid as i32) } as CFTypeRef)?;
    unsafe { AXUIElementSetMessagingTimeout(app.as_CFTypeRef(), QUERY_TIMEOUT.as_secs_f32()) };
    let mut windows = children(&app, "AXWindows");
    // Prefer the window with the matching title.
    if let Some(i) = windows.iter().position(|el| string_attr(el, "AXTitle").as_deref() == Some(w.title.as_str())) {
        windows.swap(0, i);
    }

    let deadline = Instant::now() + URL_TIMEOUT;
    let mut queue: std::collections::VecDeque<(CFType, usize)> = windows.into_iter().take(1).map(|el| (el, 0)).collect();
    let mut visited = 0;
    while let Some((el, depth)) = queue.pop_front() {
        if Instant::now() > deadline || visited > 600 {
            break;
        }
        visited += 1;
        if string_attr(&el, "AXRole").as_deref() == Some("AXWebArea")
            && let Some(url) = attr(&el, "AXURL").and_then(|u| u.downcast::<CFURL>())
        {
            return Some(url.get_string().to_string());
        }
        if depth < 12 {
            queue.extend(children(&el, "AXChildren").into_iter().map(|c| (c, depth + 1)));
        }
    }
    None
}

/// Hiding the icons rewrites Finder's `CreateDesktop` and restarts Finder, which flickers the desktop and reloads its
/// windows. Captures follow each other, so the hide is shared by everyone who holds a guard and the restore waits
/// [`RESTORE_GRACE`] after the last guard is gone: a run of captures hides and restores once, and dropping a guard
/// never blocks.
struct Icons {
    /// The value `CreateDesktop` had before we hid the icons (`None` = unset, shown); the outer `None` when not hidden.
    hidden: Option<Option<bool>>,
    holders: usize,
    /// Bumped by every hide and release, so a postponed restore can tell it was overtaken.
    epoch: u64,
}

static ICONS: Mutex<Icons> = Mutex::new(Icons { hidden: None, holders: 0, epoch: 0 });
const RESTORE_GRACE: Duration = Duration::from_secs(4);

fn icons() -> MutexGuard<'static, Icons> {
    ICONS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Keeps the desktop icons hidden until dropped (and then for a short while longer, see [`Icons`]).
pub struct IconsGuard;

fn defaults(args: &[&str]) -> Option<String> {
    let out = Command::new("defaults").args(args).output().ok().filter(|o| o.status.success())?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn restart_finder() {
    let _ = Command::new("killall").arg("Finder").status();
}

fn restore(original: Option<bool>) {
    let _ = match original {
        Some(v) => Command::new("defaults")
            .args(["write", "com.apple.finder", "CreateDesktop", "-bool", if v { "true" } else { "false" }])
            .status(),
        None => Command::new("defaults").args(["delete", "com.apple.finder", "CreateDesktop"]).status(),
    };
    restart_finder();
}

/// Slow (process spawns and a Finder restart) the first time: call it off the UI thread.
pub fn hide_desktop_icons() -> Option<IconsGuard> {
    let mut icons = icons();
    if icons.hidden.is_none() {
        let original = defaults(&["read", "com.apple.finder", "CreateDesktop"]).map(|v| !matches!(v.as_str(), "0" | "false"));
        if original == Some(false) {
            return None;
        }
        Command::new("defaults")
            .args(["write", "com.apple.finder", "CreateDesktop", "-bool", "false"])
            .status()
            .ok()
            .filter(|s| s.success())?;
        restart_finder();
        icons.hidden = Some(original);
    }
    icons.holders += 1;
    icons.epoch += 1;
    Some(IconsGuard)
}

/// Whether the icons are hidden at the moment, so that taking a guard is instant.
pub fn desktop_icons_hidden() -> bool {
    icons().hidden.is_some()
}

/// Put the icons back now, whoever still holds a guard. For shutdown, when no postponed restore would get to run.
pub fn restore_desktop_icons() {
    let mut icons = icons();
    if let Some(original) = icons.hidden.take() {
        icons.epoch += 1;
        restore(original);
    }
}

impl Drop for IconsGuard {
    fn drop(&mut self) {
        let epoch = {
            let mut icons = icons();
            icons.holders -= 1;
            icons.epoch += 1;
            if icons.holders > 0 {
                return;
            }
            icons.epoch
        };
        let later = thread::Builder::new().name("snap-icons".into()).spawn(move || {
            thread::sleep(RESTORE_GRACE);
            let mut icons = icons();
            if icons.holders == 0
                && icons.epoch == epoch
                && let Some(original) = icons.hidden.take()
            {
                restore(original);
            }
        });
        if later.is_err() {
            restore_desktop_icons();
        }
    }
}

/// Move the pointer to `p` and scroll `notches` lines (positive = down).
pub fn scroll_at(p: Point, notches: i32) -> Result<()> {
    if notches == 0 {
        return Ok(());
    }
    require_trust()?;
    let source = || CGEventSource::new(CGEventSourceStateID::HIDSystemState).map_err(|_| Error::Other(anyhow::anyhow!("CGEventSource")));
    let (x, y) = to_native(p);
    let moved = CGEvent::new_mouse_event(source()?, CGEventType::MouseMoved, CGPoint::new(x, y), CGMouseButton::Left)
        .map_err(|_| Error::Other(anyhow::anyhow!("mouse move event")))?;
    moved.post(CGEventTapLocation::HID);
    thread::sleep(Duration::from_millis(10));
    for _ in 0..notches.unsigned_abs() {
        let wheel = CGEvent::new_scroll_event(source()?, ScrollEventUnit::LINE, 1, -notches.signum(), 0, 0)
            .map_err(|_| Error::Other(anyhow::anyhow!("scroll event")))?;
        wheel.post(CGEventTapLocation::HID);
    }
    Ok(())
}
