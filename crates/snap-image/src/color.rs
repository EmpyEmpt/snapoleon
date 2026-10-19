//! Color formats and measurements: HEX/RGB/HSL/HSV/OKLCH strings, WCAG contrast,
//! region averages. All colors are straight-alpha sRGB `[r, g, b, a]`.

use image::RgbaImage;
use snap_core::Rect;

use crate::util::clamp_rect;

pub fn hex(c: [u8; 4]) -> String {
    snap_core::settings::to_hex_color(c)
}

pub fn rgb(c: [u8; 4]) -> String {
    if c[3] == 255 {
        format!("rgb({}, {}, {})", c[0], c[1], c[2])
    } else {
        format!("rgba({}, {}, {}, {})", c[0], c[1], c[2], alpha_str(c[3]))
    }
}

pub fn hsl(c: [u8; 4]) -> String {
    let (h, s, l) = to_hsl(c);
    let (h, s, l) = (h.round() as i32 % 360, (s * 100.0).round() as i32, (l * 100.0).round() as i32);
    if c[3] == 255 { format!("hsl({h}, {s}%, {l}%)") } else { format!("hsla({h}, {s}%, {l}%, {})", alpha_str(c[3])) }
}

pub fn hsv(c: [u8; 4]) -> String {
    let (h, s, v) = to_hsv(c);
    let (h, s, v) = (h.round() as i32 % 360, (s * 100.0).round() as i32, (v * 100.0).round() as i32);
    if c[3] == 255 { format!("hsv({h}, {s}%, {v}%)") } else { format!("hsva({h}, {s}%, {v}%, {})", alpha_str(c[3])) }
}

/// CSS Color 4 `oklch()` notation.
pub fn oklch(c: [u8; 4]) -> String {
    let (l, ch, h) = to_oklch(c);
    // Hue is meaningless for achromatic colors; CSS writes it as 0 (or `none`).
    let h = if ch < 0.0004 { 0.0 } else { h };
    if c[3] == 255 {
        format!("oklch({:.1}% {:.3} {:.1})", l * 100.0, ch, h)
    } else {
        format!("oklch({:.1}% {:.3} {:.1} / {})", l * 100.0, ch, h, alpha_str(c[3]))
    }
}

/// `(hue 0..360, saturation 0..1, lightness 0..1)`.
pub fn to_hsl(c: [u8; 4]) -> (f32, f32, f32) {
    let (r, g, b) = unit_rgb(c);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1e-6 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    (hue(r, g, b, max, d), s, l)
}

/// `(hue 0..360, saturation 0..1, value 0..1)`.
pub fn to_hsv(c: [u8; 4]) -> (f32, f32, f32) {
    let (r, g, b) = unit_rgb(c);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let d = max - min;
    if d < 1e-6 {
        return (0.0, 0.0, max);
    }
    (hue(r, g, b, max, d), d / max, max)
}

/// `(lightness 0..1, chroma, hue 0..360)`.
pub fn to_oklch(c: [u8; 4]) -> (f32, f32, f32) {
    let [l, a, b] = srgb_to_oklab([c[0], c[1], c[2]]);
    (l, a.hypot(b), b.atan2(a).to_degrees().rem_euclid(360.0))
}

/// Inverse of [`to_oklch`], gamut-clipped to sRGB.
pub fn from_oklch(l: f32, chroma: f32, hue_deg: f32, alpha: u8) -> [u8; 4] {
    let h = hue_deg.to_radians();
    let [r, g, b] = oklab_to_srgb([l, chroma * h.cos(), chroma * h.sin()]);
    [r, g, b, alpha]
}

/// WCAG 2.x relative luminance (0 = black, 1 = white); alpha is ignored.
pub fn relative_luminance(c: [u8; 4]) -> f32 {
    0.2126 * srgb_to_linear(c[0]) + 0.7152 * srgb_to_linear(c[1]) + 0.0722 * srgb_to_linear(c[2])
}

/// WCAG 2.x contrast ratio, `1.0..=21.0`.
pub fn contrast_ratio(a: [u8; 4], b: [u8; 4]) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WcagLevel {
    Fail,
    /// 3:1, large text and UI components.
    AaLarge,
    /// 4.5:1, normal text (AAA for large text).
    Aa,
    /// 7:1, normal text.
    Aaa,
}

pub fn wcag_level(ratio: f32) -> WcagLevel {
    match ratio {
        r if r >= 7.0 => WcagLevel::Aaa,
        r if r >= 4.5 => WcagLevel::Aa,
        r if r >= 3.0 => WcagLevel::AaLarge,
        _ => WcagLevel::Fail,
    }
}

/// Alpha-weighted mean color of `r` (clamped to the image). Transparent area yields `[0, 0, 0, 0]`.
pub fn average(img: &RgbaImage, r: Rect) -> [u8; 4] {
    let Some((x0, y0, x1, y1)) = clamp_rect(r, img.width(), img.height()) else {
        return [0, 0, 0, 0];
    };
    let (mut sr, mut sg, mut sb, mut sa) = (0u64, 0u64, 0u64, 0u64);
    for y in y0..y1 {
        for x in x0..x1 {
            let p = img.get_pixel(x, y).0;
            let a = p[3] as u64;
            sr += p[0] as u64 * a;
            sg += p[1] as u64 * a;
            sb += p[2] as u64 * a;
            sa += a;
        }
    }
    let n = ((x1 - x0) as u64) * ((y1 - y0) as u64);
    if sa == 0 {
        return [0, 0, 0, 0];
    }
    [((sr + sa / 2) / sa) as u8, ((sg + sa / 2) / sa) as u8, ((sb + sa / 2) / sa) as u8, ((sa + n / 2) / n) as u8]
}

fn alpha_str(a: u8) -> String {
    let v = format!("{:.2}", a as f32 / 255.0);
    v.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn unit_rgb(c: [u8; 4]) -> (f32, f32, f32) {
    (c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0)
}

fn hue(r: f32, g: f32, b: f32, max: f32, d: f32) -> f32 {
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    h * 60.0
}

pub(crate) fn srgb_to_linear(v: u8) -> f32 {
    let c = v as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

pub(crate) fn linear_to_srgb_f(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    255.0 * if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// sRGB (u8) to OKLab `[L, a, b]`.
pub(crate) fn srgb_to_oklab(c: [u8; 3]) -> [f32; 3] {
    let (r, g, b) = (srgb_to_linear(c[0]), srgb_to_linear(c[1]), srgb_to_linear(c[2]));
    let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// OKLab to sRGB (u8), clipped to gamut.
pub(crate) fn oklab_to_srgb(lab: [f32; 3]) -> [u8; 3] {
    let f = oklab_to_srgb_f(lab);
    [f[0].round() as u8, f[1].round() as u8, f[2].round() as u8]
}

/// OKLab to sRGB in `0.0..=255.0` floats (keeps sub-LSB precision for dithering).
pub(crate) fn oklab_to_srgb_f(lab: [f32; 3]) -> [f32; 3] {
    let l = lab[0] + 0.396_337_78 * lab[1] + 0.215_803_76 * lab[2];
    let m = lab[0] - 0.105_561_346 * lab[1] - 0.063_854_17 * lab[2];
    let s = lab[0] - 0.089_484_18 * lab[1] - 1.291_485_5 * lab[2];
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    [
        linear_to_srgb_f(4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s),
        linear_to_srgb_f(-1.268_438 * l + 2.609_757_4 * m - 0.341_319_4 * s),
        linear_to_srgb_f(-0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s),
    ]
}
