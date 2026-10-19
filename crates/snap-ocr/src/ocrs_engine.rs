//! Pure-Rust fallback (`ocrs`). Models are fetched on first use.

use std::fs::{self, File};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::anyhow;
use ocrs::{ImageSource, OcrEngine, OcrEngineParams, TextItem};
use snap_core::{Error, RectF, Result, RgbaImage, paths, t};

use crate::{OcrLine, OcrResult, OcrWord};

const MODEL_BASE: &str = "https://ocrs-models.s3-accelerate.amazonaws.com";
const DETECTION: &str = "text-detection.rten";
const RECOGNITION: &str = "text-recognition.rten";

static ENGINE: Mutex<Option<Arc<OcrEngine>>> = Mutex::new(None);

fn model_path(name: &str) -> PathBuf {
    paths::models_dir().join("ocrs").join(name)
}

pub(crate) fn status() -> (bool, String) {
    let ready = model_path(DETECTION).is_file() && model_path(RECOGNITION).is_file();
    (true, t(if ready { "ocr.note.ocrs_ready" } else { "ocr.note.ocrs_download" }).into_owned())
}

fn download(name: &str) -> Result<PathBuf> {
    let dest = model_path(name);
    if dest.is_file() {
        return Ok(dest);
    }
    fs::create_dir_all(dest.parent().expect("model path has a parent"))?;
    tracing::info!("downloading OCR model {name}");
    let resp = ureq::get(format!("{MODEL_BASE}/{name}")).call().map_err(Error::network)?;
    let tmp = dest.with_extension("part");
    io::copy(&mut resp.into_body().into_reader(), &mut File::create(&tmp)?)?;
    fs::rename(&tmp, &dest)?;
    Ok(dest)
}

fn load(name: &str) -> Result<rten::Model> {
    let path = download(name)?;
    rten::Model::load_file(&path).map_err(|e| Error::Other(anyhow!("loading {}: {e}", path.display())))
}

fn engine() -> Result<Arc<OcrEngine>> {
    let mut slot = ENGINE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(e) = slot.as_ref() {
        return Ok(e.clone());
    }
    let engine = Arc::new(OcrEngine::new(OcrEngineParams {
        detection_model: Some(load(DETECTION)?),
        recognition_model: Some(load(RECOGNITION)?),
        ..Default::default()
    })?);
    *slot = Some(engine.clone());
    Ok(engine)
}

pub(crate) fn recognize(img: &RgbaImage) -> Result<OcrResult> {
    let engine = engine()?;
    let rgb: Vec<u8> = img
        .pixels()
        .flat_map(|p| {
            let a = p[3] as u32;
            [0, 1, 2].map(|i| ((p[i] as u32 * a + 255 * (255 - a)) / 255) as u8)
        })
        .collect();
    let input = engine.prepare_input(ImageSource::from_bytes(&rgb, img.dimensions()).map_err(|e| Error::Other(anyhow!("{e}")))?)?;
    let words = engine.detect_words(&input)?;
    let line_boxes = engine.find_text_lines(&input, &words);

    let mut lines = Vec::new();
    for line in engine.recognize_text(&input, &line_boxes)?.into_iter().flatten() {
        let words: Vec<OcrWord> = line
            .words()
            .map(|w| {
                let r = w.bounding_rect();
                OcrWord { text: w.to_string(), rect: rect(r.left(), r.top(), r.right(), r.bottom()), confidence: 1.0 }
            })
            .collect();
        if !words.is_empty() {
            let r = line.bounding_rect();
            lines.push(OcrLine { text: line.to_string().trim().to_string(), rect: rect(r.left(), r.top(), r.right(), r.bottom()), words });
        }
    }
    Ok(OcrResult { lines, language: None, engine: "ocrs".into() })
}

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RectF {
    RectF::new(left as f32, top as f32, (right - left) as f32, (bottom - top) as f32)
}
