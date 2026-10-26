//! Small vector math and the readable-export scale rules shared by geometry and rendering.

use std::ops::{Add, Mul, Neg, Sub};

use snap_core::PointF;

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub(crate) struct V(pub f32, pub f32);

impl Add for V {
    type Output = V;
    fn add(self, o: V) -> V {
        V(self.0 + o.0, self.1 + o.1)
    }
}
impl Sub for V {
    type Output = V;
    fn sub(self, o: V) -> V {
        V(self.0 - o.0, self.1 - o.1)
    }
}
impl Mul<f32> for V {
    type Output = V;
    fn mul(self, k: f32) -> V {
        V(self.0 * k, self.1 * k)
    }
}
impl Neg for V {
    type Output = V;
    fn neg(self) -> V {
        V(-self.0, -self.1)
    }
}
impl From<PointF> for V {
    fn from(p: PointF) -> V {
        V(p.x, p.y)
    }
}

impl V {
    pub fn pf(self) -> PointF {
        PointF::new(self.0, self.1)
    }
    pub fn len(self) -> f32 {
        self.0.hypot(self.1)
    }
    pub fn dot(self, o: V) -> f32 {
        self.0 * o.0 + self.1 * o.1
    }
    /// Unit vector, or zero for a degenerate input.
    pub fn norm(self) -> V {
        let l = self.len();
        if l < 1e-6 { V(0.0, 0.0) } else { self * (1.0 / l) }
    }
    /// 90 degrees clockwise in a y-down space.
    pub fn perp(self) -> V {
        V(-self.1, self.0)
    }
    pub fn rot(self, a: f32) -> V {
        let (s, c) = a.sin_cos();
        V(self.0 * c - self.1 * s, self.0 * s + self.1 * c)
    }
    pub fn lerp(self, o: V, t: f32) -> V {
        self + (o - self) * t
    }
}

/// Scale-dependent minimum sizes. At export scales below 1 with `readable` set, thin strokes,
/// small text and small badges are enlarged so they stay legible after the downscale.
/// Every method returns document units (the caller's transform applies `z` afterwards).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Scale {
    pub z: f32,
    pub readable: bool,
}

impl Scale {
    pub const IDENTITY: Scale = Scale { z: 1.0, readable: false };

    fn active(self) -> bool {
        self.readable && self.z < 1.0
    }

    pub fn stroke(self, w: f32) -> f32 {
        if !self.active() {
            return w;
        }
        let s = self.z;
        (w * s).max(w * s.sqrt() * 0.75).max(1.5) / s
    }

    pub fn text(self, size: f32) -> f32 {
        if self.active() { (size * self.z).max(11.0) / self.z } else { size }
    }

    pub fn badge(self, r: f32) -> f32 {
        if self.active() { (r * self.z).max(9.0) / self.z } else { r }
    }

    pub fn stamp(self, d: f32) -> f32 {
        if self.active() { (d * self.z).max(18.0) / self.z } else { d }
    }
}

/// White or near-black, whichever reads better on `bg`; white wins while its contrast ratio
/// against `bg` is at least `min_white`.
fn ink_on(bg: [u8; 4], min_white: f32) -> [u8; 4] {
    const WHITE: [u8; 4] = [255, 255, 255, 255];
    if snap_image::color::contrast_ratio(WHITE, [bg[0], bg[1], bg[2], 255]) >= min_white { WHITE } else { [20, 20, 22, 255] }
}

/// Ink for body text on a colored box.
pub fn contrasting(bg: [u8; 4]) -> [u8; 4] {
    ink_on(bg, 3.0)
}

/// Ink for bold badge numbers and stamp glyphs: white on all but light fills.
pub(crate) fn badge_ink(bg: [u8; 4]) -> [u8; 4] {
    ink_on(bg, 1.8)
}
