//! Animated GIF encoder: per-frame NeuQuant palettes, frame differencing with
//! transparent "unchanged" pixels, optional Floyd-Steinberg dithering.

use std::io::Write;

use color_quant::NeuQuant;
use gif::{DisposalMethod, Encoder, Frame, Repeat};
use snap_core::{Error, Result};

use crate::clock::{HNS_PER_SEC, Hns};

const TRANSPARENT: u8 = 255;
const MIN_DELAY_CS: i64 = 2;

struct Pending {
    frame: Frame<'static>,
    start: Hns,
}

pub(crate) struct GifEncoder<W: Write> {
    enc: Encoder<W>,
    w: usize,
    h: usize,
    /// What a viewer currently sees (RGB); `None` until the first frame.
    canvas: Option<Vec<u8>>,
    pending: Option<Pending>,
    /// Sub-centisecond remainder carried between frame delays so timing does not drift.
    carry_hns: Hns,
    dither: bool,
}

impl<W: Write> GifEncoder<W> {
    pub fn new(writer: W, w: u32, h: u32, dither: bool) -> Result<Self> {
        let (gw, gh) = (u16::try_from(w), u16::try_from(h));
        let (Ok(gw), Ok(gh)) = (gw, gh) else {
            return Err(Error::invalid("GIF dimensions exceed 65535 pixels"));
        };
        let mut enc = Encoder::new(writer, gw, gh, &[]).map_err(gif_err)?;
        enc.set_repeat(Repeat::Infinite).map_err(gif_err)?;
        Ok(Self { enc, w: w as usize, h: h as usize, canvas: None, pending: None, carry_hns: 0, dither })
    }

    /// Add a BGRA frame (`w*h*4`) shown from `pts` until the next pushed frame.
    pub fn push(&mut self, bgra: &[u8], pts: Hns) -> Result<()> {
        debug_assert_eq!(bgra.len(), self.w * self.h * 4);
        let tol = if self.dither { 24 } else { 6 };
        let Some((x0, y0, x1, y1)) = self.changed_bounds(bgra, tol) else {
            return Ok(());
        };
        if let Some(prev) = self.pending.take() {
            self.write(prev, pts)?;
        }
        let frame = self.quantize(bgra, (x0, y0, x1, y1), tol);
        self.pending = Some(Pending { frame, start: pts });
        Ok(())
    }

    /// Flush the last frame so it ends at `end`, and finish the stream.
    pub fn finish(mut self, end: Hns) -> Result<W> {
        if let Some(prev) = self.pending.take() {
            let until = end.max(prev.start + HNS_PER_SEC / 20);
            self.write(prev, until)?;
        }
        self.enc.into_inner().map_err(gif_err)
    }

    fn write(&mut self, mut p: Pending, until: Hns) -> Result<()> {
        let span = (until - p.start).max(0) + self.carry_hns;
        let cs = (span / (HNS_PER_SEC / 100)).max(MIN_DELAY_CS);
        self.carry_hns = (span - cs * (HNS_PER_SEC / 100)).clamp(-(HNS_PER_SEC / 50), HNS_PER_SEC / 50);
        p.frame.delay = cs.min(u16::MAX as i64) as u16;
        self.enc.write_frame(&p.frame).map_err(gif_err)
    }

    /// Bounding box (x0, y0, x1 exclusive, y1 exclusive) of pixels that differ from the canvas.
    fn changed_bounds(&self, bgra: &[u8], tol: i32) -> Option<(usize, usize, usize, usize)> {
        let Some(canvas) = &self.canvas else {
            return Some((0, 0, self.w, self.h));
        };
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
        for y in 0..self.h {
            let row = &bgra[y * self.w * 4..(y + 1) * self.w * 4];
            let crow = &canvas[y * self.w * 3..(y + 1) * self.w * 3];
            let mut first = None;
            let mut last = 0;
            for x in 0..self.w {
                if differs(&row[x * 4..x * 4 + 3], &crow[x * 3..x * 3 + 3], tol) {
                    first.get_or_insert(x);
                    last = x;
                }
            }
            if let Some(f) = first {
                x0 = x0.min(f);
                x1 = x1.max(last + 1);
                y0 = y0.min(y);
                y1 = y1.max(y + 1);
            }
        }
        (x1 > x0).then_some((x0, y0, x1, y1))
    }

    fn quantize(&mut self, bgra: &[u8], (x0, y0, x1, y1): (usize, usize, usize, usize), tol: i32) -> Frame<'static> {
        let (rw, rh) = (x1 - x0, y1 - y0);
        let first = self.canvas.is_none();
        let canvas = self.canvas.get_or_insert_with(|| vec![0; self.w * self.h * 3]);

        let mut changed = vec![false; rw * rh];
        let mut samples: Vec<u8> = Vec::with_capacity(rw * rh * 4);
        for ry in 0..rh {
            for rx in 0..rw {
                let (x, y) = (x0 + rx, y0 + ry);
                let px = &bgra[(y * self.w + x) * 4..(y * self.w + x) * 4 + 3];
                let c = &canvas[(y * self.w + x) * 3..(y * self.w + x) * 3 + 3];
                if first || differs(px, c, tol) {
                    changed[ry * rw + rx] = true;
                    samples.extend_from_slice(&[px[2], px[1], px[0], 255]);
                }
            }
        }

        // Small updates are trained exhaustively; full-screen frames are subsampled for speed.
        let factor = (samples.len() / 4 / 20_000).clamp(1, 30) as i32;
        let nq = NeuQuant::new(factor, 255, &samples);
        let mut palette = nq.color_map_rgb();
        palette.resize(256 * 3, 0);
        let palette_rgb: Vec<[u8; 3]> = palette.as_chunks::<3>().0.to_vec();

        // 15-bit colour -> palette index cache; 0xFFFF marks "not computed yet".
        let mut lut = vec![0xFFFFu16; 1 << 15];
        let mut lookup = |r: u8, g: u8, b: u8| -> u8 {
            let key = ((r as usize >> 3) << 10) | ((g as usize >> 3) << 5) | (b as usize >> 3);
            if lut[key] == 0xFFFF {
                lut[key] = nq.index_of(&[r, g, b, 255]) as u16;
            }
            lut[key] as u8
        };

        let mut indices = vec![TRANSPARENT; rw * rh];
        if self.dither {
            let mut err_cur = vec![[0f32; 3]; rw + 2];
            let mut err_next = vec![[0f32; 3]; rw + 2];
            for ry in 0..rh {
                for rx in 0..rw {
                    if !changed[ry * rw + rx] {
                        continue;
                    }
                    let (x, y) = (x0 + rx, y0 + ry);
                    let px = &bgra[(y * self.w + x) * 4..(y * self.w + x) * 4 + 3];
                    let e = err_cur[rx + 1];
                    let want = [px[2] as f32 + e[0], px[1] as f32 + e[1], px[0] as f32 + e[2]];
                    let clamped = want.map(|v| v.clamp(0.0, 255.0));
                    let idx = lookup(clamped[0] as u8, clamped[1] as u8, clamped[2] as u8);
                    indices[ry * rw + rx] = idx;
                    let got = palette_rgb[idx as usize];
                    let diff = [clamped[0] - got[0] as f32, clamped[1] - got[1] as f32, clamped[2] - got[2] as f32];
                    let spread = |buf: &mut Vec<[f32; 3]>, at: usize, k: f32, active: bool| {
                        if active {
                            for c in 0..3 {
                                buf[at][c] += diff[c] * k;
                            }
                        }
                    };
                    spread(&mut err_cur, rx + 2, 7.0 / 16.0, rx + 1 < rw && changed[ry * rw + rx + 1]);
                    if ry + 1 < rh {
                        spread(&mut err_next, rx, 3.0 / 16.0, rx > 0 && changed[(ry + 1) * rw + rx - 1]);
                        spread(&mut err_next, rx + 1, 5.0 / 16.0, changed[(ry + 1) * rw + rx]);
                        spread(&mut err_next, rx + 2, 1.0 / 16.0, rx + 1 < rw && changed[(ry + 1) * rw + rx + 1]);
                    }
                }
                std::mem::swap(&mut err_cur, &mut err_next);
                err_next.iter_mut().for_each(|e| *e = [0.0; 3]);
            }
        } else {
            for ry in 0..rh {
                for rx in 0..rw {
                    if changed[ry * rw + rx] {
                        let (x, y) = (x0 + rx, y0 + ry);
                        let px = &bgra[(y * self.w + x) * 4..(y * self.w + x) * 4 + 3];
                        indices[ry * rw + rx] = lookup(px[2], px[1], px[0]);
                    }
                }
            }
        }

        // The canvas must mirror exactly what decoders will show.
        for ry in 0..rh {
            for rx in 0..rw {
                if changed[ry * rw + rx] {
                    let o = ((y0 + ry) * self.w + x0 + rx) * 3;
                    canvas[o..o + 3].copy_from_slice(&palette_rgb[indices[ry * rw + rx] as usize]);
                }
            }
        }
        Frame {
            delay: 0,
            dispose: DisposalMethod::Keep,
            transparent: Some(TRANSPARENT),
            left: x0 as u16,
            top: y0 as u16,
            width: rw as u16,
            height: rh as u16,
            palette: Some(palette),
            buffer: indices.into(),
            ..Frame::default()
        }
    }
}

fn differs(bgr: &[u8], canvas_rgb: &[u8], tol: i32) -> bool {
    (bgr[2] as i32 - canvas_rgb[0] as i32).abs() > tol || (bgr[1] as i32 - canvas_rgb[1] as i32).abs() > tol || (bgr[0] as i32 - canvas_rgb[2] as i32).abs() > tol
}

fn gif_err(e: impl std::fmt::Display) -> Error {
    Error::Other(anyhow::anyhow!("gif: {e}"))
}
