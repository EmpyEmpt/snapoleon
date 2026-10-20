//! Engine-independent driver: splits very tall or very wide images into bands and tiles and retries
//! small text on a 2x upscaled copy.

use image::imageops::{self, FilterType};
use snap_core::{RectF, Result, RgbaImage};

use crate::{OcrLine, OcrResult, OcrWord};

/// Images taller than this are recognised in bands.
const BAND_TRIGGER: u32 = 4000;
const BAND_HEIGHT: u32 = 3000;
/// Images wider than this are recognised in column tiles (Windows OCR rejects anything over ~10000 px).
const TILE_TRIGGER: u32 = 8000;
const TILE_WIDTH: u32 = 6000;
/// Neighbouring tiles share this many pixels, more than twice the widest word, so a word cut by one tile's edge sits
/// whole in the other.
const TILE_OVERLAP: u32 = 800;
/// How far above the nominal cut we look for an empty pixel row.
const CUT_SEARCH: u32 = 400;
/// Median word-box height (px) under which a x2 pass is tried.
const SMALL_TEXT: f32 = 14.0;
/// Upscaling is skipped beyond this width (engines cap image size).
const MAX_UPSCALE_WIDTH: u32 = 4500;

pub(crate) type Backend<'a> = &'a dyn Fn(&RgbaImage) -> Result<OcrResult>;

pub(crate) fn run(img: &RgbaImage, backend: Backend) -> Result<OcrResult> {
    if img.width() <= TILE_TRIGGER {
        return run_bands(img, backend);
    }
    let mut merged = OcrResult::default();
    // Each tile keeps the words between the middles of its overlaps; the pieces of a row are joined again across a seam.
    let (mut x, mut seam_in) = (0, f32::NEG_INFINITY);
    while x < img.width() {
        let last = img.width() - x <= TILE_TRIGGER;
        let cut = if last { img.width() } else { cut_column(img, x + TILE_WIDTH) };
        let end = if last { cut } else { (cut + TILE_OVERLAP).min(img.width()) };
        let seam_out = if last { f32::INFINITY } else { (cut + TILE_OVERLAP / 2) as f32 };
        let tile = imageops::crop_imm(img, x, 0, end - x, img.height()).to_image();
        let part = run_bands(&tile, backend)?;
        merged.engine = part.engine;
        merged.language = merged.language.or(part.language);
        let earlier = merged.lines.len();
        for mut line in part.lines {
            shift(&mut line, x as f32, 0.0);
            let Some(line) = owned(line, seam_in, seam_out) else { continue };
            match merged.lines[..earlier].iter().position(|before| continues(before, &line)) {
                Some(i) => merged.lines[i] = join(&merged.lines[i], line),
                None => merged.lines.push(line),
            }
        }
        x = cut;
        seam_in = seam_out;
    }
    Ok(merged)
}

/// The part of `line` whose words centre in `from..to`.
fn owned(line: OcrLine, from: f32, to: f32) -> Option<OcrLine> {
    let mine = |rect: RectF| (from..to).contains(&rect.center().x);
    if line.words.is_empty() {
        return mine(line.rect).then_some(line);
    }
    if line.words.iter().all(|w| mine(w.rect)) {
        return Some(line);
    }
    let words: Vec<OcrWord> = line.words.into_iter().filter(|w| mine(w.rect)).collect();
    let rect = words.iter().map(|w| w.rect).reduce(RectF::union)?;
    let text = words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
    Some(OcrLine { text, rect, words })
}

/// Whether `next`, from the tile to the right of a seam, is the rest of the row that `before` begins: the same
/// height and no more than a couple of characters apart.
fn continues(before: &OcrLine, next: &OcrLine) -> bool {
    let (a, b) = (before.rect, next.rect);
    let height = a.h.max(b.h);
    let shared = a.bottom().min(b.bottom()) - a.y.max(b.y);
    shared > 0.5 * a.h.min(b.h) && (b.x - a.right()).abs() <= 2.0 * height
}

fn join(before: &OcrLine, next: OcrLine) -> OcrLine {
    let mut words = before.words.clone();
    words.extend(next.words);
    OcrLine { text: format!("{} {}", before.text, next.text), rect: before.rect.union(next.rect), words }
}

fn run_bands(img: &RgbaImage, backend: Backend) -> Result<OcrResult> {
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
            shift(&mut line, 0.0, y as f32);
            merged.lines.push(line);
        }
        y = end;
    }
    Ok(merged)
}

fn shift(line: &mut crate::OcrLine, dx: f32, dy: f32) {
    line.rect.x += dx;
    line.rect.y += dy;
    for w in &mut line.words {
        w.rect.x += dx;
        w.rect.y += dy;
    }
}

/// Row at or above `target` that looks like background, so words are not cut.
fn cut_row(img: &RgbaImage, target: u32) -> u32 {
    (target.saturating_sub(CUT_SEARCH)..=target).rev().find(|&y| row_is_flat(img, y)).unwrap_or(target)
}

/// Column at or left of `target` that looks like background, so words are not cut.
fn cut_column(img: &RgbaImage, target: u32) -> u32 {
    (target.saturating_sub(CUT_SEARCH)..=target).rev().find(|&x| column_is_flat(img, x)).unwrap_or(target)
}

fn column_is_flat(img: &RgbaImage, x: u32) -> bool {
    let first = img.get_pixel(x, 0).0;
    (0..img.height()).all(|y| img.get_pixel(x, y).0.iter().zip(first).all(|(a, b)| a.abs_diff(b) <= 6))
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
