//! Thin helpers over tiny-skia's painter.

use tiny_skia::{BlendMode, FillRule, LineCap, LineJoin, Paint, Path, Pixmap, Stroke, StrokeDash, Transform};

use crate::math::V;

pub(crate) fn solid(c: [u8; 4], alpha: f32) -> Paint<'static> {
    let mut p = Paint::default();
    let a = (c[3] as f32 * alpha).round().clamp(0.0, 255.0) as u8;
    p.set_color_rgba8(c[0], c[1], c[2], a);
    p.anti_alias = true;
    p
}

pub(crate) struct StrokeSpec {
    pub width: f32,
    pub cap: LineCap,
    pub join: LineJoin,
    pub dashed: bool,
}

impl StrokeSpec {
    pub fn round(width: f32) -> Self {
        Self { width, cap: LineCap::Round, join: LineJoin::Round, dashed: false }
    }

    fn build(&self) -> Stroke {
        let mut s = Stroke { width: self.width, line_cap: self.cap, line_join: self.join, ..Stroke::default() };
        if self.dashed {
            s.line_cap = LineCap::Butt;
            s.dash = StrokeDash::new(vec![self.width * 3.0, self.width * 2.0], 0.0);
        }
        s
    }
}

/// A pixmap plus the document-to-pixmap transform every shape is drawn through.
pub(crate) struct Canvas<'a> {
    pub pm: &'a mut Pixmap,
    pub ts: Transform,
}

impl Canvas<'_> {
    pub fn fill(&mut self, path: &Path, color: [u8; 4]) {
        self.pm.fill_path(path, &solid(color, 1.0), FillRule::Winding, self.ts, None);
    }

    pub fn stroke(&mut self, path: &Path, color: [u8; 4], spec: &StrokeSpec) {
        self.pm.stroke_path(path, &solid(color, 1.0), &spec.build(), self.ts, None);
    }

    pub fn stroke_blend(&mut self, path: &Path, color: [u8; 4], alpha: f32, spec: &StrokeSpec, mode: BlendMode) {
        let mut paint = solid(color, alpha);
        paint.blend_mode = mode;
        self.pm.stroke_path(path, &paint, &spec.build(), self.ts, None);
    }

    pub fn fill_circle(&mut self, c: V, r: f32, color: [u8; 4]) {
        if let Some(p) = tiny_skia::PathBuilder::from_circle(c.0, c.1, r) {
            self.fill(&p, color);
        }
    }
}

/// Rounds a document-space point to the output pixel grid (axis-aligned transforms only).
pub(crate) fn snap(ts: &Transform, p: V) -> V {
    V(((p.0 * ts.sx + ts.tx).round() - ts.tx) / ts.sx, ((p.1 * ts.sy + ts.ty).round() - ts.ty) / ts.sy)
}
