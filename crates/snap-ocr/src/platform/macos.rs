//! Apple Vision `VNRecognizeTextRequest`.

use std::io::Cursor;

use anyhow::anyhow;
use image::{DynamicImage, ImageFormat};
use objc2::{
    AnyThread,
    rc::{Retained, autoreleasepool},
    runtime::AnyObject,
};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSError, NSRange, NSString};
use objc2_vision::{VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel};
use snap_core::{Error, RectF, Result, RgbaImage};

use crate::{OcrLine, OcrResult, OcrWord};

const DEFAULT_LANGS: [&str; 2] = ["en-US", "ru-RU"];

fn vision(e: Retained<NSError>) -> Error {
    Error::Other(anyhow!("Vision: {}", e.localizedDescription()))
}

pub(crate) fn languages() -> Vec<String> {
    autoreleasepool(|_| supported(&VNRecognizeTextRequest::new()))
}

fn supported(request: &VNRecognizeTextRequest) -> Vec<String> {
    // SAFETY: plain query on a request we own.
    unsafe { request.supportedRecognitionLanguagesAndReturnError() }.map(|a| a.iter().map(|s| s.to_string()).collect()).unwrap_or_default()
}

pub(crate) fn status() -> (bool, String) {
    (true, "Apple Vision".into())
}

/// Requested tags mapped onto languages Vision supports (exact, then same primary subtag).
fn resolve_languages(wanted: &[String], supported: &[String]) -> Vec<String> {
    let defaults: Vec<String> = DEFAULT_LANGS.iter().map(|s| s.to_string()).collect();
    let wanted = if wanted.is_empty() { defaults.as_slice() } else { wanted };
    if supported.is_empty() {
        return wanted.to_vec();
    }
    let primary = |s: &str| s.split('-').next().unwrap_or(s).to_ascii_lowercase();
    let mut out: Vec<String> = Vec::new();
    for w in wanted {
        let hit = supported.iter().find(|s| s.eq_ignore_ascii_case(w)).or_else(|| supported.iter().find(|s| primary(s) == primary(w)));
        if let Some(h) = hit
            && !out.contains(h)
        {
            out.push(h.clone());
        }
    }
    out
}

pub(crate) fn recognize(img: &RgbaImage, wanted: &[String]) -> Result<OcrResult> {
    let mut png = Vec::new();
    DynamicImage::ImageRgba8(img.clone()).write_to(&mut Cursor::new(&mut png), ImageFormat::Png)?;
    autoreleasepool(|_| run(&png, (img.width() as f64, img.height() as f64), wanted))
}

fn run(png: &[u8], size: (f64, f64), wanted: &[String]) -> Result<OcrResult> {
    let data = NSData::with_bytes(png);
    let options: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::new();
    let handler = VNImageRequestHandler::initWithData_options(VNImageRequestHandler::alloc(), &data, &options);

    let request = VNRecognizeTextRequest::new();
    request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
    request.setUsesLanguageCorrection(true);
    let langs = resolve_languages(wanted, &supported(&request));
    if !langs.is_empty() {
        let ns: Vec<Retained<NSString>> = langs.iter().map(|l| NSString::from_str(l)).collect();
        request.setRecognitionLanguages(&NSArray::from_retained_slice(&ns));
    }

    let as_request: &VNRequest = &request;
    handler.performRequests_error(&NSArray::from_slice(&[as_request])).map_err(vision)?;

    let mut lines = Vec::new();
    for obs in request.results().iter().flat_map(|r| r.iter()) {
        let Some(candidate) = obs.topCandidates(1).firstObject() else { continue };
        let text = candidate.string().to_string();
        let confidence = candidate.confidence();
        // SAFETY: boundingBox is a plain getter on a live observation.
        let b = unsafe { obs.boundingBox() };
        let rect = flip([b.origin.x, b.origin.y, b.size.width, b.size.height], size);
        let mut words = Vec::new();
        for (start, len, word) in word_ranges(&text) {
            // SAFETY: the range lies inside the candidate string (UTF-16 offsets).
            let Ok(bb) = (unsafe { candidate.boundingBoxForRange_error(NSRange::new(start, len)) }) else { continue };
            let b = unsafe { bb.boundingBox() };
            let r = flip([b.origin.x, b.origin.y, b.size.width, b.size.height], size);
            words.push(OcrWord { text: word.to_string(), rect: r, confidence });
        }
        if words.is_empty() {
            words.push(OcrWord { text: text.clone(), rect, confidence });
        }
        lines.push(OcrLine { text, rect, words });
    }
    Ok(OcrResult { lines, language: (!langs.is_empty()).then(|| langs.join(",")), engine: "vision".into() })
}

/// Normalised bottom-left-origin box to image pixels, top-left origin.
fn flip([x, y, bw, bh]: [f64; 4], (w, h): (f64, f64)) -> RectF {
    RectF::new((x * w) as f32, ((1.0 - y - bh) * h) as f32, (bw * w) as f32, (bh * h) as f32)
}

/// Whitespace-separated words as (UTF-16 start, UTF-16 length, text).
fn word_ranges(text: &str) -> Vec<(usize, usize, &str)> {
    let mut out = Vec::new();
    let (mut pos16, mut start16, mut start_byte) = (0usize, 0usize, None::<usize>);
    for (i, c) in text.char_indices() {
        if c.is_whitespace() {
            if let Some(s) = start_byte.take() {
                out.push((start16, pos16 - start16, &text[s..i]));
            }
        } else if start_byte.is_none() {
            start_byte = Some(i);
            start16 = pos16;
        }
        pos16 += c.len_utf16();
    }
    if let Some(s) = start_byte {
        out.push((start16, pos16 - start16, &text[s..]));
    }
    out
}
