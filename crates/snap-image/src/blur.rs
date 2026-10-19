//! Separable Gaussian blur over interleaved `C`-channel u8 buffers
//! (`C = 4` premultiplied RGBA, `C = 1` masks).
//!
//! Small sigmas use an exact kernel; larger ones use three box passes, whose
//! result is within about 3% of a true Gaussian and costs O(n) regardless of radius.

use crate::util::par_rows;

const EXACT_KERNEL_MAX_SIGMA: f32 = 2.5;

pub(crate) fn gaussian<const C: usize>(buf: &mut [u8], w: usize, h: usize, sigma: f32) {
    if sigma < 0.3 || w == 0 || h == 0 {
        return;
    }
    if sigma <= EXACT_KERNEL_MAX_SIGMA {
        exact::<C>(buf, w, h, sigma);
    } else {
        for r in box_radii(sigma) {
            if r > 0 {
                box_h::<C>(buf, w, r);
                box_v::<C>(buf, w, h, r);
            }
        }
    }
}

fn exact<const C: usize>(buf: &mut [u8], w: usize, h: usize, sigma: f32) {
    let radius = (sigma * 3.0).ceil() as usize;
    let mut k: Vec<f32> = (0..=2 * radius)
        .map(|i| {
            let d = i as f32 - radius as f32;
            (-(d * d) / (2.0 * sigma * sigma)).exp()
        })
        .collect();
    let sum: f32 = k.iter().sum();
    let kernel: Vec<u32> = k.iter_mut().map(|v| (*v / sum * 65536.0).round() as u32).collect();
    let row_len = w * C;

    par_rows(buf, row_len, |_, rows| {
        let mut tmp = vec![0u8; row_len];
        for row in rows.chunks_exact_mut(row_len) {
            for x in 0..w {
                for c in 0..C {
                    let mut acc = 32768u32;
                    for (i, kv) in kernel.iter().enumerate() {
                        let sx = (x + i).saturating_sub(radius).min(w - 1);
                        acc += kv * row[sx * C + c] as u32;
                    }
                    tmp[x * C + c] = (acc >> 16).min(255) as u8;
                }
            }
            row.copy_from_slice(&tmp);
        }
    });

    let src = buf.to_vec();
    par_rows(buf, row_len, |first, rows| {
        for (ri, row) in rows.chunks_exact_mut(row_len).enumerate() {
            let y = first + ri;
            let mut acc = vec![32768u32; row_len];
            for (i, kv) in kernel.iter().enumerate() {
                let sy = (y + i).saturating_sub(radius).min(h - 1);
                let s = &src[sy * row_len..(sy + 1) * row_len];
                for (a, v) in acc.iter_mut().zip(s) {
                    *a += kv * *v as u32;
                }
            }
            for (d, a) in row.iter_mut().zip(acc) {
                *d = (a >> 16).min(255) as u8;
            }
        }
    });
}

/// Box radii whose successive application approximates a Gaussian of `sigma`.
fn box_radii(sigma: f32) -> [usize; 3] {
    const N: f32 = 3.0;
    let ideal = (12.0 * sigma * sigma / N + 1.0).sqrt();
    let mut wl = ideal.floor() as i32;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wu = wl + 2;
    let wlf = wl as f32;
    let m = ((12.0 * sigma * sigma - N * wlf * wlf - 4.0 * N * wlf - 3.0 * N) / (-4.0 * wlf - 4.0)).round() as i32;
    let size = |i: i32| if i < m { wl } else { wu };
    [0, 1, 2].map(|i| ((size(i) - 1) / 2).max(0) as usize)
}

fn box_h<const C: usize>(buf: &mut [u8], w: usize, r: usize) {
    let row_len = w * C;
    let size = (2 * r + 1) as u32;
    par_rows(buf, row_len, |_, rows| {
        let mut out = vec![0u8; row_len];
        for row in rows.chunks_exact_mut(row_len) {
            for c in 0..C {
                let at = |x: usize| row[x.min(w - 1) * C + c] as u32;
                let mut sum = r as u32 * at(0);
                for i in 0..=r {
                    sum += at(i);
                }
                for x in 0..w {
                    out[x * C + c] = ((sum + size / 2) / size) as u8;
                    sum += at(x + r + 1);
                    sum -= at(x.saturating_sub(r));
                }
            }
            row.copy_from_slice(&out);
        }
    });
}

fn box_v<const C: usize>(buf: &mut [u8], w: usize, h: usize, r: usize) {
    let row_len = w * C;
    let size = (2 * r + 1) as u32;
    let src = buf.to_vec();
    let row_at = |y: isize| &src[y.clamp(0, h as isize - 1) as usize * row_len..][..row_len];
    let mut sums = vec![0u32; row_len];
    for dy in -(r as isize)..=r as isize {
        for (s, v) in sums.iter_mut().zip(row_at(dy)) {
            *s += *v as u32;
        }
    }
    for y in 0..h {
        let out = &mut buf[y * row_len..(y + 1) * row_len];
        for (o, s) in out.iter_mut().zip(&sums) {
            *o = ((s + size / 2) / size) as u8;
        }
        let add = row_at(y as isize + r as isize + 1);
        let sub = row_at(y as isize - r as isize);
        for ((s, a), b) in sums.iter_mut().zip(add).zip(sub) {
            *s = *s + *a as u32 - *b as u32;
        }
    }
}
