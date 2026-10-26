//! Backend selection. Every backend exposes the same crate-private surface:
//! `init`, `warm_up`, `monitors`, `windows`, `cursor_position`, `cursor_sprite`, `grab_monitor`, `grab_window`,
//! `elements_at`, `browser_url`, `hide_desktop_icons` (+ `IconsGuard`), `scroll_at`, `system_alert_on_screen` and
//! `prime_capture_consent`.

use snap_core::{Point, Rect, RgbaImage};

/// Pixels of one monitor region.
pub(crate) struct Grab {
    pub image: RgbaImage,
    pub tone_mapped: bool,
}

/// A captured window plus where its top-left corner sat on the virtual desktop.
pub(crate) struct WindowGrab {
    pub image: RgbaImage,
    pub screen_rect: Rect,
    pub tone_mapped: bool,
    /// The image already carries the system's own drop shadow (macOS 26 and later), so none is added.
    pub shadow: bool,
}

/// Mouse cursor image with the virtual-desktop position of its top-left pixel.
pub(crate) struct CursorSprite {
    pub image: RgbaImage,
    pub origin: Point,
}

#[cfg(windows)]
mod win;
#[cfg(windows)]
pub(crate) use win::*;

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
mod mac_sck;
#[cfg(target_os = "macos")]
pub(crate) use mac::{IconsGuard, browser_url, elements_at, hide_desktop_icons, scroll_at};
#[cfg(target_os = "macos")]
pub(crate) use mac_sck::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod xcap_backend;
#[cfg(target_os = "linux")]
pub(crate) use linux::{IconsGuard, browser_url, elements_at, hide_desktop_icons, scroll_at};
#[cfg(target_os = "linux")]
pub(crate) use xcap_backend::*;

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
compile_error!("snap-capture supports Windows, macOS and Linux only");
