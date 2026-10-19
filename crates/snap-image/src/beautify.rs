//! Beautify: put a capture on a styled background (CleanShot-style) with padding, a window
//! frame, rounded corners and a soft layered shadow.

use image::{Rgba, RgbaImage};
use snap_core::settings::{Background, BeautifyPreset, parse_hex_color};
use snap_core::CaptureContext;

use crate::blur;
use crate::color::{oklab_to_srgb_f, srgb_to_oklab};
use crate::effects::gaussian_blur;
use crate::resize::resize;
use crate::util::{blit_over, crop, div255, over, par_rows};

pub use crate::frame::draw_frame;

const LUT_SIZE: usize = 2048;
const FALLBACK_COLOR: [u8; 4] = [28, 28, 30, 255];
/// Largest dimension of the downscaled working copy used for the blurred-capture background.
const BLUR_WORK_DIV: u32 = 8;
/// Sanity caps so a corrupt preset cannot request an absurd canvas.
const MAX_PADDING: u32 = 4096;
const MAX_SHADOW: u32 = 512;

/// Applies `p` to `img`. Window frames get generic labels; use [`apply_with`] to put the real
/// window title and URL into the chrome.
pub fn apply(img: &RgbaImage, p: &BeautifyPreset) -> RgbaImage {
    apply_with(img, p, &CaptureContext::default())
}

/// Like [`apply`], with `cx` supplying the title bar / tab title / address text.
pub fn apply_with(img: &RgbaImage, p: &BeautifyPreset, cx: &CaptureContext) -> RgbaImage {
    let content = draw_frame(img, p.frame, p.corner_radius, cx);
    let (cw, ch) = content.dimensions();
    let padding = p.padding.min(MAX_PADDING);
    let (mut w, mut h) = (cw + 2 * padding, ch + 2 * padding);

    if let Some((aw, ah)) = p.aspect.filter(|(a, b)| *a > 0 && *b > 0) {
        let target = aw as f64 / ah as f64;
        if (w as f64 / h as f64) < target {
            w = (h as f64 * target).ceil() as u32;
        } else {
            h = (w as f64 / target).ceil() as u32;
        }
    }

    let mut canvas = render_background(w, h, &p.background, img);
    let (x, y) = (((w - cw) / 2) as i64, ((h - ch) / 2) as i64);
    add_shadow(&mut canvas, &content, x, y, p.shadow.min(MAX_SHADOW));
    blit_over(&mut canvas, &content, x, y);
    canvas
}

/// Renders `bg` at `w` x `h`. `source` feeds [`Background::BlurredCapture`].
pub fn render_background(w: u32, h: u32, bg: &Background, source: &RgbaImage) -> RgbaImage {
    let (w, h) = (w.max(1), h.max(1));
    match bg {
        Background::Transparent => RgbaImage::new(w, h),
        Background::Solid { color } => RgbaImage::from_pixel(w, h, Rgba(parse_color(color))),
        Background::Gradient { from, to, angle_deg } => gradient(w, h, parse_color(from), parse_color(to), *angle_deg),
        Background::BlurredCapture => blurred_capture(w, h, source),
        Background::Image { path } => match crate::encode::load(std::path::Path::new(path)) {
            Ok(img) => cover(&img, w, h),
            Err(e) => {
                tracing::warn!("beautify background image {path:?} unusable: {e}");
                gradient(w, h, [0x5E, 0x5C, 0xE6, 255], [0x0A, 0x84, 0xFF, 255], 135.0)
            }
        },
    }
}

fn parse_color(s: &str) -> [u8; 4] {
    parse_hex_color(s).unwrap_or_else(|| {
        tracing::warn!("invalid color {s:?} in beautify preset");
        FALLBACK_COLOR
    })
}

/// CSS-style linear gradient (0 deg = towards the top, 90 deg = towards the right), blended in
/// OKLab so mid-tones stay clean, with sub-LSB dithering against banding.
fn gradient(w: u32, h: u32, from: [u8; 4], to: [u8; 4], angle_deg: f32) -> RgbaImage {
    let a0 = srgb_to_oklab([from[0], from[1], from[2]]);
    let a1 = srgb_to_oklab([to[0], to[1], to[2]]);
    let lut: Vec<[f32; 4]> = (0..LUT_SIZE)
        .map(|i| {
            let t = i as f32 / (LUT_SIZE - 1) as f32;
            let lab = [0, 1, 2].map(|k| a0[k] + (a1[k] - a0[k]) * t);
            let rgb = oklab_to_srgb_f(lab);
            [rgb[0], rgb[1], rgb[2], from[3] as f32 + (to[3] as f32 - from[3] as f32) * t]
        })
        .collect();

    let rad = angle_deg.to_radians();
    let (dx, dy) = (rad.sin(), -rad.cos());
    let length = (w as f32 * dx.abs() + h as f32 * dy.abs()).max(1.0);
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);

    let mut out = vec![0u8; w as usize * h as usize * 4];
    par_rows(&mut out, w as usize * 4, |first, rows| {
        for (ri, row) in rows.chunks_exact_mut(w as usize * 4).enumerate() {
            let y = first + ri;
            for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let t = (((x as f32 + 0.5 - cx) * dx + (y as f32 + 0.5 - cy) * dy) / length + 0.5).clamp(0.0, 1.0);
                let f = t * (LUT_SIZE - 1) as f32;
                let i = (f as usize).min(LUT_SIZE - 2);
                let k = f - i as f32;
                // Interleaved gradient noise in [-0.5, 0.5).
                let n = (52.982_918 * ((0.067_110_56 * x as f32 + 0.005_837_15 * y as f32).fract())).fract() - 0.5;
                for c in 0..4 {
                    let v = lut[i][c] + (lut[i + 1][c] - lut[i][c]) * k + if c < 3 { n } else { 0.0 };
                    px[c] = v.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    });
    RgbaImage::from_raw(w, h, out).expect("buffer matches dimensions")
}

/// Scales `src` to cover `w` x `h` and crops the overflow evenly.
fn cover(src: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    let s = (w as f32 / src.width().max(1) as f32).max(h as f32 / src.height().max(1) as f32);
    let (rw, rh) = (((src.width() as f32 * s).ceil() as u32).max(w), ((src.height() as f32 * s).ceil() as u32).max(h));
    let scaled = resize(src, rw, rh);
    let (x0, y0) = ((rw - w) / 2, (rh - h) / 2);
    crop(&scaled, x0, y0, x0 + w, y0 + h)
}

/// A heavily blurred, slightly saturated copy of the capture itself, filling the canvas.
fn blurred_capture(w: u32, h: u32, src: &RgbaImage) -> RgbaImage {
    let (sw, sh) = ((w / BLUR_WORK_DIV).max(16), (h / BLUR_WORK_DIV).max(16));
    let mut small = cover(&flatten_on_white(src), sw, sh);
    let sigma = (sw.max(sh) as f32 * 0.045).clamp(3.0, 16.0);
    small = gaussian_blur(&small, sigma);
    let raw: &mut [u8] = &mut small;
    for px in raw.as_chunks_mut::<4>().0.iter_mut() {
        let lum = 0.2126 * px[0] as f32 + 0.7152 * px[1] as f32 + 0.0722 * px[2] as f32;
        for c in &mut px[..3] {
            *c = ((lum + (*c as f32 - lum) * 1.3) * 0.94).round().clamp(0.0, 255.0) as u8;
        }
        px[3] = 255;
    }
    resize(&small, w, h)
}

fn flatten_on_white(img: &RgbaImage) -> RgbaImage {
    if crate::util::is_opaque(img) {
        return img.clone();
    }
    let mut out = RgbaImage::from_pixel(img.width(), img.height(), Rgba([255, 255, 255, 255]));
    blit_over(&mut out, img, 0, 0);
    out
}

/// Two stacked soft shadows (wide ambient + tight contact), shaped by the content's alpha.
fn add_shadow(canvas: &mut RgbaImage, content: &RgbaImage, x: i64, y: i64, size: u32) {
    if size == 0 {
        return;
    }
    let size = size as f32;
    let layers = [(size, size * 0.4, 0.34f32), ((size / 4.0).max(1.0), (size * 0.1).max(1.0), 0.30)];
    for (radius, dy, strength) in layers {
        let margin = (radius * 1.5).ceil() as usize + 1;
        let (cw, ch) = (content.width() as usize, content.height() as usize);
        let (mw, mh) = (cw + 2 * margin, ch + 2 * margin);
        let mut mask = vec![0u8; mw * mh];
        for (row, src_row) in content.as_raw().chunks_exact(cw * 4).enumerate() {
            let dst = &mut mask[(row + margin) * mw + margin..][..cw];
            for (d, px) in dst.iter_mut().zip(src_row.as_chunks::<4>().0) {
                *d = px[3];
            }
        }
        blur::gaussian::<1>(&mut mask, mw, mh, radius * 0.5);

        let alpha = (strength * 255.0) as u32;
        let (ox, oy) = (x - margin as i64, y - margin as i64 + dy.round() as i64);
        for my in 0..mh {
            let cy = oy + my as i64;
            if cy < 0 || cy >= canvas.height() as i64 {
                continue;
            }
            for mx in 0..mw {
                let cx = ox + mx as i64;
                if cx < 0 || cx >= canvas.width() as i64 {
                    continue;
                }
                let a = div255(mask[my * mw + mx] as u32 * alpha) as u8;
                if a > 0 {
                    over(&mut canvas.get_pixel_mut(cx as u32, cy as u32).0, [0, 0, 0, a]);
                }
            }
        }
    }
}
