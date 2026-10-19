//! Per-OS input plumbing for step capture: a global left-click listener and an "is this our own window" probe.
//! Every backend exposes `Hook::start` and `is_own_window_at`; the cfg-gated modules below pick one.

use snap_core::{Point, Result};

#[cfg(target_os = "macos")]
mod mac;
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod none;
#[cfg(windows)]
mod win;
#[cfg(target_os = "linux")]
mod x11;

#[cfg(target_os = "macos")]
use mac as imp;
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
use none as imp;
#[cfg(windows)]
use win as imp;
#[cfg(target_os = "linux")]
use x11 as imp;

/// Reports left mouse presses until dropped.
pub(crate) struct ClickListener {
    _hook: imp::Hook,
}

/// Start listening. `on_click` runs on the listener's own thread *before* the click is delivered to its target on
/// Windows (it must return quickly), and just after on the other systems. Points are physical virtual-desktop pixels.
///
/// Errors: `Unsupported` where no backend exists, `Permission` when the OS blocks input monitoring.
pub(crate) fn listen_clicks(on_click: impl Fn(Point) + Send + 'static) -> Result<ClickListener> {
    Ok(ClickListener { _hook: imp::Hook::start(Box::new(on_click))? })
}

/// Whether the top-level window under `p` belongs to this process (our own HUD must not be captured as a step).
pub(crate) fn is_own_window_at(p: Point) -> bool {
    imp::is_own_window_at(p)
}
