//! Small pixel helpers shared by every backend.

use snap_core::{Point, Rect, RgbaImage};

/// Copy `src_rect` of `src` into `dst` at `at`, clipping to both images. Raw copy, no blending.
pub(crate) fn copy(src: &RgbaImage, src_rect: Rect, dst: &mut RgbaImage, at: Point) {
    let src_bounds = Rect::new(0, 0, src.width(), src.height());
    let Some(sr) = src_rect.intersect(src_bounds) else { return };
    // Re-derive the destination origin after clipping the source rect.
    let at = Point::new(at.x + (sr.x - src_rect.x), at.y + (sr.y - src_rect.y));
    let dst_bounds = Rect::new(0, 0, dst.width(), dst.height());
    let Some(dr) = Rect::new(at.x, at.y, sr.w, sr.h).intersect(dst_bounds) else { return };
    let sr = Rect::new(sr.x + (dr.x - at.x), sr.y + (dr.y - at.y), dr.w, dr.h);

    let (sw, dw) = (src.width() as usize, dst.width() as usize);
    let row_bytes = dr.w as usize * 4;
    let s = src.as_raw();
    let d: &mut [u8] = dst;
    for row in 0..dr.h as usize {
        let so = ((sr.y as usize + row) * sw + sr.x as usize) * 4;
        let dof = ((dr.y as usize + row) * dw + dr.x as usize) * 4;
        d[dof..dof + row_bytes].copy_from_slice(&s[so..so + row_bytes]);
    }
}

/// Source-over blend of straight-alpha `sprite` onto `dst` with its top-left at `at` (may be partly outside).
pub(crate) fn blend(dst: &mut RgbaImage, sprite: &RgbaImage, at: Point) {
    for (sx, sy, px) in sprite.enumerate_pixels() {
        let (x, y) = (at.x + sx as i32, at.y + sy as i32);
        if x < 0 || y < 0 || x >= dst.width() as i32 || y >= dst.height() as i32 || px[3] == 0 {
            continue;
        }
        let d = dst.get_pixel_mut(x as u32, y as u32);
        let a = px[3] as u32;
        if a == 255 || d[3] == 0 {
            *d = *px;
            continue;
        }
        let da = d[3] as u32;
        let out_a = a + da * (255 - a) / 255;
        for c in 0..3 {
            let num = px[c] as u32 * a * 255 + d[c] as u32 * da * (255 - a);
            d[c] = (num / (out_a * 255)).min(255) as u8;
        }
        d[3] = out_a as u8;
    }
}

/// Grow `img` by a soft drop shadow derived from its alpha channel. The shadow falls slightly downwards
/// like the OS window shadow; the image keeps its position `margin` pixels inside the new canvas.
pub(crate) fn with_shadow(img: &RgbaImage, margin: u32, offset_y: i32, opacity: f32) -> RgbaImage {
    let (w, h) = (img.width() + 2 * margin, img.height() + 2 * margin);
    let (wu, hu) = (w as usize, h as usize);
    let mut alpha = vec![0f32; wu * hu];
    for (x, y, px) in img.enumerate_pixels() {
        let ty = y as i32 + margin as i32 + offset_y;
        if (0..h as i32).contains(&ty) {
            alpha[ty as usize * wu + (x + margin) as usize] = px[3] as f32 / 255.0;
        }
    }
    let radius = (margin as usize / 3).max(1);
    let mut tmp = vec![0f32; alpha.len()];
    for _ in 0..3 {
        box_blur_h(&alpha, &mut tmp, wu, hu, radius);
        box_blur_v(&tmp, &mut alpha, wu, hu, radius);
    }

    let mut out = RgbaImage::new(w, h);
    for (i, px) in out.pixels_mut().enumerate() {
        let a = (alpha[i] * opacity * 255.0).round() as u8;
        *px = image::Rgba([0, 0, 0, a]);
    }
    blend(&mut out, img, Point::new(margin as i32, margin as i32));
    out
}

fn box_blur_h(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize) {
    let norm = 1.0 / (2 * r + 1) as f32;
    for y in 0..h {
        let row = &src[y * w..(y + 1) * w];
        let out = &mut dst[y * w..(y + 1) * w];
        let mut sum: f32 = row[..=r.min(w - 1)].iter().sum();
        for x in 0..w {
            out[x] = sum * norm;
            if x + r + 1 < w {
                sum += row[x + r + 1];
            }
            if x >= r {
                sum -= row[x - r];
            }
        }
    }
}

fn box_blur_v(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize) {
    let norm = 1.0 / (2 * r + 1) as f32;
    for x in 0..w {
        let mut sum: f32 = (0..=r.min(h - 1)).map(|y| src[y * w + x]).sum();
        for y in 0..h {
            dst[y * w + x] = sum * norm;
            if y + r + 1 < h {
                sum += src[(y + r + 1) * w + x];
            }
            if y >= r {
                sum -= src[(y - r) * w + x];
            }
        }
    }
}
