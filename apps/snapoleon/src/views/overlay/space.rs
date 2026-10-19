//! Coordinate spaces. The overlay thinks in physical pixels of the virtual desktop (what a capture
//! cuts out); egui thinks in points of one cover window. A cover window sits exactly over one
//! monitor, so the mapping is an offset and the window's pixels-per-point, nothing else.

use egui::{Pos2, Rect, pos2, vec2};
use snap_core::Rect as PxRect;

/// A physical-pixel rect as an egui rect (for float math).
pub fn rect_f(r: PxRect) -> Rect {
    Rect::from_min_size(pos2(r.x as f32, r.y as f32), vec2(r.w as f32, r.h as f32))
}

/// The nearest whole-pixel rect.
pub fn rect_px(r: Rect) -> PxRect {
    let (x0, y0) = (r.min.x.round(), r.min.y.round());
    let (x1, y1) = (r.max.x.round(), r.max.y.round());
    PxRect::new(x0 as i32, y0 as i32, (x1 - x0).max(0.0) as u32, (y1 - y0).max(0.0) as u32)
}

/// The pixel under a global position.
pub fn pixel_at(p: Pos2) -> snap_core::Point {
    snap_core::Point::new(p.x.floor() as i32, p.y.floor() as i32)
}

/// Maps between one cover window (points) and the virtual desktop (physical pixels).
#[derive(Clone, Copy, Debug)]
pub struct Space {
    /// The monitor's top-left in virtual-desktop pixels.
    pub origin: Pos2,
    pub ppp: f32,
}

impl Space {
    pub fn new(monitor: PxRect, ppp: f32) -> Self {
        Self { origin: pos2(monitor.x as f32, monitor.y as f32), ppp }
    }

    /// Window point to virtual-desktop pixels.
    pub fn global(self, local: Pos2) -> Pos2 {
        self.origin + local.to_vec2() * self.ppp
    }

    /// Virtual-desktop pixels to window point.
    pub fn local(self, global: Pos2) -> Pos2 {
        ((global - self.origin) / self.ppp).to_pos2()
    }

    pub fn local_rect(self, r: Rect) -> Rect {
        Rect::from_min_max(self.local(r.min), self.local(r.max))
    }

    /// One physical pixel in points: the width of a crisp 1 px line.
    pub fn px(self) -> f32 {
        1.0 / self.ppp
    }

    /// Centre of the physical pixel under `global`, in window points. Lines drawn here are one crisp pixel.
    pub fn pixel_center(self, global: Pos2) -> Pos2 {
        self.local(pos2(global.x.floor() + 0.5, global.y.floor() + 0.5))
    }
}
