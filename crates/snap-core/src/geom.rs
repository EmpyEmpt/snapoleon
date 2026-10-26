//! Geometry. Integer types are *physical pixels in virtual-desktop space*
//! (origin = top-left of the primary monitor, may be negative). Float types
//! are used for annotation documents (image-pixel space) and UI math.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Size {
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

impl Size {
    pub const fn new(w: u32, h: u32) -> Self {
        Self { w, h }
    }
    pub fn is_empty(self) -> bool {
        self.w == 0 || self.h == 0
    }
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    /// Normalised rect spanning two arbitrary corners (drag start/end).
    pub fn from_corners(a: Point, b: Point) -> Self {
        let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
        let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
        Self::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32)
    }

    pub fn right(self) -> i32 {
        self.x + self.w as i32
    }
    pub fn bottom(self) -> i32 {
        self.y + self.h as i32
    }
    pub fn size(self) -> Size {
        Size::new(self.w, self.h)
    }
    pub fn origin(self) -> Point {
        Point::new(self.x, self.y)
    }
    pub fn is_empty(self) -> bool {
        self.w == 0 || self.h == 0
    }
    pub fn area(self) -> u64 {
        self.w as u64 * self.h as u64
    }

    pub fn contains(self, p: Point) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.right() && p.y < self.bottom()
    }

    pub fn intersect(self, o: Rect) -> Option<Rect> {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = self.right().min(o.right());
        let y1 = self.bottom().min(o.bottom());
        (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32))
    }

    pub fn union(self, o: Rect) -> Rect {
        if self.is_empty() {
            return o;
        }
        if o.is_empty() {
            return self;
        }
        let x0 = self.x.min(o.x);
        let y0 = self.y.min(o.y);
        let x1 = self.right().max(o.right());
        let y1 = self.bottom().max(o.bottom());
        Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32)
    }

    /// Translate into a coordinate space whose origin is `origin`.
    pub fn relative_to(self, origin: Point) -> Rect {
        Rect::new(self.x - origin.x, self.y - origin.y, self.w, self.h)
    }

    pub fn inflate(self, by: i32) -> Rect {
        let w = (self.w as i64 + 2 * by as i64).max(0) as u32;
        let h = (self.h as i64 + 2 * by as i64).max(0) as u32;
        Rect::new(self.x - by, self.y - by, w, h)
    }

    pub fn to_f(self) -> RectF {
        RectF::new(self.x as f32, self.y as f32, self.w as f32, self.h as f32)
    }

    /// Parse `x,y,w,h` (CLI / URL-scheme form).
    pub fn parse(s: &str) -> Option<Rect> {
        let v: Vec<i64> = s.split([',', ' ', 'x']).filter(|p| !p.is_empty()).map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
        match v.as_slice() {
            [x, y, w, h] if *w > 0 && *h > 0 => {
                let (x, y, w, h) = (i32::try_from(*x).ok()?, i32::try_from(*y).ok()?, i32::try_from(*w).ok()?, i32::try_from(*h).ok()?);
                // The far edge must fit too, or `right()` / `bottom()` overflow.
                x.checked_add(w)?;
                y.checked_add(h)?;
                Some(Rect::new(x, y, w as u32, h as u32))
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct PointF {
    pub x: f32,
    pub y: f32,
}

impl PointF {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
    pub fn dist(self, o: PointF) -> f32 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl RectF {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    pub fn from_corners(a: PointF, b: PointF) -> Self {
        Self::new(a.x.min(b.x), a.y.min(b.y), (a.x - b.x).abs(), (a.y - b.y).abs())
    }
    pub fn right(self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(self) -> f32 {
        self.y + self.h
    }
    pub fn center(self) -> PointF {
        PointF::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    pub fn contains(self, p: PointF) -> bool {
        p.x >= self.x && p.y >= self.y && p.x <= self.right() && p.y <= self.bottom()
    }
    pub fn union(self, o: RectF) -> RectF {
        let (x, y) = (self.x.min(o.x), self.y.min(o.y));
        RectF::new(x, y, self.right().max(o.right()) - x, self.bottom().max(o.bottom()) - y)
    }
    pub fn round(self) -> Rect {
        Rect::new(self.x.round() as i32, self.y.round() as i32, self.w.round().max(0.0) as u32, self.h.round().max(0.0) as u32)
    }
}

/// One display of a desktop that the OS lays out in points (macOS), with its place in the pixel space above.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointDisplay {
    /// OS display id (`CGDirectDisplayID`).
    pub id: u32,
    /// Top-left corner in global points (top-left origin, the main display at 0,0).
    pub origin: (f64, f64),
    /// Size in points.
    pub size: (f64, f64),
    /// Pixels per point.
    pub scale: f64,
    pub is_main: bool,
    /// Where the display's pixels sit in virtual-desktop space.
    pub rect: Rect,
}

impl PointDisplay {
    /// Global points to virtual-desktop pixels at this display's scale (the result may lie outside the display).
    pub fn to_pixels(&self, x: f64, y: f64) -> (f64, f64) {
        (self.rect.x as f64 + (x - self.origin.0) * self.scale, self.rect.y as f64 + (y - self.origin.1) * self.scale)
    }

    /// A rectangle in global points to virtual-desktop pixels at this display's scale.
    pub fn rect_to_pixels(&self, x: f64, y: f64, w: f64, h: f64) -> Rect {
        let (x0, y0) = self.to_pixels(x, y);
        let (x1, y1) = self.to_pixels(x + w, y + h);
        let (x0, y0) = (x0.round() as i32, y0.round() as i32);
        Rect::new(x0, y0, (x1.round() as i32 - x0).max(0) as u32, (y1.round() as i32 - y0).max(0) as u32)
    }

    /// Virtual-desktop pixels to global points at this display's scale.
    pub fn to_points(&self, x: f64, y: f64) -> (f64, f64) {
        (self.origin.0 + (x - self.rect.x as f64) / self.scale, self.origin.1 + (y - self.rect.y as f64) / self.scale)
    }

    fn contains_points(&self, x: f64, y: f64) -> bool {
        x >= self.origin.0 && y >= self.origin.1 && x < self.origin.0 + self.size.0 && y < self.origin.1 + self.size.1
    }

    fn points_distance(&self, x: f64, y: f64) -> f64 {
        let dx = (self.origin.0 - x).max(x - (self.origin.0 + self.size.0)).max(0.0);
        let dy = (self.origin.1 - y).max(y - (self.origin.1 + self.size.1)).max(0.0);
        dx.hypot(dy)
    }

    fn pixel_distance(&self, p: Point) -> f64 {
        let r = self.rect;
        let dx = (r.x - p.x).max(p.x - r.right()).max(0) as f64;
        let dy = (r.y - p.y).max(p.y - r.bottom()).max(0) as f64;
        dx.hypot(dy)
    }
}

/// Virtual-desktop pixels for an OS that lays displays out in points (macOS), shared by every crate so monitors,
/// windows, the cursor, overlay covers and recordings agree.
///
/// A display's pixels are its points times its own scale, but scaling each origin by its own scale makes a 1x display
/// right of a 2x one land inside the 2x display's pixels. So every origin is scaled by the largest scale `k` instead,
/// anchored at the edge that faces the main display: neighbours of the main display stay flush, no two displays
/// overlap, and a lower-scale display may leave a gap next to it that holds no pixels. With one scale everywhere
/// this is plain points times scale.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PointLayout {
    /// Main display first, then left to right and top to bottom.
    pub displays: Vec<PointDisplay>,
}

impl PointLayout {
    /// Lay out displays given as `(id, [x, y, w, h] in global points, pixels per point, is main)`.
    pub fn new(displays: impl IntoIterator<Item = (u32, [f64; 4], f64, bool)>) -> Self {
        let raw: Vec<_> = displays.into_iter().collect();
        let k = raw.iter().map(|d| d.2).fold(1.0, f64::max);
        // A display's own pixels extend from its k-scaled edge that faces the origin, away from it, so they stay
        // inside its k-scaled span, which no other display's span overlaps.
        let axis = |pos: f64, len: f64, scale: f64| if pos >= 0.0 { pos * k } else { (pos + len) * k - len * scale };
        let mut displays: Vec<PointDisplay> = raw
            .into_iter()
            .map(|(id, [x, y, w, h], scale, is_main)| PointDisplay {
                id,
                origin: (x, y),
                size: (w, h),
                scale,
                is_main,
                rect: Rect::new(
                    axis(x, w, scale).round() as i32,
                    axis(y, h, scale).round() as i32,
                    (w * scale).round() as u32,
                    (h * scale).round() as u32,
                ),
            })
            .collect();
        displays.sort_by_key(|d| (!d.is_main, d.rect.x, d.rect.y));
        Self { displays }
    }

    /// The display under a point given in global points, else the nearest one.
    pub fn display_at_points(&self, x: f64, y: f64) -> Option<&PointDisplay> {
        self.displays
            .iter()
            .find(|d| d.contains_points(x, y))
            .or_else(|| self.displays.iter().min_by(|a, b| a.points_distance(x, y).total_cmp(&b.points_distance(x, y))))
    }

    /// The display under virtual-desktop pixel `p`, else the nearest one.
    pub fn display_at(&self, p: Point) -> Option<&PointDisplay> {
        self.displays
            .iter()
            .find(|d| d.rect.contains(p))
            .or_else(|| self.displays.iter().min_by(|a, b| a.pixel_distance(p).total_cmp(&b.pixel_distance(p))))
    }

    /// Global points to virtual-desktop pixels, through the display the point lies on.
    pub fn to_pixels(&self, x: f64, y: f64) -> Point {
        let (px, py) = self.display_at_points(x, y).map_or((x, y), |d| d.to_pixels(x, y));
        Point::new(px.round() as i32, py.round() as i32)
    }

    /// A rectangle in global points to virtual-desktop pixels, through the display under its centre.
    pub fn rect_to_pixels(&self, x: f64, y: f64, w: f64, h: f64) -> Rect {
        match self.display_at_points(x + w / 2.0, y + h / 2.0) {
            Some(d) => d.rect_to_pixels(x, y, w, h),
            None => RectF::new(x as f32, y as f32, w as f32, h as f32).round(),
        }
    }

    /// Virtual-desktop pixels to global points, through the display the pixel lies on.
    pub fn to_points(&self, p: Point) -> (f64, f64) {
        self.display_at(p).map_or((p.x as f64, p.y as f64), |d| d.to_points(p.x as f64, p.y as f64))
    }
}

#[cfg(target_os = "macos")]
impl PointLayout {
    /// The displays connected right now, from CoreGraphics (any thread). Mirrors of another display are left out, as
    /// they show the same pixels.
    pub fn current() -> Self {
        use std::ffi::c_void;

        #[repr(C)]
        struct CGRect {
            x: f64,
            y: f64,
            w: f64,
            h: f64,
        }
        // CoreGraphics directly, without pulling a binding crate into core.
        #[link(name = "CoreGraphics", kind = "framework")]
        unsafe extern "C" {
            fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
            fn CGMainDisplayID() -> u32;
            fn CGDisplayBounds(display: u32) -> CGRect;
            fn CGDisplayMirrorsDisplay(display: u32) -> u32;
            fn CGDisplayCopyDisplayMode(display: u32) -> *mut c_void;
            fn CGDisplayModeGetWidth(mode: *mut c_void) -> usize;
            fn CGDisplayModeGetPixelWidth(mode: *mut c_void) -> usize;
            fn CGDisplayModeRelease(mode: *mut c_void);
        }

        let mut ids = [0u32; 32];
        let mut count = 0u32;
        // SAFETY: the buffer holds `ids.len()` entries and CoreGraphics writes at most that many.
        if unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) } != 0 {
            return Self::default();
        }
        // SAFETY: plain CoreGraphics queries on display ids it just returned; the copied mode is released.
        unsafe {
            let main = CGMainDisplayID();
            Self::new(ids[..count as usize].iter().filter(|&&id| CGDisplayMirrorsDisplay(id) == 0).map(|&id| {
                let b = CGDisplayBounds(id);
                let mode = CGDisplayCopyDisplayMode(id);
                // The current mode's pixels per point: 2 for a HiDPI mode, also on a rotated display.
                let scale = if mode.is_null() {
                    1.0
                } else {
                    let (points, pixels) = (CGDisplayModeGetWidth(mode), CGDisplayModeGetPixelWidth(mode));
                    CGDisplayModeRelease(mode);
                    if points > 0 && pixels > 0 { pixels as f64 / points as f64 } else { 1.0 }
                };
                (id, [b.x, b.y, b.w, b.h], scale, id == main)
            }))
        }
    }
}
