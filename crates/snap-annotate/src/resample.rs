//! Fast base-image blit for the live view: nearest or bilinear sampling of a premultiplied
//! pixmap straight into the view buffer, in fixed point, spread over a few threads.

use tiny_skia::Pixmap;

use crate::cache::Pyramid;

/// Linear interpolation of two packed premultiplied pixels, `w` in `0..=255` towards `b`.
#[inline]
fn lerp(a: u32, b: u32, w: u32) -> u32 {
    let iw = 256 - w;
    let rb = (((a & 0x00FF_00FF) * iw + (b & 0x00FF_00FF) * w) >> 8) & 0x00FF_00FF;
    let ag = (((a >> 8) & 0x00FF_00FF) * iw + ((b >> 8) & 0x00FF_00FF) * w) & 0xFF00_FF00;
    rb | ag
}

#[inline]
fn load(px: &[[u8; 4]], i: usize) -> u32 {
    u32::from_le_bytes(px[i])
}

struct Column {
    i0: u32,
    i1: u32,
    w: u32,
}

/// Draws the part of the image behind `pyr` that maps into `dst`. Document pixel (0, 0) lands at
/// view position (`tx`, `ty`) and one document pixel covers `zoom` view pixels (`doc_w` x `doc_h`
/// is the size of the full-resolution image). Pixels outside the image stay untouched.
pub(crate) fn blit(dst: &mut Pixmap, pyr: &Pyramid, zoom: f32, doc_w: u32, doc_h: u32, tx: f32, ty: f32) {
    // Pick the mip level whose pixels are between 1 and 2 view pixels wide (or level 0 above 1:1).
    let level = if zoom >= 1.0 { 0 } else { ((1.0 / zoom).log2() + 1e-3).floor().max(0.0) as usize };
    let lvl = pyr.level(level);
    let (lw, lh) = (lvl.width() as usize, lvl.height() as usize);
    let kx = zoom * doc_w as f32 / lw as f32;
    let ky = zoom * doc_h as f32 / lh as f32;
    let nearest = kx >= 4.0;

    let (dw, dh) = (dst.width() as usize, dst.height() as usize);
    let x_lo = ((tx - 0.5).ceil().max(0.0)) as usize;
    let x_hi = (((tx + lw as f32 * kx - 0.5).ceil()).max(0.0) as usize).min(dw);
    let y_lo = ((ty - 0.5).ceil().max(0.0)) as usize;
    let y_hi = (((ty + lh as f32 * ky - 0.5).ceil()).max(0.0) as usize).min(dh);
    if x_lo >= x_hi || y_lo >= y_hi {
        return;
    }

    let cols: Vec<Column> = (x_lo..x_hi)
        .map(|x| {
            let u = (x as f32 + 0.5 - tx) / kx;
            if nearest {
                let i = (u.floor().max(0.0) as usize).min(lw - 1) as u32;
                Column { i0: i, i1: i, w: 0 }
            } else {
                let u = u - 0.5;
                let f = u.floor();
                Column {
                    i0: (f.max(0.0) as usize).min(lw - 1) as u32,
                    i1: ((f + 1.0).max(0.0) as usize).min(lw - 1) as u32,
                    w: (((u - f) * 256.0) as u32).min(255),
                }
            }
        })
        .collect();
    let span_lo = cols.iter().map(|c| c.i0).min().unwrap_or(0) as usize;
    let span_hi = cols.iter().map(|c| c.i1).max().unwrap_or(0) as usize + 1;

    let src = lvl.data().as_chunks::<4>().0;
    let rows = y_hi - y_lo;
    let threads = if rows * (x_hi - x_lo) > 300_000 { std::thread::available_parallelism().map_or(1, |n| n.get()).min(4) } else { 1 };
    let band = rows.div_ceil(threads);
    let data = &mut dst.data_mut()[y_lo * dw * 4..y_hi * dw * 4];

    let work = |first_row: usize, chunk: &mut [u8]| {
        let mut tmp = vec![0u32; span_hi - span_lo];
        for (ri, row) in chunk.chunks_exact_mut(dw * 4).enumerate() {
            let y = y_lo + first_row + ri;
            let out = &mut row[x_lo * 4..x_hi * 4];
            let v = (y as f32 + 0.5 - ty) / ky;
            let (j0, j1, wy) = if nearest {
                let j = (v.floor().max(0.0) as usize).min(lh - 1);
                (j, j, 0)
            } else {
                let v = v - 0.5;
                let f = v.floor();
                ((f.max(0.0) as usize).min(lh - 1), ((f + 1.0).max(0.0) as usize).min(lh - 1), (((v - f) * 256.0) as u32).min(255))
            };
            let r0 = &src[j0 * lw..(j0 + 1) * lw];
            if wy == 0 || j0 == j1 {
                for (t, i) in tmp.iter_mut().zip(span_lo..span_hi) {
                    *t = load(r0, i);
                }
            } else {
                let r1 = &src[j1 * lw..(j1 + 1) * lw];
                for (t, i) in tmp.iter_mut().zip(span_lo..span_hi) {
                    *t = lerp(load(r0, i), load(r1, i), wy);
                }
            }
            for (px, c) in out.as_chunks_mut::<4>().0.iter_mut().zip(&cols) {
                let a = tmp[c.i0 as usize - span_lo];
                let p = if c.w == 0 { a } else { lerp(a, tmp[c.i1 as usize - span_lo], c.w) };
                *px = p.to_le_bytes();
            }
        }
    };

    if threads == 1 {
        work(0, data);
    } else {
        std::thread::scope(|s| {
            for (n, chunk) in data.chunks_mut(band * dw * 4).enumerate() {
                let work = &work;
                s.spawn(move || work(n * band, chunk));
            }
        });
    }
}
