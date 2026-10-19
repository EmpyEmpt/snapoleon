//! Screenshot -> JPEG payload for vision models.

use image::{RgbImage, codecs::jpeg::JpegEncoder, imageops::FilterType};
use snap_core::{Error, Result, RgbaImage};

/// Longest side sent to the model.
pub const MAX_SIDE: u32 = 1568;
const QUALITY: u8 = 85;

/// Flattens alpha over white, downscales so the longest side is at most
/// `max_side` and encodes as JPEG.
pub fn jpeg_for_ai(img: &RgbaImage, max_side: u32) -> Result<Vec<u8>> {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Err(Error::invalid("empty image"));
    }
    let mut rgb = RgbImage::new(w, h);
    for (src, dst) in img.pixels().zip(rgb.pixels_mut()) {
        let a = u32::from(src[3]);
        for c in 0..3 {
            dst[c] = ((u32::from(src[c]) * a + 255 * (255 - a) + 127) / 255) as u8;
        }
    }
    let longest = w.max(h);
    if longest > max_side {
        let scale = f64::from(max_side) / f64::from(longest);
        let nw = ((f64::from(w) * scale).round() as u32).max(1);
        let nh = ((f64::from(h) * scale).round() as u32).max(1);
        rgb = image::imageops::resize(&rgb, nw, nh, FilterType::CatmullRom);
    }
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, QUALITY).encode_image(&rgb)?;
    Ok(out)
}
