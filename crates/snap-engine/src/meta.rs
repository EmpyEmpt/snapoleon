//! Removal of metadata (EXIF, XMP, GPS, text chunks) from image files we upload but did not encode ourselves.
//! Images the engine encodes carry none in the first place, only their pixel density (see `snap_image::encode`).

use snap_core::{Error, Result, settings::ImageFormat, t};

/// Quality for the fallback re-encode of rotated or unparsable photos.
const REENCODE_QUALITY: u8 = 92;

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// The raster format of `b`, by magic number: the file name says nothing about what is inside.
pub(crate) fn image_format(b: &[u8]) -> Option<ImageFormat> {
    if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(ImageFormat::Jpeg)
    } else if b.starts_with(PNG_SIGNATURE) {
        Some(ImageFormat::Png)
    } else if b.get(..4) == Some(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        Some(ImageFormat::Webp)
    } else {
        None
    }
}

/// Whether `head` (the first bytes of a file) starts an image that carries metadata, one [`strip`] works on or refuses.
pub(crate) fn is_image(head: &[u8]) -> bool {
    image_format(head).is_some() || is_unstrippable_image(head)
}

/// Image containers that carry metadata (HEIC / AVIF, TIFF) but that nothing here can rewrite.
fn is_unstrippable_image(b: &[u8]) -> bool {
    let tiff = b.starts_with(b"II*\0") || b.starts_with(b"MM\0*");
    let heif = b.get(4..8) == Some(b"ftyp") && matches!(b.get(8..12), Some(b"heic" | b"heix" | b"mif1" | b"msf1" | b"avif" | b"avis"));
    tiff || heif
}

/// Returns `bytes` without metadata. An image that cannot be cleaned losslessly is decoded and re-encoded, and when
/// even that fails the upload is refused: the original is never returned. Non-image files come back unchanged.
pub(crate) fn strip(bytes: Vec<u8>) -> Result<Vec<u8>> {
    let Some(format) = image_format(&bytes) else {
        return if is_unstrippable_image(&bytes) { Err(Error::invalid(t("engine.error.strip_unsupported"))) } else { Ok(bytes) };
    };
    let lossless = match format {
        ImageFormat::Jpeg => strip_jpeg(&bytes),
        ImageFormat::Png => strip_png(&bytes),
        ImageFormat::Webp => strip_webp(&bytes),
    };
    match lossless {
        Some(clean) => Ok(clean),
        None => {
            let img = snap_image::encode::decode(&bytes).map_err(|_| Error::invalid(t("engine.error.strip_failed")))?;
            snap_image::encode::encode(&img, format, REENCODE_QUALITY, false, None)
        }
    }
}

fn strip_jpeg(b: &[u8]) -> Option<Vec<u8>> {
    // Dropping EXIF would also drop the rotation, so rotated photos are baked upright instead.
    if exif_orientation(b).is_some_and(|o| o != 1) {
        let upright = snap_image::encode::decode(b).ok()?;
        return snap_image::encode::encode(&upright, ImageFormat::Jpeg, REENCODE_QUALITY, false, None).ok();
    }
    let mut out = Vec::with_capacity(b.len());
    out.extend_from_slice(&b[..2]);
    let mut i = 2;
    loop {
        if *b.get(i)? != 0xFF {
            return None;
        }
        let marker = *b.get(i + 1)?;
        match marker {
            0xFF => i += 1,
            // The primary image ends here. Whatever follows (MPF secondary images, motion-photo video,
            // vendor trailers) can carry its own EXIF and is dropped.
            0xD9 => {
                out.extend_from_slice(&[0xFF, 0xD9]);
                return Some(out);
            }
            // Markers without a length.
            0x01 | 0xD0..=0xD8 => {
                out.extend_from_slice(&b[i..i + 2]);
                i += 2;
            }
            _ => {
                let len = u16::from_be_bytes([*b.get(i + 2)?, *b.get(i + 3)?]) as usize;
                let segment = b.get(i..i + 2 + len)?;
                // APP1 (EXIF/XMP), APP3..APP13 (IPTC, vendor blocks), comments and the MPF index in APP2.
                // JFIF, ICC and Adobe stay.
                let mpf = marker == 0xE2 && segment.get(4..8) == Some(b"MPF\0");
                if !(matches!(marker, 0xE1 | 0xE3..=0xED | 0xFE) || mpf) {
                    out.extend_from_slice(segment);
                }
                i += 2 + len;
                if marker == 0xDA {
                    // Entropy-coded data runs to the next marker that is not a restart or a stuffed 0xFF.
                    let start = i;
                    while let Some(&byte) = b.get(i) {
                        if byte == 0xFF && !matches!(b.get(i + 1), Some(0x00 | 0xD0..=0xD7)) {
                            break;
                        }
                        i += if byte == 0xFF { 2 } else { 1 };
                    }
                    out.extend_from_slice(b.get(start..i)?);
                }
            }
        }
    }
}

/// EXIF orientation tag (1..=8) of a JPEG, when present.
fn exif_orientation(b: &[u8]) -> Option<u16> {
    let mut i = 2;
    while *b.get(i)? == 0xFF {
        let marker = *b.get(i + 1)?;
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        let len = u16::from_be_bytes([*b.get(i + 2)?, *b.get(i + 3)?]) as usize;
        if marker == 0xE1 && b.get(i + 4..i + 10)? == b"Exif\0\0" {
            return tiff_orientation(b.get(i + 10..i + 2 + len)?);
        }
        i += 2 + len;
    }
    None
}

fn tiff_orientation(t: &[u8]) -> Option<u16> {
    let little = match t.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at =
        |o: usize| t.get(o..o + 2).map(|v| if little { u16::from_le_bytes([v[0], v[1]]) } else { u16::from_be_bytes([v[0], v[1]]) });
    let u32_at = |o: usize| {
        t.get(o..o + 4)
            .map(|v| if little { u32::from_le_bytes([v[0], v[1], v[2], v[3]]) } else { u32::from_be_bytes([v[0], v[1], v[2], v[3]]) })
    };
    let ifd = u32_at(4)? as usize;
    (0..u16_at(ifd)? as usize).find_map(|n| {
        let entry = ifd + 2 + n * 12;
        (u16_at(entry)? == 0x0112).then(|| u16_at(entry + 8)).flatten()
    })
}

fn strip_png(b: &[u8]) -> Option<Vec<u8>> {
    let mut out = PNG_SIGNATURE.to_vec();
    let mut i = PNG_SIGNATURE.len();
    loop {
        let len = u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?) as usize;
        let kind = b.get(i + 4..i + 8)?;
        let chunk = b.get(i..i + 12 + len)?;
        if !matches!(kind, b"tEXt" | b"zTXt" | b"iTXt" | b"eXIf" | b"tIME") {
            out.extend_from_slice(chunk);
        }
        i += 12 + len;
        // Anything after IEND is not part of the image.
        if kind == b"IEND" {
            return Some(out);
        }
    }
}

fn strip_webp(b: &[u8]) -> Option<Vec<u8>> {
    // The RIFF length bounds the image; bytes after it are trailing data.
    let end = (u32::from_le_bytes(b.get(4..8)?.try_into().ok()?) as usize + 8).min(b.len());
    let mut out = b.get(..12)?.to_vec();
    let mut i = 12;
    while i + 8 <= end {
        let kind = &b[i..i + 4];
        let len = u32::from_le_bytes(b[i + 4..i + 8].try_into().ok()?) as usize;
        let chunk_end = (i + 8 + len + (len & 1)).min(end);
        if kind != b"EXIF" && kind != b"XMP " {
            let start = out.len();
            out.extend_from_slice(&b[i..chunk_end]);
            if kind == b"VP8X" && len >= 1 {
                // Feature flags: clear the EXIF (0x08) and XMP (0x04) bits.
                out[start + 8] &= !0x0C;
            }
        }
        i = chunk_end;
    }
    let riff_len = u32::try_from(out.len() - 8).ok()?;
    out[4..8].copy_from_slice(&riff_len.to_le_bytes());
    Some(out)
}
