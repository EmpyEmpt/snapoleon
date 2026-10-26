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
