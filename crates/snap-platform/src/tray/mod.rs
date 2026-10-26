//! System tray icon with a nested menu.
//!
//! Windows and macOS use `tray-icon` (+ `muda`); Linux speaks StatusNotifierItem
//! over D-Bus through `ksni` on its own thread, so no GTK is needed.
//!
//! On Windows and macOS the tray must be created on the main/UI thread after
//! the event loop has started, and menu/click events are delivered on that same
//! thread. On Linux the callback runs on the D-Bus service thread. In all cases
//! keep the callback light: forward the event into the app's own channel.

use std::sync::Arc;

use image::RgbaImage;
use snap_core::Result;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(any(windows, target_os = "macos"))]
mod native;

#[cfg(target_os = "linux")]
use linux as backend;
#[cfg(any(windows, target_os = "macos"))]
use native as backend;

/// One entry of the tray menu.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TrayItem {
    /// Reported back in [`TrayEvent::Menu`] when the item is activated.
    pub id: String,
    pub label: String,
    pub enabled: bool,
    /// `Some` renders a checkmark item.
    pub checked: Option<bool>,
    /// Non-empty turns the item into a submenu (nesting is unlimited).
    pub children: Vec<TrayItem>,
    pub separator: bool,
    /// The item's global shortcut as an accelerator string (`Cmd+Shift+2`), shown beside it: as the key equivalent
    /// on macOS, in the shortcut column on Windows. Linux trays do not show it.
    pub accel: Option<String>,
}

impl TrayItem {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self { id: id.into(), label: label.into(), enabled: true, ..Self::default() }
    }

    pub fn separator() -> Self {
        Self { separator: true, enabled: true, ..Self::default() }
    }

    pub fn submenu(label: impl Into<String>, children: Vec<TrayItem>) -> Self {
        Self { label: label.into(), enabled: true, children, ..Self::default() }
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    pub fn accel(mut self, accel: Option<String>) -> Self {
        self.accel = accel;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayEvent {
    /// Primary click on the icon. Not on macOS, where any click opens the menu.
    Click,
    /// Double click (Windows). On Linux, where StatusNotifierItem has no double
    /// click, a middle click is reported instead.
    DoubleClick,
    /// A menu item was activated; carries [`TrayItem::id`].
    Menu(String),
}

pub(crate) type Callback = Arc<dyn Fn(TrayEvent) + Send + Sync>;

/// A live tray icon; dropping it removes the icon.
pub struct Tray {
    inner: backend::Backend,
}

impl Tray {
    /// On macOS the icon is drawn as a template image: only its alpha counts, and the menu bar colours it.
    pub fn new(
        tooltip: &str,
        icon: &RgbaImage,
        items: Vec<TrayItem>,
        on_event: impl Fn(TrayEvent) + Send + Sync + 'static,
    ) -> Result<Self> {
        Ok(Self { inner: backend::Backend::new(tooltip, icon, items, Arc::new(on_event))? })
    }

    pub fn set_items(&mut self, items: Vec<TrayItem>) {
        self.inner.set_items(items);
    }

    pub fn set_icon(&mut self, icon: &RgbaImage) {
        self.inner.set_icon(icon);
    }

    /// Show that a recording runs. macOS tints the template icon red; elsewhere the app draws the red dot into the
    /// icon itself.
    pub fn set_recording(&mut self, recording: bool) {
        #[cfg(target_os = "macos")]
        self.inner.set_recording(recording);
        #[cfg(not(target_os = "macos"))]
        let _ = recording;
    }

    pub fn set_tooltip(&mut self, tooltip: &str) {
        self.inner.set_tooltip(tooltip);
    }

    /// Draw the eye to the icon for a moment, after the app's last window closed and it lives on in the menu bar only.
    /// macOS blinks the status item a few times; other trays have no such call, so nothing happens there.
    pub fn flash(&mut self) {
        #[cfg(target_os = "macos")]
        self.inner.flash();
    }
}
