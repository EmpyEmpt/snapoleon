//! Resampling, including the "readable" downscale that keeps UI text legible.

use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use image::RgbaImage;

use crate::effects::gaussian_blur;
use crate::util::{is_opaque, par_rows};

/// Below this scale the readable pre-filter kicks in; at or above it plain Lanczos is already crisp.
const READABLE_BELOW: f32 = 0.8;
/// Largest share of the morphologically filtered image mixed into the original.
const MORPH_BLEND: f32 = 0.6;
const UNSHARP_AMOUNT: f32 = 0.35;
const UNSHARP_SIGMA: f32 = 0.8;
const TILE: usize = 16;

/// Lanczos3, alpha-correct.
pub fn resize(img: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    let (w, h) = (w.max(1), h.max(1));
    if (img.width(), img.height()) == (w, h) {
        return img.clone();
    }
    if img.width() == 0 || img.height() == 0 {
        return RgbaImage::new(w, h);
    }
    resample(img.as_raw(), img.width(), img.height(), w, h, FilterType::Lanczos3, !is_opaque(img))
}

/// Resample tightly packed RGBA rows without alpha handling (opaque content, previews).
pub(crate) fn resize_raw(raw: &[u8], sw: u32, sh: u32, w: u32, h: u32, filter: FilterType) -> RgbaImage {
    resample(raw, sw, sh, w.max(1), h.max(1), filter, false)
}

fn resample(raw: &[u8], sw: u32, sh: u32, w: u32, h: u32, filter: FilterType, alpha: bool) -> RgbaImage {
    let src = ImageRef::new(sw, sh, raw, PixelType::U8x4).expect("RGBA8 buffer matches size");
    let mut dst = Image::new(w, h, PixelType::U8x4);
    let opts = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(filter)).use_alpha(alpha);
    Resizer::new().resize(&src, &mut dst, &opts).expect("same pixel type, valid sizes");
    RgbaImage::from_raw(w, h, dst.into_vec()).expect("buffer matches dimensions")
}

/// Downscale that keeps thin text and lines legible.
///
/// For scales below 0.8 the strokes are thickened before resampling: per 16 px tile, the
/// minority-contrast side relative to the tile's median luminance is spread with a min filter
/// (dark text on a light background) or a max filter (light on dark). The ideal radius is
/// `(1/s - 1) * 0.5` px; the window uses its rounded value (1..=3) and the blend over the
/// original is 60% scaled by ideal/window, so the effect fades in smoothly towards s = 0.8.
/// Then Lanczos3, then a mild unsharp mask. Flat areas and photos are left alone.
pub fn readable_downscale(img: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    let (w, h) = (w.max(1), h.max(1));
    if img.width() == 0 || img.height() == 0 {
        return RgbaImage::new(w, h);
    }
    let scale = (w as f32 / img.width() as f32).min(h as f32 / img.height() as f32);
    if scale >= READABLE_BELOW {
        return resize(img, w, h);
    }
    // The ideal thickening is (1/s - 1)/2 px per side. The filter works in whole pixels, so
    // fractional radii are honoured by scaling the blend instead of rounding the window.
    let ideal = ((1.0 / scale) - 1.0) * 0.5;
    let radius = ideal.round().clamp(1.0, 3.0) as usize;
    let amount = MORPH_BLEND * (ideal / radius as f32).min(1.0);
    unsharp(resize(&thicken_strokes(img, radius, amount), w, h))
}

/// Fit inside `max_w` x `max_h` keeping aspect; never upscales.
pub fn fit(img: &RgbaImage, max_w: Option<u32>, max_h: Option<u32>, readable: bool) -> RgbaImage {
    let (iw, ih) = (img.width(), img.height());
    let sw = max_w.map_or(f32::INFINITY, |m| m.max(1) as f32 / iw.max(1) as f32);
    let sh = max_h.map_or(f32::INFINITY, |m| m.max(1) as f32 / ih.max(1) as f32);
    if sw >= 1.0 && sh >= 1.0 {
        return img.clone();
    }
    let (tw, th) = if sw <= sh {
        (max_w.unwrap_or(iw).max(1), ((ih as f32 * sw).round() as u32).max(1))
    } else {
        (((iw as f32 * sh).round() as u32).max(1), max_h.unwrap_or(ih).max(1))
    };
    if readable { readable_downscale(img, tw, th) } else { resize(img, tw, th) }
}

/// Thumbnail with the longest side at most `max_side` (readable downscale; never upscales).
pub fn thumbnail(img: &RgbaImage, max_side: u32) -> RgbaImage {
    fit(img, Some(max_side), Some(max_side), true)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Keep,
    /// Min filter: dark strokes on a lighter background get thicker.
    Erode,
    /// Max filter: light strokes on a darker background get thicker.
    Dilate,
}

fn luminance(p: &[u8]) -> u8 {
    ((54 * p[0] as u32 + 183 * p[1] as u32 + 19 * p[2] as u32) >> 8) as u8
}

/// Decides per tile whether to erode, dilate or leave alone.
fn tile_modes(lum: &[u8], w: usize, h: usize) -> (Vec<Mode>, usize) {
    const STROKE_MARGIN: i32 = 24;
    const BACKGROUND_BAND: i32 = 10;
    const MIN_BACKGROUND_SHARE: u32 = 30;
    let tw = w.div_ceil(TILE);
    let th = h.div_ceil(TILE);
    let mut modes = vec![Mode::Keep; tw * th];
    for ty in 0..th {
        for tx in 0..tw {
            let mut hist = [0u32; 256];
            let (x1, y1) = (((tx + 1) * TILE).min(w), ((ty + 1) * TILE).min(h));
            for y in ty * TILE..y1 {
                for &l in &lum[y * w + tx * TILE..y * w + x1] {
                    hist[l as usize] += 1;
                }
            }
            let total: u32 = hist.iter().sum();
            let (mut acc, mut median) = (0u32, 0i32);
            for (v, n) in hist.iter().enumerate() {
                acc += n;
                if acc * 2 >= total {
                    median = v as i32;
                    break;
                }
            }
            let count = |lo: i32, hi: i32| (lo.max(0)..=hi.min(255)).map(|v| hist[v as usize]).sum::<u32>();
            let background = count(median - BACKGROUND_BAND, median + BACKGROUND_BAND);
            if background * 100 < total * MIN_BACKGROUND_SHARE {
                continue;
            }
            let darker = count(0, median - STROKE_MARGIN);
            let lighter = count(median + STROKE_MARGIN, 255);
            modes[ty * tw + tx] = match (darker, lighter) {
                (0, 0) => Mode::Keep,
                (d, l) if d >= l => Mode::Erode,
                _ => Mode::Dilate,
            };
        }
    }
    (modes, tw)
}

/// Two-pass (horizontal, vertical) luminance-ordered min/max filter that carries whole pixels,
/// so colors stay intact. Result is blended over the original.
fn thicken_strokes(img: &RgbaImage, radius: usize, amount: f32) -> RgbaImage {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let src = img.as_raw();
    let lum: Vec<u8> = src.as_chunks::<4>().0.iter().map(|p| luminance(p)).collect();
    let (modes, tw) = tile_modes(&lum, w, h);
    let mode_at = |x: usize, y: usize| modes[(y / TILE) * tw + x / TILE];

    // Pass 1: horizontal. Keeps the chosen source pixel (RGBA) and its luminance.
    let mut mid = src.clone();
    let mut mid_lum = lum.clone();
    par_rows(&mut mid, w * 4, |first, rows| {
        for (ri, row) in rows.chunks_exact_mut(w * 4).enumerate() {
            let y = first + ri;
            let src_row = &src[y * w * 4..(y + 1) * w * 4];
            for x in 0..w {
                let mode = mode_at(x, y);
                if mode == Mode::Keep {
                    continue;
                }
                let best = best_in_window(mode, x, radius, w, |i| lum[y * w + i]);
                row[x * 4..x * 4 + 4].copy_from_slice(&src_row[best * 4..best * 4 + 4]);
            }
        }
    });
    for y in 0..h {
        for x in 0..w {
            let mode = mode_at(x, y);
            if mode != Mode::Keep {
                mid_lum[y * w + x] = luminance(&mid[(y * w + x) * 4..]);
            }
        }
    }

    // Pass 2: vertical, then blend with the original.
    let mut out = mid.clone();
    let keep_orig = ((1.0 - amount) * 256.0).round() as u32;
    par_rows(&mut out, w * 4, |first, rows| {
        for (ri, row) in rows.chunks_exact_mut(w * 4).enumerate() {
            let y = first + ri;
            for x in 0..w {
                let mode = mode_at(x, y);
                if mode == Mode::Keep {
                    continue;
                }
                let best = best_in_window(mode, y, radius, h, |i| mid_lum[i * w + x]);
                let morph = &mid[(best * w + x) * 4..(best * w + x) * 4 + 4];
                let orig = &src[(y * w + x) * 4..(y * w + x) * 4 + 4];
                for c in 0..4 {
                    row[x * 4 + c] = ((orig[c] as u32 * keep_orig + morph[c] as u32 * (256 - keep_orig)) >> 8) as u8;
                }
            }
        }
    });
    RgbaImage::from_raw(w as u32, h as u32, out).expect("buffer matches dimensions")
}

/// Index in `center - radius..=center + radius` (clamped to `0..len`) with the darkest
/// (`Erode`) or lightest (`Dilate`) luminance; the center wins ties.
fn best_in_window(mode: Mode, center: usize, radius: usize, len: usize, lum: impl Fn(usize) -> u8) -> usize {
    let lo = center.saturating_sub(radius);
    let hi = (center + radius).min(len - 1);
    let mut best = center;
    let mut best_l = lum(center);
    for i in lo..=hi {
        let l = lum(i);
        let better = if mode == Mode::Erode { l < best_l } else { l > best_l };
        if better {
            best = i;
            best_l = l;
        }
    }
    best
}

fn unsharp(mut img: RgbaImage) -> RgbaImage {
    let blurred = gaussian_blur(&img, UNSHARP_SIGMA);
    let amount = (UNSHARP_AMOUNT * 256.0) as i32;
    let raw: &mut [u8] = &mut img;
    for (px, bl) in raw.as_chunks_mut::<4>().0.iter_mut().zip(blurred.as_raw().as_chunks::<4>().0) {
        if px[3] != 255 {
            continue;
        }
        for c in 0..3 {
            let v = px[c] as i32;
            px[c] = (v + (((v - bl[c] as i32) * amount) >> 8)).clamp(0, 255) as u8;
        }
    }
    img
}
