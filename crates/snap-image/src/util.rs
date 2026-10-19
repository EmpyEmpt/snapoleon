//! Small helpers shared by the modules: pixel plumbing, rect clamping, threading.

use image::RgbaImage;
use snap_core::{Error, Rect, Result, t};
use tiny_skia::Pixmap;

/// Rect clamped to an image: `(x0, y0, x1, y1)` with `x1 > x0 && y1 > y0`.
pub(crate) fn clamp_rect(r: Rect, w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
    let c = r.intersect(Rect::new(0, 0, w, h))?;
    Some((c.x as u32, c.y as u32, c.right() as u32, c.bottom() as u32))
}

pub(crate) fn ensure_non_empty(img: &RgbaImage) -> Result<()> {
    if img.width() == 0 || img.height() == 0 {
        return Err(Error::invalid(t("image.error.empty")));
    }
    Ok(())
}

pub(crate) fn is_opaque(img: &RgbaImage) -> bool {
    img.as_raw().as_chunks::<4>().0.iter().all(|p| p[3] == 255)
}

/// Runs `f(first_row, rows)` over disjoint bands of `data` on scoped threads.
/// Small buffers stay on the calling thread.
pub(crate) fn par_rows(data: &mut [u8], row_len: usize, f: impl Fn(usize, &mut [u8]) + Sync) {
    const SINGLE_THREAD_BYTES: usize = 1 << 19;
    let rows = data.len().checked_div(row_len).unwrap_or(0);
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(8).min(rows);
    if threads <= 1 || data.len() < SINGLE_THREAD_BYTES {
        f(0, data);
        return;
    }
    let band = rows.div_ceil(threads);
    std::thread::scope(|s| {
        for (i, chunk) in data.chunks_mut(band * row_len).enumerate() {
            let f = &f;
            s.spawn(move || f(i * band, chunk));
        }
    });
}

#[inline]
pub(crate) fn div255(v: u32) -> u32 {
    (v + 128 + ((v + 128) >> 8)) >> 8
}

/// Straight-alpha source-over of one pixel.
#[inline]
pub(crate) fn over(dst: &mut [u8; 4], src: [u8; 4]) {
    let sa = src[3] as u32;
    if sa == 0 {
        return;
    }
    let da = dst[3] as u32;
    if sa == 255 || da == 0 {
        *dst = src;
        return;
    }
    if da == 255 {
        for c in 0..3 {
            let (s, d) = (src[c] as u32, dst[c] as u32);
            dst[c] = div255(s * sa + d * (255 - sa)) as u8;
        }
        return;
    }
    let out_a = sa * 255 + da * (255 - sa);
    for c in 0..3 {
        let num = src[c] as u32 * sa * 255 + dst[c] as u32 * da * (255 - sa);
        dst[c] = ((num + out_a / 2) / out_a) as u8;
    }
    dst[3] = ((out_a + 127) / 255) as u8;
}

/// Alpha-composites `src` onto `dst` with `src`'s top-left at `(x, y)`; clips to `dst`.
pub(crate) fn blit_over(dst: &mut RgbaImage, src: &RgbaImage, x: i64, y: i64) {
    let (dw, dh) = (dst.width() as i64, dst.height() as i64);
    let (sw, sh) = (src.width() as i64, src.height() as i64);
    let (x0, y0) = (x.max(0), y.max(0));
    let (x1, y1) = ((x + sw).min(dw), (y + sh).min(dh));
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let n = (x1 - x0) as usize;
    let dst_stride = dw as usize * 4;
    let src_stride = sw as usize * 4;
    let dst_raw: &mut [u8] = dst;
    let src_raw: &[u8] = src;
    for row in y0..y1 {
        let d = row as usize * dst_stride + x0 as usize * 4;
        let s = (row - y) as usize * src_stride + (x0 - x) as usize * 4;
        for (dp, sp) in dst_raw[d..d + n * 4].as_chunks_mut::<4>().0.iter_mut().zip(src_raw[s..s + n * 4].as_chunks::<4>().0) {
            over(dp, *sp);
        }
    }
}

/// Copies `src` into `dst` at `(x, y)` replacing pixels (no blending); clips to `dst`.
pub(crate) fn blit_copy(dst: &mut RgbaImage, src: &RgbaImage, x: i64, y: i64) {
    let (dw, dh) = (dst.width() as i64, dst.height() as i64);
    let (sw, sh) = (src.width() as i64, src.height() as i64);
    let (x0, y0) = (x.max(0), y.max(0));
    let (x1, y1) = ((x + sw).min(dw), (y + sh).min(dh));
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let n = (x1 - x0) as usize * 4;
    let dst_raw: &mut [u8] = dst;
    let src_raw: &[u8] = src;
    for row in y0..y1 {
        let d = (row as usize * dw as usize + x0 as usize) * 4;
        let s = ((row - y) as usize * sw as usize + (x0 - x) as usize) * 4;
        dst_raw[d..d + n].copy_from_slice(&src_raw[s..s + n]);
    }
}

pub(crate) fn crop(img: &RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32) -> RgbaImage {
    image::imageops::crop_imm(img, x0, y0, x1 - x0, y1 - y0).to_image()
}

/// Straight-alpha image to tiny-skia's premultiplied pixmap.
pub(crate) fn to_pixmap(img: &RgbaImage) -> Pixmap {
    let mut pm = Pixmap::new(img.width().max(1), img.height().max(1)).expect("non-zero pixmap size");
    for (d, s) in pm.data_mut().as_chunks_mut::<4>().0.iter_mut().zip(img.as_raw().as_chunks::<4>().0) {
        let a = s[3] as u32;
        d[0] = div255(s[0] as u32 * a) as u8;
        d[1] = div255(s[1] as u32 * a) as u8;
        d[2] = div255(s[2] as u32 * a) as u8;
        d[3] = s[3];
    }
    pm
}

pub(crate) fn from_pixmap(pm: Pixmap) -> RgbaImage {
    let (w, h) = (pm.width(), pm.height());
    RgbaImage::from_raw(w, h, pm.take_demultiplied()).expect("pixmap buffer matches its size")
}

pub(crate) fn new_pixmap(w: u32, h: u32) -> Pixmap {
    Pixmap::new(w.max(1), h.max(1)).expect("non-zero pixmap size")
}

/// Tiny xorshift generator seeded from the OS-backed `rand`; plenty for dither/noise.
pub(crate) struct Noise(u64);

impl Noise {
    pub(crate) fn new() -> Self {
        Self(rand::random::<u64>() | 1)
    }
    #[inline]
    pub(crate) fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}
