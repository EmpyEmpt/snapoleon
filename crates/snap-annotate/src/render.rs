//! Export rendering and the live editor view.

use std::borrow::Cow;

use image::RgbaImage;
use snap_core::settings::RedactStyle;
use snap_core::{PointF, RectF};
use snap_image::{beautify, effects as image_effects, resize};
use tiny_skia::{BlendMode, Color, Paint, Pixmap, PixmapPaint, Transform};

use crate::cache::{premultiply, unpremultiply};
use crate::effects::{blur_plane, composite_shadow, pixel_rect};
use crate::geometry::{norm_rect, rounded_rect_path};
use crate::interact::bounds_with;
use crate::math::Scale;
use crate::model::{Document, Shape, ShapeKind};
use crate::paint::Canvas;
use crate::resample;
use crate::shapes::{Ctx, draw_kind};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderOptions {
    /// Output size relative to the (cropped) document.
    pub scale: f32,
    /// When shrinking: enlarge thin strokes, small text and badges, and use readable downscaling.
    pub readable: bool,
    /// Apply the document's beautify preset (padding, background, frame, shadow).
    pub beautify: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self { scale: 1.0, readable: true, beautify: true }
    }
}

/// Document-to-view mapping: `view = (doc - offset) * zoom`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub offset: PointF,
    pub zoom: f32,
    pub size: (u32, u32),
}

pub(crate) fn scaled_size(w: u32, h: u32, scale: f32) -> (u32, u32) {
    let s = if scale.is_finite() && scale > 0.0 { scale.min(16.0) } else { 1.0 };
    (((w as f32 * s).round() as u32).max(1), ((h as f32 * s).round() as u32).max(1))
}

fn visible(doc: &Document) -> impl Iterator<Item = &Shape> {
    doc.shapes.iter().filter(|s| !s.hidden)
}

fn redact_list(doc: &Document) -> Vec<(RectF, RedactStyle)> {
    visible(doc)
        .filter_map(|s| match s.kind {
            ShapeKind::Redact { rect, style } => Some((rect, style)),
            _ => None,
        })
        .collect()
}

/// Renders the final image. Redactions are destructive: they overwrite the base pixels before
/// anything is scaled or drawn on top, so no downstream step can recover the original content.
pub fn render(doc: &Document, o: &RenderOptions) -> RgbaImage {
    let scale = if o.scale.is_finite() && o.scale > 0.0 { o.scale.min(16.0) } else { 1.0 };
    let crop = doc.crop_rect();
    let (bw, bh) = doc.canvas_size();
    if bw == 0 || bh == 0 {
        return RgbaImage::new(1, 1);
    }

    let mut full: Cow<RgbaImage> = Cow::Borrowed(&*doc.base);
    for (rect, style) in redact_list(doc) {
        if let Some(r) = pixel_rect(rect, bw, bh) {
            image_effects::redact(full.to_mut(), r, style);
        }
    }

    let region: Cow<RgbaImage> = if crop.w == bw && crop.h == bh {
        Cow::Borrowed(&*full)
    } else {
        Cow::Owned(image::imageops::crop_imm(&*full, crop.x as u32, crop.y as u32, crop.w, crop.h).to_image())
    };
    let (ow, oh) = scaled_size(crop.w, crop.h, scale);
    let base: Cow<RgbaImage> = if (ow, oh) == (crop.w, crop.h) {
        region
    } else if scale < 1.0 && o.readable {
        Cow::Owned(resize::readable_downscale(&region, ow, oh))
    } else {
        Cow::Owned(resize::resize(&region, ow, oh))
    };

    let mut pm = premultiply(&base);
    let (sx, sy) = (ow as f32 / crop.w as f32, oh as f32 / crop.h as f32);
    let ts = Transform::from_row(sx, 0.0, 0.0, sy, -(crop.x as f32) * sx, -(crop.y as f32) * sy);
    let cx = Ctx { sc: Scale { z: sx, readable: o.readable }, src: &full, preview_redacts: &[], cache: &doc.cache };
    draw_spotlights(&mut pm, ts, doc);
    for shape in visible(doc).filter(|s| !matches!(s.kind, ShapeKind::Redact { .. } | ShapeKind::Spotlight { .. })) {
        draw_shape(&mut pm, ts, shape, &cx);
    }

    let img = unpremultiply(pm);
    match (&doc.beautify, o.beautify) {
        (Some(preset), true) => {
            let mut p = preset.clone();
            if (scale - 1.0).abs() > f32::EPSILON {
                let k = |v: u32| (v as f32 * scale).round() as u32;
                (p.padding, p.corner_radius, p.shadow) = (k(p.padding), k(p.corner_radius), k(p.shadow));
            }
            beautify::apply_with(&img, &p, &doc.meta.context)
        }
        _ => img,
    }
}

/// Renders the document for on-screen editing at view resolution. With `base_too` the base image
/// is drawn too (from a cached mip pyramid, so cost scales with the view, not the capture);
/// without it only annotations are drawn, for compositing over a GPU texture of the base.
/// Redactions are drawn as opaque previews of their effect either way, and the export crop is ignored.
pub fn render_view(doc: &Document, view: &View, base_too: bool) -> Pixmap {
    let (w, h) = (view.size.0.max(1), view.size.1.max(1));
    let mut pm = Pixmap::new(w, h).expect("view size is non-zero");
    let z = if view.zoom.is_finite() && view.zoom > 0.0 { view.zoom } else { 1.0 };
    let (tx, ty) = (-view.offset.x * z, -view.offset.y * z);
    let ts = Transform::from_row(z, 0.0, 0.0, z, tx, ty);

    if base_too {
        let (bw, bh) = doc.canvas_size();
        resample::blit(&mut pm, &doc.cache.pyramid(&doc.base), z, bw, bh, tx, ty);
    }

    let redacts = redact_list(doc);
    let cx = Ctx { sc: Scale { z, readable: false }, src: &doc.base, preview_redacts: &redacts, cache: &doc.cache };
    // Shapes farther than this (in document pixels) outside the view cannot reach it, shadows included.
    let margin = 50.0;
    let on_screen = RectF::new(view.offset.x - margin, view.offset.y - margin, w as f32 / z + 2.0 * margin, h as f32 / z + 2.0 * margin);
    let reaches = |s: &Shape| {
        let b = bounds_with(s, cx.sc);
        b.x < on_screen.right() && b.right() > on_screen.x && b.y < on_screen.bottom() && b.bottom() > on_screen.y
    };

    for shape in visible(doc).filter(|s| matches!(s.kind, ShapeKind::Redact { .. }) && reaches(s)) {
        draw_kind(&mut Canvas { pm: &mut pm, ts }, shape, &cx);
    }
    draw_spotlights(&mut pm, ts, doc);
    for shape in visible(doc).filter(|s| !matches!(s.kind, ShapeKind::Redact { .. } | ShapeKind::Spotlight { .. }) && reaches(s)) {
        draw_shape(&mut pm, ts, shape, &cx);
    }
    pm
}

/// Dims everything outside the union of all spotlight shapes.
fn draw_spotlights(pm: &mut Pixmap, ts: Transform, doc: &Document) {
    let holes: Vec<(RectF, bool)> = visible(doc)
        .filter_map(|s| match s.kind {
            ShapeKind::Spotlight { rect, ellipse } => Some((norm_rect(rect), ellipse)),
            _ => None,
        })
        .collect();
    if holes.is_empty() {
        return;
    }
    let mut dim = Pixmap::new(pm.width(), pm.height()).expect("same size as the target");
    dim.fill(Color::from_rgba8(0, 0, 0, 140));
    let clear = Paint { blend_mode: BlendMode::Clear, anti_alias: true, ..Paint::default() };
    for (r, ellipse) in holes {
        let path = if ellipse {
            tiny_skia::Rect::from_xywh(r.x, r.y, r.w, r.h).and_then(tiny_skia::PathBuilder::from_oval)
        } else {
            rounded_rect_path(r, 6.0)
        };
        if let Some(p) = path {
            dim.fill_path(&p, &clear, tiny_skia::FillRule::Winding, ts, None);
        }
    }
    pm.draw_pixmap(0, 0, dim.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
}

fn shadow_weight(shape: &Shape) -> f32 {
    let w = match shape.kind {
        ShapeKind::Text { size, .. } | ShapeKind::Callout { size, .. } => size * 0.25,
        ShapeKind::Step { radius, .. } => radius * 0.3,
        ShapeKind::Stamp { size, .. } => size * 0.15,
        ShapeKind::Magnifier { radius, .. } => radius * 0.12,
        _ => shape.style.width.clamp(2.0, 10.0),
    };
    w.clamp(2.0, 30.0)
}

/// Draws a shape with its shadow and whole-shape opacity, which need an isolated layer.
fn draw_shape(pm: &mut Pixmap, ts: Transform, shape: &Shape, cx: &Ctx) {
    let st = &shape.style;
    let plain = matches!(shape.kind, ShapeKind::Highlighter { .. });
    let shadow = !plain && (st.shadow || matches!(shape.kind, ShapeKind::Magnifier { .. }));
    let faded = !plain && st.opacity < 0.995;
    if !shadow && !faded {
        draw_kind(&mut Canvas { pm, ts }, shape, cx);
        return;
    }

    let z = ts.sx;
    let sigma = ((1.5 + shadow_weight(shape) * 0.35) * z).max(1.0);
    let offset = (sigma * 0.8).round() as i32;
    let pad = if shadow { (sigma * 3.0).ceil() as i32 + offset + 2 } else { 2 };

    let b = bounds_with(shape, cx.sc);
    let x0 = ((b.x * ts.sx + ts.tx).floor() as i32 - pad).max(-pad);
    let y0 = ((b.y * ts.sy + ts.ty).floor() as i32 - pad).max(-pad);
    let x1 = ((b.right() * ts.sx + ts.tx).ceil() as i32 + pad).min(pm.width() as i32 + pad);
    let y1 = ((b.bottom() * ts.sy + ts.ty).ceil() as i32 + pad).min(pm.height() as i32 + pad);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let (lw, lh) = ((x1 - x0) as u32, (y1 - y0) as u32);
    let Some(mut layer) = Pixmap::new(lw, lh) else { return };
    draw_kind(&mut Canvas { pm: &mut layer, ts: ts.post_translate(-(x0 as f32), -(y0 as f32)) }, shape, cx);

    if shadow {
        let mut alpha: Vec<u8> = layer.data().as_chunks::<4>().0.iter().map(|p| p[3]).collect();
        blur_plane(&mut alpha, lw as usize, lh as usize, sigma);
        composite_shadow(pm, &alpha, lw as usize, lh as usize, x0, y0 + offset, 0.42 * st.opacity);
    }
    let paint = PixmapPaint { opacity: st.opacity.clamp(0.0, 1.0), ..PixmapPaint::default() };
    pm.draw_pixmap(x0, y0, layer.as_ref(), &paint, Transform::identity(), None);
}
