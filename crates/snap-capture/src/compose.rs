//! Backend-independent assembly: stitching monitor grabs, window context, snapshots.

use snap_core::{Capture, CaptureContext, CaptureKind, Error, Point, Rect, Result, RgbaImage};

use crate::{
    CaptureOptions, Frozen, MonitorInfo, WindowInfo, blit,
    platform::{self, Grab},
};

/// Shadow margin for window captures, in physical pixels at 100 % scale.
const SHADOW_MARGIN: f32 = 32.0;

/// Run `f` over `items`, in parallel when there is more than one.
fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync + Send) -> Vec<R> {
    if items.len() <= 1 {
        return items.iter().map(f).collect();
    }
    use rayon::prelude::*;
    items.par_iter().map(&f).collect()
}

fn grab_parts(parts: &[(&MonitorInfo, Rect)], o: &CaptureOptions) -> Result<Vec<Grab>> {
    par_map(parts, |(m, r)| platform::grab_monitor(m, *r, o)).into_iter().collect()
}

fn draw_cursor(img: &mut RgbaImage, origin: Point) {
    if let Some(c) = platform::cursor_sprite() {
        blit::blend(img, &c.image, Point::new(c.origin.x - origin.x, c.origin.y - origin.y));
    }
}

fn foreground_context() -> CaptureContext {
    platform::foreground_window().map(|w| window_context(&w, false)).unwrap_or_default()
}

pub(crate) fn window_context(w: &WindowInfo, with_url: bool) -> CaptureContext {
    CaptureContext {
        app_name: Some(w.app_name.clone()).filter(|s| !s.is_empty()),
        app_id: w.app_id.clone(),
        window_title: Some(w.title.clone()).filter(|s| !s.is_empty()),
        url: if with_url { platform::browser_url(w) } else { None },
        element: None,
        monitor_name: None,
    }
}

fn dominant(monitors: &[MonitorInfo], r: Rect) -> Option<&MonitorInfo> {
    monitors.iter().max_by_key(|m| m.rect.intersect(r).map_or(0, Rect::area)).filter(|m| m.rect.intersect(r).is_some())
}

/// Pixels of `area` (virtual-desktop pixels) from whichever monitors it touches, with the cursor when asked for.
/// Returns the part of `area` that lies on a monitor, its pixels, and whether any of them were tone-mapped.
fn grab_area(monitors: &[MonitorInfo], area: Rect, o: &CaptureOptions) -> Result<(Rect, RgbaImage, bool)> {
    let parts: Vec<_> = monitors.iter().filter_map(|m| m.rect.intersect(area).map(|r| (m, r))).collect();
    if parts.is_empty() {
        return Err(Error::invalid("capture area is outside every monitor"));
    }
    // Only the part that lies on a monitor has pixels; a request for more (a huge region from a
    // script or MCP client) must not allocate the whole rect.
    let area = parts.iter().map(|(_, r)| *r).reduce(Rect::union).unwrap_or(area);
    let grabs = grab_parts(&parts, o)?;
    let tone_mapped = grabs.iter().any(|g| g.tone_mapped);

    let mut image = match (parts.as_slice(), grabs.as_slice()) {
        ([(_, r)], [g]) if *r == area => g.image.clone(),
        _ => {
            let mut out = RgbaImage::new(area.w, area.h);
            for ((_, r), g) in parts.iter().zip(&grabs) {
                blit::copy(&g.image, Rect::new(0, 0, r.w, r.h), &mut out, r.relative_to(area.origin()).origin());
            }
            out
        }
    };
    if o.include_cursor {
        draw_cursor(&mut image, area.origin());
    }
    Ok((area, image, tone_mapped))
}

/// Capture `area` (virtual-desktop pixels) from whichever monitors it touches.
pub(crate) fn capture_area(monitors: &[MonitorInfo], area: Rect, kind: CaptureKind, o: &CaptureOptions) -> Result<Capture> {
    let (area, image, tone_mapped) = grab_area(monitors, area, o)?;
    let scale = dominant(monitors, area).map_or(1.0, |m| m.scale_factor);
    let mut cap = Capture::new(kind, image, area, scale);
    cap.tone_mapped = tone_mapped;
    cap.context = foreground_context();
    cap.context.monitor_name = dominant(monitors, area).map(|m| m.name.clone());
    Ok(cap)
}

/// Just the pixels of `area`, without looking up any context.
pub(crate) fn area_pixels(monitors: &[MonitorInfo], area: Rect, o: &CaptureOptions) -> Result<RgbaImage> {
    grab_area(monitors, area, o).map(|(_, image, _)| image)
}

pub(crate) fn capture_window(id: u64, o: &CaptureOptions) -> Result<Capture> {
    let (grab, info) =
        rayon::join(|| platform::grab_window(id, o), || platform::windows().ok().and_then(|ws| ws.into_iter().find(|w| w.id == id)));
    let grab = grab?;
    let monitors = platform::monitors().unwrap_or_default();
    let scale = dominant(&monitors, grab.screen_rect).map_or(1.0, |m| m.scale_factor);

    let (mut image, mut screen_rect) = (grab.image, grab.screen_rect);
    if o.window_shadow && !grab.shadow {
        let margin = (SHADOW_MARGIN * scale).round() as u32;
        image = blit::with_shadow(&image, margin, (margin / 4) as i32, 0.45);
        screen_rect = screen_rect.inflate(margin as i32);
    }
    if o.include_cursor {
        draw_cursor(&mut image, screen_rect.origin());
    }

    let mut cap = Capture::new(CaptureKind::Window, image, screen_rect, scale);
    cap.tone_mapped = grab.tone_mapped;
    cap.context = info.as_ref().map(|w| window_context(w, true)).unwrap_or_default();
    cap.context.monitor_name = dominant(&monitors, screen_rect).map(|m| m.name.clone());
    Ok(cap)
}

pub(crate) fn freeze(o: &CaptureOptions) -> Result<Frozen> {
    let monitors = platform::monitors()?;
    let parts: Vec<_> = monitors.iter().map(|m| (m, m.rect)).collect();
    let (grabs, windows) = rayon::join(|| grab_parts(&parts, o), || platform::windows().unwrap_or_default());
    let grabs = grabs?;
    let cursor = platform::cursor_position();
    let taken_at_ms = snap_core::now_ms();

    let monitors = monitors
        .into_iter()
        .zip(grabs)
        .map(|(m, g)| {
            let mut image = g.image;
            if o.include_cursor {
                draw_cursor(&mut image, m.rect.origin());
            }
            (m, image)
        })
        .collect();
    Ok(Frozen { monitors, windows, cursor, taken_at_ms })
}
