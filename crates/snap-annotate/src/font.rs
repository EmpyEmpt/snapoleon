//! Text layout and glyph outlines (Inter, JetBrains Mono) as tiny-skia paths.

use std::sync::OnceLock;

use ab_glyph::{Font, FontRef, OutlineCurve, Point};
use tiny_skia::PathBuilder;

use snap_core::fonts::INTER_BOLD as BOLD;
use snap_core::fonts::INTER_REGULAR as REGULAR;
use snap_core::fonts::JETBRAINS_MONO as MONO;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Face {
    Regular,
    Bold,
    Mono,
}

impl Face {
    pub fn pick(bold: bool) -> Face {
        if bold { Face::Bold } else { Face::Regular }
    }

    fn font(self) -> &'static FontRef<'static> {
        static FONTS: OnceLock<[FontRef<'static>; 3]> = OnceLock::new();
        let fonts = FONTS.get_or_init(|| {
            let parse = |b: &'static [u8]| FontRef::try_from_slice(b).expect("embedded font parses");
            [parse(REGULAR), parse(BOLD), parse(MONO)]
        });
        &fonts[self as usize]
    }

    /// Height of capital letters and digits, in em.
    pub fn cap_height(self) -> f32 {
        match self {
            Face::Mono => 0.73,
            _ => 0.727,
        }
    }
}

/// Line pitch in em.
const LINE_HEIGHT: f32 = 1.3;
/// Distance from the line box top to the baseline, in em (ascent plus half the leading).
const BASELINE: f32 = 1.015;

fn units(f: &FontRef<'static>) -> f32 {
    f.units_per_em().unwrap_or(2048.0)
}

/// Advance width of `text` at em size `size`, including kerning.
pub(crate) fn advance(face: Face, text: &str, size: f32) -> f32 {
    let f = face.font();
    let k = size / units(f);
    let mut w = 0.0;
    let mut prev = None;
    for c in text.chars() {
        let id = f.glyph_id(c);
        if let Some(p) = prev {
            w += f.kern_unscaled(p, id) * k;
        }
        w += f.h_advance_unscaled(id) * k;
        prev = Some(id);
    }
    w
}

#[derive(Clone, Debug)]
pub(crate) struct Line {
    pub text: String,
    pub width: f32,
}

#[derive(Clone, Debug)]
pub(crate) struct Layout {
    pub lines: Vec<Line>,
    pub size: f32,
    pub width: f32,
}

impl Layout {
    pub fn line_height(&self) -> f32 {
        self.size * LINE_HEIGHT
    }

    pub fn height(&self) -> f32 {
        self.line_height() * self.lines.len() as f32
    }

    /// Baseline offset of line `i` from the top of the layout.
    pub fn baseline(&self, i: usize) -> f32 {
        self.line_height() * i as f32 + self.size * BASELINE
    }
}

/// Greedy word wrap at `max_w` (long words are broken by character); `\n` always breaks.
pub(crate) fn layout(face: Face, text: &str, size: f32, max_w: Option<f32>) -> Layout {
    let size = size.max(1.0);
    let text = text.replace('\r', "").replace('\t', "    ");
    let measure = |s: &str| advance(face, s, size);
    let mut lines: Vec<Line> = Vec::new();
    for para in text.split('\n') {
        match max_w {
            None => lines.push(Line { text: para.to_string(), width: measure(para) }),
            Some(mw) => wrap_paragraph(para, mw.max(size), &measure, &mut lines),
        }
    }
    let width = lines.iter().map(|l| l.width).fold(0.0, f32::max);
    Layout { lines, size, width }
}

fn wrap_paragraph(para: &str, mw: f32, measure: &dyn Fn(&str) -> f32, out: &mut Vec<Line>) {
    let mut cur = String::new();
    for word in para.split(' ') {
        let candidate = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
        if measure(&candidate) <= mw {
            cur = candidate;
            continue;
        }
        if !cur.is_empty() {
            let width = measure(&cur);
            out.push(Line { text: std::mem::take(&mut cur), width });
        }
        // A single word wider than the box is split into pieces that fit.
        let mut piece = String::new();
        for ch in word.chars() {
            let mut next = piece.clone();
            next.push(ch);
            if !piece.is_empty() && measure(&next) > mw {
                let width = measure(&piece);
                out.push(Line { text: std::mem::take(&mut piece), width });
                piece.push(ch);
            } else {
                piece = next;
            }
        }
        cur = piece;
    }
    let width = measure(&cur);
    out.push(Line { text: cur, width });
}

/// Appends the outlines of `text` with its baseline at `y`, starting at `x`. Returns the advance.
pub(crate) fn push_text(pb: &mut PathBuilder, face: Face, text: &str, x: f32, y: f32, size: f32) -> f32 {
    let f = face.font();
    let k = size / units(f);
    let mut caret = 0.0;
    let mut prev = None;
    for c in text.chars() {
        let id = f.glyph_id(c);
        if let Some(p) = prev {
            caret += f.kern_unscaled(p, id) * k;
        }
        if let Some(outline) = f.outline(id) {
            push_outline(pb, &outline.curves, x + caret, y, k);
        }
        caret += f.h_advance_unscaled(id) * k;
        prev = Some(id);
    }
    caret
}

fn push_outline(pb: &mut PathBuilder, curves: &[OutlineCurve], ox: f32, oy: f32, k: f32) {
    let map = |p: &Point| (ox + p.x * k, oy - p.y * k);
    let mut end: Option<(f32, f32)> = None;
    for curve in curves {
        let start = match curve {
            OutlineCurve::Line(a, _) | OutlineCurve::Quad(a, ..) | OutlineCurve::Cubic(a, ..) => map(a),
        };
        if end.is_none_or(|e| (e.0 - start.0).abs() > 1e-3 || (e.1 - start.1).abs() > 1e-3) {
            if end.is_some() {
                pb.close();
            }
            pb.move_to(start.0, start.1);
        }
        end = Some(match curve {
            OutlineCurve::Line(_, b) => {
                let b = map(b);
                pb.line_to(b.0, b.1);
                b
            }
            OutlineCurve::Quad(_, c, b) => {
                let (c, b) = (map(c), map(b));
                pb.quad_to(c.0, c.1, b.0, b.1);
                b
            }
            OutlineCurve::Cubic(_, c1, c2, b) => {
                let (c1, c2, b) = (map(c1), map(c2), map(b));
                pb.cubic_to(c1.0, c1.1, c2.0, c2.1, b.0, b.1);
                b
            }
        });
    }
    if end.is_some() {
        pb.close();
    }
}
