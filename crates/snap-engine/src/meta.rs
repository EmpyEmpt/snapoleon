//! Lossless removal of metadata (EXIF, XMP, GPS, text chunks) from image files we upload but did not encode ourselves.
//! Images the engine encodes never carry metadata in the first place (see `snap_image::encode`).

use snap_core::settings::ImageFormat;

/// JPEG quality for the fallback re-encode of rotated photos.
const REENCODE_QUALITY: u8 = 92;

/// Returns `bytes` without metadata; unknown formats and files without metadata come back unchanged.
pub(crate) fn strip(ext: &str, bytes: Vec<u8>) -> Vec<u8> {
    let stripped = match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" | "jpe" => strip_jpeg(&bytes),
        "png" => strip_png(&bytes),
        "webp" => strip_webp(&bytes),
        _ => None,
    };
    stripped.unwrap_or(bytes)
}

fn strip_jpeg(b: &[u8]) -> Option<Vec<u8>> {
    if b.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    // Dropping EXIF would also drop the rotation, so rotated photos are baked upright instead.
    if exif_orientation(b).is_some_and(|o| o != 1) {
        let upright = snap_image::encode::decode(b).ok()?;
        return snap_image::encode::encode(&upright, ImageFormat::Jpeg, REENCODE_QUALITY, false).ok();
    }
    let mut out = Vec::with_capacity(b.len());
    out.extend_from_slice(&b[..2]);
    let (mut i, mut changed) = (2, false);
    loop {
        if *b.get(i)? != 0xFF {
            return None;
        }
        let marker = *b.get(i + 1)?;
        match marker {
            0xFF => i += 1,
            // End of image, or start of scan: everything after is entropy-coded data.
            0xD9 | 0xDA => {
                out.extend_from_slice(&b[i..]);
                return changed.then_some(out);
            }
            // Markers without a length.
            0x01 | 0xD0..=0xD8 => {
                out.extend_from_slice(&b[i..i + 2]);
                i += 2;
            }
            _ => {
                let len = u16::from_be_bytes([*b.get(i + 2)?, *b.get(i + 3)?]) as usize;
                let segment = b.get(i..i + 2 + len)?;
                // APP1 (EXIF/XMP), APP3..APP13 (IPTC, vendor blocks) and comments. JFIF, ICC and Adobe stay.
                if matches!(marker, 0xE1 | 0xE3..=0xED | 0xFE) {
                    changed = true;
                } else {
                    out.extend_from_slice(segment);
                }
                i += 2 + len;
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
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if b.get(..8)? != SIGNATURE {
        return None;
    }
    let mut out = SIGNATURE.to_vec();
    let (mut i, mut changed) = (8, false);
    while i < b.len() {
        let len = u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?) as usize;
        let kind = b.get(i + 4..i + 8)?;
        let chunk = b.get(i..i + 12 + len)?;
        if matches!(kind, b"tEXt" | b"zTXt" | b"iTXt" | b"eXIf" | b"tIME") {
            changed = true;
        } else {
            out.extend_from_slice(chunk);
        }
        i += 12 + len;
    }
    changed.then_some(out)
}

fn strip_webp(b: &[u8]) -> Option<Vec<u8>> {
    if b.get(..4)? != b"RIFF" || b.get(8..12)? != b"WEBP" {
        return None;
    }
    let mut out = b[..12].to_vec();
    let (mut i, mut changed) = (12, false);
    while i + 8 <= b.len() {
        let kind = &b[i..i + 4];
        let len = u32::from_le_bytes(b[i + 4..i + 8].try_into().ok()?) as usize;
        let end = (i + 8 + len + (len & 1)).min(b.len());
        if kind == b"EXIF" || kind == b"XMP " {
            changed = true;
        } else {
            let start = out.len();
            out.extend_from_slice(&b[i..end]);
            if kind == b"VP8X" && len >= 1 {
                // Feature flags: clear the EXIF (0x08) and XMP (0x04) bits.
                out[start + 8] &= !0x0C;
            }
        }
        i = end;
    }
    let riff_len = u32::try_from(out.len() - 8).ok()?;
    out[4..8].copy_from_slice(&riff_len.to_le_bytes());
    changed.then_some(out)
}
