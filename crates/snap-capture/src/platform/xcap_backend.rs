//! Linux capture on top of `xcap`. Desktop extras (pointer, input, accessibility) live in `linux.rs`.
//!
//! `xcap` reports Linux geometry in pixels of the root window, which already is virtual-desktop space.

use snap_core::{Error, Point, Rect, Result, RgbaImage};
use xcap::{Monitor, Window};

use super::{CursorSprite, Grab, WindowGrab, linux};
use crate::{CaptureOptions, MonitorInfo, WindowInfo, blit};

pub(crate) fn init() {}

pub(crate) fn warm_up() {}

fn xerr(e: xcap::XCapError) -> Error {
    Error::Other(anyhow::anyhow!("{e}"))
}

/// Run xcap calls so a panic inside its Wayland layer (libwayshot panics on compositors without wlr-screencopy and
/// xdg-output) surfaces as an error instead of taking the whole app down.
fn guarded<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .unwrap_or_else(|_| Err(Error::unsupported(snap_core::t("capture.error.backend_crashed"))))
}

fn displays() -> Result<Vec<MonitorInfo>> {
    guarded(displays_unguarded)
}

fn displays_unguarded() -> Result<Vec<MonitorInfo>> {
    let mut out = Vec::new();
    for m in Monitor::all().map_err(xerr)? {
        let rect = Rect::new(m.x().map_err(xerr)?, m.y().map_err(xerr)?, m.width().map_err(xerr)?, m.height().map_err(xerr)?);
        out.push(MonitorInfo {
            index: 0,
            id: m.id().map_err(xerr)?,
            name: m.friendly_name().or_else(|_| m.name()).unwrap_or_default(),
            rect,
            work_area: rect,
            scale_factor: m.scale_factor().unwrap_or(1.0).max(1.0),
            is_primary: m.is_primary().unwrap_or(false),
            hdr: false,
            refresh_hz: m.frequency().unwrap_or(0.0),
        });
    }
    if out.is_empty() {
        return Err(Error::NotFound("no monitors".into()));
    }
    out.sort_by_key(|m| (!m.is_primary, m.rect.x, m.rect.y));
    for (i, m) in out.iter_mut().enumerate() {
        m.index = i;
    }
    Ok(out)
}

fn round_point(x: f64, y: f64) -> Point {
    Point::new(x.round() as i32, y.round() as i32)
}

pub(crate) fn monitors() -> Result<Vec<MonitorInfo>> {
    displays()
}

fn describe(w: &Window, own_pid: u32) -> Option<(i32, WindowInfo)> {
    let pid = w.pid().unwrap_or(0);
    if pid == own_pid {
        return None;
    }
    let rect = Rect::new(w.x().ok()?, w.y().ok()?, w.width().ok()?, w.height().ok()?);
    if rect.is_empty() {
        return None;
    }
    let info = WindowInfo {
        id: w.id().ok()? as u64,
        pid,
        title: w.title().unwrap_or_default(),
        app_name: w.app_name().unwrap_or_default(),
        app_id: linux::exe_path(pid),
        rect,
        z: 0,
        minimized: w.is_minimized().unwrap_or(false),
    };
    Some((w.z().unwrap_or(0), info))
}

pub(crate) fn windows() -> Result<Vec<WindowInfo>> {
    let own_pid = std::process::id();
    let all = guarded(|| Window::all().map_err(xerr))?;
    let mut list: Vec<(i32, WindowInfo)> = all.iter().filter_map(|w| describe(w, own_pid)).collect();
    // xcap's z grows towards the viewer; ours is 0 = front-most.
    list.sort_by_key(|(z, _)| std::cmp::Reverse(*z));
    Ok(list
        .into_iter()
        .enumerate()
        .map(|(i, (_, mut w))| {
            w.z = i as u32;
            w
        })
        .collect())
}

pub(crate) fn foreground_window() -> Option<WindowInfo> {
    let focused = guarded(|| Window::all().map_err(xerr)).ok()?.into_iter().find(|w| w.is_focused().unwrap_or(false))?.id().ok()? as u64;
    windows().ok()?.into_iter().find(|w| w.id == focused)
}

pub(crate) fn cursor_position() -> Point {
    linux::pointer().map_or(Point::default(), |(x, y)| round_point(x, y))
}

pub(crate) fn grab_monitor(m: &MonitorInfo, region: Rect, _o: &CaptureOptions) -> Result<Grab> {
    let full = guarded(|| {
        let mon = Monitor::all()
            .map_err(xerr)?
            .into_iter()
            .find(|x| x.id().ok() == Some(m.id))
            .ok_or_else(|| Error::NotFound(format!("monitor {}", m.id)))?;
        mon.capture_image().map_err(xerr)
    })?;
    if region == m.rect && full.width() == m.rect.w && full.height() == m.rect.h {
        return Ok(Grab { image: full, tone_mapped: false });
    }
    let mut image = RgbaImage::new(region.w, region.h);
    blit::copy(&full, region.relative_to(m.rect.origin()), &mut image, Point::default());
    Ok(Grab { image, tone_mapped: false })
}

pub(crate) fn grab_window(id: u64, _o: &CaptureOptions) -> Result<WindowGrab> {
    // Bare X servers (no EWMH window manager, as in WSLg) cannot list windows, but a known id can still be read.
    guarded(|| grab_listed_window(id)).or_else(|e| match e {
        Error::NotFound(_) | Error::Invalid(_) => Err(e),
        _ => linux::window_by_id(id)
            .map(|(image, screen_rect)| WindowGrab { image, screen_rect, tone_mapped: false, shadow: false })
            .map_err(|_| e),
    })
}

fn grab_listed_window(id: u64) -> Result<WindowGrab> {
    let w = Window::all()
        .map_err(xerr)?
        .into_iter()
        .find(|w| w.id().ok().map(u64::from) == Some(id))
        .ok_or_else(|| Error::NotFound(format!("window {id}")))?;
    if w.is_minimized().unwrap_or(false) {
        return Err(Error::invalid("window is minimized"));
    }
    let (x, y) = (w.x().map_err(xerr)?, w.y().map_err(xerr)?);
    let image = w.capture_image().map_err(xerr)?;
    let screen_rect = Rect::new(x, y, image.width(), image.height());
    Ok(WindowGrab { image, screen_rect, tone_mapped: false, shadow: false })
}

/// xcap does not draw the pointer, so synthesise a standard arrow at the cursor position.
pub(crate) fn cursor_sprite() -> Option<CursorSprite> {
    let (x, y) = linux::pointer()?;
    Some(CursorSprite { image: arrow(1.0), origin: round_point(x, y) })
}

/// No system prompt can cover a Linux capture.
pub(crate) fn system_alert_on_screen() -> bool {
    false
}

/// The desktop portal asks at capture time; there is nothing to prime.
pub(crate) fn prime_capture_consent() {}

/// Classic arrow pointer, tip at the top-left pixel, `scale` times the 1x size.
fn arrow(scale: f32) -> RgbaImage {
    const POLY: [(f32, f32); 7] = [(0.0, 0.0), (0.0, 16.0), (4.0, 12.5), (6.8, 19.0), (9.3, 18.0), (6.6, 11.8), (11.5, 11.5)];
    const SS: u32 = 4;
    let size = (22.0 * scale).ceil() as u32;
    let poly: Vec<(f32, f32)> = POLY.iter().map(|(x, y)| (x * scale, y * scale)).collect();
    let border = 1.1 * scale;
    let mut img = RgbaImage::new(size, size);
    for (px, py, out) in img.enumerate_pixels_mut() {
        let (mut white, mut black) = (0u32, 0u32);
        for sy in 0..SS {
            for sx in 0..SS {
                let p = (px as f32 + (sx as f32 + 0.5) / SS as f32, py as f32 + (sy as f32 + 0.5) / SS as f32);
                let inside = inside_poly(&poly, p);
                let edge = poly.iter().zip(poly.iter().cycle().skip(1)).map(|(a, b)| dist_to_segment(p, *a, *b)).fold(f32::MAX, f32::min);
                if inside && edge > border {
                    white += 1;
                } else if inside || edge <= border {
                    black += 1;
                }
            }
        }
        let total = SS * SS;
        let covered = white + black;
        if let Some(shade) = (white * 255).checked_div(covered) {
            *out = image::Rgba([shade as u8, shade as u8, shade as u8, (covered * 255 / total) as u8]);
        }
    }
    img
}

fn inside_poly(poly: &[(f32, f32)], p: (f32, f32)) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn dist_to_segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0) };
    let (cx, cy) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt()
}
