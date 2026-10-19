use std::{fs, path::PathBuf};

use image::{ImageFormat, RgbaImage, imageops::FilterType};
use snap_core::{Error, Result, capture::CaptureId, paths};

/// Longest edge of a thumbnail, in pixels.
const THUMB_EDGE: u32 = 320;

/// Scale `img` so its longest side is at most 320 px and store it as
/// `<thumbnails_dir>/<id>.png`. Smaller images are stored unscaled.
pub fn write_thumbnail(img: &RgbaImage, id: &CaptureId) -> Result<PathBuf> {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Err(Error::invalid("cannot thumbnail an empty image"));
    }
    let dir = paths::thumbnails_dir();
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.png", id.0));

    let longest = w.max(h);
    if longest <= THUMB_EDGE {
        img.save_with_format(&path, ImageFormat::Png)?;
    } else {
        let scale = f64::from(THUMB_EDGE) / f64::from(longest);
        let nw = ((f64::from(w) * scale).round() as u32).max(1);
        let nh = ((f64::from(h) * scale).round() as u32).max(1);
        let small = image::imageops::resize(img, nw, nh, FilterType::CatmullRom);
        small.save_with_format(&path, ImageFormat::Png)?;
    }
    Ok(path)
}
