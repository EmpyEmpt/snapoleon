//! macOS capture: ScreenCaptureKit for pixels (`CGWindowListCreateImage` on macOS 12.3 and 13, which have no
//! ScreenCaptureKit screenshot call), the CoreGraphics window list for windows, and `snap_core::geom::PointLayout`
//! for every conversion between the points macOS works in and virtual-desktop pixels.
//!
//! Pixels always come out in sRGB: captures arrive in the display's colour profile (Color LCD, Display P3), while the
//! overlay and every encoder treat bytes as sRGB.

use std::{
    cell::RefCell,
    path::Path,
    sync::Once,
    time::{Duration, Instant},
};

use block2::{DynBlock, RcBlock};
use objc2::{AllocAnyThread, MainThreadMarker, available, rc::Retained};
use objc2_app_kit::{NSScreen, NSWorkspace};
use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGDisplayCopyDisplayMode, CGDisplayMode, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, CGMainDisplayID, CGRectMakeWithDictionaryRepresentation, CGRectNull, CGWindowID, CGWindowImageOption,
    CGWindowListCopyWindowInfo, CGWindowListOption, kCGColorSpaceSRGB, kCGMainMenuWindowLevel, kCGNormalWindowLevel, kCGNullWindowID,
    kCGPopUpMenuWindowLevel, kCGWindowAlpha, kCGWindowBounds, kCGWindowLayer, kCGWindowName, kCGWindowNumber, kCGWindowOwnerName,
    kCGWindowOwnerPID, kCGWindowSharingState,
};
use objc2_foundation::{NSArray, NSError, NSNumber, ns_string};
use objc2_screen_capture_kit::{
    SCCaptureResolutionType, SCContentFilter, SCScreenshotConfiguration, SCScreenshotDynamicRange, SCScreenshotManager, SCScreenshotOutput,
    SCShareableContent, SCStreamConfiguration, SCWindow,
};
use parking_lot::Mutex;
use snap_core::{Error, Point, Rect, Result, RgbaImage, geom::PointLayout, t, tf};

use super::{CursorSprite, Grab, WindowGrab, mac};
use crate::{CaptureOptions, MonitorInfo, WindowInfo, blit, labels::permission_error};

/// `SCStreamErrorUserDeclined`: Screen Recording is not granted.
const USER_DECLINED: isize = -3801;
/// How long one ScreenCaptureKit request may take. A capture normally answers in tens of milliseconds.
const SCK_TIMEOUT: Duration = Duration::from_secs(5);
/// Display content is reused this long, so the monitors of one freeze and the frames of a scroll do not each pay
/// for a fresh content query (about 30 ms).
const CONTENT_TTL: Duration = Duration::from_secs(1);

/// System surfaces that are never useful capture targets, by owner name.
const SHELL_APPS: &[&str] =
    &["Window Server", "Dock", "SystemUIServer", "Control Center", "Notification Center", "Spotlight", "WindowManager"];

/// Processes that put up system prompts: UserNotificationCenter (the screen capture approval), CoreServicesUIAgent
/// (Gatekeeper and other consent prompts) and SecurityAgent (password prompts).
const ALERT_PROCESSES: &[&str] = &["UserNotificationCenter", "CoreServicesUIAgent", "SecurityAgent"];

pub(crate) fn init() {
    static SESSION: Once = Once::new();
    // Connects the process to the window server. A process that has not done that yet (the CLI, the MCP server)
    // trips an assertion inside SCScreenshotConfiguration.
    SESSION.call_once(|| {
        CGMainDisplayID();
    });
}

pub(crate) fn warm_up() {}

fn failed(reason: &str) -> Error {
    Error::Other(anyhow::anyhow!(tf("capture.sck.failed", &[("reason", reason)])))
}

fn timed_out() -> Error {
    Error::Other(anyhow::anyhow!(t("capture.sck.timeout").into_owned()))
}

fn ns_error(e: *mut NSError) -> Option<Error> {
    // SAFETY: ScreenCaptureKit passes either null or a valid NSError.
    let e = unsafe { e.as_ref() }?;
    if e.code() == USER_DECLINED {
        return Some(permission_error("capture.permission.screen_recording"));
    }
    Some(failed(&e.localizedDescription().to_string()))
}

/// What only AppKit knows about a display. NSScreen is main-thread only, so this is read whenever a capture call runs
/// on the main thread and kept for calls from other threads.
#[derive(Clone)]
struct ScreenInfo {
    id: u32,
    name: String,
    /// Area without the menu bar and the Dock: x, y, w, h in global points, top-left origin.
    visible: [f64; 4],
    hdr: bool,
    max_fps: f32,
}

static SCREENS: Mutex<Vec<ScreenInfo>> = Mutex::new(Vec::new());

fn screens() -> Vec<ScreenInfo> {
    let Some(mtm) = MainThreadMarker::new() else { return SCREENS.lock().clone() };
    let screens = NSScreen::screens(mtm);
    // AppKit frames grow upwards from the bottom of the main screen, which is listed first.
    let Some(main_height) = screens.firstObject().map(|s| s.frame().size.height) else { return Vec::new() };
    let fresh: Vec<ScreenInfo> = screens
        .iter()
        .filter_map(|s| {
            let id = s.deviceDescription().objectForKey(ns_string!("NSScreenNumber"))?.downcast::<NSNumber>().ok()?.unsignedIntValue();
            let v = s.visibleFrame();
            Some(ScreenInfo {
                id,
                name: s.localizedName().to_string(),
                visible: [v.origin.x, main_height - v.origin.y - v.size.height, v.size.width, v.size.height],
                hdr: s.maximumPotentialExtendedDynamicRangeColorComponentValue() > 1.0,
                max_fps: s.maximumFramesPerSecond() as f32,
            })
        })
        .collect();
    *SCREENS.lock() = fresh.clone();
    fresh
}

fn refresh_rate(id: u32) -> f32 {
    CGDisplayMode::refresh_rate(CGDisplayCopyDisplayMode(id).as_deref()) as f32
}

pub(crate) fn monitors() -> Result<Vec<MonitorInfo>> {
    let layout = PointLayout::current();
    if layout.displays.is_empty() {
        return Err(Error::NotFound("no monitors".into()));
    }
    let screens = screens();
    Ok(layout
        .displays
        .iter()
        .enumerate()
        .map(|(index, d)| {
            let screen = screens.iter().find(|s| s.id == d.id);
            // Built-in panels with a variable refresh rate report 0 for their display mode.
            let refresh = refresh_rate(d.id);
            MonitorInfo {
                index,
                id: d.id,
                name: screen.map(|s| s.name.clone()).unwrap_or_default(),
                rect: d.rect,
                work_area: screen
                    .and_then(|s| {
                        let [x, y, w, h] = s.visible;
                        d.rect_to_pixels(x, y, w, h).intersect(d.rect)
                    })
                    .unwrap_or(d.rect),
                scale_factor: d.scale as f32,
                is_primary: d.is_main,
                hdr: screen.is_some_and(|s| s.hdr),
                refresh_hz: if refresh > 0.0 { refresh } else { screen.map_or(0.0, |s| s.max_fps) },
            }
        })
        .collect())
}

/// One entry of the CoreGraphics window list, parsed once.
struct CgWindow {
    id: u32,
    pid: u32,
    layer: i32,
    /// Global points.
    bounds: CGRect,
    /// Empty without the Screen Recording permission.
    title: String,
    owner: String,
    /// False for windows that opted out of capture (`NSWindowSharingNone`).
    shared: bool,
    alpha: f64,
}

/// The window list, front to back.
fn window_list(options: CGWindowListOption, relative_to: CGWindowID) -> Vec<CgWindow> {
    let Some(list) = CGWindowListCopyWindowInfo(options, relative_to) else { return Vec::new() };
    // SAFETY: the window list is an array of dictionaries keyed by strings.
    let list = unsafe { CFRetained::cast_unchecked::<CFArray<CFDictionary<CFString, CFType>>>(list) };
    // SAFETY: the keys are constant CoreGraphics strings.
    let (number_key, pid_key, layer_key, bounds_key, name_key, owner_key, sharing_key, alpha_key) = unsafe {
        (
            kCGWindowNumber,
            kCGWindowOwnerPID,
            kCGWindowLayer,
            kCGWindowBounds,
            kCGWindowName,
            kCGWindowOwnerName,
            kCGWindowSharingState,
            kCGWindowAlpha,
        )
    };
    list.iter()
        .filter_map(|w| {
            let number = |key: &CFString| w.get(key)?.downcast::<CFNumber>().ok();
            let text = |key: &CFString| w.get(key).and_then(|v| v.downcast::<CFString>().ok()).map(|s| s.to_string()).unwrap_or_default();
            let bounds_dict = w.get(bounds_key)?.downcast::<CFDictionary>().ok()?;
            let mut bounds = CGRect::ZERO;
            // SAFETY: a bounds dictionary as CoreGraphics writes it, and a valid out pointer.
            unsafe { CGRectMakeWithDictionaryRepresentation(Some(&bounds_dict), &mut bounds) }.then_some(())?;
            Some(CgWindow {
                id: number(number_key)?.as_i64()? as u32,
                pid: number(pid_key)?.as_i64()? as u32,
                layer: number(layer_key).and_then(|n| n.as_i64()).unwrap_or(0) as i32,
                bounds,
                title: text(name_key),
                owner: text(owner_key),
                shared: number(sharing_key).and_then(|n| n.as_i64()) != Some(0),
                alpha: number(alpha_key).and_then(|n| n.as_f64()).unwrap_or(1.0),
            })
        })
        .collect()
}

fn pixel_rect(layout: &PointLayout, r: CGRect) -> Rect {
    layout.rect_to_pixels(r.origin.x, r.origin.y, r.size.width, r.size.height)
}

/// File name of a process's executable.
fn exe_name(pid: u32) -> Option<String> {
    mac::exe_path(pid).and_then(|p| Path::new(&p).file_name().map(|n| n.to_string_lossy().into_owned()))
}

pub(crate) fn windows() -> Result<Vec<WindowInfo>> {
    let layout = PointLayout::current();
    let own_pid = std::process::id();
    let list = window_list(CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements, kCGNullWindowID);
    Ok(list
        .into_iter()
        .filter(|w| {
            // Normal, floating and panel levels, and pop-up menus; not the Dock, the menu bar or status items.
            let level = (kCGNormalWindowLevel..kCGMainMenuWindowLevel).contains(&w.layer) || w.layer == kCGPopUpMenuWindowLevel;
            level
                && w.pid != own_pid
                && w.shared
                && w.alpha > 0.0
                && w.bounds.size.width >= 1.0
                && w.bounds.size.height >= 1.0
                && !SHELL_APPS.contains(&w.owner.as_str())
        })
        .enumerate()
        .map(|(z, w)| WindowInfo {
            id: u64::from(w.id),
            pid: w.pid,
            app_id: mac::exe_path(w.pid),
            rect: pixel_rect(&layout, w.bounds),
            title: w.title,
            app_name: w.owner,
            z: z as u32,
            // The list only holds windows on screen.
            minimized: false,
        })
        .collect())
}

/// The active application. `activeApplication` is read live, while `frontmostApplication` is only refreshed by a
/// running main run loop, which the CLI and the MCP server do not have.
fn active_pid() -> Option<u32> {
    let workspace = NSWorkspace::sharedWorkspace();
    #[allow(deprecated)]
    let live = workspace
        .activeApplication()
        .and_then(|app| app.objectForKey(ns_string!("NSApplicationProcessIdentifier")))
        .and_then(|pid| pid.downcast::<NSNumber>().ok())
        .map(|pid| pid.intValue());
    let pid = live.or_else(|| workspace.frontmostApplication().map(|app| app.processIdentifier()))?;
    u32::try_from(pid).ok()
}

pub(crate) fn foreground_window() -> Option<WindowInfo> {
    let pid = active_pid()?;
    windows().ok()?.into_iter().find(|w| w.pid == pid)
}

pub(crate) fn cursor_position() -> Point {
    mac::pointer().map_or(Point::default(), |(x, y)| PointLayout::current().to_pixels(x, y))
}

/// ScreenCaptureKit draws the real pointer into the capture when it is asked to (`showsCursor`).
pub(crate) fn cursor_sprite() -> Option<CursorSprite> {
    None
}

/// Draw `image` into an sRGB RGBA bitmap: CoreGraphics converts from the image's colour profile and swaps BGRA to RGBA
/// in the same pass. Translucent pixels (window corners, shadows) come back straight, not premultiplied.
fn to_rgba(image: &CGImage) -> Result<RgbaImage> {
    let (w, h) = (CGImage::width(Some(image)), CGImage::height(Some(image)));
    if w == 0 || h == 0 {
        return Err(failed("empty image"));
    }
    let mut buf = vec![0u8; w * h * 4];
    // SAFETY: kCGColorSpaceSRGB is a constant CoreGraphics string.
    let srgb = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }));
    let info = CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0;
    // SAFETY: the buffer holds `h` rows of `w * 4` bytes and outlives the context, which is dropped right after drawing.
    let ctx = unsafe { CGBitmapContextCreate(buf.as_mut_ptr().cast(), w, h, 8, w * 4, srgb.as_deref(), info) }
        .ok_or_else(|| failed("no bitmap context"))?;
    CGContext::draw_image(Some(&ctx), CGRect::new(CGPoint::ZERO, CGSize::new(w as f64, h as f64)), Some(image));
    drop(ctx);
    for px in buf.as_chunks_mut::<4>().0 {
        let a = u32::from(px[3]);
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    RgbaImage::from_raw(w as u32, h as u32, buf).ok_or_else(|| failed("image size"))
}

/// Run one ScreenCaptureKit image request and wait for the result. The completion handler runs on a ScreenCaptureKit
/// queue, never on the calling thread, so waiting here does not deadlock, not even on the main thread.
fn sck_image(request: impl FnOnce(&DynBlock<dyn Fn(*mut CGImage, *mut NSError)>)) -> Result<RgbaImage> {
    let (tx, rx) = crossbeam_channel::bounded::<Result<RgbaImage>>(1);
    let handler = RcBlock::new(move |image: *mut CGImage, err: *mut NSError| {
        let result = match ns_error(err) {
            Some(e) => Err(e),
            // SAFETY: on success the image is valid for the duration of the call.
            None => unsafe { image.as_ref() }.map_or_else(|| Err(failed("no image")), to_rgba),
        };
        let _ = tx.send(result);
    });
    request(&handler);
    rx.recv_timeout(SCK_TIMEOUT).map_err(|_| timed_out())?
}

/// macOS 12.3 and 13 have no ScreenCaptureKit screenshot call; CoreGraphics' own still works there. It never draws the
/// pointer and cannot leave the desktop icons out.
#[allow(deprecated)]
fn legacy_image(bounds: CGRect, list: CGWindowListOption, window: CGWindowID, options: CGWindowImageOption) -> Result<RgbaImage> {
    let image = objc2_core_graphics::CGWindowListCreateImage(bounds, list, window, options | CGWindowImageOption::BestResolution)
        .ok_or_else(|| failed("no image"))?;
    to_rgba(&image)
}

fn shareable_content(desktop_windows: bool, on_screen_only: bool) -> Result<Retained<SCShareableContent>> {
    let (tx, rx) = crossbeam_channel::bounded::<Result<Retained<SCShareableContent>>>(1);
    let handler = RcBlock::new(move |content: *mut SCShareableContent, err: *mut NSError| {
        let result = match ns_error(err) {
            Some(e) => Err(e),
            // SAFETY: on success ScreenCaptureKit passes a valid SCShareableContent.
            None => unsafe { Retained::retain(content) }.ok_or_else(|| failed("no shareable content")),
        };
        let _ = tx.send(result);
    });
    // SAFETY: ScreenCaptureKit copies the handler, which only sends on a channel.
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            !desktop_windows,
            on_screen_only,
            &handler,
        );
    }
    rx.recv_timeout(SCK_TIMEOUT).map_err(|_| timed_out())?
}

/// Finder draws the desktop icons in a window of its own below every normal window.
fn is_desktop_icons(w: &SCWindow) -> bool {
    // SAFETY: reading immutable properties of a listed window.
    unsafe { w.windowLayer() < 0 && w.owningApplication().is_some_and(|app| app.bundleIdentifier().to_string() == "com.apple.finder") }
}

thread_local! {
    /// Recent display content with or without the desktop windows. ScreenCaptureKit objects stay on the thread that
    /// asked for them.
    static DISPLAY_CONTENT: RefCell<Option<(Instant, bool, Retained<SCShareableContent>)>> = const { RefCell::new(None) };
}

/// A filter for one whole display, without Finder's desktop icons when `hide_icons`.
fn display_filter(id: u32, hide_icons: bool) -> Result<Retained<SCContentFilter>> {
    let build = |c: &SCShareableContent| {
        // SAFETY: reading immutable properties of ScreenCaptureKit objects, and creating a filter from them.
        unsafe {
            let display = c.displays().iter().find(|d| d.displayID() == id)?;
            let excluded: Vec<Retained<SCWindow>> =
                if hide_icons { c.windows().iter().filter(|w| is_desktop_icons(w)).collect() } else { Vec::new() };
            Some(SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                &display,
                &NSArray::from_retained_slice(&excluded),
            ))
        }
    };
    let cached = DISPLAY_CONTENT.with_borrow(|slot| {
        slot.as_ref().filter(|(at, icons, _)| *icons == hide_icons && at.elapsed() < CONTENT_TTL).and_then(|(_, _, c)| build(c))
    });
    if let Some(filter) = cached {
        return Ok(filter);
    }
    let content = shareable_content(hide_icons, true)?;
    let filter = build(&content).ok_or_else(|| Error::NotFound(format!("display {id}")))?;
    DISPLAY_CONTENT.set(Some((Instant::now(), hide_icons, content)));
    Ok(filter)
}

/// Settings every filter capture shares: exact pixel size, sRGB, and the pointer only when asked for (it defaults to on).
fn stream_config(width: usize, height: usize, cursor: bool) -> Retained<SCStreamConfiguration> {
    // SAFETY: setters on a fresh configuration; kCGColorSpaceSRGB is a constant CoreGraphics string.
    unsafe {
        let c = SCStreamConfiguration::new();
        c.setWidth(width);
        c.setHeight(height);
        c.setShowsCursor(cursor);
        c.setColorSpaceName(kCGColorSpaceSRGB);
        c.setCaptureResolution(SCCaptureResolutionType::Best);
        c
    }
}

/// Whether the display under virtual-desktop pixel `p` can show HDR. Captures come out SDR, so content on such a
/// display has been tone-mapped by the system.
fn hdr_at(layout: &PointLayout, p: Point) -> bool {
    layout.display_at(p).is_some_and(|d| screens().iter().any(|s| s.id == d.id && s.hdr))
}

pub(crate) fn grab_monitor(m: &MonitorInfo, region: Rect, o: &CaptureOptions) -> Result<Grab> {
    mac::check_capture_access()?;
    let layout = PointLayout::current();
    let d = layout.displays.iter().find(|d| d.id == m.id).ok_or_else(|| Error::NotFound(format!("monitor {}", m.id)))?;
    let s = d.scale;
    // Capture whole points around the region (half a point at 2x would be resampled), then cut the region out.
    let local = region.relative_to(m.rect.origin());
    let (x0, y0) = ((f64::from(local.x) / s).floor(), (f64::from(local.y) / s).floor());
    let (x1, y1) = ((f64::from(local.right()) / s).ceil(), (f64::from(local.bottom()) / s).ceil());
    let size = CGSize::new(x1 - x0, y1 - y0);
    let global = CGRect::new(CGPoint::new(d.origin.0 + x0, d.origin.1 + y0), size);
    let full = if !available!(macos = 14.0) {
        legacy_image(global, CGWindowListOption::OptionOnScreenOnly, kCGNullWindowID, CGWindowImageOption::Default)?
    } else if available!(macos = 15.2) && !o.include_cursor && !o.hide_desktop_icons {
        // The fastest call (about 15 ms for a whole display), but it never draws the pointer and cannot leave a
        // window out.
        // SAFETY: a plain request; the handler outlives it.
        sck_image(|h| unsafe { SCScreenshotManager::captureImageInRect_completionHandler(global, Some(h)) })?
    } else {
        let filter = display_filter(d.id, o.hide_desktop_icons)?;
        let config = stream_config((size.width * s).round() as usize, (size.height * s).round() as usize, o.include_cursor);
        // SAFETY: setters on a fresh configuration, then a plain request; the handler outlives it.
        unsafe {
            config.setSourceRect(CGRect::new(CGPoint::new(x0, y0), size));
            config.setShouldBeOpaque(true);
            sck_image(|h| SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(&filter, &config, Some(h)))?
        }
    };
    let offset = Point::new(local.x - (x0 * s).round() as i32, local.y - (y0 * s).round() as i32);
    let image = if offset == Point::default() && full.dimensions() == (region.w, region.h) {
        full
    } else {
        let mut out = RgbaImage::new(region.w, region.h);
        blit::copy(&full, Rect::new(offset.x, offset.y, region.w, region.h), &mut out, Point::default());
        out
    };
    Ok(Grab { image, tone_mapped: m.hdr })
}

/// A window without its shadow, at its full pixel size, also when other windows cover it (macOS 14 to 25, and later
/// ones when no shadow is wanted).
fn window_image(filter: &SCContentFilter, cursor: bool) -> Result<RgbaImage> {
    // SAFETY: reading the filter's geometry, setters on a fresh configuration, then a plain request.
    unsafe {
        let r = filter.contentRect();
        let s = f64::from(filter.pointPixelScale());
        // With the shadow on, this size would squeeze window and shadow together, so the shadow stays off here and
        // the synthetic one is added later when asked for.
        let config = stream_config((r.size.width * s).round() as usize, (r.size.height * s).round() as usize, cursor);
        config.setIgnoreShadowsSingleWindow(true);
        config.setShouldBeOpaque(false);
        if available!(macos = 14.2) {
            config.setIncludeChildWindows(true);
        }
        sck_image(|h| SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(filter, &config, Some(h)))
    }
}

/// A window with the system's own drop shadow, sized by the system (macOS 26 and later), like Cmd-Shift-4 then Space.
fn window_screenshot(filter: &SCContentFilter, cursor: bool) -> Result<RgbaImage> {
    let (tx, rx) = crossbeam_channel::bounded::<Result<RgbaImage>>(1);
    let handler = RcBlock::new(move |output: *mut SCScreenshotOutput, err: *mut NSError| {
        let result = match ns_error(err) {
            Some(e) => Err(e),
            // SAFETY: on success the output is valid for the duration of the call.
            None => unsafe { output.as_ref().and_then(|o| o.sdrImage()) }.map_or_else(|| Err(failed("no image")), |img| to_rgba(&img)),
        };
        let _ = tx.send(result);
    });
    // SAFETY: setters on a fresh configuration, then a plain request; the handler only sends on a channel.
    unsafe {
        let config = SCScreenshotConfiguration::new();
        config.setShowsCursor(cursor);
        config.setIgnoreShadows(false);
        config.setIncludeChildWindows(true);
        config.setDynamicRange(SCScreenshotDynamicRange::SDR);
        SCScreenshotManager::captureScreenshotWithFilter_configuration_completionHandler(filter, &config, Some(&handler));
    }
    rx.recv_timeout(SCK_TIMEOUT).map_err(|_| timed_out())?
}

pub(crate) fn grab_window(id: u64, o: &CaptureOptions) -> Result<WindowGrab> {
    mac::check_capture_access()?;
    let window_id = CGWindowID::try_from(id).map_err(|_| Error::invalid("window id"))?;
    let not_found = || Error::NotFound(format!("window {id}"));
    let bounds = window_list(CGWindowListOption::OptionIncludingWindow, window_id).into_iter().next().ok_or_else(not_found)?.bounds;
    let layout = PointLayout::current();
    let frame = pixel_rect(&layout, bounds);

    let (image, shadow) = if !available!(macos = 14.0) {
        // SAFETY: CGRectNull is a constant; with it the image is exactly the window's own bounds.
        let everything = unsafe { CGRectNull };
        (legacy_image(everything, CGWindowListOption::OptionIncludingWindow, window_id, CGWindowImageOption::BoundsIgnoreFraming)?, false)
    } else {
        let content = shareable_content(false, false)?;
        // SAFETY: reading immutable properties of listed windows, and creating a filter from one of them.
        let filter = unsafe {
            let window = content.windows().iter().find(|w| w.windowID() == window_id).ok_or_else(not_found)?;
            SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &window)
        };
        if o.window_shadow && available!(macos = 26.0) {
            (window_screenshot(&filter, o.include_cursor)?, true)
        } else {
            (window_image(&filter, o.include_cursor)?, false)
        }
    };
    // The image decides the size: ScreenCaptureKit's window bounds can be taller than the window list's, and the
    // system shadow adds a margin around the frame, taken as centred.
    let (dx, dy) =
        if shadow { ((image.width() as i32 - frame.w as i32) / 2, (image.height() as i32 - frame.h as i32) / 2) } else { (0, 0) };
    let screen_rect = Rect::new(frame.x - dx, frame.y - dy, image.width(), image.height());
    let centre = Point::new(frame.x + (frame.w / 2) as i32, frame.y + (frame.h / 2) as i32);
    Ok(WindowGrab { image, screen_rect, tone_mapped: hdr_at(&layout, centre), shadow })
}

/// Whether a system prompt window is on screen: the screen capture approval, a Gatekeeper or a password prompt.
pub(crate) fn system_alert_on_screen() -> bool {
    window_list(CGWindowListOption::OptionOnScreenOnly, kCGNullWindowID).iter().any(|w| {
        w.alpha > 0.0
            && w.bounds.size.width > 1.0
            && w.bounds.size.height > 1.0
            // The owner name is the process name; the executable's file name also holds where a name is localised.
            && (ALERT_PROCESSES.contains(&w.owner.as_str()) || exe_name(w.pid).is_some_and(|n| ALERT_PROCESSES.contains(&n.as_str())))
    })
}

/// A throwaway capture of a few pixels of the main display, so that macOS asks its "bypass the private window picker"
/// question now, while nothing covers the prompt. Skipped without the Screen Recording grant, which only an explicit
/// user action may request.
pub(crate) fn prime_capture_consent() {
    if mac::check_capture_access().is_err() {
        return;
    }
    let Some(main) = monitors().ok().and_then(|ms| ms.into_iter().find(|m| m.is_primary)) else { return };
    let corner = Rect::new(main.rect.x, main.rect.y, 2, 2);
    if let Err(e) = grab_monitor(&main, corner, &CaptureOptions::default()) {
        tracing::debug!("consent capture: {e}");
    }
}
