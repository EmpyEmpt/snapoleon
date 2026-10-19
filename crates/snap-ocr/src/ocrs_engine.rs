//! Pure-Rust fallback (`ocrs`). Models are fetched on first use.

use std::fs::{self, File};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::anyhow;
use ocrs::{ImageSource, OcrEngine, OcrEngineParams, TextItem};
use snap_core::{Error, RectF, Result, RgbaImage, paths, t};

use crate::{OcrLine, OcrResult, OcrWord};

const MODEL_BASE: &str = "https://ocrs-models.s3-accelerate.amazonaws.com";
const DETECTION: &str = "text-detection.rten";
const RECOGNITION: &str = "text-recognition.rten";

/// Models are large; a connection that goes quiet for longer than this is treated as dead.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
const BODY_TIMEOUT: Duration = Duration::from_secs(300);

static ENGINE: Mutex<Option<Arc<OcrEngine>>> = Mutex::new(None);
/// Serialises model downloads so two callers never write the same file; the engine slot itself is never held
/// across a download.
static DOWNLOAD: Mutex<()> = Mutex::new(());

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
    let _serial = DOWNLOAD.lock().unwrap_or_else(|e| e.into_inner());
    if dest.is_file() {
        return Ok(dest);
    }
    fs::create_dir_all(dest.parent().expect("model path has a parent"))?;
    tracing::info!("downloading OCR model {name}");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_recv_response(Some(RESPONSE_TIMEOUT))
        .timeout_recv_body(Some(BODY_TIMEOUT))
        .build()
        .into();
    let resp = agent.get(format!("{MODEL_BASE}/{name}")).call().map_err(Error::network)?;
    let tmp = dest.with_extension("part");
    let copied = File::create(&tmp).and_then(|mut f| io::copy(&mut resp.into_body().into_reader(), &mut f));
    if let Err(e) = copied {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    fs::rename(&tmp, &dest)?;
    Ok(dest)
}

fn load(name: &str) -> Result<rten::Model> {
    let path = download(name)?;
    rten::Model::load_file(&path).map_err(|e| Error::Other(anyhow!("loading {}: {e}", path.display())))
}

fn engine() -> Result<Arc<OcrEngine>> {
    if let Some(e) = ENGINE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        return Ok(e.clone());
    }
    // Downloading and loading happen outside the lock: a slow network must not block every other OCR caller.
    let engine = Arc::new(OcrEngine::new(OcrEngineParams {
        detection_model: Some(load(DETECTION)?),
        recognition_model: Some(load(RECOGNITION)?),
        ..Default::default()
    })?);
    Ok(ENGINE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert(engine).clone())
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
