//! Encoding, decoding and size-targeted export. Output never carries metadata.

use std::borrow::Cow;
use std::io::Cursor;
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageReader, RgbaImage};
use snap_core::settings::ImageFormat;
use snap_core::{Error, Result, tf};

use crate::resize::readable_downscale;
use crate::util::{ensure_non_empty, is_opaque, over};

/// Longest side libwebp can encode.
const WEBP_MAX_SIDE: u32 = 16383;
const QUALITY_MAX: u8 = 95;
const QUALITY_MIN: u8 = 30;
/// Smallest side `encode_to_size` will shrink to before giving up.
const MIN_SIDE: u32 = 16;
const SCALE_STEP: f32 = 0.9;

/// What `encode_to_size` had to give up to reach the target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SizeFit {
    /// Linear scale applied to the image (1.0 = untouched).
    pub scale: f32,
    /// Quality used (100 for lossless PNG).
    pub quality: u8,
}

/// Encodes without metadata. `quality` is 1..=100 for JPEG and WebP (WebP at 100 is lossless);
/// PNG ignores it. `optimize_png` recompresses with oxipng when the `oxipng` feature is on.
pub fn encode(img: &RgbaImage, fmt: ImageFormat, quality: u8, optimize_png: bool) -> Result<Vec<u8>> {
    ensure_non_empty(img)?;
    match fmt {
        ImageFormat::Png => encode_png(img, optimize_png),
        ImageFormat::Jpeg => encode_jpeg(img, quality),
        ImageFormat::Webp => encode_webp(img, quality),
    }
}

/// Smallest-loss encoding that fits `max_bytes`: binary-searches quality at full size first,
/// then shrinks the image in 0.9 steps (re-searching quality at each). PNG, being lossless,
/// only shrinks. Scaled copies are made from the original, never cumulatively.
pub fn encode_to_size(img: &RgbaImage, fmt: ImageFormat, max_bytes: usize) -> Result<(Vec<u8>, SizeFit)> {
    ensure_non_empty(img)?;
    let longest = img.width().max(img.height());
    let mut step = 0i32;
    if fmt == ImageFormat::Webp && longest > WEBP_MAX_SIDE {
        step = ((WEBP_MAX_SIDE as f32 / longest as f32).ln() / SCALE_STEP.ln()).ceil() as i32;
    }
    loop {
        let scale = SCALE_STEP.powi(step);
        let (w, h) = ((img.width() as f32 * scale).round() as u32, (img.height() as f32 * scale).round() as u32);
        if w.min(h) < MIN_SIDE.min(img.width().min(img.height())) {
            return Err(Error::invalid(tf("image.error.cannot_fit", &[("bytes", &max_bytes.to_string())])));
        }
        let scaled: Cow<RgbaImage> = if step == 0 { Cow::Borrowed(img) } else { Cow::Owned(readable_downscale(img, w, h)) };
        match fit_quality(&scaled, fmt, max_bytes)? {
            Fit::Done(bytes, quality) => return Ok((bytes, SizeFit { scale, quality })),
            Fit::TooBig(smallest) => {
                // Size grows with area; jump straight to the first 0.9^k that could fit,
                // with slack because the estimate is rough.
                let ratio = (max_bytes as f32 / smallest as f32).sqrt() * 1.1;
                let jump = (ratio.min(1.0).ln() / SCALE_STEP.ln()).floor() as i32;
                step = (step + 1).max(step + jump);
            }
        }
    }
}

enum Fit {
    Done(Vec<u8>, u8),
    /// Even the lowest-quality encoding was this many bytes.
    TooBig(usize),
}

fn fit_quality(img: &RgbaImage, fmt: ImageFormat, max_bytes: usize) -> Result<Fit> {
    if fmt == ImageFormat::Png {
        let plain = encode_png(img, false)?;
        if plain.len() <= max_bytes {
            return Ok(Fit::Done(encode_png(img, true).ok().filter(|b| b.len() < plain.len()).unwrap_or(plain), 100));
        }
        if plain.len() <= max_bytes + max_bytes / 3
            && let Ok(opt) = encode_png(img, true)
            && opt.len() <= max_bytes
        {
            return Ok(Fit::Done(opt, 100));
        }
        return Ok(Fit::TooBig(plain.len()));
    }

    let enc = |q: u8| encode(img, fmt, q, false);
    let top = enc(QUALITY_MAX)?;
    if top.len() <= max_bytes {
        return Ok(Fit::Done(top, QUALITY_MAX));
    }
    let bottom = enc(QUALITY_MIN)?;
    if bottom.len() > max_bytes {
        return Ok(Fit::TooBig(bottom.len()));
    }
    let (mut lo, mut hi) = (QUALITY_MIN, QUALITY_MAX);
    let mut best = bottom;
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        let bytes = enc(mid)?;
        if bytes.len() <= max_bytes {
            lo = mid;
            best = bytes;
        } else {
            hi = mid;
        }
    }
    Ok(Fit::Done(best, lo))
}

/// Decodes any supported format, applying EXIF orientation and dropping all metadata.
pub fn decode(bytes: &[u8]) -> Result<RgbaImage> {
    let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    Ok(img.into_rgba8())
}

pub fn load(path: &Path) -> Result<RgbaImage> {
    decode(&read(path)?)
}

/// [`load`], plus the pixels per point the file records, if it does (a PNG's pHYs chunk, relative to 72 dpi: the
/// system's own Retina screenshots say 144 dpi, that is 2).
pub fn load_with_scale(path: &Path) -> Result<(RgbaImage, Option<f32>)> {
    let bytes = read(path)?;
    Ok((decode(&bytes)?, png_scale(&bytes)))
}

fn read(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => Error::NotFound(tf("image.error.no_file", &[("path", &path.display().to_string())])),
        _ => Error::Io(e),
    })
}

/// The density in a PNG's pHYs chunk as pixels per 72-dpi point, never below 1. The chunk must come before the image
/// data, so the walk stops there.
fn png_scale(bytes: &[u8]) -> Option<f32> {
    let mut rest = bytes.strip_prefix(b"\x89PNG\r\n\x1a\n")?;
    while let [l0, l1, l2, l3, k0, k1, k2, k3, ..] = *rest {
        let len = u32::from_be_bytes([l0, l1, l2, l3]) as usize;
        match &[k0, k1, k2, k3] {
            // Unit 1 is the metre; unit 0 only gives the aspect ratio.
            b"pHYs" => {
                let data = rest.get(8..8 + len).filter(|d| d.len() == 9 && d[8] == 1)?;
                let per_metre = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
                return (per_metre > 0).then(|| (per_metre as f32 * 0.0254 / 72.0).max(1.0));
            }
            b"IDAT" | b"IEND" => return None,
            _ => rest = rest.get(len.checked_add(12)?..)?,
        }
    }
    None
}

/// Writes atomically (temp file + rename) and returns the number of bytes written.
pub fn save(img: &RgbaImage, path: &Path, fmt: ImageFormat, quality: u8, optimize_png: bool) -> Result<u64> {
    let bytes = encode(img, fmt, quality, optimize_png)?;
    write_atomic(path, &bytes)?;
    Ok(bytes.len() as u64)
}

pub fn format_from_path(path: &Path) -> Option<ImageFormat> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some(ImageFormat::Png),
        "jpg" | "jpeg" | "jpe" => Some(ImageFormat::Jpeg),
        "webp" => Some(ImageFormat::Webp),
        _ => None,
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".part");
    let tmp = std::path::PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}

fn encode_png(img: &RgbaImage, optimize: bool) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let encoder = PngEncoder::new_with_quality(&mut out, CompressionType::Default, FilterType::Adaptive);
    // Opaque captures are stored as RGB: a quarter smaller before any optimiser runs.
    if is_opaque(img) {
        let rgb: Vec<u8> = img.as_raw().as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
        encoder.write_image(&rgb, img.width(), img.height(), ExtendedColorType::Rgb8)?;
    } else {
        encoder.write_image(img.as_raw(), img.width(), img.height(), ExtendedColorType::Rgba8)?;
    }
    #[cfg(feature = "oxipng")]
    if optimize {
        let mut opts = oxipng::Options::from_preset(2);
        opts.strip = oxipng::StripChunks::Safe;
        match oxipng::optimize_from_memory(&out, &opts) {
            Ok(smaller) if smaller.len() < out.len() => return Ok(smaller),
            Ok(_) => {}
            Err(e) => tracing::warn!("oxipng failed, keeping unoptimised PNG: {e}"),
        }
    }
    #[cfg(not(feature = "oxipng"))]
    let _ = optimize;
    Ok(out)
}

fn encode_jpeg(img: &RgbaImage, quality: u8) -> Result<Vec<u8>> {
    // JPEG has no alpha: flatten onto white.
    let mut rgb = Vec::with_capacity(img.width() as usize * img.height() as usize * 3);
    for p in img.as_raw().as_chunks::<4>().0 {
        let mut px = [255, 255, 255, 255];
        over(&mut px, *p);
        rgb.extend_from_slice(&px[..3]);
    }
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100)).write_image(&rgb, img.width(), img.height(), ExtendedColorType::Rgb8)?;
    Ok(out)
}

fn encode_webp(img: &RgbaImage, quality: u8) -> Result<Vec<u8>> {
    if img.width().max(img.height()) > WEBP_MAX_SIDE {
        return Err(Error::invalid(tf("image.error.webp_too_large", &[("max", &WEBP_MAX_SIDE.to_string())])));
    }
    let encoder = webp::Encoder::from_rgba(img.as_raw(), img.width(), img.height());
    let lossless = quality >= 100;
    let mem = encoder
        .encode_simple(lossless, quality.clamp(1, 100) as f32)
        .map_err(|e| Error::invalid(tf("image.error.webp_encode", &[("reason", &format!("{e:?}"))])))?;
    Ok(mem.to_vec())
}
