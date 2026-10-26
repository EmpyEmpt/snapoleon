use snap_core::{Error, Rect, Result};
use winit::{
    dpi::{PhysicalPosition, PhysicalSize},
    event_loop::ActiveEventLoop,
    monitor::MonitorHandle,
    platform::{
        wayland::ActiveEventLoopExtWayland,
        x11::{WindowAttributesExtX11, WindowType},
    },
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::{Fullscreen, Window, WindowAttributes, WindowLevel},
};

use crate::WindowSpec;

/// Window managers may round or clamp placements by a pixel.
pub(crate) const COVER_TOLERANCE: i32 = 1;

pub(crate) fn window_attributes(el: &ActiveEventLoop, mut attrs: WindowAttributes, spec: &WindowSpec, app_id: &str) -> WindowAttributes {
    attrs = attrs.with_name(app_id, app_id);
    if spec.is_tool() {
        attrs = attrs.with_x11_window_type(vec![WindowType::Utility]);
    }
    if let Some(cover) = spec.cover {
        attrs = attrs.with_window_level(WindowLevel::AlwaysOnTop);
        // Wayland cannot place toplevels; the closest thing to an exact cover
        // is fullscreen on the monitor that has exactly that rect.
        if el.is_wayland()
            && let Some(monitor) = el.available_monitors().find(|m| monitor_rect(m) == cover)
        {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(Some(monitor))));
        }
    }
    attrs
}

fn monitor_rect(m: &MonitorHandle) -> Rect {
    let (p, s) = (m.position(), m.size());
    Rect::new(p.x, p.y, s.width, s.height)
}

pub(crate) fn configure_window(_window: &Window, spec: &WindowSpec) {
    if spec.exclude_from_capture {
        tracing::debug!("exclude_from_capture is not supported on this platform");
    }
}

/// X11 and Wayland have no portable window shape; the window stays a rectangle.
pub(crate) fn set_pill(_window: &Window, _on: bool) {}

pub(crate) fn refresh(_window: &Window, _tool_style: bool) {}

pub(crate) fn set_exclude_from_capture(_window: &Window, _on: bool) -> Result<()> {
    Err(Error::unsupported("excluding a window from screen capture"))
}

pub(crate) fn place_cover(window: &Window, r: Rect, _monitors: &[(Rect, f32)]) {
    if window.fullscreen().is_none() {
        winit_set_outer_rect(window, r);
    }
}

pub(crate) fn set_outer_rect(window: &Window, r: Rect, _monitors: &[(Rect, f32)]) {
    winit_set_outer_rect(window, r);
}

/// Position and size a window through winit only (no native calls).
fn winit_set_outer_rect(window: &Window, r: Rect) {
    let outer = window.outer_size();
    let inner = window.inner_size();
    let frame_w = outer.width.saturating_sub(inner.width);
    let frame_h = outer.height.saturating_sub(inner.height);
    window.set_outer_position(PhysicalPosition::new(r.x, r.y));
    let _ = window.request_inner_size(PhysicalSize::new(r.w.saturating_sub(frame_w).max(1), r.h.saturating_sub(frame_h).max(1)));
}

#[allow(clippy::unnecessary_cast)] // `c_ulong` is 32-bit on 32-bit targets
pub(crate) fn native_id(window: &Window) -> Option<u64> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Xlib(h) => Some(h.window as u64),
        RawWindowHandle::Xcb(h) => Some(u64::from(h.window.get())),
        _ => None,
    }
}

pub(crate) fn activate(_window: &Window) {}

pub(crate) fn show(window: &Window, _key: bool) {
    window.set_visible(true);
}

pub(crate) fn app_active(windows: &mut dyn Iterator<Item = &Window>) -> bool {
    windows.any(Window::has_focus)
}

/// The OS gives the focus back to the window below when ours goes away.
pub(crate) fn remember_frontmost() {}

pub(crate) fn restore_frontmost() {}

pub(crate) fn safe_area_top(_window: &Window) -> f32 {
    0.0
}
