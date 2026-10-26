//! Windows backend: GDI snapshots, Windows.Graphics.Capture, DXGI/DisplayConfig metadata, UI Automation.

mod com;
mod cursor;
mod desktop;
mod dpi;
mod gdi;
mod hdr;
mod input;
mod monitors;
mod process;
mod uia;
mod wgc;
mod window;

use snap_core::{Error, Point, Rect, Result};

use super::{CursorSprite, Grab, WindowGrab};
use crate::{CaptureOptions, ElementInfo, MonitorInfo, WindowInfo};

pub(crate) use desktop::IconsGuard;

pub(crate) fn init() {
    dpi::ensure();
}

pub(crate) fn monitors() -> Result<Vec<MonitorInfo>> {
    monitors::list()
}

pub(crate) fn windows() -> Result<Vec<WindowInfo>> {
    window::list()
}

pub(crate) fn foreground_window() -> Option<WindowInfo> {
    window::foreground()
}

pub(crate) fn cursor_position() -> Point {
    cursor::position()
}

pub(crate) fn cursor_sprite() -> Option<CursorSprite> {
    cursor::sprite()
}

/// `region` is inside `m.rect`, in virtual-desktop pixels.
pub(crate) fn grab_monitor(m: &MonitorInfo, region: Rect, o: &CaptureOptions) -> Result<Grab> {
    if m.hdr && o.hdr == snap_core::settings::HdrMode::Auto {
        match wgc::capture_monitor(m, region) {
            Ok(g) => return Ok(g),
            Err(e) => tracing::warn!("HDR capture failed, falling back to GDI: {e}"),
        }
    }
    Ok(Grab { image: gdi::grab(region)?, tone_mapped: false })
}

pub(crate) fn grab_window(id: u64, o: &CaptureOptions) -> Result<WindowGrab> {
    let hwnd = window::hwnd_from_id(id);
    let frame = window::visible_frame(hwnd).ok_or_else(|| Error::NotFound(format!("window {id}")))?;
    if window::is_minimized(hwnd) {
        return Err(Error::invalid("window is minimized"));
    }
    match wgc::capture_window(hwnd, frame, o) {
        Ok(g) => Ok(g),
        Err(e) => {
            tracing::warn!("Windows.Graphics.Capture failed ({e}); copying the window area from the screen");
            let bounds = monitors::virtual_bounds();
            let r = frame.intersect(bounds).ok_or_else(|| Error::invalid("window is off screen"))?;
            Ok(WindowGrab { image: gdi::grab(r)?, screen_rect: r, tone_mapped: false, shadow: false })
        }
    }
}

pub(crate) fn warm_up() {
    uia::warm_up();
}

pub(crate) fn elements_at(p: Point) -> Result<Vec<ElementInfo>> {
    uia::elements_at(p)
}

pub(crate) fn browser_url(w: &WindowInfo) -> Option<String> {
    uia::browser_url(w)
}

pub(crate) fn hide_desktop_icons() -> Option<IconsGuard> {
    desktop::hide()
}

pub(crate) fn scroll_at(p: Point, notches: i32) -> Result<()> {
    input::scroll_at(p, notches)
}

/// Windows asks no consent for screen capture, so no system prompt can be waiting.
pub(crate) fn system_alert_on_screen() -> bool {
    false
}

pub(crate) fn prime_capture_consent() {}

pub(super) fn rect_of(r: windows::Win32::Foundation::RECT) -> Rect {
    Rect::new(r.left, r.top, (r.right - r.left).max(0) as u32, (r.bottom - r.top).max(0) as u32)
}

/// UTF-16 buffer up to the first NUL.
pub(super) fn wide_to_string(w: &[u16]) -> String {
    String::from_utf16_lossy(&w[..w.iter().position(|&c| c == 0).unwrap_or(w.len())])
}

pub(super) fn os_err(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Other(anyhow::anyhow!("{what}: {e}"))
}
