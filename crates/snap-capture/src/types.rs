use serde::{Deserialize, Serialize};
use snap_core::{
    Capture, CaptureContext, CaptureKind, Point, Rect, RgbaImage,
    settings::{CaptureSettings, HdrMode},
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonitorInfo {
    pub index: usize,
    /// Stable for the lifetime of the display configuration (OS handle / display id).
    pub id: u32,
    pub name: String,
    /// Physical pixels in virtual-desktop space.
    pub rect: Rect,
    pub work_area: Rect,
    /// 1.0 = 96 DPI.
    pub scale_factor: f32,
    pub is_primary: bool,
    pub hdr: bool,
    pub refresh_hz: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowInfo {
    pub id: u64,
    pub pid: u32,
    pub title: String,
    pub app_name: String,
    /// Executable path (Windows/Linux) or bundle id (macOS) when known.
    pub app_id: Option<String>,
    /// Visible frame, without invisible resize borders.
    pub rect: Rect,
    /// 0 = front-most.
    pub z: u32,
    pub minimized: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ElementInfo {
    pub rect: Rect,
    pub name: String,
    /// Lower-case, non-localised control kind ("button", "edit", "list item", ...).
    pub role: String,
    pub app_name: Option<String>,
    pub window_id: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CaptureOptions {
    pub include_cursor: bool,
    pub hdr: HdrMode,
    /// Window captures: keep the (synthesised) drop shadow instead of cropping to the visible frame.
    pub window_shadow: bool,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self { include_cursor: false, hdr: HdrMode::Auto, window_shadow: false }
    }
}

impl From<&CaptureSettings> for CaptureOptions {
    fn from(s: &CaptureSettings) -> Self {
        Self { include_cursor: s.include_cursor, hdr: s.hdr, window_shadow: s.window_shadow }
    }
}

/// Instant snapshot for the selection overlay: every monitor image, the window list and the cursor.
#[derive(Debug, Clone)]
pub struct Frozen {
    pub monitors: Vec<(MonitorInfo, RgbaImage)>,
    pub windows: Vec<WindowInfo>,
    pub cursor: Point,
    pub taken_at_ms: i64,
}

impl Frozen {
    /// Bounding box of all monitors in virtual-desktop pixels.
    pub fn bounds(&self) -> Rect {
        self.monitors.iter().fold(Rect::default(), |acc, (m, _)| acc.union(m.rect))
    }

    /// Pixels of `r` (virtual-desktop space). Areas outside every monitor are transparent.
    pub fn crop(&self, r: Rect) -> RgbaImage {
        let mut out = RgbaImage::new(r.w, r.h);
        for (m, img) in &self.monitors {
            if let Some(i) = m.rect.intersect(r) {
                let src = i.relative_to(m.rect.origin());
                let dst = i.relative_to(r.origin());
                crate::blit::copy(img, src, &mut out, dst.origin());
            }
        }
        out
    }

    /// RGBA of the pixel under `p`.
    pub fn pixel(&self, p: Point) -> Option<[u8; 4]> {
        let (m, img) = self.monitors.iter().find(|(m, _)| m.rect.contains(p))?;
        let (x, y) = ((p.x - m.rect.x) as u32, (p.y - m.rect.y) as u32);
        img.get_pixel_checked(x, y).map(|px| px.0)
    }

    /// Front-most visible window containing `p`.
    pub fn window_at(&self, p: Point) -> Option<&WindowInfo> {
        self.windows.iter().find(|w| !w.minimized && w.rect.contains(p))
    }

    /// Cut `r` out of the snapshot as a [`Capture`], with monitor/app context filled in.
    pub fn to_capture(&self, r: Rect, kind: CaptureKind) -> Capture {
        let (scale, monitor_name) = self.dominant_monitor(r).map_or((1.0, None), |m| (m.scale_factor, Some(m.name.clone())));
        let mut cap = Capture::new(kind, self.crop(r), r, scale);
        cap.taken_at_ms = self.taken_at_ms;
        cap.context = CaptureContext { monitor_name, ..CaptureContext::default() };
        let center = Point::new(r.x + (r.w / 2) as i32, r.y + (r.h / 2) as i32);
        if let Some(w) = self.window_at(center) {
            cap.context.app_name = Some(w.app_name.clone()).filter(|s| !s.is_empty());
            cap.context.app_id = w.app_id.clone();
            cap.context.window_title = Some(w.title.clone()).filter(|s| !s.is_empty());
        }
        cap
    }

    fn dominant_monitor(&self, r: Rect) -> Option<&MonitorInfo> {
        self.monitors.iter().map(|(m, _)| m).max_by_key(|m| m.rect.intersect(r).map_or(0, Rect::area))
    }
}
