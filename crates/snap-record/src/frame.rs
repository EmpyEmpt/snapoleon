//! The raw frame type shared by sources, overlays and sinks.

use image::RgbaImage;

/// A tightly packed BGRA8 frame, opaque.
#[derive(Clone)]
pub(crate) struct Frame {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

impl Frame {
    pub fn black(w: u32, h: u32) -> Self {
        let mut data = vec![0u8; w as usize * h as usize * 4];
        data.iter_mut().skip(3).step_by(4).for_each(|a| *a = 255);
        Self { w, h, data }
    }

    pub fn stride(&self) -> usize {
        self.w as usize * 4
    }

    /// Copy `rows` of `src` (with `src_stride` bytes per row, `width` pixels used) to `(dx, dy)`, clipped.
    #[cfg(any(windows, target_os = "macos"))]
    pub fn blit(&mut self, src: &[u8], src_stride: usize, width: u32, rows: u32, dx: i32, dy: i32) {
        let x0 = dx.max(0) as u32;
        let y0 = dy.max(0) as u32;
        let x1 = ((dx as i64 + width as i64).min(self.w as i64)).max(0) as u32;
        let y1 = ((dy as i64 + rows as i64).min(self.h as i64)).max(0) as u32;
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let bytes = (x1 - x0) as usize * 4;
        for y in y0..y1 {
            let sy = (y as i64 - dy as i64) as usize;
            let sx = (x0 as i64 - dx as i64) as usize;
            let s = sy * src_stride + sx * 4;
            let d = y as usize * self.stride() + x0 as usize * 4;
            self.data[d..d + bytes].copy_from_slice(&src[s..s + bytes]);
        }
    }

    /// Alpha-blend a premultiplied RGBA pixmap over this frame at `(dx, dy)`.
    pub fn blend_premul_rgba(&mut self, src: &[u8], sw: u32, sh: u32, dx: i32, dy: i32, opacity: f32) {
        let op = (opacity.clamp(0.0, 1.0) * 256.0) as u32;
        if op == 0 {
            return;
        }
        for sy in 0..sh as i32 {
            let y = dy + sy;
            if y < 0 || y >= self.h as i32 {
                continue;
            }
            for sx in 0..sw as i32 {
                let x = dx + sx;
                if x < 0 || x >= self.w as i32 {
                    continue;
                }
                let s = (sy as usize * sw as usize + sx as usize) * 4;
                let a = (src[s + 3] as u32 * op) >> 8;
                if a == 0 {
                    continue;
                }
                let d = y as usize * self.stride() + x as usize * 4;
                let inv = 255 - a;
                let px = &mut self.data[d..d + 4];
                // src is premultiplied RGBA; scale its colour by the extra opacity too.
                px[0] = (((src[s + 2] as u32 * op) >> 8) + (px[0] as u32 * inv) / 255).min(255) as u8;
                px[1] = (((src[s + 1] as u32 * op) >> 8) + (px[1] as u32 * inv) / 255).min(255) as u8;
                px[2] = (((src[s] as u32 * op) >> 8) + (px[2] as u32 * inv) / 255).min(255) as u8;
            }
        }
    }

    pub fn to_rgba_image(&self) -> RgbaImage {
        let mut data = self.data.clone();
        bgra_to_rgba(&mut data);
        RgbaImage::from_raw(self.w, self.h, data).expect("frame buffer matches its size")
    }

    pub fn from_rgba_image(img: &RgbaImage) -> Self {
        let mut data = img.as_raw().clone();
        bgra_to_rgba(&mut data); // the swap is symmetric
        data.iter_mut().skip(3).step_by(4).for_each(|a| *a = 255);
        Self { w: img.width(), h: img.height(), data }
    }
}

/// Swap B and R in place (BGRA <-> RGBA).
pub(crate) fn bgra_to_rgba(data: &mut [u8]) {
    for px in data.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
}

/// Round down to an even number, at least 2 (H.264 4:2:0 needs even dimensions).
pub(crate) fn even(v: u32) -> u32 {
    (v & !1).max(2)
}

/// Frame dimensions after scaling to at most `max_w` wide (aspect preserved, even sizes).
pub(crate) fn fit_width(w: u32, h: u32, max_w: u32) -> (u32, u32) {
    if max_w == 0 || w <= max_w {
        return (even(w), even(h));
    }
    let nh = (h as u64 * max_w as u64 / w as u64) as u32;
    (even(max_w), even(nh))
}

/// Area-averaging (box) downscale of a BGRA/RGBA buffer; also upscales by nearest neighbour.
pub(crate) fn scale_box(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    if sw == dw && sh == dh {
        return src.to_vec();
    }
    let mut out = vec![0u8; dw as usize * dh as usize * 4];
    let (swu, shu) = (sw as usize, sh as usize);
    for dy in 0..dh as usize {
        let y0 = dy * shu / dh as usize;
        let y1 = (((dy + 1) * shu).div_ceil(dh as usize)).clamp(y0 + 1, shu);
        for dx in 0..dw as usize {
            let x0 = dx * swu / dw as usize;
            let x1 = (((dx + 1) * swu).div_ceil(dw as usize)).clamp(x0 + 1, swu);
            let mut acc = [0u32; 4];
            for y in y0..y1 {
                let row = &src[(y * swu + x0) * 4..(y * swu + x1) * 4];
                for px in row.as_chunks::<4>().0 {
                    acc[0] += px[0] as u32;
                    acc[1] += px[1] as u32;
                    acc[2] += px[2] as u32;
                    acc[3] += px[3] as u32;
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            let o = (dy * dw as usize + dx) * 4;
            out[o] = (acc[0] / n) as u8;
            out[o + 1] = (acc[1] / n) as u8;
            out[o + 2] = (acc[2] / n) as u8;
            out[o + 3] = (acc[3] / n) as u8;
        }
    }
    out
}
