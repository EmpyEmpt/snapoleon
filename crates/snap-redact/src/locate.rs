//! Maps matches onto OCR word boxes.

use snap_core::geom::RectF;
use snap_core::settings::RedactKind;

use crate::find;

/// Pixels added on every side of a hit so the cover fully hides the glyphs.
const INFLATE: f32 = 2.0;

/// Words of one OCR line, in reading order, with their boxes in image pixels.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LineWords {
    pub words: Vec<(String, RectF)>,
}

/// A region to cover. `kind` is `None` for substrings supplied by an AI suggestion.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub kind: Option<RedactKind>,
    pub rect: RectF,
    pub text: String,
}

/// Detects `kinds` on each line and returns the covering rectangles.
///
/// Each line is scanned on its own: words are joined with single spaces and a
/// match covers the union of every word it touches.
pub fn locate(lines: &[LineWords], kinds: &[RedactKind]) -> Vec<Hit> {
    let mut hits = Vec::new();
    for line in lines {
        let (text, spans) = join(line);
        for m in find(&text, kinds) {
            if let Some(rect) = cover(&spans, m.start, m.end) {
                hits.push(Hit {
                    kind: Some(m.kind),
                    rect: inflate(rect),
                    text: m.text,
                });
            }
        }
    }
    dedupe(hits)
}

/// Covers every occurrence of each needle, for text that detection cannot
/// classify (for example spans an AI model suggested).
pub fn locate_substrings(lines: &[LineWords], needles: &[String]) -> Vec<Hit> {
    let mut hits = Vec::new();
    for line in lines {
        let (text, spans) = join(line);
        for needle in needles.iter().map(|n| n.trim()).filter(|n| !n.is_empty()) {
            for (start, _) in text.match_indices(needle) {
                if let Some(rect) = cover(&spans, start, start + needle.len()) {
                    hits.push(Hit {
                        kind: None,
                        rect: inflate(rect),
                        text: needle.to_owned(),
                    });
                }
            }
        }
    }
    dedupe(hits)
}

/// Joins a line's words with single spaces and records each word's byte range.
fn join(line: &LineWords) -> (String, Vec<(usize, usize, RectF)>) {
    let mut text = String::new();
    let mut spans = Vec::new();
    for (word, rect) in &line.words {
        if word.is_empty() {
            continue;
        }
        if !text.is_empty() {
            text.push(' ');
        }
        let start = text.len();
        text.push_str(word);
        spans.push((start, text.len(), *rect));
    }
    (text, spans)
}

/// Union of the word boxes that overlap the byte range `start..end`.
fn cover(spans: &[(usize, usize, RectF)], start: usize, end: usize) -> Option<RectF> {
    spans
        .iter()
        .filter(|&&(s, e, _)| s < end && start < e)
        .map(|&(_, _, r)| r)
        .reduce(union)
}

fn union(a: RectF, b: RectF) -> RectF {
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    RectF::new(
        x,
        y,
        a.right().max(b.right()) - x,
        a.bottom().max(b.bottom()) - y,
    )
}

fn inflate(r: RectF) -> RectF {
    RectF::new(
        r.x - INFLATE,
        r.y - INFLATE,
        r.w + 2.0 * INFLATE,
        r.h + 2.0 * INFLATE,
    )
}

/// Drops a hit that mostly overlaps one already kept.
fn dedupe(hits: Vec<Hit>) -> Vec<Hit> {
    let mut kept: Vec<Hit> = Vec::with_capacity(hits.len());
    for h in hits {
        if !kept.iter().any(|k| overlap_fraction(k.rect, h.rect) > 0.5) {
            kept.push(h);
        }
    }
    kept
}

/// Intersection area as a fraction of the smaller rectangle.
fn overlap_fraction(a: RectF, b: RectF) -> f32 {
    let iw = (a.right().min(b.right()) - a.x.max(b.x)).max(0.0);
    let ih = (a.bottom().min(b.bottom()) - a.y.max(b.y)).max(0.0);
    let smaller = (a.w * a.h).min(b.w * b.h);
    if smaller <= 0.0 {
        0.0
    } else {
        iw * ih / smaller
    }
}
