//! Non-interactive capture: turns a [`CaptureRequest`] into a [`Capture`].

use std::time::Duration;

use snap_capture::{CaptureOptions, WindowInfo};
use snap_core::{Capture, CaptureContext, CaptureKind, Error, Result, Settings, t, tf};

use crate::{CaptureRequest, Engine};

/// Time the shell needs to repaint after the desktop icons are hidden.
const ICONS_SETTLE: Duration = Duration::from_millis(120);

pub(crate) fn capture(engine: &Engine, req: &CaptureRequest) -> Result<Capture> {
    let settings = engine.settings();
    let opts = CaptureOptions::from(&settings.capture);
    match req {
        CaptureRequest::Clipboard => {
            let img =
                snap_platform::clipboard::get_image().ok_or_else(|| Error::NotFound(t("engine.error.clipboard_empty").into_owned()))?;
            Ok(Capture::from_image(CaptureKind::Clipboard, img))
        }
        CaptureRequest::File(path) => {
            let img = snap_image::encode::load(path)?;
            let mut cap = Capture::from_image(CaptureKind::File, img);
            cap.context.window_title = path.file_name().map(|n| n.to_string_lossy().into_owned());
            Ok(cap)
        }
        CaptureRequest::Window(id) => {
            let id = match id {
                Some(id) => *id,
                None => {
                    snap_capture::foreground_window()
                        .ok_or_else(|| Error::NotFound(t("engine.error.no_foreground_window").into_owned()))?
                        .id
                }
            };
            snap_capture::capture_window(id, &opts)
        }
        CaptureRequest::WindowTitled(title) => snap_capture::capture_window(find_window(title)?.id, &opts),
        CaptureRequest::ElementAtCursor => element_at_cursor(&opts),
        CaptureRequest::Monitor(i) => on_desktop(&settings, || snap_capture::capture_monitor(*i, &opts)),
        CaptureRequest::ActiveMonitor => on_desktop(&settings, || snap_capture::capture_monitor(monitor_under_cursor()?, &opts)),
        CaptureRequest::AllMonitors => on_desktop(&settings, || snap_capture::capture_all(&opts)),
        CaptureRequest::Rect(r) => on_desktop(&settings, || snap_capture::capture_rect(*r, &opts)),
        CaptureRequest::LastRegion => {
            let r = settings.capture.last_region.ok_or_else(|| Error::NotFound(t("engine.error.no_last_region").into_owned()))?;
            on_desktop(&settings, || snap_capture::capture_rect(r, &opts))
        }
    }
}

/// Hide the desktop icons when the user asked for that, and wait for the shell to repaint. They come back when the
/// guard is dropped. Every way of looking at the desktop takes this first.
pub fn icons_hidden(settings: &Settings) -> Option<snap_capture::DesktopIconsGuard> {
    let icons = settings.capture.hide_desktop_icons.then(snap_capture::hide_desktop_icons).flatten();
    if icons.is_some() {
        std::thread::sleep(ICONS_SETTLE);
    }
    icons
}

/// Grab pixels straight off the desktop, with the icons out of the way when the user asked for that.
pub(crate) fn on_desktop(settings: &Settings, grab: impl FnOnce() -> Result<Capture>) -> Result<Capture> {
    let _icons = icons_hidden(settings);
    grab()
}

/// Index of the monitor containing the cursor, or the primary one.
fn monitor_under_cursor() -> Result<usize> {
    let monitors = snap_capture::monitors()?;
    let cursor = snap_capture::cursor_position();
    monitors
        .iter()
        .find(|m| m.rect.contains(cursor))
        .or_else(|| monitors.iter().find(|m| m.is_primary))
        .or(monitors.first())
        .map(|m| m.index)
        .ok_or_else(|| Error::NotFound(t("engine.error.no_monitor").into_owned()))
}

/// First window whose title (then app name) contains `title`, preferring windows that are not minimised.
pub(crate) fn find_window(title: &str) -> Result<WindowInfo> {
    let needle = title.to_lowercase();
    let windows = snap_capture::windows()?;
    let pick =
        |keep: &dyn Fn(&WindowInfo) -> bool| windows.iter().find(|w| keep(w) && !w.minimized).or_else(|| windows.iter().find(|w| keep(w)));
    pick(&|w| w.title.to_lowercase().contains(&needle))
        .or_else(|| pick(&|w| w.app_name.to_lowercase().contains(&needle)))
        .cloned()
        .ok_or_else(|| Error::NotFound(tf("engine.error.no_window", &[("title", title)])))
}

/// The smallest accessibility element under the cursor.
fn element_at_cursor(opts: &CaptureOptions) -> Result<Capture> {
    let at = snap_capture::cursor_position();
    let elements = snap_capture::elements_at(at)?;
    let element =
        elements.into_iter().find(|e| !e.rect.is_empty()).ok_or_else(|| Error::NotFound(t("engine.error.no_element").into_owned()))?;
    let mut cap = snap_capture::capture_rect(element.rect, opts)?;
    cap.kind = CaptureKind::Element;
    let monitor_name = cap.context.monitor_name.take();
    cap.context = CaptureContext { element: Some(element.name).filter(|n| !n.is_empty()), monitor_name, ..snap_capture::context_at(at) };
    Ok(cap)
}
