//! Pixel helpers: soft shadows from a layer's alpha, and redaction patches for the live view.

use image::RgbaImage;
use snap_core::settings::RedactStyle;
use snap_core::{Rect, RectF};
use snap_image::effects;
use tiny_skia::Pixmap;

/// Box radii whose three successive passes approximate a Gaussian of `sigma`.
fn box_radii(sigma: f32) -> [usize; 3] {
    let ideal = (12.0 * sigma * sigma / 3.0 + 1.0).sqrt();
    let mut lower = ideal.floor() as i32;
    if lower % 2 == 0 {
        lower -= 1;
    }
    let lower = lower.max(1);
    let m = ((12.0 * sigma * sigma - 3.0 * (lower * lower) as f32 - 12.0 * lower as f32 - 9.0) / (-4.0 * lower as f32 - 4.0)).round() as i32;
    let size = |i: i32| if i < m { lower } else { lower + 2 };
    [0, 1, 2].map(|i| ((size(i) - 1) / 2) as usize)
}

fn box_h(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize) {
    let div = (2 * r + 1) as u32;
    for y in 0..h {
        let row = &src[y * w..(y + 1) * w];
        let out = &mut dst[y * w..(y + 1) * w];
        let mut acc: u32 = row[..r.min(w - 1) + 1].iter().map(|&v| v as u32).sum();
        for x in 0..w {
            out[x] = ((acc + div / 2) / div) as u8;
            if x + r + 1 < w {
                acc += row[x + r + 1] as u32;
            }
            if x >= r {
                acc -= row[x - r] as u32;
            }
        }
    }
}

fn box_v(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize) {
    let div = (2 * r + 1) as u32;
    let mut acc = vec![0u32; w];
    for y in 0..=r.min(h - 1) {
        for (a, v) in acc.iter_mut().zip(&src[y * w..(y + 1) * w]) {
            *a += *v as u32;
        }
    }
    for y in 0..h {
        for (o, a) in dst[y * w..(y + 1) * w].iter_mut().zip(&acc) {
            *o = ((*a + div / 2) / div) as u8;
        }
        if y + r + 1 < h {
            for (a, v) in acc.iter_mut().zip(&src[(y + r + 1) * w..(y + r + 2) * w]) {
                *a += *v as u32;
            }
        }
        if y >= r {
            for (a, v) in acc.iter_mut().zip(&src[(y - r) * w..(y - r + 1) * w]) {
                *a -= *v as u32;
            }
        }
    }
}

/// Gaussian-like blur of a single-channel plane (zero outside).
pub(crate) fn blur_plane(plane: &mut [u8], w: usize, h: usize, sigma: f32) {
    if w == 0 || h == 0 || sigma < 0.4 {
        return;
    }
    let mut tmp = vec![0u8; plane.len()];
    for r in box_radii(sigma) {
        if r == 0 {
            continue;
        }
        box_h(plane, &mut tmp, w, h, r);
        box_v(&tmp, plane, w, h, r);
    }
}

/// Composites `alpha` (a layer-sized plane) as a black shadow onto `dst` at (`x`, `y`).
pub(crate) fn composite_shadow(dst: &mut Pixmap, alpha: &[u8], w: usize, h: usize, x: i32, y: i32, strength: f32) {
    let (dw, dh) = (dst.width() as i32, dst.height() as i32);
    let k = (strength.clamp(0.0, 1.0) * 256.0) as u32;
    let data = dst.data_mut();
    for sy in 0..h as i32 {
        let ty = y + sy;
        if ty < 0 || ty >= dh {
            continue;
        }
        for sx in 0..w as i32 {
            let tx = x + sx;
            if tx < 0 || tx >= dw {
                continue;
            }
            let a = alpha[sy as usize * w + sx as usize] as u32;
            if a == 0 {
                continue;
            }
            let sa = (a * k) >> 8;
            let i = (ty as usize * dw as usize + tx as usize) * 4;
            let inv = 255 - sa;
            for c in 0..3 {
                data[i + c] = ((data[i + c] as u32 * inv + 127) / 255) as u8;
            }
            data[i + 3] = (sa + (data[i + 3] as u32 * inv + 127) / 255).min(255) as u8;
        }
    }
}

/// `rect` rounded to whole pixels and clipped to a `w` x `h` image.
pub(crate) fn pixel_rect(rect: RectF, w: u32, h: u32) -> Option<Rect> {
    let x0 = rect.x.min(rect.right()).round().max(0.0);
    let y0 = rect.y.min(rect.bottom()).round().max(0.0);
    let x1 = rect.x.max(rect.right()).round().min(w as f32);
    let y1 = rect.y.max(rect.bottom()).round().min(h as f32);
    (x1 > x0 && y1 > y0).then(|| Rect::new(x0 as i32, y0 as i32, (x1 - x0) as u32, (y1 - y0) as u32))
}

/// The redacted pixels of `rect` for on-screen display: same parameters as
/// `snap_image::effects::redact`, without its random noise so the preview does not shimmer.
pub(crate) fn preview_patch(src: &RgbaImage, rect: RectF, style: RedactStyle) -> Option<(RgbaImage, Rect)> {
    let r = pixel_rect(rect, src.width(), src.height())?;
    let mut patch = image::imageops::crop_imm(src, r.x as u32, r.y as u32, r.w, r.h).to_image();
    let whole = Rect::new(0, 0, r.w, r.h);
    match style {
        RedactStyle::Solid => effects::fill(&mut patch, whole, [0, 0, 0, 255]),
        RedactStyle::Pixelate => effects::pixelate(&mut patch, whole, 12.max(r.h / 2)),
        RedactStyle::Blur => effects::blur(&mut patch, whole, 16.0f32.max(r.h as f32)),
    }
    for p in patch.pixels_mut() {
        p.0[3] = 255;
    }
    Some((patch, r))
}
