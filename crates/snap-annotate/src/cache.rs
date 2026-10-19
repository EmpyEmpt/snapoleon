//! Per-document caches for the live editor view: premultiplied mip pyramids of the base image
//! and of pasted images, so redraws never touch full-resolution pixels at low zoom.

use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};

use image::RgbaImage;
use tiny_skia::Pixmap;

/// Premultiplied copies of an image at 1/1, 1/2, 1/4, ... resolution, built lazily.
pub(crate) struct Pyramid {
    src: Arc<RgbaImage>,
    levels: Vec<OnceLock<Pixmap>>,
}

impl Pyramid {
    fn new(src: Arc<RgbaImage>) -> Self {
        let short = src.width().min(src.height()).max(1);
        let count = (32 - short.leading_zeros()) as usize;
        Self { levels: (0..count).map(|_| OnceLock::new()).collect(), src }
    }

    pub fn max_level(&self) -> usize {
        self.levels.len() - 1
    }

    pub fn level(&self, n: usize) -> &Pixmap {
        let n = n.min(self.max_level());
        self.levels[n].get_or_init(|| if n == 0 { premultiply(&self.src) } else { halve(self.level(n - 1)) })
    }
}

pub(crate) fn premultiply(img: &RgbaImage) -> Pixmap {
    let mut pm = Pixmap::new(img.width().max(1), img.height().max(1)).expect("non-zero pixmap size");
    for (d, s) in pm.data_mut().as_chunks_mut::<4>().0.iter_mut().zip(img.as_raw().as_chunks::<4>().0) {
        let a = s[3] as u32;
        if a == 255 {
            *d = *s;
        } else {
            for k in 0..3 {
                d[k] = div255(s[k] as u32 * a) as u8;
            }
            d[3] = s[3];
        }
    }
    pm
}

pub(crate) fn unpremultiply(pm: Pixmap) -> RgbaImage {
    let (w, h) = (pm.width(), pm.height());
    RgbaImage::from_raw(w, h, pm.take_demultiplied()).expect("pixmap buffer matches its size")
}

#[inline]
pub(crate) fn div255(v: u32) -> u32 {
    (v + 128 + ((v + 128) >> 8)) >> 8
}

/// 2x2 box average of a premultiplied pixmap.
fn halve(src: &Pixmap) -> Pixmap {
    let (sw, sh) = (src.width() as usize, src.height() as usize);
    let (dw, dh) = ((sw / 2).max(1), (sh / 2).max(1));
    let mut out = Pixmap::new(dw as u32, dh as u32).expect("non-zero pixmap size");
    let s = src.data();
    let d = out.data_mut();
    for y in 0..dh {
        let (y0, y1) = ((y * 2).min(sh - 1), (y * 2 + 1).min(sh - 1));
        for x in 0..dw {
            let (x0, x1) = ((x * 2).min(sw - 1), (x * 2 + 1).min(sw - 1));
            for c in 0..4 {
                let sum = s[(y0 * sw + x0) * 4 + c] as u32
                    + s[(y0 * sw + x1) * 4 + c] as u32
                    + s[(y1 * sw + x0) * 4 + c] as u32
                    + s[(y1 * sw + x1) * 4 + c] as u32;
                d[(y * dw + x) * 4 + c] = ((sum + 2) >> 2) as u8;
            }
        }
    }
    out
}

/// Pyramids keyed by the identity of the source `Arc`, owned by one document.
#[derive(Default)]
pub(crate) struct ImageCache {
    entries: Mutex<Vec<Arc<Pyramid>>>,
}

impl fmt::Debug for ImageCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ImageCache")
    }
}

impl ImageCache {
    pub fn pyramid(&self, img: &Arc<RgbaImage>) -> Arc<Pyramid> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        // Drop pyramids whose image nobody else references any more.
        entries.retain(|p| Arc::strong_count(&p.src) > 1);
        if let Some(p) = entries.iter().find(|p| Arc::ptr_eq(&p.src, img)) {
            return p.clone();
        }
        let p = Arc::new(Pyramid::new(img.clone()));
        entries.push(p.clone());
        p
    }
}
