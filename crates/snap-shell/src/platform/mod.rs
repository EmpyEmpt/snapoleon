//! Per-OS window tweaks that winit does not expose. Every backend provides the
//! same set of functions; the shell never branches on the OS itself.

use snap_core::Rect;
use winit::{
    dpi::{PhysicalPosition, PhysicalSize},
    event_loop::ActiveEventLoop,
    window::Window,
};

#[cfg(not(any(windows, target_os = "macos")))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

/// The app as a whole rather than one window: the app menu, OS requests (Apple Events), the activation policy and the
/// native chrome around a title bar the app draws. Only macOS has these; the other systems get the plain behaviour.
#[cfg(target_os = "macos")]
#[path = "macos_app.rs"]
pub(crate) mod app;

#[cfg(not(target_os = "macos"))]
pub(crate) mod app {
    use snap_core::{Point, Rect};
    use winit::{
        event_loop::ActiveEventLoop,
        monitor::MonitorHandle,
        window::{Window, WindowAttributes},
    };

    use super::NewWindowArea;
    use crate::{WindowSpec, proxy::AppHook};

    pub(crate) fn install(_sink: Box<dyn Fn(AppHook) -> bool>) {}

    pub(crate) fn start() -> Vec<AppHook> {
        Vec::new()
    }

    pub(crate) fn build_menu() {}

    pub(crate) fn set_regular(_regular: bool) {}

    pub(crate) fn allow_terminate() -> bool {
        false
    }

    pub(crate) fn chrome_attributes(attrs: WindowAttributes, _spec: &WindowSpec) -> WindowAttributes {
        attrs
    }

    pub(crate) fn layout_title_bar(_window: &Window, _height: f32) -> Option<f32> {
        None
    }

    pub(crate) fn title_bar_double_click(window: &Window) {
        window.set_maximized(!window.is_maximized());
    }

    /// The whole monitor: winit knows no work area here. The OS places the window.
    pub(crate) fn new_window_area(el: &ActiveEventLoop, at: Option<Point>) -> Option<NewWindowArea> {
        let monitor = at.and_then(|p| monitor_at(el, p)).or_else(|| el.primary_monitor()).or_else(|| el.available_monitors().next())?;
        let scale = monitor.scale_factor();
        let (p, s) = (monitor.position(), monitor.size());
        Some(NewWindowArea {
            x: f64::from(p.x) / scale,
            y: f64::from(p.y) / scale,
            w: f64::from(s.width) / scale,
            h: f64::from(s.height) / scale,
            centre: false,
        })
    }

    pub(crate) fn work_area(el: &ActiveEventLoop, at: Point) -> Option<Rect> {
        monitor_at(el, at).map(|m| monitor_rect(&m))
    }

    fn monitor_at(el: &ActiveEventLoop, p: Point) -> Option<MonitorHandle> {
        el.available_monitors().find(|m| monitor_rect(m).contains(p))
    }

    fn monitor_rect(m: &MonitorHandle) -> Rect {
        let (p, s) = (m.position(), m.size());
        Rect::new(p.x, p.y, s.width, s.height)
    }
}

/// Where a new app window may go, in logical pixels of winit's desktop coordinates.
pub(crate) struct NewWindowArea {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// Centre the window in the area when the spec gives no position; otherwise the OS places it.
    pub centre: bool,
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) use linux::*;
#[cfg(target_os = "macos")]
pub(crate) use macos::*;
#[cfg(windows)]
pub(crate) use windows::*;

/// Every monitor as winit reports it: physical rect in virtual-desktop space and scale factor.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(crate) fn winit_monitors(el: &ActiveEventLoop) -> Vec<(Rect, f32)> {
    el.available_monitors()
        .map(|m| {
            let (p, s) = (m.position(), m.size());
            (Rect::new(p.x, p.y, s.width, s.height), m.scale_factor() as f32)
        })
        .collect()
}

/// Window rect in physical virtual-desktop pixels, as winit reports it.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(crate) fn winit_outer_rect(window: &Window) -> Option<Rect> {
    let pos = window.outer_position().ok()?;
    let size = window.outer_size();
    Some(Rect::new(pos.x, pos.y, size.width, size.height))
}

/// Position and size a window through winit only (no native calls).
#[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
pub(crate) fn winit_set_outer_rect(window: &Window, r: Rect) {
    let outer = window.outer_size();
    let inner = window.inner_size();
    let frame_w = outer.width.saturating_sub(inner.width);
    let frame_h = outer.height.saturating_sub(inner.height);
    window.set_outer_position(PhysicalPosition::new(r.x, r.y));
    let _ = window.request_inner_size(PhysicalSize::new(r.w.saturating_sub(frame_w).max(1), r.h.saturating_sub(frame_h).max(1)));
}
