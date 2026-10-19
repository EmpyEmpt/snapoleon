//! Screen, window and element capture.
//!
//! All integer geometry is in physical pixels of the virtual desktop (see `snap_core::geom`), so mixed-DPI
//! layouts need no scaling on the caller's side.
//!
//! Backends: Windows uses GDI for fast monitor snapshots, Windows.Graphics.Capture for windows and HDR
//! (tone-mapped to SDR) and UI Automation for elements; macOS and Linux build on `xcap`.

mod blit;
mod compose;
mod labels;
mod platform;
mod types;

use snap_core::{Capture, CaptureContext, CaptureKind, Error, Point, Rect, Result};

pub use labels::{error_message, monitor_label, role_label};
pub use types::{CaptureOptions, ElementInfo, Frozen, MonitorInfo, WindowInfo};

/// Make the process per-monitor-v2 DPI aware (Windows). Every entry point calls this; it is also safe to
/// call early from `main`. Already-aware processes are left untouched. No-op on other OSes.
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

/// Hide the desktop icons (Windows: the SHELLDLL_DefView list view; macOS: Finder's desktop; Linux: not
/// supported, returns `None`). `None` also when there is nothing to hide.
pub fn hide_desktop_icons() -> Option<DesktopIconsGuard> {
    ensure_dpi_aware();
    platform::hide_desktop_icons().map(|g| DesktopIconsGuard { _restore: g })
}

/// Scroll the wheel over `p`: positive `notches` scroll down (content moves up), negative scroll up. The
/// cursor is moved to `p`.
pub fn scroll_at(p: Point, notches: i32) -> Result<()> {
    ensure_dpi_aware();
    platform::scroll_at(p, notches)
}
