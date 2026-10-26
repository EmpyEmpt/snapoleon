//! Per-OS window tweaks that winit does not expose. Every backend provides the
//! same set of functions; the shell never branches on the OS itself.

use snap_core::Rect;
use winit::window::Window;

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

/// Window rect in physical virtual-desktop pixels, as the OS reports it.
pub(crate) fn outer_rect(window: &Window) -> Option<Rect> {
    let pos = window.outer_position().ok()?;
    let size = window.outer_size();
    Some(Rect::new(pos.x, pos.y, size.width, size.height))
}
