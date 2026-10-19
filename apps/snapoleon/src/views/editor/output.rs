//! Turning the document into pixels and files: the final render, size estimates and the messages
//! worker threads send back to the editor.

use std::path::PathBuf;

use image::RgbaImage;
use snap_annotate::{Document, RenderOptions, render};
use snap_core::{CaptureId, Result, Settings, settings::ImageFormat};
use snap_engine::{Hit, Link, OcrResult};
use snap_image::{encode, resize};

/// How the document is turned into an image, from the export settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExportParams {
    pub format: ImageFormat,
    pub quality: u8,
    pub optimize_png: bool,
    /// Widest the result may be; 0 keeps the natural size.
    pub max_width: u32,
    pub readable: bool,
}

impl ExportParams {
    pub fn from_settings(s: &Settings) -> Self {
        let format = s.capture.format;
        let quality = match format {
            ImageFormat::Png => 100,
            ImageFormat::Jpeg => s.capture.jpeg_quality,
            ImageFormat::Webp => s.capture.webp_quality,
        };
        Self {
            format,
            quality,
            optimize_png: s.capture.optimize_png,
            max_width: s.editor.max_export_width,
            readable: s.editor.readable_downscale,
        }
    }
}

/// The finished image: crop, annotations, redactions and beautify applied, shrunk to the maximum width.
pub fn render_export(doc: &Document, p: &ExportParams) -> RgbaImage {
    let crop = doc.crop_rect();
    let padding = doc.beautify.as_ref().map_or(0, |b| 2 * b.padding);
    let natural = (crop.w + padding) as f32;
    let scale = if p.max_width > 0 && natural > p.max_width as f32 { p.max_width as f32 / natural } else { 1.0 };
    let img = render(doc, &RenderOptions { scale, readable: p.readable, beautify: true });
    // Window frames and aspect ratios can add a little beyond the estimate above.
    if p.max_width > 0 && img.width() > p.max_width {
        return resize::fit(&img, Some(p.max_width), None, p.readable);
    }
    img
}

/// Bytes the image takes in the chosen format.
pub fn encoded_len(img: &RgbaImage, p: &ExportParams) -> Result<u64> {
    Ok(encode::encode(img, p.format, p.quality, p.optimize_png)?.len() as u64)
}

/// A small copy of `img` for previews, at most `max_side` on its longest side.
pub fn thumbnail(img: &RgbaImage, max_side: u32) -> RgbaImage {
    resize::thumbnail(img, max_side)
}

/// The size, encoded weight and a small picture of the finished image.
pub struct Estimate {
    pub size: (u32, u32),
    /// `None` when the image could not be encoded in the chosen format, or was not asked for.
    pub bytes: Option<u64>,
    pub preview: RgbaImage,
}

/// What a save wrote: the picture, the editable project and the library entry (id) they belong to.
pub struct Saved {
    pub id: CaptureId,
    pub file: Option<PathBuf>,
    pub project: Option<PathBuf>,
}

/// Everything worker threads send back to the editor window.
pub enum Msg {
    /// A long step of an output (redaction, upload) reports; an empty label means it is over.
    Progress {
        label: String,
        frac: Option<f32>,
    },
    Copied(Result<()>),
    Saved(Result<Saved>),
    /// `None` when the dialog was dismissed.
    SavedAs(Result<Option<Saved>>),
    Uploaded(Result<Link>),
    Finished(RgbaImage),
    Pinned(RgbaImage),
    Estimate(Estimate),
    /// Thumbnails for the Beautify tab, one per preset, in the presets' order.
    Thumbs(Vec<RgbaImage>),
    Ocr(Result<OcrResult>),
    Translated(Result<String>),
    AltText(Result<String>),
    Redactions(Result<Vec<Hit>>),
}
