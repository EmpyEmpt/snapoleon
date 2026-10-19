//! Building new shapes from pointer input: what each tool makes while it is dragged and when it is clicked.

use snap_annotate::{ShapeKind, Style};
use snap_core::{PointF, RectF};

use super::tools::{Tool, ToolSet};

/// Pointer travel (screen points) before a press becomes a drag.
pub const DRAG_THRESHOLD: f32 = 4.0;

/// Everything shape building needs to know about the document and the pointer.
pub struct Build<'a> {
    pub tools: &'a ToolSet,
    /// Display density of the capture: sizes below are for a 1x display and scaled by this.
    pub scale: f32,
    pub image: (u32, u32),
    pub shift: bool,
}

/// The shape a drag from `start` to `cur` makes with `tool`. `None` for tools that only react to clicks.
pub fn dragged(tool: Tool, start: PointF, cur: PointF, b: &Build, previous: Option<&ShapeKind>) -> Option<ShapeKind> {
    let x = &b.tools.extras;
    Some(match tool {
        Tool::Arrow => {
            let to = if b.shift { snap_angle(start, cur) } else { cur };
            let ctrl = x.curved.then(|| bend(start, to));
            ShapeKind::Arrow { from: start, to, ctrl, head: x.arrow_head, double: x.double }
        }
        Tool::Line => ShapeKind::Line { from: start, to: if b.shift { snap_angle(start, cur) } else { cur } },
        Tool::Measure => ShapeKind::Measure { from: start, to: if b.shift { snap_angle(start, cur) } else { cur } },
        Tool::Rect => ShapeKind::Rect { rect: corners(start, cur, b.shift), radius: if x.rounded { 10.0 * b.scale } else { 0.0 } },
        Tool::Ellipse => ShapeKind::Ellipse { rect: corners(start, cur, b.shift) },
        Tool::Pen => ShapeKind::Pen { points: stroke_points(previous, start, cur, b.shift, false) },
        Tool::Highlighter => ShapeKind::Highlighter { points: stroke_points(previous, start, cur, b.shift, true) },
        Tool::Redact => ShapeKind::Redact { rect: corners(start, cur, false), style: x.redact },
        Tool::Spotlight => ShapeKind::Spotlight { rect: corners(start, cur, false), ellipse: x.spot_ellipse },
        Tool::Magnifier => magnifier(corners(start, cur, false), b),
        Tool::Callout => callout(corners(start, cur, false), b),
        Tool::Text => {
            let width = (cur.x - start.x).abs();
            ShapeKind::Text {
                pos: PointF::new(start.x.min(cur.x), start.y),
                text: String::new(),
                size: x.font_size,
                bold: x.bold,
                bg: x.text_bg,
                max_width: (width > 40.0 * b.scale).then_some(width),
            }
        }
        Tool::Select | Tool::Crop | Tool::Step | Tool::Stamp => return None,
    })
}

/// The shape and style a plain click at `p` places with `tool`, when the tool has a click action.
pub fn clicked(tool: Tool, p: PointF, step_number: u32, b: &Build) -> Option<(ShapeKind, Style)> {
    let x = &b.tools.extras;
    let style = b.tools.styles.get(&tool).copied().unwrap_or_default();
    let s = b.scale;
    let kind = match tool {
        Tool::Step => ShapeKind::Step { center: p, number: step_number, radius: 16.0 * s },
        Tool::Stamp => ShapeKind::Stamp { center: p, size: 56.0 * s, glyph: x.stamp },
        Tool::Text => ShapeKind::Text { pos: p, text: String::new(), size: x.font_size, bold: x.bold, bg: x.text_bg, max_width: None },
        Tool::Callout => callout(RectF::new(p.x - 120.0 * s, p.y - 48.0 * s, 240.0 * s, 96.0 * s), b),
        Tool::Magnifier => magnifier(RectF::new(p.x - 60.0 * s, p.y - 40.0 * s, 120.0 * s, 80.0 * s), b),
        _ => return None,
    };
    Some((kind, style))
}

/// Whether a press-and-release without travel does something for this tool.
pub fn has_click_action(tool: Tool) -> bool {
    matches!(tool, Tool::Step | Tool::Stamp | Tool::Text | Tool::Callout | Tool::Magnifier)
}

/// Whether the tool opens the inline text editor on the shape it makes.
pub fn edits_text(tool: Tool) -> bool {
    matches!(tool, Tool::Text | Tool::Callout)
}

fn corners(a: PointF, b: PointF, square: bool) -> RectF {
    if !square {
        return RectF::from_corners(a, b);
    }
    let side = (b.x - a.x).abs().max((b.y - a.y).abs());
    RectF::from_corners(a, PointF::new(a.x + side.copysign(b.x - a.x), a.y + side.copysign(b.y - a.y)))
}

/// `cur` moved onto the nearest multiple of 45 degrees around `start`.
fn snap_angle(start: PointF, cur: PointF) -> PointF {
    let (dx, dy) = (cur.x - start.x, cur.y - start.y);
    let len = dx.hypot(dy);
    if len < 1.0 {
        return cur;
    }
    let step = std::f32::consts::FRAC_PI_4;
    let angle = (dy.atan2(dx) / step).round() * step;
    PointF::new(start.x + len * angle.cos(), start.y + len * angle.sin())
}

/// A control point that bows the arrow sideways by a fifth of its length.
fn bend(from: PointF, to: PointF) -> PointF {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    PointF::new((from.x + to.x) / 2.0 - dy * 0.2, (from.y + to.y) / 2.0 + dx * 0.2)
}

fn stroke_points(previous: Option<&ShapeKind>, start: PointF, cur: PointF, straight: bool, horizontal: bool) -> Vec<PointF> {
    if straight {
        let end = if horizontal { PointF::new(cur.x, start.y) } else { cur };
        return vec![start, end];
    }
    let mut points = match previous {
        Some(ShapeKind::Pen { points } | ShapeKind::Highlighter { points }) => points.clone(),
        _ => vec![start],
    };
    // Dropping points that are closer than a pixel keeps long scribbles light without visible change.
    if points.last().is_none_or(|last| last.dist(cur) >= 1.0) {
        points.push(cur);
    }
    points
}

fn callout(rect: RectF, b: &Build) -> ShapeKind {
    let rect = RectF::new(rect.x, rect.y, rect.w.max(80.0 * b.scale), rect.h.max(48.0 * b.scale));
    let tail = PointF::new(rect.x + rect.w * 0.25, rect.bottom() + (rect.h * 0.4).max(28.0 * b.scale));
    ShapeKind::Callout { rect, tail, text: String::new(), size: b.tools.extras.font_size }
}

/// A loupe over `source`, placed beside it and showing it at about 2x.
fn magnifier(source: RectF, b: &Build) -> ShapeKind {
    let s = b.scale;
    let source = RectF::new(source.x, source.y, source.w.max(24.0 * s), source.h.max(24.0 * s));
    let radius = source.w.max(source.h).clamp(48.0 * s, 220.0 * s);
    let gap = radius + 24.0 * s;
    let right = source.right() + gap;
    let cx = if right + radius <= b.image.0 as f32 { right } else { source.x - gap };
    ShapeKind::Magnifier { source, center: PointF::new(cx, source.center().y), radius }
}
