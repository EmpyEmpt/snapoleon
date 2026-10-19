use objc2::{MainThreadMarker, rc::Retained};
use objc2_app_kit::{
    NSApplication, NSFloatingWindowLevel, NSScreenSaverWindowLevel, NSView, NSWindow, NSWindowCollectionBehavior, NSWindowSharingType,
};
use snap_core::{Error, Rect, Result};
use winit::{
    dpi::{LogicalPosition, LogicalSize},
    event_loop::ActiveEventLoop,
    platform::macos::WindowAttributesExtMacOS,
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::{Window, WindowAttributes},
};

use crate::WindowSpec;

/// Logical-point rounding on mixed-DPI setups can be off by a pixel or two.
pub(crate) const COVER_TOLERANCE: i32 = 2;

pub(crate) fn window_attributes(_el: &ActiveEventLoop, attrs: WindowAttributes, spec: &WindowSpec, _app_id: &str) -> WindowAttributes {
    if spec.cover.is_some() { attrs.with_has_shadow(false).with_movable_by_window_background(false) } else { attrs }
}

fn ns_window(window: &Window) -> Option<Retained<NSWindow>> {
    let RawWindowHandle::AppKit(h) = window.window_handle().ok()?.as_raw() else {
        return None;
    };
    // SAFETY: winit keeps the NSView alive for as long as `window` lives, and
    // everything here runs on the main thread.
    unsafe { h.ns_view.cast::<NSView>().as_ref() }.window()
}

/// Level and Spaces behaviour for overlays and tool windows, applied while hidden.
pub(crate) fn configure_window(window: &Window, spec: &WindowSpec) {
    let Some(ns) = ns_window(window) else { return };
    if spec.exclude_from_capture {
        ns.setSharingType(NSWindowSharingType::None);
    }
    if spec.is_tool() {
        let mut behavior = NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::IgnoresCycle;
        if spec.cover.is_some() {
            behavior |= NSWindowCollectionBehavior::Stationary;
        }
        ns.setCollectionBehavior(behavior);
        ns.setHidesOnDeactivate(false);
        ns.setLevel(NSFloatingWindowLevel);
    }
    if spec.cover.is_some() {
        // Above the menu bar and the Dock, so AppKit does not push the frame below the menu bar.
        ns.setLevel(NSScreenSaverWindowLevel);
        ns.setHasShadow(false);
    }
}

/// AppKit windows keep their rectangle; only Windows clips to a pill.
pub(crate) fn set_pill(_window: &Window, _on: bool) {}

pub(crate) fn refresh(_window: &Window, _tool_style: bool) {}

pub(crate) fn set_exclude_from_capture(window: &Window, on: bool) -> Result<()> {
    let ns = ns_window(window).ok_or_else(|| Error::unsupported("window has no NSWindow"))?;
    ns.setSharingType(if on { NSWindowSharingType::None } else { NSWindowSharingType::ReadOnly });
    Ok(())
}

/// winit positions in logical points, but covers are specified in physical
/// pixels of the monitor they sit on, so convert with that monitor's scale.
pub(crate) fn place_cover(window: &Window, r: Rect, monitors: &[(Rect, f32)]) {
    let scale = monitors.iter().find(|(m, _)| m.contains(r.origin())).map_or_else(|| window.scale_factor(), |(_, s)| f64::from(*s));
    window.set_outer_position(LogicalPosition::new(f64::from(r.x) / scale, f64::from(r.y) / scale));
    let _ = window.request_inner_size(LogicalSize::new(f64::from(r.w) / scale, f64::from(r.h) / scale));
}

pub(crate) fn set_outer_rect(window: &Window, r: Rect) {
    super::winit_set_outer_rect(window, r);
}

/// The `NSWindow.windowNumber`, which is what `CGWindowList` reports as the window id.
pub(crate) fn native_id(window: &Window) -> Option<u64> {
    ns_window(window).map(|ns| ns.windowNumber() as u64)
}

/// Accessory apps are never active on their own; bring the app forward so the window gets keys.
pub(crate) fn activate(_window: &Window) {
    if let Some(mtm) = MainThreadMarker::new() {
        #[allow(deprecated)]
        NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    }
}
