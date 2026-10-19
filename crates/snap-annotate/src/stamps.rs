//! Stamp glyphs as vector art on a unit disc (radius 1, origin at the centre, y down).

use tiny_skia::{Path, PathBuilder};

use crate::font::{self, Face};
use crate::geometry::{build, polygon_path, polyline_path, push_rounded_rect};
use crate::math::V;
use crate::model::Stamp;
use snap_core::RectF;

pub(crate) struct Art {
    pub fills: Vec<Path>,
    /// Round-capped strokes: path and width.
    pub strokes: Vec<(Path, f32)>,
}

pub(crate) fn art(glyph: Stamp) -> Art {
    let mut art = Art { fills: Vec::new(), strokes: Vec::new() };
    match glyph {
        Stamp::Check => art.strokes.extend(polyline_path(&[V(-0.42, 0.04), V(-0.12, 0.34), V(0.44, -0.30)]).map(|p| (p, 0.22))),
        Stamp::Cross => {
            art.strokes.extend(polyline_path(&[V(-0.32, -0.32), V(0.32, 0.32)]).map(|p| (p, 0.22)));
            art.strokes.extend(polyline_path(&[V(0.32, -0.32), V(-0.32, 0.32)]).map(|p| (p, 0.22)));
        }
        Stamp::Question => art.fills.extend(glyph_path(Face::Bold, '?', 1.0)),
        Stamp::Info => {
            art.fills.extend(PathBuilder::from_circle(0.0, -0.40, 0.14));
            art.strokes.extend(polyline_path(&[V(0.0, -0.06), V(0.0, 0.42)]).map(|p| (p, 0.24)));
        }
        Stamp::Warning => {
            art.strokes.extend(polyline_path(&[V(0.0, -0.46), V(0.0, 0.10)]).map(|p| (p, 0.24)));
            art.fills.extend(PathBuilder::from_circle(0.0, 0.44, 0.14));
        }
        Stamp::Star => {
            let pts: Vec<V> = (0..10)
                .map(|i| {
                    let r = if i % 2 == 0 { 0.66 } else { 0.28 };
                    let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
                    V(r * a.cos(), r * a.sin() + 0.04)
                })
                .collect();
            art.fills.extend(polygon_path(&pts));
            art.strokes.extend(polygon_path(&pts).map(|p| (p, 0.1)));
        }
        Stamp::Heart => art.fills.extend(build(|pb| {
            pb.move_to(0.0, 0.54);
            pb.cubic_to(-0.16, 0.42, -0.62, 0.12, -0.62, -0.18);
            pb.cubic_to(-0.62, -0.46, -0.2, -0.58, 0.0, -0.28);
            pb.cubic_to(0.2, -0.58, 0.62, -0.46, 0.62, -0.18);
            pb.cubic_to(0.62, 0.12, 0.16, 0.42, 0.0, 0.54);
            pb.close();
        })),
        Stamp::ThumbsUp => {
            art.fills.extend(build(|pb| push_rounded_rect(pb, RectF::new(-0.62, -0.04, 0.28, 0.6), 0.06)));
            art.fills.extend(build(|pb| {
                pb.move_to(-0.26, 0.0);
                pb.cubic_to(-0.26, -0.12, -0.02, -0.2, 0.06, -0.4);
                pb.cubic_to(0.1, -0.5, 0.2, -0.6, 0.3, -0.5);
                pb.cubic_to(0.4, -0.42, 0.32, -0.3, 0.26, -0.2);
                pb.line_to(0.5, -0.2);
                pb.cubic_to(0.62, -0.2, 0.64, -0.06, 0.6, 0.02);
                pb.cubic_to(0.64, 0.1, 0.62, 0.2, 0.56, 0.26);
                pb.cubic_to(0.58, 0.34, 0.56, 0.42, 0.5, 0.46);
                pb.cubic_to(0.48, 0.52, 0.42, 0.56, 0.34, 0.56);
                pb.line_to(-0.26, 0.56);
                pb.close();
            }));
        }
    }
    art
}

/// One glyph of `face`, scaled so capital height equals `cap` and centred on the origin.
fn glyph_path(face: Face, c: char, cap: f32) -> Option<Path> {
    let size = cap / face.cap_height();
    let text = c.to_string();
    let adv = font::advance(face, &text, size);
    build(|pb| {
        font::push_text(pb, face, &text, -adv / 2.0, cap / 2.0, size);
    })
}
