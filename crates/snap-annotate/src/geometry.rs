//! Shape geometry: arrows, smoothed strokes, rounded rectangles, speech bubbles and text boxes.

use std::f32::consts::PI;

use snap_core::{PointF, RectF};
use tiny_skia::{Path, PathBuilder};

use crate::font::{self, Face, Layout};
use crate::math::{Scale, V};
use crate::model::{ArrowHead, TextBg};

pub(crate) fn build(f: impl FnOnce(&mut PathBuilder)) -> Option<Path> {
    let mut pb = PathBuilder::new();
    f(&mut pb);
    pb.finish()
}

pub(crate) fn polygon_path(pts: &[V]) -> Option<Path> {
    build(|pb| {
        let Some(first) = pts.first() else { return };
        pb.move_to(first.0, first.1);
        for p in &pts[1..] {
            pb.line_to(p.0, p.1);
        }
        pb.close();
    })
}

pub(crate) fn polyline_path(pts: &[V]) -> Option<Path> {
    build(|pb| {
        let Some(first) = pts.first() else { return };
        pb.move_to(first.0, first.1);
        for p in &pts[1..] {
            pb.line_to(p.0, p.1);
        }
    })
}

/// Clockwise (in a y-down space) rounded rectangle.
pub(crate) fn push_rounded_rect(pb: &mut PathBuilder, r: RectF, radius: f32) {
    let rad = radius.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
    let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
    if rad < 0.05 {
        pb.move_to(x0, y0);
        pb.line_to(x1, y0);
        pb.line_to(x1, y1);
        pb.line_to(x0, y1);
        pb.close();
        return;
    }
    let c = rad * 0.552_284_8;
    pb.move_to(x0 + rad, y0);
    pb.line_to(x1 - rad, y0);
    pb.cubic_to(x1 - rad + c, y0, x1, y0 + rad - c, x1, y0 + rad);
    pb.line_to(x1, y1 - rad);
    pb.cubic_to(x1, y1 - rad + c, x1 - rad + c, y1, x1 - rad, y1);
    pb.line_to(x0 + rad, y1);
    pb.cubic_to(x0 + rad - c, y1, x0, y1 - rad + c, x0, y1 - rad);
    pb.line_to(x0, y0 + rad);
    pb.cubic_to(x0, y0 + rad - c, x0 + rad - c, y0, x0 + rad, y0);
    pb.close();
}

pub(crate) fn rounded_rect_path(r: RectF, radius: f32) -> Option<Path> {
    build(|pb| push_rounded_rect(pb, r, radius))
}

/// Catmull-Rom spline through `points`, converted to cubic segments. Coincident points are dropped.
pub(crate) fn smooth_path(points: &[PointF]) -> Option<Path> {
    let mut pts: Vec<V> = Vec::with_capacity(points.len());
    for p in points {
        let v = V::from(*p);
        if pts.last().is_none_or(|l| (*l - v).len() > 0.01) {
            pts.push(v);
        }
    }
    let n = pts.len();
    if n < 2 {
        return None;
    }
    build(|pb| {
        pb.move_to(pts[0].0, pts[0].1);
        for i in 0..n - 1 {
            let (p0, p1, p2, p3) = (pts[i.saturating_sub(1)], pts[i], pts[i + 1], pts[(i + 2).min(n - 1)]);
            let c1 = p1 + (p2 - p0) * (1.0 / 6.0);
            let c2 = p2 - (p3 - p1) * (1.0 / 6.0);
            pb.cubic_to(c1.0, c1.1, c2.0, c2.1, p2.0, p2.1);
        }
    })
}

// ---------------------------------------------------------------------------------------------
// Arrows
// ---------------------------------------------------------------------------------------------

/// Fully resolved arrow outline in document space.
#[derive(Debug, Default)]
pub(crate) struct ArrowGeom {
    /// Closed polygon of the tapered shaft and any triangular heads.
    pub poly: Vec<V>,
    /// Open-head chevrons: `[end, tip, end]`.
    pub chevrons: Vec<[V; 3]>,
    pub chevron_width: f32,
}

struct Curve {
    p0: V,
    c: V,
    p1: V,
    /// Cumulative arc length at evenly spaced parameters.
    table: Vec<f32>,
}

const TABLE_STEPS: usize = 64;

impl Curve {
    fn new(from: V, to: V, ctrl: Option<V>) -> Self {
        let c = ctrl.unwrap_or_else(|| from.lerp(to, 0.5));
        let mut curve = Curve { p0: from, c, p1: to, table: Vec::with_capacity(TABLE_STEPS + 1) };
        let mut acc = 0.0;
        let mut prev = from;
        curve.table.push(0.0);
        for i in 1..=TABLE_STEPS {
            let p = curve.at(i as f32 / TABLE_STEPS as f32);
            acc += (p - prev).len();
            curve.table.push(acc);
            prev = p;
        }
        curve
    }

    fn length(&self) -> f32 {
        self.table[TABLE_STEPS]
    }

    fn at(&self, t: f32) -> V {
        let u = 1.0 - t;
        self.p0 * (u * u) + self.c * (2.0 * u * t) + self.p1 * (t * t)
    }

    fn tangent(&self, t: f32) -> V {
        ((self.c - self.p0) * (1.0 - t) + (self.p1 - self.c) * t).norm()
    }

    /// Parameter at arc length `s`.
    fn t_at(&self, s: f32) -> f32 {
        let s = s.clamp(0.0, self.length());
        let i = self.table.partition_point(|&l| l < s).clamp(1, TABLE_STEPS);
        let (a, b) = (self.table[i - 1], self.table[i]);
        let f = if b > a { (s - a) / (b - a) } else { 0.0 };
        ((i - 1) as f32 + f) / TABLE_STEPS as f32
    }
}

/// Arrow outline for a stroke width `w` (already readable-adjusted).
///
/// The shaft is a smooth wedge: hairline-thin at the rounded tail and widening towards the head,
/// which is a sharp triangle. Open heads are two round-capped strokes; double arrows taper the
/// shaft towards its middle.
pub(crate) fn arrow_geom(from: PointF, to: PointF, ctrl: Option<PointF>, head: ArrowHead, double: bool, w: f32) -> ArrowGeom {
    let curve = Curve::new(from.into(), to.into(), ctrl.map(V::from));
    let len = curve.length();
    if !len.is_finite() || len < 0.5 {
        return ArrowGeom::default();
    }
    let heads = if double { 2.0 } else { 1.0 };
    let tri = head == ArrowHead::Triangle;
    let fac = (len * if tri { 0.5 / heads } else { 0.4 / heads } / (w * 4.4 + 6.0)).min(1.0);
    let head_len = (w * 4.4 + 6.0) * fac;
    let head_hw = (w * 1.6 + 3.0) * fac;
    let end_hw = (w * 0.5).min(head_hw * 0.45).max(0.4);
    let tail_hw = (w * 0.2).max(0.7).min(end_hw);

    let (s0, s1) = match head {
        ArrowHead::Triangle => (if double { head_len } else { 0.0 }, len - head_len),
        _ => (0.0, len),
    };
    let tip_end: V = to.into();
    let tip_start: V = from.into();

    let n = if ctrl.is_some() { ((len / 2.0) as usize).clamp(12, 96) } else { 1 };
    let mut left = Vec::with_capacity(n + 1);
    let mut right = Vec::with_capacity(n + 1);
    let mut centers = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let u = i as f32 / n as f32;
        let t = curve.t_at(s0 + (s1 - s0) * u);
        let hw = if double { tail_hw + (end_hw - tail_hw) * (2.0 * u - 1.0).abs() } else { tail_hw + (end_hw - tail_hw) * u };
        let (pt, mut normal) = (curve.at(t), curve.tangent(t).perp());
        if tri && i == n {
            normal = (tip_end - pt).norm().perp();
        }
        if tri && double && i == 0 {
            normal = (pt - tip_start).norm().perp();
        }
        left.push(pt + normal * hw);
        right.push(pt - normal * hw);
        centers.push((pt, normal, hw));
    }

    let mut poly = left;
    let (last_c, last_n, last_hw) = *centers.last().unwrap();
    if tri {
        poly.push(last_c + last_n * head_hw);
        poly.push(tip_end);
        poly.push(last_c - last_n * head_hw);
    } else {
        let dir = last_n.perp() * -1.0;
        cap(&mut poly, last_c, last_n, dir, last_hw, true);
    }
    poly.extend(right.iter().rev());
    let (first_c, first_n, first_hw) = centers[0];
    if tri && double {
        poly.push(first_c - first_n * head_hw);
        poly.push(tip_start);
        poly.push(first_c + first_n * head_hw);
    } else {
        let dir = first_n.perp();
        cap(&mut poly, first_c, first_n, dir, first_hw, false);
    }

    let mut geom = ArrowGeom { poly, chevrons: Vec::new(), chevron_width: 0.0 };
    if head == ArrowHead::Open {
        geom.chevron_width = (w * 0.85).max(1.2);
        let reach = head_len * 1.05;
        let mut chevron = |tip: V, back: V| {
            let d = (tip - back).norm();
            geom.chevrons.push([tip - d.rot(0.5) * reach, tip, tip - d.rot(-0.5) * reach]);
        };
        chevron(tip_end, curve.at(curve.t_at(len - head_len)));
        if double {
            chevron(tip_start, curve.at(curve.t_at(head_len)));
        }
    }
    geom
}

/// Appends a semicircular cap at `c`. `normal` is the side we arrive from, `dir` the outward direction.
fn cap(poly: &mut Vec<V>, c: V, normal: V, dir: V, r: f32, forward: bool) {
    const STEPS: usize = 8;
    let side = if forward { normal } else { -normal };
    for i in 1..STEPS {
        let a = PI * i as f32 / STEPS as f32;
        poly.push(c + (side * a.cos() + dir * a.sin()) * r);
    }
}

pub(crate) fn polygon_bounds(pts: &[V]) -> Option<RectF> {
    let first = pts.first()?;
    let (mut x0, mut y0, mut x1, mut y1) = (first.0, first.1, first.0, first.1);
    for p in pts {
        x0 = x0.min(p.0);
        y0 = y0.min(p.1);
        x1 = x1.max(p.0);
        y1 = y1.max(p.1);
    }
    Some(RectF::new(x0, y0, x1 - x0, y1 - y0))
}

pub(crate) fn union(a: RectF, b: RectF) -> RectF {
    let (x0, y0) = (a.x.min(b.x), a.y.min(b.y));
    RectF::new(x0, y0, a.right().max(b.right()) - x0, a.bottom().max(b.bottom()) - y0)
}

pub(crate) fn inflate(r: RectF, by: f32) -> RectF {
    RectF::new(r.x - by, r.y - by, r.w + 2.0 * by, r.h + 2.0 * by)
}

// ---------------------------------------------------------------------------------------------
// Bubbles and text boxes
// ---------------------------------------------------------------------------------------------

/// Rounded rectangle plus an optional triangular tail pointing at `tip`.
pub(crate) fn bubble_path(rect: RectF, radius: f32, tip: Option<V>, base_w: f32) -> Option<Path> {
    build(|pb| {
        push_rounded_rect(pb, rect, radius);
        if let Some(tail) = tip.and_then(|t| tail_triangle(rect, radius, t, base_w)) {
            pb.move_to(tail[0].0, tail[0].1);
            pb.line_to(tail[1].0, tail[1].1);
            pb.line_to(tail[2].0, tail[2].1);
            pb.close();
        }
    })
}

/// Tail base on the edge of `rect` nearest to `tip`, wound clockwise like [`push_rounded_rect`].
pub(crate) fn tail_triangle(rect: RectF, radius: f32, tip: V, base_w: f32) -> Option<[V; 3]> {
    if rect.contains(tip.pf()) {
        return None;
    }
    let c = V::from(rect.center());
    let d = tip - c;
    let (ax, ay) = (d.0.abs() / (rect.w / 2.0).max(1.0), d.1.abs() / (rect.h / 2.0).max(1.0));
    let horizontal_edge = ay >= ax;
    let (len, lo) = if horizontal_edge { (rect.w, rect.x) } else { (rect.h, rect.y) };
    let half = base_w.min(len * 0.45) / 2.0;
    let margin = (radius + half).min(len / 2.0);
    let along = if horizontal_edge { tip.0 } else { tip.1 };
    let mid = along.clamp(lo + margin, lo + len - margin);
    let (a, b) = if horizontal_edge {
        let y = if d.1 > 0.0 { rect.bottom() } else { rect.y };
        (V(mid - half, y), V(mid + half, y))
    } else {
        let x = if d.0 > 0.0 { rect.right() } else { rect.x };
        (V(x, mid - half), V(x, mid + half))
    };
    let area = (b - a).0 * (tip - a).1 - (b - a).1 * (tip - a).0;
    Some(if area >= 0.0 { [a, b, tip] } else { [b, a, tip] })
}

/// A laid-out text shape.
pub(crate) struct TextBox {
    pub layout: Layout,
    pub face: Face,
    /// Top-left of the first line.
    pub origin: V,
    /// Background rectangle (equals the text rectangle without a background).
    pub outer: RectF,
    pub radius: f32,
    pub tip: Option<V>,
    pub base_w: f32,
}

impl TextBox {
    /// Box including the bubble tail.
    pub fn bounds(&self) -> RectF {
        match self.tip {
            Some(t) => union(self.outer, RectF::new(t.0, t.1, 0.0, 0.0)),
            None => self.outer,
        }
    }
}

pub(crate) fn text_box(pos: PointF, text: &str, size: f32, bold: bool, bg: TextBg, max_width: Option<f32>, sc: Scale) -> TextBox {
    let size = sc.text(size);
    let face = Face::pick(bold);
    let layout = font::layout(face, text, size, max_width);
    let (w, h) = (layout.width, layout.height());
    let origin = V(pos.x, pos.y);
    let (px, py) = (size * 0.45, size * 0.25);
    let (outer, radius) = match bg {
        TextBg::None => (RectF::new(pos.x, pos.y, w, h), 0.0),
        _ => (RectF::new(pos.x - px, pos.y - py, w + 2.0 * px, h + 2.0 * py), size * 0.4),
    };
    let base_w = size * 0.9;
    let tip = (bg == TextBg::Bubble).then(|| V(outer.x + radius + size * 0.35, outer.bottom() + size * 0.75));
    TextBox { layout, face, origin, outer, radius, tip, base_w }
}

/// Width of the outline around plain text that has a fill colour (half of it lies outside the glyphs).
pub(crate) fn text_outline_width(size: f32) -> f32 {
    (size * 0.16).max(1.5)
}

/// Text inside a callout rectangle: wrapped to the padded width and centred vertically.
pub(crate) fn callout_box(rect: RectF, tail: PointF, text: &str, size: f32, sc: Scale) -> TextBox {
    let size = sc.text(size);
    let face = Face::Regular;
    let pad = size * 0.6;
    let layout = font::layout(face, text, size, Some((rect.w - 2.0 * pad).max(size)));
    let h = layout.height();
    let origin = V(rect.x + pad, rect.y + ((rect.h - h) / 2.0).max(size * 0.3));
    TextBox { layout, face, origin, outer: rect, radius: size * 0.6, tip: Some(tail.into()), base_w: size * 1.1 }
}

// ---------------------------------------------------------------------------------------------
// Measure and magnifier
// ---------------------------------------------------------------------------------------------

pub(crate) struct MeasureGeom {
    pub a: V,
    pub b: V,
    pub line_w: f32,
    pub tick: f32,
    pub pill: RectF,
    pub label: String,
    pub font_size: f32,
    pub text_x: f32,
    pub baseline: f32,
}

pub(crate) fn measure_geom(from: PointF, to: PointF, width: f32, sc: Scale) -> MeasureGeom {
    let (a, b) = (V::from(from), V::from(to));
    let line_w = sc.stroke((width * 0.5).max(1.5));
    let label = format!("{} px", (b - a).len().round() as i64);
    let font_size = sc.text(12.5);
    let adv = font::advance(Face::Mono, &label, font_size);
    let (pw, ph) = (adv + font_size * 1.2, font_size * 1.6);
    let mid = a.lerp(b, 0.5);
    MeasureGeom {
        a,
        b,
        line_w,
        tick: 5.0 + line_w * 1.5,
        pill: RectF::new(mid.0 - pw / 2.0, mid.1 - ph / 2.0, pw, ph),
        label,
        font_size,
        text_x: mid.0 - adv / 2.0,
        baseline: mid.1 + Face::Mono.cap_height() * font_size / 2.0,
    }
}

pub(crate) struct MagGeom {
    pub center: V,
    pub radius: f32,
    /// Magnification of the source pixels inside the circle.
    pub zoom: f32,
    pub ring_w: f32,
    pub source: RectF,
    /// Segment from the source rectangle's edge to the circle's edge.
    pub connector: Option<(V, V)>,
}

pub(crate) fn magnifier_geom(source: RectF, center: PointF, radius: f32, sc: Scale) -> MagGeom {
    let source = norm_rect(source);
    let radius = radius.max(4.0);
    let zoom = 2.0 * radius / source.w.max(source.h).max(1.0);
    let c = V::from(center);
    let s = V::from(source.center());
    let d = c - s;
    let dist = d.len();
    let connector = (dist > radius + 2.0).then(|| {
        let dir = d.norm();
        let tx = if dir.0.abs() > 1e-4 { source.w / 2.0 / dir.0.abs() } else { f32::MAX };
        let ty = if dir.1.abs() > 1e-4 { source.h / 2.0 / dir.1.abs() } else { f32::MAX };
        (s + dir * tx.min(ty), c - dir * radius)
    });
    MagGeom { center: c, radius, zoom, ring_w: sc.stroke(2.0), source, connector: connector.filter(|(a, b)| (*b - *a).dot(d) > 0.0) }
}

pub(crate) fn norm_rect(r: RectF) -> RectF {
    RectF::new(r.x.min(r.right()), r.y.min(r.bottom()), r.w.abs(), r.h.abs())
}
