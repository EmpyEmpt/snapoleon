//! QR / barcode decoding (rxing) and QR generation (qrcode).

use image::{
    GrayImage, Luma, Rgba, RgbaImage,
    imageops::{self, FilterType},
};
use qrcode::{Color, EcLevel, QrCode};
use rxing::{DecodeHints, helpers::detect_multiple_in_luma_with_hints};
use serde::{Deserialize, Serialize};
use snap_core::{RectF, Result};

/// Quiet-zone width in modules required by the QR spec.
const QUIET_ZONE: u32 = 4;
/// Longest payload a version-40 QR code can hold at the lowest EC level.
const QR_MAX_BYTES: usize = 2953;
/// Images whose longer side exceeds this also get a downscaled pass.
const DOWNSCALE_ABOVE: u32 = 1400;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Code {
    /// Symbology, e.g. `qrcode`, `ean 13`, `code 128`, `datamatrix`.
    pub format: String,
    pub text: String,
    /// Bounding box of the detected symbol in `img` pixels, when the reader reports points.
    pub rect: Option<RectF>,
}

/// Find all QR, DataMatrix, Aztec, PDF417, EAN/UPC, Code 128/39 ... symbols.
///
/// Each pass scans the image and its inverse (light on dark): first as-is,
/// then 2x upscaled for small codes, and finally half-size for very large
/// images. The first pass that finds anything wins.
pub fn decode(img: &RgbaImage) -> Vec<Code> {
    if img.width() == 0 || img.height() == 0 {
        return Vec::new();
    }
    let gray = luma(img);
    let mut scales = vec![1.0, 2.0];
    if img.width().max(img.height()) > DOWNSCALE_ABOVE {
        scales.push(0.5);
    }
    for scale in scales {
        let mut found = scan(&gray, scale, false);
        for c in scan(&gray, scale, true) {
            if !found.iter().any(|f| f.format == c.format && f.text == c.text) {
                found.push(c);
            }
        }
        if !found.is_empty() {
            return found;
        }
    }
    Vec::new()
}

fn luma(img: &RgbaImage) -> GrayImage {
    GrayImage::from_fn(img.width(), img.height(), |x, y| {
        let p = img.get_pixel(x, y).0;
        let a = p[3] as u32;
        let flat = |c: u8| (c as u32 * a + 255 * (255 - a)) / 255;
        Luma([((299 * flat(p[0]) + 587 * flat(p[1]) + 114 * flat(p[2])) / 1000) as u8])
    })
}

fn scan(gray: &GrayImage, scale: f32, invert: bool) -> Vec<Code> {
    let mut g = if scale == 1.0 {
        gray.clone()
    } else {
        let filter = if scale > 1.0 { FilterType::CatmullRom } else { FilterType::Triangle };
        imageops::resize(gray, (gray.width() as f32 * scale).round() as u32, (gray.height() as f32 * scale).round() as u32, filter)
    };
    if invert {
        imageops::invert(&mut g);
    }
    let (w, h) = g.dimensions();
    let mut hints = DecodeHints { TryHarder: Some(true), ..Default::default() };
    let Ok(results) = detect_multiple_in_luma_with_hints(g.into_raw(), w, h, &mut hints) else { return Vec::new() };

    let mut codes: Vec<Code> = Vec::new();
    for r in results {
        let pts = r.getPoints();
        let rect = (!pts.is_empty()).then(|| {
            let (x0, x1) = pts.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p.x), hi.max(p.x)));
            let (y0, y1) = pts.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p.y), hi.max(p.y)));
            RectF::new(x0 / scale, y0 / scale, (x1 - x0) / scale, (y1 - y0) / scale)
        });
        let code = Code { format: r.getBarcodeFormat().to_string(), text: r.getText().to_string(), rect };
        if !codes.iter().any(|c| c.format == code.format && c.text == code.text) {
            codes.push(code);
        }
    }
    codes
}

/// Render `text` as a QR code with a 4-module quiet zone, `module_px` pixels
/// per module (clamped to 1..=64), black on white. Payloads beyond the QR
/// capacity are cut at a character boundary; use [`try_qr`] to detect that.
pub fn qr(text: &str, module_px: u32) -> RgbaImage {
    if let Ok(img) = try_qr(text, module_px) {
        return img;
    }
    let mut end = text.len().min(QR_MAX_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    tracing::warn!("QR payload of {} bytes truncated to {end}", text.len());
    try_qr(&text[..end], module_px).expect("payload within QR capacity")
}

/// Like [`qr`] but fails when the payload does not fit in a QR code.
pub fn try_qr(text: &str, module_px: u32) -> Result<RgbaImage> {
    let px = module_px.clamp(1, 64);
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::M)
        .or_else(|_| QrCode::with_error_correction_level(text.as_bytes(), EcLevel::L))
        .map_err(|e| snap_core::Error::invalid(format!("QR payload: {e}")))?;
    let modules = code.width() as u32;
    let side = (modules + 2 * QUIET_ZONE) * px;
    let colors = code.to_colors();
    let mut img = RgbaImage::from_pixel(side, side, Rgba([255, 255, 255, 255]));
    for (i, c) in colors.iter().enumerate() {
        if *c != Color::Dark {
            continue;
        }
        let (mx, my) = (i as u32 % modules, i as u32 / modules);
        for y in 0..px {
            for x in 0..px {
                img.put_pixel((mx + QUIET_ZONE) * px + x, (my + QUIET_ZONE) * px + y, Rgba([0, 0, 0, 255]));
            }
        }
    }
    Ok(img)
}
