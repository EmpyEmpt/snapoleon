//! Measure mode: drag a ruler line (length and angle), click two elements for the gap between them, or
//! hold Ctrl to see how far the pointer is from the nearest edge in each direction.

use egui::{Color32, Context, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};
use snap_capture::Frozen;
use snap_core::{Point, Rect as PxRect};
use snap_ui::{Palette, palette};

use super::{
    paint::{self, PillText},
    space::{Space, rect_f, rect_px},
    targets::Target,
};
use crate::views::colors::Measurement;

/// A press that moves less than this (physical pixels) is a click on an element, not a ruler.
const CLICK_SLOP: f32 = 4.0;
/// Per-channel colour change that counts as an edge when scanning from the pointer.
const EDGE_TOLERANCE: i32 = 16;
/// The scan gives up after this many pixels.
const MAX_SCAN: i32 = 4096;

#[derive(Default)]
pub struct Measure {
    /// The ruler, in virtual-desktop pixels.
    pub line: Option<(Pos2, Pos2)>,
    dragging: bool,
    /// The first and second clicked elements.
    first: Option<PxRect>,
    second: Option<PxRect>,
}

impl Measure {
    pub fn is_empty(&self) -> bool {
        self.line.is_none() && self.first.is_none()
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn press(&mut self, at: Pos2) {
        self.line = Some((at, at));
        self.dragging = true;
    }

    /// Follow the pointer; Shift snaps the ruler to multiples of 45 degrees.
    pub fn drag(&mut self, to: Pos2, shift: bool) {
        let Some((from, end)) = self.line.as_mut().filter(|_| self.dragging) else { return };
        *end = if shift { snap_angle(*from, to) } else { to };
    }

    /// The press ended. A tiny drag counts as a click on `target`.
    pub fn release(&mut self, target: Option<&Target>) {
        if !std::mem::take(&mut self.dragging) {
            return;
        }
        let Some((from, to)) = self.line else { return };
        if (to - from).length() >= CLICK_SLOP {
            self.first = None;
            self.second = None;
            return;
        }
        self.line = None;
        match (target.map(|t| t.rect), self.first, self.second) {
            (None, ..) => {
                self.first = None;
                self.second = None;
            }
            (rect, None, _) => self.first = rect,
            (rect, Some(_), None) => self.second = rect,
            (rect, Some(_), Some(_)) => {
                self.first = rect;
                self.second = None;
            }
        }
    }

    /// The measurement to hand to the tool window, if there is one.
    pub fn result(&self) -> Option<Measurement> {
        if let Some((a, b)) = self.line {
            return Some(Measurement { a: rect_px(Rect::from_two_pos(a, b)), b: None });
        }
        self.first.map(|a| Measurement { a, b: self.second })
    }
}

fn snap_angle(from: Pos2, to: Pos2) -> Pos2 {
    let d = to - from;
    let step = std::f32::consts::FRAC_PI_4;
    let angle = (d.y.atan2(d.x) / step).round() * step;
    from + vec2(angle.cos(), angle.sin()) * d.length()
}

/// Distances from `at` to the nearest colour change (or the hovered element's border, if nearer) in the
/// four directions: left, right, up, down. Each is the number of pixels between the pointer pixel and the edge.
pub fn rays(frozen: &Frozen, at: Point, element: Option<PxRect>) -> [i32; 4] {
    let Some(base) = frozen.pixel(at) else { return [0; 4] };
    let differs = |p: [u8; 4]| (0..3).any(|i| (p[i] as i32 - base[i] as i32).abs() > EDGE_TOLERANCE);
    let mut out = [0; 4];
    for (slot, (dx, dy)) in [(-1, 0), (1, 0), (0, -1), (0, 1)].into_iter().enumerate() {
        let mut steps = 0;
        while steps < MAX_SCAN {
            let next = Point::new(at.x + dx * (steps + 1), at.y + dy * (steps + 1));
            match frozen.pixel(next) {
                Some(px) if !differs(px) => steps += 1,
                _ => break,
            }
        }
        if let Some(e) = element {
            let to_edge = match (dx, dy) {
                (-1, _) => at.x - e.x,
                (1, _) => e.right() - 1 - at.x,
                (_, -1) => at.y - e.y,
                _ => e.bottom() - 1 - at.y,
            };
            if to_edge >= 0 {
                steps = steps.min(to_edge);
            }
        }
        out[slot] = steps;
    }
    out
}

/// What measure mode draws in one window.
pub struct Scene<'a> {
    pub measure: &'a Measure,
    /// The element under the pointer, when no ruler is being dragged.
    pub hover: Option<&'a Target>,
    /// Pointer in virtual-desktop pixels.
    pub cursor: Option<Pos2>,
    /// Ctrl is held: show distances to the nearest edges.
    pub rays: Option<[i32; 4]>,
}

pub fn paint(painter: &Painter, ctx: &Context, space: Space, win: Rect, scene: &Scene) {
    let p = palette(ctx);
    let measure = scene.measure;
    let local = |r: PxRect| space.local_rect(rect_f(r));

    // Clicked elements, and the gap between two of them (the second may just be the hovered element).
    let second = measure.second.or_else(|| scene.hover.filter(|_| measure.first.is_some()).map(|t| t.rect));
    for rect in measure.first.iter().chain(measure.second.iter()) {
        paint::highlight(painter, &p, local(*rect));
        paint::size_label(painter, ctx, win, local(*rect), (rect.w, rect.h), None);
    }
    if let (Some(a), Some(b)) = (measure.first, second) {
        if measure.second.is_none() {
            paint::highlight(painter, &p, local(b));
        }
        gaps(painter, ctx, space, win, a, b);
    } else if measure.line.is_none()
        && let Some(target) = scene.hover
    {
        paint::highlight(painter, &p, local(target.rect));
        paint::size_label(painter, ctx, win, local(target.rect), (target.rect.w, target.rect.h), Some(&target.label));
    }

    if let Some((a, b)) = measure.line {
        ruler(painter, ctx, space, win, a, b, &p);
    }

    if let (Some(cursor), Some(distances)) = (scene.cursor, scene.rays) {
        let c = space.pixel_center(cursor);
        let directions = [vec2(-1.0, 0.0), vec2(1.0, 0.0), vec2(0.0, -1.0), vec2(0.0, 1.0)];
        for (distance, dir) in distances.into_iter().zip(directions) {
            if distance <= 0 {
                continue;
            }
            // The scan counts pixels from the pointer pixel; the line runs from its centre to the far pixel's centre.
            let end = c + dir * (distance as f32 / space.ppp);
            paint::cased_line(painter, space, c, end, p.accent);
            let tick = vec2(dir.y, dir.x) * 5.0;
            painter.line_segment([end - tick, end + tick], Stroke::new(space.px(), p.accent));
            let mid = c + (end - c) / 2.0;
            let text = distance.to_string();
            paint::pill(painter, ctx, &[(&text, PillText::Mono, p.text)], |size| {
                let at = mid + if dir.x != 0.0 { vec2(-size.x / 2.0, -size.y - 6.0) } else { vec2(8.0, -size.y / 2.0) };
                pos2(at.x.clamp(win.left() + 4.0, win.right() - size.x - 4.0), at.y.clamp(win.top() + 4.0, win.bottom() - size.y - 4.0))
            });
        }
    }
}

/// The ruler: a cased line with end ticks, the dx/dy legs dashed, and a pill with length and angle.
fn ruler(painter: &Painter, ctx: &Context, space: Space, win: Rect, a: Pos2, b: Pos2, p: &Palette) {
    let (la, lb) = (space.pixel_center(a), space.pixel_center(b));
    let delta = b - a;
    let length = delta.length();
    if length < 1.0 {
        return;
    }
    let corner = pos2(lb.x, la.y);
    let leg = Stroke::new(space.px(), Color32::from_white_alpha(130));
    painter.extend(Shape::dashed_line(&[la, corner], leg, 3.0, 3.0));
    painter.extend(Shape::dashed_line(&[corner, lb], leg, 3.0, 3.0));
    paint::cased_line(painter, space, la, lb, p.accent);
    let dir = (lb - la).normalized();
    let tick = vec2(-dir.y, dir.x) * 6.0;
    for end in [la, lb] {
        paint::cased_line(painter, space, end - tick, end + tick, p.accent);
    }

    let angle = (-delta.y).atan2(delta.x).to_degrees().rem_euclid(360.0);
    let main = format!("{} px  {}°", length.round(), angle.round());
    let sides = format!("  {} × {}", delta.x.abs().round(), delta.y.abs().round());
    // At the middle of the line, on the upper side, so neither the pointer nor the loupe covers it.
    let perp = vec2(-dir.y, dir.x);
    let normal = if perp.y > 0.0 { -perp } else { perp };
    let mid = la + (lb - la) / 2.0;
    paint::pill(painter, ctx, &[(&main, PillText::Mono, p.text), (&sides, PillText::Caption, p.text_dim)], |size| {
        let at = mid + normal * (size.y / 2.0 + 14.0) - size / 2.0;
        pos2(
            at.x.clamp(win.left() + 4.0, (win.right() - size.x - 4.0).max(win.left() + 4.0)),
            at.y.clamp(win.top() + 4.0, (win.bottom() - size.y - 4.0).max(win.top() + 4.0)),
        )
    });
}

/// Gap lines between two rectangles along each axis on which they do not overlap.
fn gaps(painter: &Painter, ctx: &Context, space: Space, win: Rect, a: PxRect, b: PxRect) {
    let p = palette(ctx);
    let (ra, rb) = (rect_f(a), rect_f(b));
    let line = |from: Pos2, to: Pos2, value: f32| {
        let (la, lb) = (space.local(from), space.local(to));
        paint::cased_line(painter, space, la, lb, p.accent);
        let text = value.round().to_string();
        paint::pill(painter, ctx, &[(&text, PillText::Mono, p.text)], |size| {
            let mid = la + (lb - la) / 2.0;
            let at = if (lb.x - la.x).abs() > (lb.y - la.y).abs() {
                mid + vec2(-size.x / 2.0, -size.y - 6.0)
            } else {
                mid + vec2(8.0, -size.y / 2.0)
            };
            pos2(at.x.clamp(win.left() + 4.0, win.right() - size.x - 4.0), at.y.clamp(win.top() + 4.0, win.bottom() - size.y - 4.0))
        });
    };
    // Horizontal gap, drawn at the middle of the vertical overlap (or of the first rectangle when there is none).
    let (x0, x1) = if ra.max.x <= rb.min.x { (ra.max.x, rb.min.x) } else { (rb.max.x, ra.min.x) };
    if x1 > x0 {
        let y = overlap_mid(ra.min.y, ra.max.y, rb.min.y, rb.max.y);
        line(pos2(x0, y), pos2(x1, y), x1 - x0);
    }
    let (y0, y1) = if ra.max.y <= rb.min.y { (ra.max.y, rb.min.y) } else { (rb.max.y, ra.min.y) };
    if y1 > y0 {
        let x = overlap_mid(ra.min.x, ra.max.x, rb.min.x, rb.max.x);
        line(pos2(x, y0), pos2(x, y1), y1 - y0);
    }
}

/// Middle of the overlap of two ranges; when they do not overlap, the middle of the first.
fn overlap_mid(a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    let (lo, hi) = (a0.max(b0), a1.min(b1));
    if hi > lo { (lo + hi) / 2.0 } else { (a0 + a1) / 2.0 }
}
