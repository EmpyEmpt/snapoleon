//! Engine-independent driver: splits very tall images into bands and retries
//! small text on a 2x upscaled copy.

use image::imageops::{self, FilterType};
use snap_core::{Result, RgbaImage};

use crate::OcrResult;

/// Images taller than this are recognised in bands.
const BAND_TRIGGER: u32 = 4000;
const BAND_HEIGHT: u32 = 3000;
/// How far above the nominal cut we look for an empty pixel row.
const CUT_SEARCH: u32 = 400;
/// Median word-box height (px) under which a x2 pass is tried.
const SMALL_TEXT: f32 = 14.0;
/// Upscaling is skipped beyond this width (engines cap image size).
const MAX_UPSCALE_WIDTH: u32 = 4500;

pub(crate) type Backend<'a> = &'a dyn Fn(&RgbaImage) -> Result<OcrResult>;

pub(crate) fn run(img: &RgbaImage, backend: Backend) -> Result<OcrResult> {
    if img.height() <= BAND_TRIGGER {
        return with_upscale(img, backend);
    }
    let mut merged = OcrResult::default();
    let mut y = 0;
    while y < img.height() {
        let end = if img.height() - y <= BAND_TRIGGER { img.height() } else { cut_row(img, y + BAND_HEIGHT) };
        let band = imageops::crop_imm(img, 0, y, img.width(), end - y).to_image();
        let part = with_upscale(&band, backend)?;
        merged.engine = part.engine;
        merged.language = merged.language.or(part.language);
        for mut line in part.lines {
            offset(&mut line, y as f32);
            merged.lines.push(line);
        }
        y = end;
    }
    Ok(merged)
}

fn offset(line: &mut crate::OcrLine, dy: f32) {
    line.rect.y += dy;
    for w in &mut line.words {
        w.rect.y += dy;
    }
}

/// Row at or above `target` that looks like background, so words are not cut.
fn cut_row(img: &RgbaImage, target: u32) -> u32 {
    (target.saturating_sub(CUT_SEARCH)..=target).rev().find(|&y| row_is_flat(img, y)).unwrap_or(target)
}

fn row_is_flat(img: &RgbaImage, y: u32) -> bool {
    let first = img.get_pixel(0, y).0;
    (0..img.width()).all(|x| img.get_pixel(x, y).0.iter().zip(first).all(|(a, b)| a.abs_diff(b) <= 6))
}

fn with_upscale(img: &RgbaImage, backend: Backend) -> Result<OcrResult> {
    let first = backend(img)?;
    if img.width() > MAX_UPSCALE_WIDTH || !wants_upscale(&first, img) {
        return Ok(first);
    }
    let big = imageops::resize(img, img.width() * 2, img.height() * 2, FilterType::CatmullRom);
    match backend(&big) {
        Ok(mut second) => {
            second.scale(0.5);
            Ok(merge(vec![first, second]))
        }
        Err(_) => Ok(first),
    }
}

/// True when some line has small text (median word-box height under
/// [`SMALL_TEXT`]); with nothing found, only small crops are retried.
fn wants_upscale(first: &OcrResult, img: &RgbaImage) -> bool {
    if first.lines.is_empty() {
        return img.height() <= 600;
    }
    first.lines.iter().any(|l| {
        let mut heights: Vec<f32> = l.words.iter().map(|w| w.rect.h).collect();
        heights.sort_by(f32::total_cmp);
        heights.get(heights.len() / 2).is_some_and(|h| *h < SMALL_TEXT)
    })
}

impl OcrResult {
    pub(crate) fn scale(&mut self, f: f32) {
        let s = |r: &mut snap_core::RectF| {
            r.x *= f;
            r.y *= f;
            r.w *= f;
            r.h *= f;
        };
        for l in &mut self.lines {
            s(&mut l.rect);
            l.words.iter_mut().for_each(|w| s(&mut w.rect));
        }
    }
}

/// How much a recognised word looks like real text: stray digits inside
/// letters, mixed scripts and camelCase noise are typical of an engine
/// reading a script it was not built for.
fn word_weight(w: &str) -> f32 {
    let (mut latin, mut cyrillic, mut digit, mut other) = (false, false, false, false);
    let mut inner_case = false;
    let mut prev_lower = false;
    for c in w.chars() {
        if c.is_ascii_digit() {
            digit = true;
        } else if c.is_alphabetic() {
            match c {
                'a'..='z' | 'A'..='Z' | '\u{c0}'..='\u{24f}' => latin = true,
                '\u{400}'..='\u{4ff}' => cyrillic = true,
                _ => other = true,
            }
            inner_case |= prev_lower && c.is_uppercase();
        }
        prev_lower = c.is_lowercase();
    }
    let letters = latin || cyrillic || other;
    if (latin && cyrillic) || inner_case {
        0.25
    } else if (latin && consonant_run(w) >= 4) || (cyrillic && consonant_run(w) >= 5) {
        0.2
    } else if letters && digit {
        0.6
    } else {
        1.0
    }
}

/// Longest run of consonants in a Latin or Cyrillic word; long runs are what
/// an engine emits when reading text in a script it was not built for.
fn consonant_run(w: &str) -> usize {
    let (mut run, mut best) = (0, 0);
    for c in w.chars().flat_map(char::to_lowercase) {
        if c.is_alphabetic() && !"aeiouyаеёиоуыэюяъь".contains(c) {
            run += 1;
            best = best.max(run);
        } else {
            run = 0;
        }
    }
    best
}

/// Plausibility-weighted count of alphanumeric characters in a line.
pub(crate) fn line_score(text: &str) -> f32 {
    text.split_whitespace().map(|w| word_weight(w) * w.chars().filter(|c| c.is_alphanumeric()).count() as f32).sum()
}

/// Combine results of several passes (language engines, upscaled copy) over
/// the same image. The first result sets reading order; for every region the
/// line that looks most like real text wins, and the earlier pass wins ties.
pub(crate) fn merge(results: Vec<OcrResult>) -> OcrResult {
    let mut results = results.into_iter();
    let Some(mut merged) = results.next() else { return OcrResult::default() };
    let mut languages: Vec<String> = merged.language.iter().cloned().collect();

    for other in results {
        let mut used = false;
        for line in other.lines {
            let hits: Vec<usize> = (0..merged.lines.len()).filter(|&i| overlaps(&merged.lines[i].rect, &line.rect)).collect();
            if hits.is_empty() {
                let at = merged.lines.iter().position(|l| l.rect.center().y > line.rect.center().y).unwrap_or(merged.lines.len());
                merged.lines.insert(at, line);
                used = true;
            } else if let [i] = hits[..] {
                let (keep, other) = if line_score(&line.text) > line_score(&merged.lines[i].text) {
                    (line, merged.lines[i].clone())
                } else {
                    (merged.lines[i].clone(), line)
                };
                used |= keep.rect != merged.lines[i].rect || keep.text != merged.lines[i].text;
                merged.lines[i] = patch_words(keep, &other);
            } else if hits.iter().all(|&i| line_score(&line.text) > line_score(&merged.lines[i].text)) {
                merged.lines[hits[0]] = line;
                for &i in hits[1..].iter().rev() {
                    merged.lines.remove(i);
                }
                used = true;
            }
        }
        if used
            && let Some(l) = other.language
            && !languages.contains(&l)
        {
            languages.push(l);
        }
    }
    merged.language = (!languages.is_empty()).then(|| languages.join(","));
    merged
}

fn overlaps(a: &snap_core::RectF, b: &snap_core::RectF) -> bool {
    let w = a.right().min(b.right()) - a.x.max(b.x);
    let h = a.bottom().min(b.bottom()) - a.y.max(b.y);
    w > 0.0 && h > 0.0 && w * h > 0.4 * (a.w * a.h).min(b.w * b.h)
}

/// Replace words of `line` that look like noise with the aligned word from
/// `other` when that one looks strictly more like real text.
fn patch_words(mut line: crate::OcrLine, other: &crate::OcrLine) -> crate::OcrLine {
    let aligned = line.words.len() == other.words.len()
        && line.words.iter().zip(&other.words).all(|(x, y)| (x.rect.center().x - y.rect.center().x).abs() < 0.5 * x.rect.w.max(y.rect.w));
    if !aligned {
        return line;
    }
    let mut changed = false;
    for (w, o) in line.words.iter_mut().zip(&other.words) {
        if word_weight(&o.text) > word_weight(&w.text) {
            w.text = o.text.clone();
            changed = true;
        }
    }
    if changed {
        line.text = line.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
    }
    line
}
