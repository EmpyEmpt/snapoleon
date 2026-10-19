//! Drawing of individual shapes onto a [`Canvas`].

use image::RgbaImage;
use snap_core::settings::RedactStyle;
use snap_core::{PointF, RectF};
use tiny_skia::{BlendMode, FilterQuality, LineCap, LineJoin, Paint, Path, PathBuilder, Pattern, Pixmap, SpreadMode, Transform};

use crate::cache::{ImageCache, premultiply};
use crate::effects::{pixel_rect, preview_patch};
use crate::font::{self, Face};
use crate::geometry::{
    ArrowGeom, TextBox, arrow_geom, bubble_path, build, callout_box, magnifier_geom, measure_geom, norm_rect, polygon_path, polyline_path,
    rounded_rect_path, smooth_path, text_box,
};
use crate::math::{Scale, V, badge_ink, contrasting};
use crate::model::{Shape, ShapeKind, Style, TextBg};
use crate::paint::{Canvas, StrokeSpec, snap};
use crate::stamps;

pub(crate) struct Ctx<'a> {
    pub sc: Scale,
    /// Pixels the magnifier samples; already redacted when exporting.
    pub src: &'a RgbaImage,
    /// Redactions to overlay on magnifier samples (live view only).
    pub preview_redacts: &'a [(RectF, RedactStyle)],
    pub cache: &'a ImageCache,
}

/// Draws one shape without shadow or whole-shape opacity (see `render::draw_shape`).
pub(crate) fn draw_kind(cv: &mut Canvas, shape: &Shape, cx: &Ctx) {
    let st = &shape.style;
    let sc = cx.sc;
    let w = sc.stroke(st.width);
    match &shape.kind {
        ShapeKind::Arrow { from, to, ctrl, head, double } => arrow(cv, &arrow_geom(*from, *to, *ctrl, *head, *double, w), st, w),
        ShapeKind::Line { from, to } => {
            if let Some(p) = polyline_path(&[V::from(*from), V::from(*to)]) {
                cv.stroke(&p, st.stroke, &StrokeSpec { dashed: st.dashed, ..StrokeSpec::round(w) });
            }
        }
        ShapeKind::Rect { rect, radius } => {
            let Some(p) = rounded_rect_path(norm_rect(*rect), *radius) else { return };
            outlined(cv, &p, st, w, if *radius > 0.5 { LineJoin::Round } else { LineJoin::Miter });
        }
        ShapeKind::Ellipse { rect } => {
            let r = norm_rect(*rect);
            let Some(p) = tiny_skia::Rect::from_xywh(r.x, r.y, r.w, r.h).and_then(PathBuilder::from_oval) else { return };
            outlined(cv, &p, st, w, LineJoin::Round);
        }
        ShapeKind::Pen { points } => match points.len() {
            0 => {}
            1 => cv.fill_circle(points[0].into(), w / 2.0, st.stroke),
            _ => {
                if let Some(p) = smooth_path(points) {
                    cv.stroke(&p, st.stroke, &StrokeSpec { dashed: st.dashed, ..StrokeSpec::round(w) });
                }
            }
        },
        ShapeKind::Highlighter { points } => {
            if let Some(p) = smooth_path(points) {
                let spec = StrokeSpec { width: sc.stroke(st.width * 3.0), cap: LineCap::Butt, join: LineJoin::Round, dashed: false };
                cv.stroke_blend(&p, st.stroke, 0.4 * st.opacity, &spec, BlendMode::Multiply);
            }
        }
        ShapeKind::Text { pos, text, size, bold, bg, max_width } => {
            let tb = text_box(*pos, text, *size, *bold, *bg, *max_width, sc);
            let (back, ink) = match bg {
                TextBg::None => (None, st.stroke),
                _ => {
                    let f = st.fill.unwrap_or(st.stroke);
                    (Some(f), contrasting(f))
                }
            };
            if let Some(f) = back
                && let Some(p) = bubble_path(tb.outer, tb.radius, tb.tip.filter(|_| *bg == TextBg::Bubble), tb.base_w)
            {
                cv.fill(&p, f);
            }
            draw_text(cv, &tb, ink);
        }
        ShapeKind::Callout { rect, tail, text, size } => {
            let tb = callout_box(norm_rect(*rect), *tail, text, *size, sc);
            let f = st.fill.unwrap_or(st.stroke);
            if let Some(p) = bubble_path(tb.outer, tb.radius, tb.tip, tb.base_w) {
                cv.fill(&p, f);
            }
            draw_text(cv, &tb, contrasting(f));
        }
        ShapeKind::Step { center, number, radius } => step(cv, st, (*center).into(), *number, sc.badge(*radius)),
        ShapeKind::Stamp { center, size, glyph } => {
            let r = sc.stamp(*size) / 2.0;
            let c = V::from(*center);
            cv.fill_circle(c, r, st.stroke);
            let art = stamps::art(*glyph);
            let ink = badge_ink(st.stroke);
            let local = Transform::from_row(r, 0.0, 0.0, r, c.0, c.1);
            let mut inner = Canvas { pm: &mut *cv.pm, ts: cv.ts.pre_concat(local) };
            for p in &art.fills {
                inner.fill(p, ink);
            }
            for (p, width) in &art.strokes {
                inner.stroke(p, ink, &StrokeSpec::round(*width));
            }
        }
        ShapeKind::Redact { rect, style } => redact_preview(cv, *rect, *style, cx),
        ShapeKind::Spotlight { .. } => {}
        ShapeKind::Magnifier { source, center, radius } => magnifier(cv, st, *source, *center, *radius, cx),
        ShapeKind::Image { rect, image } => image_shape(cv, *rect, image, cx),
        ShapeKind::Measure { from, to } => measure(cv, st, *from, *to, sc),
    }
}

fn outlined(cv: &mut Canvas, path: &Path, st: &Style, w: f32, join: LineJoin) {
    if let Some(f) = st.fill {
        cv.fill(path, f);
    }
    if w > 0.0 {
        cv.stroke(path, st.stroke, &StrokeSpec { width: w, cap: LineCap::Butt, join, dashed: st.dashed });
    }
}

fn arrow(cv: &mut Canvas, g: &ArrowGeom, st: &Style, w: f32) {
    let body = polygon_path(&g.poly);
    let chevrons: Vec<Path> = g.chevrons.iter().filter_map(|c| polyline_path(c)).collect();
    if let Some(halo) = st.fill {
        let hw = (w * 0.3).clamp(1.2, 3.0);
        if let Some(p) = &body {
            cv.stroke(p, halo, &StrokeSpec::round(hw * 2.0));
        }
        for c in &chevrons {
            cv.stroke(c, halo, &StrokeSpec::round(g.chevron_width + hw * 2.0));
        }
    }
    if let Some(p) = &body {
        cv.fill(p, st.stroke);
    }
    for c in &chevrons {
        cv.stroke(c, st.stroke, &StrokeSpec::round(g.chevron_width));
    }
}

fn draw_text(cv: &mut Canvas, tb: &TextBox, ink: [u8; 4]) {
    let path = build(|pb| {
        for (i, line) in tb.layout.lines.iter().enumerate() {
            if line.text.is_empty() {
                continue;
            }
            let p = snap(&cv.ts, V(tb.origin.0, tb.origin.1 + tb.layout.baseline(i)));
            font::push_text(pb, tb.face, &line.text, p.0, p.1, tb.layout.size);
        }
    });
    if let Some(p) = path {
        cv.fill(&p, ink);
    }
}

fn step(cv: &mut Canvas, st: &Style, c: V, number: u32, r: f32) {
    if let Some(ring) = st.fill {
        let rw = (r * 0.14).max(1.5);
        if let Some(p) = PathBuilder::from_circle(c.0, c.1, r + rw / 2.0) {
            cv.stroke(&p, ring, &StrokeSpec::round(rw));
        }
    }
    cv.fill_circle(c, r, st.stroke);
    let label = number.to_string();
    let fs = r * match label.len() {
        1 => 1.1,
        2 => 0.95,
        _ => 0.75,
    };
    let adv = font::advance(Face::Bold, &label, fs);
    let baseline = c.1 + Face::Bold.cap_height() * fs / 2.0;
    if let Some(p) = build(|pb| {
        font::push_text(pb, Face::Bold, &label, c.0 - adv / 2.0, baseline, fs);
    }) {
        cv.fill(&p, badge_ink(st.stroke));
    }
}

fn measure(cv: &mut Canvas, st: &Style, from: PointF, to: PointF, sc: Scale) {
    let g = measure_geom(from, to, st.width, sc);
    let spec = StrokeSpec::round(g.line_w);
    let dir = (g.b - g.a).norm();
    let n = dir.perp() * g.tick;
    for (a, b) in [(g.a, g.b), (g.a - n, g.a + n), (g.b - n, g.b + n)] {
        if let Some(p) = polyline_path(&[a, b]) {
            cv.stroke(&p, st.stroke, &spec);
        }
    }
    if let Some(p) = rounded_rect_path(g.pill, g.pill.h / 2.0) {
        cv.fill(&p, st.stroke);
    }
    if let Some(p) = build(|pb| {
        font::push_text(pb, Face::Mono, &g.label, g.text_x, g.baseline, g.font_size);
    }) {
        cv.fill(&p, contrasting(st.stroke));
    }
}

/// Redaction as it appears while editing: the redacted pixels drawn over the base.
fn redact_preview(cv: &mut Canvas, rect: RectF, style: RedactStyle, cx: &Ctx) {
    let Some((patch, r)) = preview_patch(cx.src, rect, style) else { return };
    let pm = premultiply(&patch);
    let quality = if cv.ts.is_translate() { FilterQuality::Nearest } else { FilterQuality::Bilinear };
    let paint = tiny_skia::PixmapPaint { quality, ..Default::default() };
    cv.pm.draw_pixmap(r.x, r.y, pm.as_ref(), &paint, cv.ts, None);
}

fn magnifier(cv: &mut Canvas, st: &Style, source: RectF, center: PointF, radius: f32, cx: &Ctx) {
    let g = magnifier_geom(source, center, radius, cx.sc);
    let thin = cx.sc.stroke(1.5);
    let white = [255, 255, 255, 230];
    if let Some(r) = rounded_rect_path(g.source, 0.0) {
        cv.stroke(&r, white, &StrokeSpec { width: thin + 2.0, cap: LineCap::Butt, join: LineJoin::Round, dashed: false });
        cv.stroke(&r, st.stroke, &StrokeSpec { width: thin, cap: LineCap::Butt, join: LineJoin::Round, dashed: false });
    }
    if let Some((a, b)) = g.connector
        && let Some(p) = polyline_path(&[a, b])
    {
        cv.stroke(&p, white, &StrokeSpec::round(thin + 2.0));
        cv.stroke(&p, st.stroke, &StrokeSpec::round(thin));
    }

    let Some(circle) = PathBuilder::from_circle(g.center.0, g.center.1, g.radius) else { return };
    let sc = V::from(g.source.center());
    let half = g.source.w.max(g.source.h) / 2.0;
    let region = RectF::new(sc.0 - half, sc.1 - half, 2.0 * half, 2.0 * half);
    match sample(cx, region) {
        Some((pix, ox, oy)) => {
            let quality = match g.zoom {
                z if z < 1.0 => FilterQuality::Bilinear,
                z if z < 4.0 => FilterQuality::Bicubic,
                _ => FilterQuality::Nearest,
            };
            let t = Transform::from_row(g.zoom, 0.0, 0.0, g.zoom, g.center.0 + (ox as f32 - sc.0) * g.zoom, g.center.1 + (oy as f32 - sc.1) * g.zoom);
            let paint = Paint { shader: Pattern::new(pix.as_ref(), SpreadMode::Pad, quality, 1.0, t), anti_alias: true, ..Paint::default() };
            cv.pm.fill_path(&circle, &paint, tiny_skia::FillRule::Winding, cv.ts, None);
        }
        None => cv.fill(&circle, [128, 128, 128, 255]),
    }
    cv.stroke(&circle, [255, 255, 255, 255], &StrokeSpec::round(g.ring_w));
}

/// Source pixels for a magnifier, with preview redactions applied. Returns the pixmap and its image origin.
fn sample(cx: &Ctx, region: RectF) -> Option<(Pixmap, i32, i32)> {
    let r = pixel_rect(region, cx.src.width(), cx.src.height())?;
    let mut crop = image::imageops::crop_imm(cx.src, r.x as u32, r.y as u32, r.w, r.h).to_image();
    for (rect, style) in cx.preview_redacts {
        let Some((patch, pr)) = preview_patch(cx.src, *rect, *style) else { continue };
        let Some(hit) = r.intersect(pr) else { continue };
        let part = image::imageops::crop_imm(&patch, (hit.x - pr.x) as u32, (hit.y - pr.y) as u32, hit.w, hit.h).to_image();
        image::imageops::replace(&mut crop, &part, (hit.x - r.x) as i64, (hit.y - r.y) as i64);
    }
    Some((premultiply(&crop), r.x, r.y))
}

fn image_shape(cv: &mut Canvas, rect: RectF, image: &std::sync::Arc<RgbaImage>, cx: &Ctx) {
    let r = norm_rect(rect);
    let Some(target) = tiny_skia::Rect::from_xywh(r.x, r.y, r.w, r.h) else { return };
    let pyr = cx.cache.pyramid(image);
    let k = r.w * cv.ts.sx / image.width().max(1) as f32;
    let level = if k < 1.0 { ((1.0 / k).log2() + 1e-3).floor().max(0.0) as usize } else { 0 };
    let lvl = pyr.level(level);
    let quality = match k {
        k if k >= 4.0 => FilterQuality::Nearest,
        k if k > 1.0 => FilterQuality::Bicubic,
        _ => FilterQuality::Bilinear,
    };
    let t = Transform::from_row(r.w / lvl.width() as f32, 0.0, 0.0, r.h / lvl.height() as f32, r.x, r.y);
    let paint = Paint { shader: Pattern::new(lvl.as_ref(), SpreadMode::Pad, quality, 1.0, t), anti_alias: true, ..Paint::default() };
    cv.pm.fill_rect(target, &paint, cv.ts, None);
}
