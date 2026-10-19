//! Visual comparison of two images.

use image::{Rgba, RgbaImage};
use snap_core::Rect;

use crate::util::{blit_copy, over};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DiffMode {
    /// `b` desaturated, with every changed pixel painted red.
    Highlight,
    /// `a` with `b` blended on top at `opacity` (onion skin).
    Overlay { opacity: f32 },
    /// `a` and `b` next to each other, the changed bounding box outlined in both.
    SideBySide,
    /// Per-channel absolute difference; unchanged pixels are black.
    Difference,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiffStats {
    pub changed_pixels: u64,
    /// `changed_pixels` over the area of the larger canvas, 0.0..=1.0.
    pub changed_ratio: f32,
    pub bbox: Option<Rect>,
}

const HIGHLIGHT: [u8; 4] = [255, 45, 85, 255];
const SIDE_GAP: u32 = 12;
const OUTLINE: u32 = 2;

/// Compares over the union canvas of both images (area covered by only one image counts as
/// changed). A pixel is changed when any RGBA channel differs by more than `threshold`.
pub fn diff(a: &RgbaImage, b: &RgbaImage, mode: DiffMode, threshold: u8) -> (RgbaImage, DiffStats) {
    let (w, h) = (a.width().max(b.width()).max(1), a.height().max(b.height()).max(1));
    let mut mask = vec![false; (w * h) as usize];
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (u32::MAX, u32::MAX, 0u32, 0u32);
    let mut changed = 0u64;
    for y in 0..h {
        for x in 0..w {
            let pa = pixel(a, x, y);
            let pb = pixel(b, x, y);
            let differs = match (pa, pb) {
                (Some(pa), Some(pb)) => pa.iter().zip(&pb).any(|(p, q)| p.abs_diff(*q) > threshold),
                _ => true,
            };
            if differs {
                mask[(y * w + x) as usize] = true;
                changed += 1;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    let bbox = (changed > 0).then(|| Rect::new(min_x as i32, min_y as i32, max_x - min_x + 1, max_y - min_y + 1));
    let stats = DiffStats { changed_pixels: changed, changed_ratio: changed as f32 / (w as f32 * h as f32), bbox };

    let out = match mode {
        DiffMode::Highlight => highlight(b, &mask, w, h),
        DiffMode::Overlay { opacity } => overlay(a, b, opacity.clamp(0.0, 1.0), w, h),
        DiffMode::SideBySide => side_by_side(a, b, bbox, w, h),
        DiffMode::Difference => difference(a, b, &mask, w, h),
    };
    (out, stats)
}

fn pixel(img: &RgbaImage, x: u32, y: u32) -> Option<[u8; 4]> {
    (x < img.width() && y < img.height()).then(|| img.get_pixel(x, y).0)
}

fn highlight(b: &RgbaImage, mask: &[bool], w: u32, h: u32) -> RgbaImage {
    let mut out = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let base = match pixel(b, x, y) {
                Some([r, g, bl, _]) => {
                    // Washed-out grayscale keeps context readable under the red.
                    let l = (54 * r as u32 + 183 * g as u32 + 19 * bl as u32) >> 8;
                    let v = (l * 2 / 5 + 153) as u8;
                    [v, v, v, 255]
                }
                None => [235, 235, 235, 255],
            };
            let mut px = base;
            if mask[(y * w + x) as usize] {
                over(&mut px, [HIGHLIGHT[0], HIGHLIGHT[1], HIGHLIGHT[2], 170]);
            }
            out.put_pixel(x, y, Rgba(px));
        }
    }
    out
}

fn overlay(a: &RgbaImage, b: &RgbaImage, opacity: f32, w: u32, h: u32) -> RgbaImage {
    let mut out = RgbaImage::new(w, h);
    blit_copy(&mut out, a, 0, 0);
    let alpha = (opacity * 255.0).round() as u32;
    for y in 0..b.height() {
        for x in 0..b.width() {
            let p = b.get_pixel(x, y).0;
            let a8 = (p[3] as u32 * alpha / 255) as u8;
            over(&mut out.get_pixel_mut(x, y).0, [p[0], p[1], p[2], a8]);
        }
    }
    out
}

fn side_by_side(a: &RgbaImage, b: &RgbaImage, bbox: Option<Rect>, w: u32, h: u32) -> RgbaImage {
    let mut out = RgbaImage::from_pixel(w * 2 + SIDE_GAP, h, Rgba([38, 38, 42, 255]));
    blit_copy(&mut out, &padded(a, w, h), 0, 0);
    blit_copy(&mut out, &padded(b, w, h), (w + SIDE_GAP) as i64, 0);
    if let Some(r) = bbox {
        for origin in [0i32, (w + SIDE_GAP) as i32] {
            outline(&mut out, Rect::new(origin + r.x, r.y, r.w, r.h).inflate(1));
        }
    }
    out
}

fn padded(img: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    if img.dimensions() == (w, h) {
        return img.clone();
    }
    let mut out = RgbaImage::from_pixel(w, h, Rgba([38, 38, 42, 255]));
    blit_copy(&mut out, img, 0, 0);
    out
}

fn outline(img: &mut RgbaImage, r: Rect) {
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    for t in 0..OUTLINE as i32 {
        let (x0, y0, x1, y1) = (r.x - t, r.y - t, r.right() - 1 + t, r.bottom() - 1 + t);
        for x in x0..=x1 {
            for y in [y0, y1] {
                if (0..iw).contains(&x) && (0..ih).contains(&y) {
                    img.put_pixel(x as u32, y as u32, Rgba(HIGHLIGHT));
                }
            }
        }
        for y in y0..=y1 {
            for x in [x0, x1] {
                if (0..iw).contains(&x) && (0..ih).contains(&y) {
                    img.put_pixel(x as u32, y as u32, Rgba(HIGHLIGHT));
                }
            }
        }
    }
}

fn difference(a: &RgbaImage, b: &RgbaImage, mask: &[bool], w: u32, h: u32) -> RgbaImage {
    let mut out = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 255]));
    for y in 0..h {
        for x in 0..w {
            if !mask[(y * w + x) as usize] {
                continue;
            }
            let px = match (pixel(a, x, y), pixel(b, x, y)) {
                (Some(pa), Some(pb)) => [pa[0].abs_diff(pb[0]), pa[1].abs_diff(pb[1]), pa[2].abs_diff(pb[2]), 255],
                // Present in only one image: full-intensity marker.
                _ => [255, 255, 255, 255],
            };
            out.put_pixel(x, y, Rgba(px));
        }
    }
    out
}
