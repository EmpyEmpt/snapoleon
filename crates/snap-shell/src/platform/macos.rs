use std::cell::RefCell;

use objc2::{MainThreadMarker, available, rc::Retained};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSFloatingWindowLevel, NSRunningApplication, NSScreenSaverWindowLevel, NSView, NSWindow,
    NSWindowCollectionBehavior, NSWindowSharingType, NSWorkspace,
};
use snap_core::{Error, Point, Rect, Result};
use winit::{
    dpi::{LogicalPosition, LogicalSize},
    event_loop::ActiveEventLoop,
    platform::macos::WindowAttributesExtMacOS,
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::{Window, WindowAttributes},
};

use crate::{WindowSpec, shell::monitor_list};

/// Logical-point rounding on mixed-DPI setups can be off by a pixel or two.
pub(crate) const COVER_TOLERANCE: i32 = 2;

pub(crate) fn window_attributes(el: &ActiveEventLoop, attrs: WindowAttributes, spec: &WindowSpec, _app_id: &str) -> WindowAttributes {
    if spec.cover.is_some() {
        return attrs.with_has_shadow(false).with_movable_by_window_background(false);
    }
    // winit converts a physical position with the main screen's scale; the window belongs on the monitor that holds
    // the position, at that monitor's scale.
    let Some(p) = spec.position else { return attrs };
    let monitors = monitor_list(el);
    let main = monitors.first().map_or(1.0, |(_, s)| f64::from(*s));
    attrs.with_position(to_points(p, &monitors, main).0)
}

/// winit places windows in points. A physical virtual-desktop position is converted with the scale of the monitor
/// that holds it (`fallback` when none does); that scale is returned with it. Every placement goes through here.
fn to_points(p: Point, monitors: &[(Rect, f32)], fallback: f64) -> (LogicalPosition<f64>, f64) {
    let scale = monitors.iter().find(|(m, _)| m.contains(p)).map_or(fallback, |(_, s)| f64::from(*s));
    (LogicalPosition::new(f64::from(p.x) / scale, f64::from(p.y) / scale), scale)
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
        // Above the menu bar (24) and the Dock (20), so AppKit does not push the frame below the menu bar. System
        // alerts (UserNotificationCenter, the screen-capture and privacy prompts) sit at the modal panel level (8),
        // below both, so no level can cover the menu bar and leave them visible: the overlay checks for an alert
        // before it shows, and goes away when one takes the activation.
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

/// Covers are specified in physical pixels of the monitor they sit on, so convert with that monitor's scale.
pub(crate) fn place_cover(window: &Window, r: Rect, monitors: &[(Rect, f32)]) {
    let (origin, scale) = to_points(r.origin(), monitors, window.scale_factor());
    window.set_outer_position(origin);
    let _ = window.request_inner_size(LogicalSize::new(f64::from(r.w) / scale, f64::from(r.h) / scale));
}

/// As [`place_cover`], with the title bar (if any) taken off the inner size. winit alone would convert with the scale
/// of the screen the window is on now, which puts a pin bound for another monitor at the wrong spot and size.
pub(crate) fn set_outer_rect(window: &Window, r: Rect, monitors: &[(Rect, f32)]) {
    let now = window.scale_factor();
    let (origin, scale) = to_points(r.origin(), monitors, now);
    let outer = window.outer_size().to_logical::<f64>(now);
    let inner = window.inner_size().to_logical::<f64>(now);
    window.set_outer_position(origin);
    let width = (f64::from(r.w) / scale - (outer.width - inner.width)).max(1.0);
    let height = (f64::from(r.h) / scale - (outer.height - inner.height)).max(1.0);
    let _ = window.request_inner_size(LogicalSize::new(width, height));
}

/// The `NSWindow.windowNumber`, which is what `CGWindowList` reports as the window id.
pub(crate) fn native_id(window: &Window) -> Option<u64> {
    ns_window(window).map(|ns| ns.windowNumber() as u64)
}

/// winit shows a window with `makeKeyAndOrderFront`, which takes the keyboard from whichever window of ours has it. A
/// window that must not take it (a pin, a HUD, a count-down, the covers away from the pointer) is only ordered front.
pub(crate) fn show(window: &Window, key: bool) {
    match ns_window(window) {
        Some(ns) if !key => ns.orderFrontRegardless(),
        _ => window.set_visible(true),
    }
}

/// Accessory apps are never active on their own; bring the app forward so the window gets keys. The window is put in
/// front by itself too, so it is seen even when macOS declines the activation (it is cooperative since macOS 14).
pub(crate) fn activate(window: &Window) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    if let Some(ns) = ns_window(window) {
        ns.orderFrontRegardless();
    }
    let app = NSApplication::sharedApplication(mtm);
    if available!(macos = 14.0) {
        app.activate();
    } else {
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
    }
}

pub(crate) fn app_active(_windows: &mut dyn Iterator<Item = &Window>) -> bool {
    MainThreadMarker::new().is_some_and(|mtm| NSApplication::sharedApplication(mtm).isActive())
}

thread_local! {
    /// The app that was frontmost before ours took the keyboard; `None` when ours already was.
    static FRONTMOST: RefCell<Option<Retained<NSRunningApplication>>> = const { RefCell::new(None) };
}

pub(crate) fn remember_frontmost() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let before = if NSApplication::sharedApplication(mtm).isActive() { None } else { NSWorkspace::sharedWorkspace().frontmostApplication() };
    FRONTMOST.set(before);
}

pub(crate) fn restore_frontmost() {
    let Some(before) = FRONTMOST.take() else { return };
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    // The user has moved on to another app already (or a system alert took over): leave it there.
    if !app.isActive() {
        return;
    }
    if available!(macos = 14.0) {
        // Cooperative activation: the active app yields, then the other app asks for it.
        app.yieldActivationToApplication(&before);
        before.activateFromApplication_options(&NSRunningApplication::currentApplication(), NSApplicationActivationOptions::empty());
    } else {
        before.activateWithOptions(NSApplicationActivationOptions::empty());
    }
}

/// The notch: `safeAreaInsets.top` of the screen the window is on (macOS 12+, 0 on screens without one).
pub(crate) fn safe_area_top(window: &Window) -> f32 {
    ns_window(window).and_then(|ns| ns.screen()).map_or(0.0, |screen| screen.safeAreaInsets().top as f32)
}
