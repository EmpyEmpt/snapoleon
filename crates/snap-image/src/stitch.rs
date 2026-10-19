//! Scrolling-capture stitcher: joins successive viewport frames into one tall image.
//!
//! Every frame row is reduced to a signature of eight horizontal segment hashes (low color bits
//! dropped to survive dithering; the scrollbar strip is ignored). Rows count as equal when most
//! segments agree, so fixed sidebars and animated corners don't break matching.
//!
//! The second frame reveals sticky bands: leading rows identical between consecutive frames
//! are the header, trailing ones the footer. They are kept once (header at the top, the latest
//! footer at the bottom) and excluded from matching. Each following frame's body is then
//! aligned against the bottom of the accumulated image by scoring candidate offsets. Frames must
//! share at least 24 rows of body, i.e. scroll by clearly less than one viewport per step.

use image::RgbaImage;

use crate::resize::{resize, resize_raw};
use fast_image_resize::FilterType;

const SEGMENTS: usize = 8;
/// Segments that must agree for two rows to be "the same row".
const MATCH_SEGMENTS: usize = 6;
/// Informative (non-uniform) rows that must match for an alignment to be trusted.
const MIN_INFORMATIVE_MATCHES: u32 = 4;
/// Fewest rows two frames may share and still be aligned.
const MIN_OVERLAP: usize = 24;
/// Overlap needed before a blank-only alignment may be guessed from the usual scroll step.
const MIN_BLANK_OVERLAP: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    /// New rows were added below the existing image.
    Appended { rows: u32 },
    /// The frame adds nothing: identical to the previous one, or scrolled back (end reached).
    NoChange,
    /// No alignment was found; the frame was appended as-is.
    NoOverlap,
}

#[derive(Clone, Copy)]
struct RowSig {
    segs: [u32; SEGMENTS],
    /// False for single-color rows, which match anywhere and so prove nothing.
    informative: bool,
}

impl RowSig {
    fn matches(&self, other: &RowSig) -> bool {
        self.segs.iter().zip(&other.segs).filter(|(a, b)| a == b).count() >= MATCH_SEGMENTS
    }
}

pub struct Stitcher {
    width: u32,
    /// Header + body rows, tightly packed RGBA.
    pixels: Vec<u8>,
    sigs: Vec<RowSig>,
    /// Bottom band of the most recent accepted frame, RGBA.
    footer: Vec<u8>,
    header_rows: usize,
    footer_rows: usize,
    bands_known: bool,
    /// Signatures of the last accepted frame.
    prev: Vec<RowSig>,
    /// Row in `pixels` where the last accepted frame's body begins.
    body_start: usize,
    last_delta: usize,
}

impl Default for Stitcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Stitcher {
    pub fn new() -> Self {
        Self {
            width: 0,
            pixels: Vec::new(),
            sigs: Vec::new(),
            footer: Vec::new(),
            header_rows: 0,
            footer_rows: 0,
            bands_known: false,
            prev: Vec::new(),
            body_start: 0,
            last_delta: 0,
        }
    }

    /// Adds the next viewport frame (scrolled downwards relative to the previous one).
    pub fn push(&mut self, frame: &RgbaImage) -> PushOutcome {
        if frame.width() == 0 || frame.height() == 0 {
            return PushOutcome::NoChange;
        }
        if self.prev.is_empty() {
            self.width = frame.width();
            let sigs = row_signatures(frame);
            self.pixels = frame.as_raw().clone();
            self.sigs = sigs.clone();
            self.prev = sigs;
            return PushOutcome::Appended { rows: frame.height() };
        }
        let resized;
        let frame = if frame.width() == self.width {
            frame
        } else {
            let h = (frame.height() as f32 * self.width as f32 / frame.width() as f32).round() as u32;
            resized = resize(frame, self.width, h);
            &resized
        };
        let sigs = row_signatures(frame);
        let h = sigs.len();

        if h == self.prev.len() && same_view(&self.prev, &sigs) {
            return PushOutcome::NoChange;
        }
        if !self.bands_known {
            self.detect_bands(&sigs);
        }
        let (hdr, ftr) = (self.header_rows, self.footer_rows);
        if hdr + ftr >= h {
            return self.append_blind(frame, sigs, 0, 0);
        }
        let body_h = h - hdr - ftr;

        match self.align(&sigs, hdr, body_h) {
            None => self.append_blind(frame, sigs, hdr, ftr),
            Some(p) => {
                let acc_rows = self.sigs.len();
                let overlap = acc_rows - p;
                if overlap >= body_h {
                    return PushOutcome::NoChange;
                }
                let new_rows = body_h - overlap;
                let from = hdr + overlap;
                self.take_rows(frame, from, from + new_rows);
                self.sigs.extend_from_slice(&sigs[from..from + new_rows]);
                self.last_delta = p.saturating_sub(self.body_start);
                self.body_start = p;
                self.set_footer(frame, ftr);
                self.prev = sigs;
                PushOutcome::Appended { rows: new_rows as u32 }
            }
        }
    }

    /// Everything stitched so far, downscaled to at most `max_h` rows tall (never upscaled).
    /// Cheap enough to call after every frame.
    pub fn preview(&self, max_h: u32) -> RgbaImage {
        let total = self.height();
        if total == 0 {
            return RgbaImage::new(1, 1);
        }
        let row_bytes = self.width as usize * 4;
        if total <= max_h.max(1) {
            return RgbaImage::from_raw(self.width, total, [self.pixels.as_slice(), self.footer.as_slice()].concat()).expect("rows are tightly packed");
        }
        let scale = max_h.max(1) as f32 / total as f32;
        let tw = ((self.width as f32 * scale).round() as u32).max(1);
        let main_rows = self.sigs.len() as u32;
        let footer_rows = self.footer.len() / row_bytes;
        let main_h = ((main_rows as f32 * scale).round() as u32).max(1);
        let foot_h = if footer_rows == 0 { 0 } else { ((footer_rows as f32 * scale).round() as u32).max(1) };
        let mut out = RgbaImage::new(tw, main_h + foot_h);
        let main = resize_raw(&self.pixels, self.width, main_rows, tw, main_h, FilterType::Box);
        image::imageops::replace(&mut out, &main, 0, 0);
        if foot_h > 0 {
            let foot = resize_raw(&self.footer, self.width, footer_rows as u32, tw, foot_h, FilterType::Box);
            image::imageops::replace(&mut out, &foot, 0, main_h as i64);
        }
        out
    }

    /// Current stitched height in pixels.
    pub fn height(&self) -> u32 {
        (self.sigs.len() + self.footer.len() / (self.width.max(1) as usize * 4)) as u32
    }

    pub fn finish(mut self) -> RgbaImage {
        if self.width == 0 {
            return RgbaImage::new(1, 1);
        }
        self.pixels.extend_from_slice(&self.footer);
        let h = (self.pixels.len() / (self.width as usize * 4)) as u32;
        RgbaImage::from_raw(self.width, h, self.pixels).expect("rows are tightly packed")
    }

    /// Derives the sticky header/footer from the first frame pair that actually moved, and
    /// trims the footer that the first frame contributed to the accumulated image.
    fn detect_bands(&mut self, next: &[RowSig]) {
        self.bands_known = true;
        let prev = &self.prev;
        if prev.len() != next.len() {
            return;
        }
        let h = prev.len();
        let top = prev.iter().zip(next).take_while(|(a, b)| a.matches(b)).count();
        let bottom = prev.iter().rev().zip(next.iter().rev()).take_while(|(a, b)| a.matches(b)).count();
        let (hdr, ftr) = (top.min(h * 2 / 5), bottom.min(h * 2 / 5));
        if hdr + ftr > h * 3 / 5 {
            return;
        }
        self.header_rows = hdr;
        self.footer_rows = ftr;
        self.body_start = hdr;
        // Only the first frame is in the accumulator at this point; its footer moves to the side.
        let keep = self.sigs.len() - ftr;
        self.sigs.truncate(keep);
        self.footer = self.pixels.split_off(keep * self.width as usize * 4);
    }

    /// Row of the accumulated image where the new frame's body starts, if a trustworthy alignment exists.
    fn align(&self, sigs: &[RowSig], hdr: usize, body_h: usize) -> Option<usize> {
        let acc = &self.sigs;
        let acc_rows = acc.len();
        let min_overlap = MIN_OVERLAP.min(body_h);
        if acc_rows < hdr + min_overlap {
            return None;
        }
        let lo = hdr.max(acc_rows.saturating_sub(2 * body_h));
        let hi = acc_rows - min_overlap;
        let expected = self.body_start + if self.last_delta > 0 { self.last_delta } else { body_h / 2 };

        let mut best: Option<(i64, usize, usize)> = None; // (score, distance from expected, p)
        // Alignments whose overlap is blank on both sides prove nothing, but are the only option
        // on sparse pages; the scroll step the user has been using then decides.
        let mut best_blank: Option<(usize, usize)> = None; // (distance from expected, p)
        for p in lo..=hi {
            let overlap = body_h.min(acc_rows - p);
            let budget = overlap as u32 / 10 + 1;
            let (mut informative, mut informative_eq, mut mismatches) = (0u32, 0u32, 0u32);
            for j in 0..overlap {
                let new = &sigs[hdr + j];
                let eq = new.matches(&acc[p + j]);
                if new.informative {
                    informative += 1;
                    informative_eq += eq as u32;
                }
                if !eq {
                    mismatches += 1;
                    if mismatches > budget {
                        break;
                    }
                }
            }
            if mismatches > budget {
                continue;
            }
            if informative == 0 {
                let dist = p.abs_diff(expected);
                if mismatches == 0 && overlap >= MIN_BLANK_OVERLAP && best_blank.is_none_or(|(d, _)| dist < d) {
                    best_blank = Some((dist, p));
                }
                continue;
            }
            if informative_eq * 10 < informative * 8 {
                continue;
            }
            // Few distinctive rows are weak evidence: trust them only when perfect and where the
            // scroll step so far says the frame should land.
            let dist = p.abs_diff(expected);
            if informative_eq < MIN_INFORMATIVE_MATCHES && (mismatches > 0 || dist > body_h / 8) {
                continue;
            }
            let score = informative_eq as i64 - 3 * mismatches as i64;
            if best.is_none_or(|(s, d, _)| score > s || (score == s && dist < d)) {
                best = Some((score, dist, p));
            }
        }
        best.map(|(_, _, p)| p).or(best_blank.map(|(_, p)| p))
    }

    fn append_blind(&mut self, frame: &RgbaImage, sigs: Vec<RowSig>, hdr: usize, ftr: usize) -> PushOutcome {
        let h = sigs.len();
        self.body_start = self.sigs.len();
        self.take_rows(frame, hdr, h - ftr);
        self.sigs.extend_from_slice(&sigs[hdr..h - ftr]);
        self.set_footer(frame, ftr);
        self.prev = sigs;
        PushOutcome::NoOverlap
    }

    fn take_rows(&mut self, frame: &RgbaImage, from: usize, to: usize) {
        let row_bytes = self.width as usize * 4;
        self.pixels.extend_from_slice(&frame.as_raw()[from * row_bytes..to * row_bytes]);
    }

    fn set_footer(&mut self, frame: &RgbaImage, rows: usize) {
        let row_bytes = self.width as usize * 4;
        let h = frame.height() as usize;
        self.footer.clear();
        self.footer.extend_from_slice(&frame.as_raw()[(h - rows) * row_bytes..]);
    }
}

/// True when the two frames differ only in one small local region (caret, spinner, hover):
/// nothing scrolled.
fn same_view(a: &[RowSig], b: &[RowSig]) -> bool {
    let mut changed = a.iter().zip(b).enumerate().filter(|(_, (x, y))| !x.matches(y)).map(|(i, _)| i);
    let Some(first) = changed.next() else { return true };
    let last = changed.next_back().unwrap_or(first);
    last - first < (a.len() / 16).max(3)
}

fn row_signatures(frame: &RgbaImage) -> Vec<RowSig> {
    let w = frame.width() as usize;
    // The right edge is usually a scrollbar whose thumb moves between frames.
    let scrollbar = (w / 40).clamp(4, 24).min(w - 1);
    let used = w - scrollbar;
    frame
        .as_raw()
        .chunks_exact(w * 4)
        .map(|row| {
            let px = |x: usize| u32::from_le_bytes([row[x * 4], row[x * 4 + 1], row[x * 4 + 2], 0]) & 0x00FC_FCFC;
            let first = px(0);
            let mut informative = false;
            let mut segs = [0u32; SEGMENTS];
            for (s, seg) in segs.iter_mut().enumerate() {
                let (x0, x1) = (s * used / SEGMENTS, (s + 1) * used / SEGMENTS);
                let mut hash = 0x811c_9dc5u32;
                for x in x0..x1 {
                    let p = px(x);
                    informative |= p != first;
                    hash = (hash ^ p).wrapping_mul(0x0100_0193);
                    hash ^= hash >> 15;
                }
                *seg = hash;
            }
            RowSig { segs, informative }
        })
        .collect()
}
