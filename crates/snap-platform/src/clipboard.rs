//! Clipboard access.
//!
//! All operations run on one dedicated thread that owns a long-lived
//! `arboard::Clipboard`. On X11 and Wayland the application that set the
//! clipboard must keep serving it, so the instance has to outlive the call;
//! on Windows and macOS the same thread simply serialises access.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::OnceLock;

use crossbeam_channel::{Sender, bounded, unbounded};
use image::RgbaImage;
use snap_core::{Error, Result};

use crate::util::other;

type Job = Box<dyn FnOnce(&mut Option<arboard::Clipboard>) + Send>;

fn worker() -> &'static Sender<Job> {
    static WORKER: OnceLock<Sender<Job>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = unbounded::<Job>();
        std::thread::Builder::new()
            .name("snap-clipboard".into())
            .spawn(move || {
                let mut clipboard = None;
                for job in rx {
                    if catch_unwind(AssertUnwindSafe(|| job(&mut clipboard))).is_err() {
                        clipboard = None;
                    }
                }
            })
            .expect("spawn clipboard thread");
        tx
    })
}

fn run<R: Send + 'static>(f: impl FnOnce(&mut arboard::Clipboard) -> Result<R, arboard::Error> + Send + 'static) -> Result<R> {
    let (tx, rx) = bounded(1);
    let job: Job = Box::new(move |slot| {
        if slot.is_none() {
            match arboard::Clipboard::new() {
                Ok(c) => *slot = Some(c),
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            }
        }
        if let Some(clipboard) = slot.as_mut() {
            let _ = tx.send(f(clipboard));
        }
    });
    worker().send(job).map_err(|_| other("clipboard thread stopped"))?;
    rx.recv().map_err(|_| other("clipboard operation failed"))?.map_err(convert)
}

fn convert(e: arboard::Error) -> Error {
    match e {
        arboard::Error::ClipboardNotSupported => Error::unsupported("clipboard format not supported here"),
        arboard::Error::ContentNotAvailable => Error::NotFound("clipboard content".into()),
        arboard::Error::ConversionFailure => Error::invalid("clipboard content could not be converted"),
        other_err => other(other_err),
    }
}

pub fn set_text(text: &str) -> Result<()> {
    let text = text.to_owned();
    run(move |c| c.set_text(text))
}

/// Put an image on the clipboard at one pixel per point. Prefer [`set_image_scaled`] for screen captures.
pub fn set_image(img: &RgbaImage) -> Result<()> {
    set_image_scaled(img, 1.0)
}

/// Put an image on the clipboard; `scale` is its pixels per point (the capture's display scale, 2 on a Retina
/// display). macOS gets PNG and TIFF flavours sized in points, so the image pastes into Notes, Mail or Keynote at the
/// size it had on screen, like the system's own screenshots. Elsewhere the clipboard has no notion of scale.
pub fn set_image_scaled(img: &RgbaImage, scale: f32) -> Result<()> {
    #[cfg(target_os = "macos")]
    return mac::set_image(img, scale);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = scale;
        let bytes = std::borrow::Cow::Owned(img.as_raw().clone());
        let data = arboard::ImageData { width: img.width() as usize, height: img.height() as usize, bytes };
        run(move |c| c.set_image(data))
    }
}

/// Place HTML on the clipboard; `alt` is the plain-text fallback for targets
/// that do not understand HTML.
pub fn set_html(html: &str, alt: &str) -> Result<()> {
    let (html, alt) = (html.to_owned(), alt.to_owned());
    run(move |c| c.set_html(html, Some(alt)))
}

/// Place files on the clipboard so they can be pasted into a file manager
/// (`CF_HDROP` on Windows, file URLs on macOS, `text/uri-list` on Linux).
pub fn set_files(paths: &[PathBuf]) -> Result<()> {
    if paths.is_empty() {
        return Err(Error::invalid("no files to copy"));
    }
    let paths: Vec<PathBuf> = paths.iter().map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone())).collect();
    run(move |c| c.set().file_list(&paths))
}

pub fn get_image() -> Option<RgbaImage> {
    let data = run(|c| c.get_image()).ok()?;
    RgbaImage::from_raw(data.width as u32, data.height as u32, data.bytes.into_owned())
}

pub fn get_text() -> Option<String> {
    run(|c| c.get_text()).ok()
}

/// arboard writes a single TIFF at 72 dpi built from the pixel size, and no PNG.
#[cfg(target_os = "macos")]
mod mac {
    use image::RgbaImage;
    use objc2_app_kit::{NSBitmapImageRep, NSPasteboard, NSPasteboardTypePNG, NSPasteboardTypeTIFF};
    use objc2_foundation::{NSData, NSSize};
    use snap_core::Result;

    use crate::util::other;

    pub fn set_image(img: &RgbaImage, scale: f32) -> Result<()> {
        let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
        let png_data = encode_png(img, scale)?;
        let points = NSSize::new(f64::from(img.width()) / f64::from(scale), f64::from(img.height()) / f64::from(scale));
        // On the clipboard thread, so it is ordered with every other clipboard call.
        super::run(move |_| objc2::rc::autoreleasepool(|_| write(&png_data, points)))
    }

    /// RGBA PNG tagged sRGB, with a pHYs chunk of 72 dpi per point so readers of the PNG alone also get the size right.
    fn encode_png(img: &RgbaImage, scale: f32) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, img.width(), img.height());
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        let per_meter = (72.0 * f64::from(scale) / 0.0254).round() as u32;
        encoder.set_pixel_dims(Some(png::PixelDimensions { xppu: per_meter, yppu: per_meter, unit: png::Unit::Meter }));
        let mut writer = encoder.write_header().map_err(other)?;
        writer.write_image_data(img.as_raw()).map_err(other)?;
        writer.finish().map_err(other)?;
        Ok(out)
    }

    fn write(png_data: &[u8], points: NSSize) -> Result<(), arboard::Error> {
        let fail = |what: &str| arboard::Error::Unknown { description: what.to_string() };
        let png = NSData::with_bytes(png_data);
        let rep = NSBitmapImageRep::imageRepWithData(&png).ok_or_else(|| fail("the PNG could not be read back"))?;
        // The TIFF's resolution comes from the representation's size in points.
        rep.setSize(points);
        let tiff = rep.TIFFRepresentation().ok_or_else(|| fail("the TIFF could not be made"))?;
        let board = NSPasteboard::generalPasteboard();
        board.clearContents();
        // SAFETY: constant NSStrings exported by AppKit.
        let (png_type, tiff_type) = unsafe { (NSPasteboardTypePNG, NSPasteboardTypeTIFF) };
        if board.setData_forType(Some(&png), png_type) && board.setData_forType(Some(&tiff), tiff_type) {
            Ok(())
        } else {
            Err(fail("the pasteboard did not take the image"))
        }
    }
}
