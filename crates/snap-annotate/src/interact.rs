//! Editor interaction: bounds, hit-testing, handles and dragging.

use snap_core::{PointF, RectF};

use crate::geometry::{self, arrow_geom, callout_box, inflate, magnifier_geom, measure_geom, norm_rect, polygon_bounds, text_box, union};
use crate::math::{Scale, V};
use crate::model::{Document, Shape, ShapeKind, TextBg};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Handle {
    /// Moves the whole shape. Accepted by [`drag`] for every shape; never listed by [`handles`].
    Body,
    Start,
    End,
    /// Arrow bend; positioned on the curve's midpoint so dragging it feels direct.
    Ctrl,
    /// 0 = top-left, 1 = top-right, 2 = bottom-right, 3 = bottom-left.
    Corner(u8),
    /// 0 = top, 1 = right, 2 = bottom, 3 = left.
    Edge(u8),
    Tail,
}

/// Visual extent of the shape in document pixels (stroke, heads, bubbles and tails included).
pub fn bounds(shape: &Shape) -> RectF {
    bounds_with(shape, Scale::IDENTITY)
}

pub(crate) fn bounds_with(shape: &Shape, sc: Scale) -> RectF {
    let w = sc.stroke(shape.style.width);
    match &shape.kind {
        ShapeKind::Arrow { from, to, ctrl, head, double } => {
            let g = arrow_geom(*from, *to, *ctrl, *head, *double, w);
            let mut b = polygon_bounds(&g.poly).unwrap_or(RectF::new(from.x, from.y, 0.0, 0.0));
            for c in &g.chevrons {
                if let Some(cb) = polygon_bounds(c) {
                    b = union(b, inflate(cb, g.chevron_width / 2.0));
                }
            }
            inflate(b, 3.0)
        }
        ShapeKind::Line { from, to } => inflate(RectF::from_corners(*from, *to), w / 2.0 + 1.0),
        ShapeKind::Rect { rect, .. } | ShapeKind::Ellipse { rect } => inflate(norm_rect(*rect), w / 2.0 + 1.0),
        ShapeKind::Pen { points } => points_bounds(points, w / 2.0 + 1.5),
        ShapeKind::Highlighter { points } => points_bounds(points, w * 1.5 + 1.5),
        ShapeKind::Text { pos, text, size, bold, bg, max_width } => {
            inflate(text_box(*pos, text, *size, *bold, *bg, *max_width, sc).bounds(), 1.0)
        }
        ShapeKind::Callout { rect, tail, text, size } => inflate(callout_box(norm_rect(*rect), *tail, text, *size, sc).bounds(), 1.0),
        ShapeKind::Step { center, radius, .. } => circle_bounds(*center, sc.badge(*radius) * 1.15 + 2.0),
        ShapeKind::Stamp { center, size, .. } => circle_bounds(*center, sc.stamp(*size) / 2.0 + 1.0),
        ShapeKind::Redact { rect, .. } | ShapeKind::Spotlight { rect, .. } | ShapeKind::Image { rect, .. } => norm_rect(*rect),
        ShapeKind::Magnifier { source, center, radius } => {
            let g = magnifier_geom(*source, *center, *radius, sc);
            let circle = circle_bounds(g.center.pf(), g.radius + g.ring_w);
            inflate(union(circle, inflate(g.source, 2.0)), 2.0)
        }
        ShapeKind::Measure { from, to } => {
            let g = measure_geom(*from, *to, shape.style.width, sc);
            let line = inflate(RectF::from_corners(*from, *to), g.tick + g.line_w);
            union(line, g.pill)
        }
    }
}

fn circle_bounds(c: PointF, r: f32) -> RectF {
    RectF::new(c.x - r, c.y - r, 2.0 * r, 2.0 * r)
}

fn points_bounds(points: &[PointF], pad: f32) -> RectF {
    let Some(first) = points.first() else { return RectF::default() };
    let mut r = RectF::new(first.x, first.y, 0.0, 0.0);
    for p in points {
        r = union(r, RectF::new(p.x, p.y, 0.0, 0.0));
    }
    inflate(r, pad)
}

// ---------------------------------------------------------------------------------------------
// Hit testing
// ---------------------------------------------------------------------------------------------

/// Top-most visible shape under `p`; `tol` is the extra grab distance in document pixels.
pub fn hit_test(doc: &Document, p: PointF, tol: f32) -> Option<u64> {
    doc.shapes.iter().rev().find(|s| !s.hidden && hits(s, p, tol)).map(|s| s.id)
}

fn hits(shape: &Shape, p: PointF, tol: f32) -> bool {
    if !inflate(bounds(shape), tol).contains(p) {
        return false;
    }
    let sc = Scale::IDENTITY;
    let w = shape.style.width;
    let pv = V::from(p);
    let filled = shape.style.fill.is_some_and(|f| f[3] > 0);
    match &shape.kind {
        ShapeKind::Arrow { from, to, ctrl, head, double } => {
            let g = arrow_geom(*from, *to, *ctrl, *head, *double, w);
            let near_poly = point_in_polygon(&g.poly, pv) || ring_distance(&g.poly, pv) <= tol;
            near_poly || g.chevrons.iter().any(|c| polyline_distance(c, pv) <= tol + g.chevron_width / 2.0)
        }
        ShapeKind::Line { from, to } => segment_distance((*from).into(), (*to).into(), pv) <= tol + w / 2.0,
        ShapeKind::Rect { rect, radius } => {
            let r = norm_rect(*rect);
            (filled && r.contains(p)) || rect_border_distance(r, *radius, pv) <= tol + w / 2.0
        }
        ShapeKind::Ellipse { rect } => {
            let r = norm_rect(*rect);
            if filled && ellipse_value(r, pv) <= 1.0 {
                return true;
            }
            ellipse_border_distance(r, pv) <= tol + w / 2.0
        }
        ShapeKind::Pen { points } => points_distance(points, pv) <= tol + w / 2.0,
        ShapeKind::Highlighter { points } => points_distance(points, pv) <= tol + w * 1.5,
        ShapeKind::Text { pos, text, size, bold, bg, max_width } => {
            inflate(text_box(*pos, text, *size, *bold, *bg, *max_width, sc).outer, tol).contains(p)
                || (*bg == TextBg::Bubble && bubble_tail_hit(shape, p, tol))
        }
        ShapeKind::Callout { rect, tail, text, size } => {
            let tb = callout_box(norm_rect(*rect), *tail, text, *size, sc);
            inflate(tb.outer, tol).contains(p)
                || tb.tip.and_then(|t| geometry::tail_triangle(tb.outer, tb.radius, t, tb.base_w)).is_some_and(|t| {
                    point_in_polygon(&t, pv) || ring_distance(&t, pv) <= tol
                })
        }
        ShapeKind::Step { center, radius, .. } => center.dist(p) <= *radius + tol,
        ShapeKind::Stamp { center, size, .. } => center.dist(p) <= size / 2.0 + tol,
        ShapeKind::Redact { rect, .. } | ShapeKind::Image { rect, .. } => inflate(norm_rect(*rect), tol).contains(p),
        ShapeKind::Spotlight { rect, ellipse } => {
            let r = norm_rect(*rect);
            if *ellipse { ellipse_value(inflate(r, tol), pv) <= 1.0 } else { inflate(r, tol).contains(p) }
        }
        ShapeKind::Magnifier { source, center, radius } => {
            let g = magnifier_geom(*source, *center, *radius, sc);
            center.dist(p) <= g.radius + g.ring_w + tol
                || rect_border_distance(g.source, 0.0, pv) <= tol + 1.0
                || g.connector.is_some_and(|(a, b)| segment_distance(a, b, pv) <= tol + 1.0)
        }
        ShapeKind::Measure { from, to } => {
            let g = measure_geom(*from, *to, w, sc);
            inflate(g.pill, tol).contains(p)
                || segment_distance(g.a, g.b, pv) <= tol + g.line_w / 2.0
                || [g.a, g.b].iter().any(|e| {
                    let d = (g.b - g.a).norm().perp() * g.tick;
                    segment_distance(*e - d, *e + d, pv) <= tol + g.line_w / 2.0
                })
        }
    }
}

fn bubble_tail_hit(shape: &Shape, p: PointF, tol: f32) -> bool {
    let ShapeKind::Text { pos, text, size, bold, bg, max_width } = &shape.kind else { return false };
    let tb = text_box(*pos, text, *size, *bold, *bg, *max_width, Scale::IDENTITY);
    let Some(tip) = tb.tip else { return false };
    geometry::tail_triangle(tb.outer, tb.radius, tip, tb.base_w)
        .is_some_and(|t| point_in_polygon(&t, p.into()) || ring_distance(&t, p.into()) <= tol)
}

fn segment_distance(a: V, b: V, p: V) -> f32 {
    let ab = b - a;
    let l2 = ab.dot(ab);
    let t = if l2 < 1e-9 { 0.0 } else { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) };
    (p - (a + ab * t)).len()
}

fn polyline_distance(pts: &[V], p: V) -> f32 {
    pts.windows(2).map(|s| segment_distance(s[0], s[1], p)).fold(f32::MAX, f32::min)
}

fn ring_distance(pts: &[V], p: V) -> f32 {
    let closing = match (pts.first(), pts.last()) {
        (Some(a), Some(b)) => segment_distance(*b, *a, p),
        _ => f32::MAX,
    };
    polyline_distance(pts, p).min(closing)
}

fn points_distance(points: &[PointF], p: V) -> f32 {
    let pts: Vec<V> = points.iter().map(|q| V::from(*q)).collect();
    match pts.len() {
        0 => f32::MAX,
        1 => (pts[0] - p).len(),
        _ => polyline_distance(&pts, p),
    }
}

fn point_in_polygon(poly: &[V], p: V) -> bool {
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
    }
    inside
}

fn rect_border_distance(r: RectF, radius: f32, p: V) -> f32 {
    let c = V::from(r.center());
    let q = V((p.0 - c.0).abs() - (r.w / 2.0 - radius), (p.1 - c.1).abs() - (r.h / 2.0 - radius));
    let outside = V(q.0.max(0.0), q.1.max(0.0)).len();
    let signed = outside + q.0.max(q.1).min(0.0) - radius;
    signed.abs()
}

/// Implicit ellipse function: 1 on the border, below 1 inside.
fn ellipse_value(r: RectF, p: V) -> f32 {
    let (a, b) = ((r.w / 2.0).max(0.5), (r.h / 2.0).max(0.5));
    let c = r.center();
    ((p.0 - c.x) / a).powi(2) + ((p.1 - c.y) / b).powi(2)
}

/// First-order distance to the ellipse border.
fn ellipse_border_distance(r: RectF, p: V) -> f32 {
    let (a, b) = ((r.w / 2.0).max(0.5), (r.h / 2.0).max(0.5));
    let c = r.center();
    let (dx, dy) = (p.0 - c.x, p.1 - c.y);
    let f = (dx / a).powi(2) + (dy / b).powi(2) - 1.0;
    let grad = V(2.0 * dx / (a * a), 2.0 * dy / (b * b)).len().max(1e-6);
    f.abs() / grad
}

// ---------------------------------------------------------------------------------------------
// Handles
// ---------------------------------------------------------------------------------------------

fn rect_handles(r: RectF, out: &mut Vec<(Handle, PointF)>) {
    let (x1, y1, c) = (r.right(), r.bottom(), r.center());
    out.extend([
        (Handle::Corner(0), PointF::new(r.x, r.y)),
        (Handle::Corner(1), PointF::new(x1, r.y)),
        (Handle::Corner(2), PointF::new(x1, y1)),
        (Handle::Corner(3), PointF::new(r.x, y1)),
        (Handle::Edge(0), PointF::new(c.x, r.y)),
        (Handle::Edge(1), PointF::new(x1, c.y)),
        (Handle::Edge(2), PointF::new(c.x, y1)),
        (Handle::Edge(3), PointF::new(r.x, c.y)),
    ]);
}

/// Control points for editing `shape`, in document pixels.
pub fn handles(shape: &Shape) -> Vec<(Handle, PointF)> {
    let sc = Scale::IDENTITY;
    let mut out = Vec::new();
    match &shape.kind {
        ShapeKind::Arrow { from, to, ctrl, .. } => {
            let mid = match ctrl {
                Some(c) => V::from(*from) * 0.25 + V::from(*c) * 0.5 + V::from(*to) * 0.25,
                None => V::from(*from).lerp((*to).into(), 0.5),
            };
            out.extend([(Handle::Start, *from), (Handle::End, *to), (Handle::Ctrl, mid.pf())]);
        }
        ShapeKind::Line { from, to } | ShapeKind::Measure { from, to } => {
            out.extend([(Handle::Start, *from), (Handle::End, *to)]);
        }
        ShapeKind::Rect { rect, .. }
        | ShapeKind::Ellipse { rect }
        | ShapeKind::Redact { rect, .. }
        | ShapeKind::Spotlight { rect, .. }
        | ShapeKind::Image { rect, .. } => rect_handles(norm_rect(*rect), &mut out),
        ShapeKind::Callout { rect, tail, .. } => {
            rect_handles(norm_rect(*rect), &mut out);
            out.push((Handle::Tail, *tail));
        }
        ShapeKind::Text { pos, text, size, bold, bg, max_width } => {
            let tb = text_box(*pos, text, *size, *bold, *bg, *max_width, sc);
            let b = tb.outer;
            out.extend([(Handle::Edge(1), PointF::new(b.right(), b.center().y)), (Handle::Corner(2), PointF::new(b.right(), b.bottom()))]);
        }
        ShapeKind::Step { center, radius, .. } => {
            let d = radius * std::f32::consts::FRAC_1_SQRT_2;
            out.push((Handle::Corner(2), PointF::new(center.x + d, center.y + d)));
        }
        ShapeKind::Stamp { center, size, .. } => {
            let d = size / 2.0 * std::f32::consts::FRAC_1_SQRT_2;
            out.push((Handle::Corner(2), PointF::new(center.x + d, center.y + d)));
        }
        ShapeKind::Magnifier { source, center, radius } => {
            let s = norm_rect(*source);
            out.push((Handle::Start, s.center()));
            out.extend([
                (Handle::Corner(0), PointF::new(s.x, s.y)),
                (Handle::Corner(1), PointF::new(s.right(), s.y)),
                (Handle::Corner(2), PointF::new(s.right(), s.bottom())),
                (Handle::Corner(3), PointF::new(s.x, s.bottom())),
                (Handle::Edge(1), PointF::new(center.x + radius, center.y)),
            ]);
        }
        ShapeKind::Pen { .. } | ShapeKind::Highlighter { .. } => {}
    }
    out
}

fn add(p: &mut PointF, d: PointF) {
    p.x += d.x;
    p.y += d.y;
}

fn resize_rect(r: &mut RectF, h: Handle, d: PointF) {
    const MIN: f32 = 2.0;
    let mut n = norm_rect(*r);
    let (mut left, mut top, mut right, mut bottom) = (false, false, false, false);
    match h {
        Handle::Corner(0) => (left, top) = (true, true),
        Handle::Corner(1) => (right, top) = (true, true),
        Handle::Corner(2) => (right, bottom) = (true, true),
        Handle::Corner(3) => (left, bottom) = (true, true),
        Handle::Edge(0) => top = true,
        Handle::Edge(1) => right = true,
        Handle::Edge(2) => bottom = true,
        Handle::Edge(3) => left = true,
        _ => {}
    }
    let (x1, y1) = (n.right(), n.bottom());
    if left {
        n.x = (n.x + d.x).min(x1 - MIN);
        n.w = x1 - n.x;
    }
    if right {
        n.w = (n.w + d.x).max(MIN);
    }
    if top {
        n.y = (n.y + d.y).min(y1 - MIN);
        n.h = y1 - n.y;
    }
    if bottom {
        n.h = (n.h + d.y).max(MIN);
    }
    *r = n;
}

/// Corner resize that keeps the aspect ratio (pasted images).
fn resize_rect_aspect(r: &mut RectF, h: Handle, d: PointF) {
    let before = norm_rect(*r);
    resize_rect(r, h, d);
    let Handle::Corner(i) = h else { return };
    let ratio = before.w / before.h.max(0.001);
    let mut n = norm_rect(*r);
    let new_h = (n.w / ratio).max(2.0);
    match i {
        0 | 1 => n.y = before.bottom() - new_h,
        _ => {}
    }
    n.h = new_h;
    *r = n;
}

fn translate_points(points: &mut [PointF], d: PointF) {
    for p in points {
        add(p, d);
    }
}

fn translate_rect(r: &mut RectF, d: PointF) {
    r.x += d.x;
    r.y += d.y;
}

/// Applies a pointer movement of `delta` to handle `h`.
pub fn drag(shape: &mut Shape, h: Handle, delta: PointF) {
    let sc = Scale::IDENTITY;
    if h == Handle::Body {
        translate_shape(shape, delta);
        return;
    }
    let size_factor = |size: f32, w: f32, delta: PointF| (size * (1.0 + (delta.x + delta.y) / w.max(10.0))).clamp(6.0, 600.0);
    let text_geometry = (match &shape.kind {
        ShapeKind::Text { pos, text, size, bold, bg, max_width } => Some(text_box(*pos, text, *size, *bold, *bg, *max_width, sc).outer),
        _ => None,
    })
    .unwrap_or_default();
    match (&mut shape.kind, h) {
        (ShapeKind::Arrow { from, .. } | ShapeKind::Line { from, .. } | ShapeKind::Measure { from, .. }, Handle::Start) => add(from, delta),
        (ShapeKind::Arrow { to, .. } | ShapeKind::Line { to, .. } | ShapeKind::Measure { to, .. }, Handle::End) => add(to, delta),
        (ShapeKind::Arrow { from, to, ctrl, .. }, Handle::Ctrl) => {
            let mut c = ctrl.unwrap_or_else(|| V::from(*from).lerp((*to).into(), 0.5).pf());
            // The handle sits on the curve at t = 0.5, which moves half as far as the control point.
            add(&mut c, PointF::new(delta.x * 2.0, delta.y * 2.0));
            *ctrl = Some(c);
        }
        (ShapeKind::Image { rect, .. }, Handle::Corner(_)) => resize_rect_aspect(rect, h, delta),
        (
            ShapeKind::Rect { rect, .. }
            | ShapeKind::Ellipse { rect }
            | ShapeKind::Redact { rect, .. }
            | ShapeKind::Spotlight { rect, .. }
            | ShapeKind::Image { rect, .. }
            | ShapeKind::Callout { rect, .. },
            Handle::Corner(_) | Handle::Edge(_),
        ) => resize_rect(rect, h, delta),
        (ShapeKind::Callout { tail, .. }, Handle::Tail) => add(tail, delta),
        (ShapeKind::Text { size, max_width, .. }, Handle::Edge(1)) => {
            let current = max_width.unwrap_or(text_geometry.w);
            *max_width = Some((current + delta.x).max(*size * 2.0));
        }
        (ShapeKind::Text { size, max_width, .. }, Handle::Corner(2)) => {
            let factor = size_factor(*size, text_geometry.w + text_geometry.h, delta) / *size;
            *size *= factor;
            if let Some(w) = max_width {
                *w *= factor;
            }
        }
        (ShapeKind::Step { radius, .. }, Handle::Corner(2)) => *radius = (*radius + (delta.x + delta.y) / 2.0).clamp(6.0, 200.0),
        (ShapeKind::Stamp { size, .. }, Handle::Corner(2)) => *size = (*size + delta.x + delta.y).clamp(12.0, 400.0),
        (ShapeKind::Magnifier { source, .. }, Handle::Start) => translate_rect(source, delta),
        (ShapeKind::Magnifier { source, .. }, Handle::Corner(_)) => resize_rect(source, h, delta),
        (ShapeKind::Magnifier { radius, .. }, Handle::Edge(1)) => *radius = (*radius + delta.x).clamp(12.0, 600.0),
        _ => {}
    }
}

fn translate_shape(shape: &mut Shape, d: PointF) {
    match &mut shape.kind {
        ShapeKind::Arrow { from, to, ctrl, .. } => {
            add(from, d);
            add(to, d);
            if let Some(c) = ctrl {
                add(c, d);
            }
        }
        ShapeKind::Line { from, to } | ShapeKind::Measure { from, to } => {
            add(from, d);
            add(to, d);
        }
        ShapeKind::Rect { rect, .. }
        | ShapeKind::Ellipse { rect }
        | ShapeKind::Redact { rect, .. }
        | ShapeKind::Spotlight { rect, .. }
        | ShapeKind::Image { rect, .. } => translate_rect(rect, d),
        ShapeKind::Pen { points } | ShapeKind::Highlighter { points } => translate_points(points, d),
        ShapeKind::Text { pos, .. } => add(pos, d),
        ShapeKind::Callout { rect, tail, .. } => {
            translate_rect(rect, d);
            add(tail, d);
        }
        ShapeKind::Step { center, .. } | ShapeKind::Stamp { center, .. } => add(center, d),
        ShapeKind::Magnifier { center, .. } => add(center, d),
    }
}
