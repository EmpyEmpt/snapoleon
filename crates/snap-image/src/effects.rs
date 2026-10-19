//! Pixel effects: redaction, blur, spotlight, shadows, rounded corners.
//!
//! "Radius" parameters follow the CSS blur convention: the visible extent of the
//! blur, i.e. a Gaussian with sigma = radius / 2.

use image::RgbaImage;
use snap_core::Rect;
use snap_core::settings::RedactStyle;

use crate::blur;
use crate::util::{Noise, blit_over, clamp_rect, crop, div255};

/// Mosaic: every `block`x`block` cell (anchored at the rect's top-left) becomes its mean color.
pub fn pixelate(img: &mut RgbaImage, r: Rect, block: u32) {
    let Some((x0, y0, x1, y1)) = clamp_rect(r, img.width(), img.height()) else { return };
    let block = block.max(1);
    let mut cy = y0;
    while cy < y1 {
        let by1 = (cy + block).min(y1);
        let mut cx = x0;
        while cx < x1 {
            let bx1 = (cx + block).min(x1);
            let avg = mean_color(img, cx, cy, bx1, by1);
            for y in cy..by1 {
                for x in cx..bx1 {
                    img.put_pixel(x, y, image::Rgba(avg));
                }
            }
            cx = bx1;
        }
        cy = by1;
    }
}

/// Gaussian blur confined to `r`; pixels outside the rect are neither read nor written.
pub fn blur(img: &mut RgbaImage, r: Rect, radius: f32) {
    let Some((x0, y0, x1, y1)) = clamp_rect(r, img.width(), img.height()) else { return };
    if radius <= 0.0 {
        return;
    }
    let region = crop(img, x0, y0, x1, y1);
    let blurred = gaussian_blur(&region, radius * 0.5);
    image::imageops::replace(img, &blurred, x0 as i64, y0 as i64);
}

pub fn fill(img: &mut RgbaImage, r: Rect, rgba: [u8; 4]) {
    let Some((x0, y0, x1, y1)) = clamp_rect(r, img.width(), img.height()) else { return };
    for y in y0..y1 {
        for x in x0..x1 {
            img.put_pixel(x, y, image::Rgba(rgba));
        }
    }
}

/// Irreversible redaction. The result is always fully opaque.
///
/// * `Pixelate`: blocks of at least `max(12, h / 2)` px, so a text line collapses to a couple of
///   cells, each shifted by its own large random offset. (Noise added per pixel would average
///   out over a cell and leave the exact mean, which can be matched against rendered glyphs.)
/// * `Blur`: a blur wider than the text height, plus noise.
/// * `Solid`: opaque black.
pub fn redact(img: &mut RgbaImage, r: Rect, style: RedactStyle) {
    let Some((x0, y0, x1, y1)) = clamp_rect(r, img.width(), img.height()) else { return };
    let region = Rect::new(x0 as i32, y0 as i32, x1 - x0, y1 - y0);
    let height = y1 - y0;
    match style {
        RedactStyle::Solid => {
            fill(img, region, [0, 0, 0, 255]);
            return;
        }
        RedactStyle::Pixelate => {
            let cell = 12.max(height / 2);
            pixelate(img, region, cell);
            scramble_cells(img, region, cell);
            return;
        }
        RedactStyle::Blur => blur(img, region, 16.0f32.max(height as f32)),
    }
    let mut noise = Noise::new();
    for y in y0..y1 {
        for x in x0..x1 {
            let p = img.get_pixel_mut(x, y);
            let n = noise.next();
            for c in 0..3 {
                let delta = ((n >> (c * 8)) & 0x0F) as i32 - 8;
                p.0[c] = (p.0[c] as i32 + delta).clamp(0, 255) as u8;
            }
            p.0[3] = 255;
        }
    }
}

/// Shift every `cell` x `cell` block of `region` by one random colour offset of up to +-64 per channel and make it opaque.
fn scramble_cells(img: &mut RgbaImage, region: Rect, cell: u32) {
    let (x0, y0) = (region.x as u32, region.y as u32);
    let (x1, y1) = (x0 + region.w, y0 + region.h);
    let mut noise = Noise::new();
    for cy in (y0..y1).step_by(cell as usize) {
        for cx in (x0..x1).step_by(cell as usize) {
            let n = noise.next();
            let delta: [i32; 3] = std::array::from_fn(|c| ((n >> (c * 8)) & 0x7F) as i32 - 64);
            for y in cy..(cy + cell).min(y1) {
                for x in cx..(cx + cell).min(x1) {
                    let p = img.get_pixel_mut(x, y);
                    for (channel, shift) in p.0.iter_mut().zip(delta) {
                        *channel = (*channel as i32 + shift).clamp(0, 255) as u8;
                    }
                    p.0[3] = 255;
                }
            }
        }
    }
}

/// Gaussian blur of the whole image (alpha-correct), `sigma` in pixels.
pub fn gaussian_blur(img: &RgbaImage, sigma: f32) -> RgbaImage {
    if sigma < 0.3 || img.width() == 0 || img.height() == 0 {
        return img.clone();
    }
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mut buf = premultiplied(img);
    blur::gaussian::<4>(&mut buf, w, h, sigma);
    unpremultiplied(buf, img.width(), img.height())
}

/// Spotlight: darkens everything outside the union of `keep` by `amount` (0 = none, 1 = black).
pub fn dim_except(img: &mut RgbaImage, keep: &[Rect], amount: f32) {
    let amount = amount.clamp(0.0, 1.0);
    if amount == 0.0 {
        return;
    }
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mut kept = vec![false; w * h];
    for r in keep {
        if let Some((x0, y0, x1, y1)) = clamp_rect(*r, w as u32, h as u32) {
            for y in y0 as usize..y1 as usize {
                kept[y * w + x0 as usize..y * w + x1 as usize].fill(true);
            }
        }
    }
    let scale = ((1.0 - amount) * 256.0).round() as u32;
    let raw: &mut [u8] = img;
    for (i, px) in raw.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        if !kept[i] {
            for c in &mut px[..3] {
                *c = ((*c as u32 * scale) >> 8) as u8;
            }
        }
    }
}

/// Image on a larger canvas with a soft shadow of `rgba` (its alpha is the shadow strength).
/// The canvas grows just enough that neither the blur nor the offset gets clipped.
pub fn drop_shadow(img: &RgbaImage, radius: u32, offset: (i32, i32), rgba: [u8; 4]) -> RgbaImage {
    let margin = (radius as f32 * 0.75).ceil() as i32 + 1;
    let left = (margin - offset.0).max(0) as u32;
    let top = (margin - offset.1).max(0) as u32;
    let right = (margin + offset.0).max(0) as u32;
    let bottom = (margin + offset.1).max(0) as u32;
    let (w, h) = (img.width() + left + right, img.height() + top + bottom);

    let mut mask = vec![0u8; (w * h) as usize];
    let (sx, sy) = (left as i32 + offset.0, top as i32 + offset.1);
    for y in 0..img.height() {
        for x in 0..img.width() {
            let a = img.get_pixel(x, y).0[3];
            mask[((sy + y as i32) as u32 * w + (sx + x as i32) as u32) as usize] = a;
        }
    }
    blur::gaussian::<1>(&mut mask, w as usize, h as usize, radius as f32 * 0.5);

    let mut out = RgbaImage::new(w, h);
    let strength = rgba[3] as u32;
    let raw: &mut [u8] = &mut out;
    for (px, m) in raw.as_chunks_mut::<4>().0.iter_mut().zip(&mask) {
        *px = [rgba[0], rgba[1], rgba[2], div255(*m as u32 * strength) as u8];
    }
    blit_over(&mut out, img, left as i64, top as i64);
    out
}

/// Rounds the four corners with an anti-aliased alpha mask. Radius is clamped to half the short side.
pub fn round_corners(img: &mut RgbaImage, radius: u32) {
    let (w, h) = (img.width(), img.height());
    let r = radius.min(w / 2).min(h / 2);
    if r == 0 {
        return;
    }
    let rf = r as f32;
    for dy in 0..r {
        for dx in 0..r {
            // Distance from this pixel's center to the corner circle's center.
            let (cx, cy) = (rf - dx as f32 - 0.5, rf - dy as f32 - 0.5);
            let coverage = (rf - cx.hypot(cy) + 0.5).clamp(0.0, 1.0);
            if coverage >= 1.0 {
                continue;
            }
            let k = (coverage * 256.0) as u32;
            for (x, y) in [(dx, dy), (w - 1 - dx, dy), (dx, h - 1 - dy), (w - 1 - dx, h - 1 - dy)] {
                let p = img.get_pixel_mut(x, y);
                p.0[3] = ((p.0[3] as u32 * k) >> 8) as u8;
            }
        }
    }
}

/// Alpha-weighted mean of a block.
fn mean_color(img: &RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32) -> [u8; 4] {
    let (mut r, mut g, mut b, mut a) = (0u64, 0u64, 0u64, 0u64);
    for y in y0..y1 {
        for x in x0..x1 {
            let p = img.get_pixel(x, y).0;
            let pa = p[3] as u64;
            r += p[0] as u64 * pa;
            g += p[1] as u64 * pa;
            b += p[2] as u64 * pa;
            a += pa;
        }
    }
    let n = ((x1 - x0) as u64) * ((y1 - y0) as u64);
    if a == 0 {
        return [0, 0, 0, 0];
    }
    [((r + a / 2) / a) as u8, ((g + a / 2) / a) as u8, ((b + a / 2) / a) as u8, ((a + n / 2) / n) as u8]
}

pub(crate) fn premultiplied(img: &RgbaImage) -> Vec<u8> {
    let mut buf = img.as_raw().clone();
    for px in buf.as_chunks_mut::<4>().0.iter_mut() {
        let a = px[3] as u32;
        if a != 255 {
            for c in &mut px[..3] {
                *c = div255(*c as u32 * a) as u8;
            }
        }
    }
    buf
}

pub(crate) fn unpremultiplied(mut buf: Vec<u8>, w: u32, h: u32) -> RgbaImage {
    for px in buf.as_chunks_mut::<4>().0.iter_mut() {
        let a = px[3] as u32;
        if a != 255 && a != 0 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    RgbaImage::from_raw(w, h, buf).expect("buffer matches dimensions")
}
