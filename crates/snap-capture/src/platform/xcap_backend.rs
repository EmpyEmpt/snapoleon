//! macOS and Linux capture on top of `xcap`. OS-specific extras live in `mac.rs` / `linux.rs` (`os`).
//!
//! `xcap` reports macOS geometry in points; everything crossing this module's boundary is converted to
//! physical pixels with each monitor's own scale factor (`mac_scale`). On Linux coordinates already are
//! pixels.

use snap_core::{Error, Point, Rect, Result, RgbaImage};
use xcap::{Monitor, Window};

use super::{CursorSprite, Grab, WindowGrab, os};
use crate::{CaptureOptions, MonitorInfo, WindowInfo, blit};

pub(crate) fn init() {}

pub(crate) fn warm_up() {}

fn xerr(e: xcap::XCapError) -> Error {
    Error::Other(anyhow::anyhow!("{e}"))
}

/// Points-to-pixels multiplier for geometry reported by xcap.
fn unit_scale(display_scale: f32) -> f64 {
    if cfg!(target_os = "macos") { display_scale.max(1.0) as f64 } else { 1.0 }
}

/// A monitor together with its native (point) geometry.
struct Display {
    info: MonitorInfo,
    /// x, y, w, h in xcap units.
    native: [f64; 4],
    k: f64,
}

fn displays() -> Result<Vec<Display>> {
    let mut out = Vec::new();
    for m in Monitor::all().map_err(xerr)? {
        let scale = m.scale_factor().unwrap_or(1.0);
        let k = unit_scale(scale);
        let native =
            [m.x().map_err(xerr)? as f64, m.y().map_err(xerr)? as f64, m.width().map_err(xerr)? as f64, m.height().map_err(xerr)? as f64];
        let rect = Rect::new(
            (native[0] * k).round() as i32,
            (native[1] * k).round() as i32,
            (native[2] * k).round() as u32,
            (native[3] * k).round() as u32,
        );
        out.push(Display {
            info: MonitorInfo {
                index: 0,
                id: m.id().map_err(xerr)?,
                name: m.friendly_name().or_else(|_| m.name()).unwrap_or_default(),
                rect,
                work_area: rect,
                scale_factor: scale.max(1.0),
                is_primary: m.is_primary().unwrap_or(false),
                hdr: false,
                refresh_hz: m.frequency().unwrap_or(0.0),
            },
            native,
            k,
        });
    }
    if out.is_empty() {
        return Err(Error::NotFound("no monitors".into()));
    }
    out.sort_by_key(|d| (!d.info.is_primary, d.info.rect.x, d.info.rect.y));
    for (i, d) in out.iter_mut().enumerate() {
        d.info.index = i;
    }
    Ok(out)
}

impl Display {
    fn contains_native(&self, x: f64, y: f64) -> bool {
        x >= self.native[0] && y >= self.native[1] && x < self.native[0] + self.native[2] && y < self.native[1] + self.native[3]
    }
}

/// Native point to physical pixel, using the display the point lies on (primary when none does).
fn to_physical(ds: &[Display], x: f64, y: f64) -> (Point, f64) {
    let d = ds.iter().find(|d| d.contains_native(x, y)).unwrap_or(&ds[0]);
    let p = Point::new(d.info.rect.x + ((x - d.native[0]) * d.k).round() as i32, d.info.rect.y + ((y - d.native[1]) * d.k).round() as i32);
    (p, d.k)
}

/// Physical pixel to native point (inverse of [`to_physical`]).
#[cfg(target_os = "macos")]
pub(super) fn to_native(p: Point) -> (f64, f64) {
    let Ok(ds) = displays() else { return (p.x as f64, p.y as f64) };
    let d = ds.iter().find(|d| d.info.rect.contains(p)).unwrap_or(&ds[0]);
    (d.native[0] + (p.x - d.info.rect.x) as f64 / d.k, d.native[1] + (p.y - d.info.rect.y) as f64 / d.k)
}

/// Native point to physical pixel for the OS modules.
pub(super) fn native_to_physical(x: f64, y: f64) -> Point {
    displays().map_or(Point::new(x.round() as i32, y.round() as i32), |ds| to_physical(&ds, x, y).0)
}

fn rect_to_physical(ds: &[Display], x: f64, y: f64, w: f64, h: f64) -> Rect {
    let (_, k) = to_physical(ds, x + w / 2.0, y + h / 2.0);
    let origin = to_physical(ds, x, y).0;
    Rect::new(origin.x, origin.y, (w * k).round().max(0.0) as u32, (h * k).round().max(0.0) as u32)
}

/// Native rectangle to physical pixels for the OS modules.
#[cfg(target_os = "macos")]
pub(super) fn native_rect_to_physical(x: f64, y: f64, w: f64, h: f64) -> Rect {
    match displays() {
        Ok(ds) => rect_to_physical(&ds, x, y, w, h),
        Err(_) => Rect::new(x.round() as i32, y.round() as i32, w.round().max(0.0) as u32, h.round().max(0.0) as u32),
    }
}

pub(crate) fn monitors() -> Result<Vec<MonitorInfo>> {
    Ok(displays()?.into_iter().map(|d| d.info).collect())
}

/// System/shell surfaces that are never useful capture targets.
const SHELL_APPS: &[&str] =
    &["Window Server", "Dock", "SystemUIServer", "Control Center", "Notification Center", "Spotlight", "WindowManager"];

fn describe(w: &Window, ds: &[Display], own_pid: u32) -> Option<(i32, WindowInfo)> {
    let pid = w.pid().unwrap_or(0);
    if pid == own_pid {
        return None;
    }
    let (x, y, wd, ht) = (w.x().ok()? as f64, w.y().ok()? as f64, w.width().ok()? as f64, w.height().ok()? as f64);
    if wd < 1.0 || ht < 1.0 {
        return None;
    }
    let title = w.title().unwrap_or_default();
    let app_name = w.app_name().unwrap_or_default();
    if SHELL_APPS.contains(&app_name.as_str()) {
        return None;
    }
    let rect = rect_to_physical(ds, x, y, wd, ht);
    let info = WindowInfo {
        id: w.id().ok()? as u64,
        pid,
        title,
        app_name,
        app_id: os::exe_path(pid),
        rect,
        z: 0,
        minimized: w.is_minimized().unwrap_or(false),
    };
    Some((w.z().unwrap_or(0), info))
}

pub(crate) fn windows() -> Result<Vec<WindowInfo>> {
    let ds = displays()?;
    let own_pid = std::process::id();
    let mut list: Vec<(i32, WindowInfo)> = Window::all().map_err(xerr)?.iter().filter_map(|w| describe(w, &ds, own_pid)).collect();
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
    let focused = Window::all().ok()?.into_iter().find(|w| w.is_focused().unwrap_or(false))?.id().ok()? as u64;
    windows().ok()?.into_iter().find(|w| w.id == focused)
}

pub(crate) fn cursor_position() -> Point {
    os::pointer().map_or(Point::default(), |(x, y)| native_to_physical(x, y))
}

pub(crate) fn grab_monitor(m: &MonitorInfo, region: Rect, _o: &CaptureOptions) -> Result<Grab> {
    os::check_capture_access()?;
    let mon = Monitor::all()
        .map_err(xerr)?
        .into_iter()
        .find(|x| x.id().ok() == Some(m.id))
        .ok_or_else(|| Error::NotFound(format!("monitor {}", m.id)))?;
    let full = mon.capture_image().map_err(xerr)?;
    if region == m.rect && full.width() == m.rect.w && full.height() == m.rect.h {
        return Ok(Grab { image: full, tone_mapped: false });
    }
    let mut image = RgbaImage::new(region.w, region.h);
    blit::copy(&full, region.relative_to(m.rect.origin()), &mut image, Point::default());
    Ok(Grab { image, tone_mapped: false })
}

pub(crate) fn grab_window(id: u64, _o: &CaptureOptions) -> Result<WindowGrab> {
    os::check_capture_access()?;
    let w = Window::all()
        .map_err(xerr)?
        .into_iter()
        .find(|w| w.id().ok().map(u64::from) == Some(id))
        .ok_or_else(|| Error::NotFound(format!("window {id}")))?;
    if w.is_minimized().unwrap_or(false) {
        return Err(Error::invalid("window is minimized"));
    }
    let ds = displays()?;
    let (x, y, wd, ht) =
        (w.x().map_err(xerr)? as f64, w.y().map_err(xerr)? as f64, w.width().map_err(xerr)? as f64, w.height().map_err(xerr)? as f64);
    let screen_rect = rect_to_physical(&ds, x, y, wd, ht);
    let image = os::trim_window_frame(w.capture_image().map_err(xerr)?, screen_rect);
    let screen_rect = Rect::new(screen_rect.x, screen_rect.y, image.width(), image.height());
    Ok(WindowGrab { image, screen_rect, tone_mapped: false })
}

/// xcap does not draw the pointer, so synthesise a standard arrow at the cursor position.
pub(crate) fn cursor_sprite() -> Option<CursorSprite> {
    let pos = os::pointer()?;
    let ds = displays().ok()?;
    let (origin, k) = to_physical(&ds, pos.0, pos.1);
    Some(CursorSprite { image: arrow(k as f32), origin })
}

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
