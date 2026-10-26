use std::cell::RefCell;

use objc2::{MainThreadMarker, available, rc::Retained};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSFloatingWindowLevel, NSRunningApplication, NSScreenSaverWindowLevel, NSView, NSWindow,
    NSWindowCollectionBehavior, NSWindowSharingType, NSWorkspace,
};
use snap_core::{Error, Point, Rect, Result, geom::PointLayout};
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

/// Global points of virtual-desktop pixel `p`, and the pixels per point there, from the layout every crate shares.
fn to_points(p: Point) -> Option<(f64, f64, f64)> {
    let layout = PointLayout::current();
    let d = layout.display_at(p)?;
    let (x, y) = d.to_points(f64::from(p.x), f64::from(p.y));
    Some((x, y, d.scale))
}

pub(crate) fn window_attributes(_el: &ActiveEventLoop, attrs: WindowAttributes, spec: &WindowSpec, _app_id: &str) -> WindowAttributes {
    // winit turns a physical position into points with the main screen's scale, which misplaces windows on a display
    // of another scale; hand it points from the shared layout instead.
    match (spec.cover, spec.position) {
        (Some(r), _) => {
            let attrs = attrs.with_has_shadow(false).with_movable_by_window_background(false);
            match to_points(r.origin()) {
                Some((x, y, s)) => attrs
                    .with_position(LogicalPosition::new(x, y))
                    .with_inner_size(LogicalSize::new(f64::from(r.w) / s, f64::from(r.h) / s)),
                None => attrs,
            }
        }
        (None, Some(p)) => match to_points(p) {
            Some((x, y, _)) => attrs.with_position(LogicalPosition::new(x, y)),
            None => attrs,
        },
        (None, None) => attrs,
    }
}

/// Every display as (rect, scale) in the shared layout, so the rects match `snap_capture::monitors()` also when the
/// displays have different scales (winit scales each origin by its own display's scale, so those rects overlap).
pub(crate) fn monitors(_el: &ActiveEventLoop) -> Vec<(Rect, f32)> {
    PointLayout::current().displays.iter().map(|d| (d.rect, d.scale as f32)).collect()
}

/// The window's outer rect in the shared layout, from its frame in points.
pub(crate) fn outer_rect(window: &Window) -> Option<Rect> {
    let scale = window.scale_factor();
    let pos = window.outer_position().ok()?.to_logical::<f64>(scale);
    let size = window.outer_size().to_logical::<f64>(scale);
    Some(PointLayout::current().rect_to_pixels(pos.x, pos.y, size.width, size.height))
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

/// Covers have no frame, so placing one is placing its outer rect.
pub(crate) fn place_cover(window: &Window, r: Rect) {
    set_outer_rect(window, r);
}

/// winit places windows in points; `r` is in the shared layout's pixels, converted through the display that holds
/// its top-left corner and at that display's scale.
pub(crate) fn set_outer_rect(window: &Window, r: Rect) {
    let (x, y, scale) = to_points(r.origin()).unwrap_or_else(|| {
        let s = window.scale_factor();
        (f64::from(r.x) / s, f64::from(r.y) / s, s)
    });
    let own = window.scale_factor();
    let (outer, inner) = (window.outer_size().to_logical::<f64>(own), window.inner_size().to_logical::<f64>(own));
    let (frame_w, frame_h) = (outer.width - inner.width, outer.height - inner.height);
    window.set_outer_position(LogicalPosition::new(x, y));
    let _ = window
        .request_inner_size(LogicalSize::new((f64::from(r.w) / scale - frame_w).max(1.0), (f64::from(r.h) / scale - frame_h).max(1.0)));
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
