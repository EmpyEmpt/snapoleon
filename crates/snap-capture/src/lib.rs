//! Screen, window and element capture.
//!
//! All integer geometry is in physical pixels of the virtual desktop (see `snap_core::geom`; on macOS laid out by
//! `snap_core::geom::PointLayout`), so mixed-DPI layouts need no scaling on the caller's side.
//!
//! Backends: Windows uses GDI for fast monitor snapshots, Windows.Graphics.Capture for windows and HDR
//! (tone-mapped to SDR) and UI Automation for elements; macOS uses ScreenCaptureKit (sRGB pixels) and the
//! Accessibility API; Linux builds on `xcap` and AT-SPI.

mod blit;
mod compose;
mod labels;
mod platform;
mod types;

use snap_core::{Capture, CaptureContext, CaptureKind, Error, Point, Rect, Result, RgbaImage};

pub use labels::{monitor_label, role_label};
pub use types::{CaptureOptions, ElementInfo, Frozen, MonitorInfo, WindowInfo};

/// Make the process per-monitor-v2 DPI aware (Windows; already-aware processes are left untouched) or connect it to
/// the window server (macOS). Every entry point calls this; it is also safe to call early from `main`. No-op on Linux.
pub fn ensure_dpi_aware() {
    platform::init();
}

/// Connected monitors, primary first, then left-to-right / top-to-bottom.
pub fn monitors() -> Result<Vec<MonitorInfo>> {
    ensure_dpi_aware();
    platform::monitors()
}

/// Visible top-level windows front to back. Excludes our own process, tool windows, cloaked/zero-size
/// windows and shell/desktop windows.
pub fn windows() -> Result<Vec<WindowInfo>> {
    ensure_dpi_aware();
    platform::windows()
}

/// The focused top-level window, when it is one of [`windows`].
pub fn foreground_window() -> Option<WindowInfo> {
    ensure_dpi_aware();
    platform::foreground_window()
}

/// Mouse position in virtual-desktop pixels.
pub fn cursor_position() -> Point {
    ensure_dpi_aware();
    platform::cursor_position()
}

pub fn capture_monitor(index: usize, o: &CaptureOptions) -> Result<Capture> {
    ensure_dpi_aware();
    let monitors = platform::monitors()?;
    let m = monitors.get(index).ok_or_else(|| Error::NotFound(format!("monitor {index}")))?;
    let mut cap = compose::capture_area(&monitors, m.rect, CaptureKind::Monitor, o)?;
    cap.context.monitor_name = Some(m.name.clone());
    Ok(cap)
}

/// Composite of all monitors in physical pixels.
pub fn capture_all(o: &CaptureOptions) -> Result<Capture> {
    ensure_dpi_aware();
    let monitors = platform::monitors()?;
    let bounds = monitors.iter().fold(Rect::default(), |acc, m| acc.union(m.rect));
    compose::capture_area(&monitors, bounds, CaptureKind::AllMonitors, o)
}

/// Any rectangle of the desktop; may span monitors.
pub fn capture_rect(r: Rect, o: &CaptureOptions) -> Result<Capture> {
    ensure_dpi_aware();
    if r.is_empty() {
        return Err(Error::invalid("empty capture rectangle"));
    }
    let monitors = platform::monitors()?;
    compose::capture_area(&monitors, r, CaptureKind::Region, o)
}

/// Only the pixels of `r` (the part that lies on a monitor), without the window and app context [`capture_rect`]
/// looks up: for grabbing one area again and again, as the frames of a scrolling capture do.
pub fn capture_rect_pixels(r: Rect, o: &CaptureOptions) -> Result<RgbaImage> {
    ensure_dpi_aware();
    if r.is_empty() {
        return Err(Error::invalid("empty capture rectangle"));
    }
    let monitors = platform::monitors()?;
    compose::area_pixels(&monitors, r, o)
}

/// One window, also when other windows cover it where the OS allows.
pub fn capture_window(id: u64, o: &CaptureOptions) -> Result<Capture> {
    ensure_dpi_aware();
    compose::capture_window(id, o)
}

/// Every monitor image plus the window list and cursor, taken as close together as possible.
pub fn freeze(o: &CaptureOptions) -> Result<Frozen> {
    ensure_dpi_aware();
    compose::freeze(o)
}

/// Prepare the accessibility backend in the background (Windows: starts the UI Automation worker) so
/// the first [`elements_at`] call is fast. Optional.
pub fn warm_up() {
    ensure_dpi_aware();
    platform::warm_up();
}

/// Accessibility elements under `p`, smallest first, each fully containing the previous one's area.
pub fn elements_at(p: Point) -> Result<Vec<ElementInfo>> {
    ensure_dpi_aware();
    platform::elements_at(p)
}

/// App name, title and (for browsers) the address bar URL of `w`.
pub fn context_for_window(w: &WindowInfo) -> CaptureContext {
    ensure_dpi_aware();
    compose::window_context(w, true)
}

/// Context of whatever window is under `p`.
pub fn context_at(p: Point) -> CaptureContext {
    ensure_dpi_aware();
    let Ok(windows) = platform::windows() else { return CaptureContext::default() };
    let monitor_name = platform::monitors().ok().and_then(|ms| ms.into_iter().find(|m| m.rect.contains(p)).map(|m| m.name));
    let mut cx = windows.iter().find(|w| !w.minimized && w.rect.contains(p)).map(|w| compose::window_context(w, true)).unwrap_or_default();
    cx.monitor_name = monitor_name;
    cx
}

/// Keeps desktop icons hidden until dropped.
#[must_use = "icons are restored when the guard is dropped"]
pub struct DesktopIconsGuard {
    _restore: platform::IconsGuard,
}

/// Hide the desktop icons on screen (Windows: the SHELLDLL_DefView list view). `None` when there is nothing to hide,
/// and always on macOS, where captures leave Finder's icons out themselves
/// ([`CaptureOptions::hide_desktop_icons`]), and on Linux, which has no common icon layer.
pub fn hide_desktop_icons() -> Option<DesktopIconsGuard> {
    ensure_dpi_aware();
    platform::hide_desktop_icons().map(|g| DesktopIconsGuard { _restore: g })
}

/// Whether a system prompt is on screen right now (macOS: the screen capture approval, a Gatekeeper or a password
/// prompt). A full-screen overlay must not cover one, or it can never be answered. Always `false` on Windows and
/// Linux.
pub fn system_alert_on_screen() -> bool {
    ensure_dpi_aware();
    platform::system_alert_on_screen()
}

/// Right after Screen Recording is granted (macOS), take a throwaway capture of a few pixels so that macOS asks its
/// recurring "bypass the private window picker" question now, while nothing of ours covers the prompt; then watch
/// [`system_alert_on_screen`]. Does nothing without the grant, and nothing on Windows and Linux. Blocks for the
/// capture (normally well under 100 ms, a few seconds at worst).
pub fn prime_capture_consent() {
    ensure_dpi_aware();
    platform::prime_capture_consent();
}

/// Scroll the wheel over `p`: positive `notches` scroll down (content moves up), negative scroll up. The
/// cursor is moved to `p`.
pub fn scroll_at(p: Point, notches: i32) -> Result<()> {
    ensure_dpi_aware();
    platform::scroll_at(p, notches)
}
