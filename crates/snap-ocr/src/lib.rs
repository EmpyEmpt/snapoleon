//! OCR (native + fallback engines) and QR/barcode decoding.
//!
//! [`recognize`] runs the engine chosen in [`OcrSettings`]; with
//! [`OcrEngine::Auto`] the order is native (Windows.Media.Ocr / Apple Vision),
//! the `tesseract` CLI, then the optional pure-Rust `ocrs` engine. All
//! rectangles are in pixel space of the image that was passed in.

pub mod codes;
mod layout;
mod platform;
mod run;
mod tesseract;

#[cfg(feature = "ocrs")]
mod ocrs_engine;

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use snap_core::settings::{OcrEngine, OcrSettings};
use snap_core::{Error, RectF, Result, RgbaImage, Settings, t, tf};

/// One recognised word. `confidence` is 0..1; engines that do not report one
/// (Windows.Media.Ocr) use 1.0.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrWord {
    pub text: String,
    pub rect: RectF,
    pub confidence: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrLine {
    pub text: String,
    pub rect: RectF,
    pub words: Vec<OcrWord>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct OcrResult {
    pub lines: Vec<OcrLine>,
    /// Language the engine used (BCP-47 tag or engine specific code).
    pub language: Option<String>,
    /// Engine that produced the result: `windows`, `vision`, `tesseract` or `ocrs`.
    pub engine: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineStatus {
    pub engine: OcrEngine,
    pub available: bool,
    pub note: String,
}

/// Recognise text in `img`. Blocking; small text is upscaled x2 when that
/// improves the result, very tall images are processed in bands.
pub fn recognize(img: &RgbaImage, s: &OcrSettings) -> Result<OcrResult> {
    if img.width() == 0 || img.height() == 0 {
        return Ok(OcrResult::default());
    }
    let chain: &[OcrEngine] = match s.engine {
        OcrEngine::Auto => &[OcrEngine::Native, OcrEngine::Tesseract, OcrEngine::Ocrs],
        OcrEngine::Native => &[OcrEngine::Native],
        OcrEngine::Tesseract => &[OcrEngine::Tesseract],
        OcrEngine::Ocrs => &[OcrEngine::Ocrs],
    };

    let mut failures = Vec::new();
    for &engine in chain {
        match run_engine(engine, img, s) {
            Ok(result) => return Ok(result),
            Err(e) if chain.len() == 1 => return Err(e),
            Err(e) => {
                tracing::debug!("ocr engine {engine:?} failed: {e}");
                failures.push(format!("{}: {e}", engine_name(engine)));
            }
        }
    }
    Err(Error::NotFound(tf("ocr.error.no_engine", &[("details", &failures.join("; "))])))
}

/// A native request has finished in this process, so Apple Vision's models are compiled.
static NATIVE_READY: AtomicBool = AtomicBool::new(false);

/// Whether a recognition with `s` would first have to wait for Apple Vision to prepare its models (macOS, until the
/// first native request of this process has finished: about half a minute after an install or update, see
/// [`prewarm`]). UIs show "Preparing text recognition" instead of an apparently frozen progress meanwhile.
pub fn warming_up(s: &OcrSettings) -> bool {
    cfg!(target_os = "macos") && matches!(s.engine, OcrEngine::Auto | OcrEngine::Native) && !NATIVE_READY.load(Ordering::Relaxed)
}

/// Run one tiny recognition in the background so the first real one is fast. Apple Vision compiles its models on
/// the first request of every new executable, which takes about half a minute after each install or update; after
/// that a request takes about 0.1 s, so this runs on every launch. The other native engines start quickly, so
/// elsewhere this does nothing.
pub fn prewarm(s: &OcrSettings) {
    if !cfg!(target_os = "macos") || !matches!(s.engine, OcrEngine::Auto | OcrEngine::Native) {
        return;
    }
    let languages = s.languages.clone();
    let spawned = std::thread::Builder::new().name("snap-ocr-prewarm".into()).spawn(move || {
        // Startup work comes first; nobody asks for text in the first seconds.
        std::thread::sleep(Duration::from_secs(3));
        let began = Instant::now();
        // A blank picture is enough: the models are compiled before anything is detected.
        let blank = RgbaImage::from_pixel(64, 32, image::Rgba([255; 4]));
        match platform::recognize(&blank, &languages) {
            Ok(_) => tracing::info!("text recognition ready after {} ms", began.elapsed().as_millis()),
            Err(e) => tracing::debug!("text recognition warm-up: {e}"),
        }
        NATIVE_READY.store(true, Ordering::Relaxed);
    });
    if let Err(e) = spawned {
        tracing::debug!("text recognition warm-up: {e}");
    }
}

fn run_engine(engine: OcrEngine, img: &RgbaImage, s: &OcrSettings) -> Result<OcrResult> {
    match engine {
        OcrEngine::Native => {
            let result = run::run(img, &|part| platform::recognize(part, &s.languages));
            NATIVE_READY.store(true, Ordering::Relaxed);
            result
        }
        OcrEngine::Tesseract => run::run(img, &|part| tesseract::recognize(part, s)),
        OcrEngine::Ocrs => run_ocrs(img),
        OcrEngine::Auto => unreachable!("Auto is expanded by recognize()"),
    }
}

#[cfg(feature = "ocrs")]
fn run_ocrs(img: &RgbaImage) -> Result<OcrResult> {
    run::run(img, &ocrs_engine::recognize)
}

#[cfg(not(feature = "ocrs"))]
fn run_ocrs(_: &RgbaImage) -> Result<OcrResult> {
    Err(Error::unsupported(t("ocr.note.ocrs_disabled").into_owned()))
}

fn engine_name(engine: OcrEngine) -> String {
    t(match engine {
        OcrEngine::Auto => "ocr.engine.auto",
        OcrEngine::Native => "ocr.engine.native",
        OcrEngine::Tesseract => "ocr.engine.tesseract",
        OcrEngine::Ocrs => "ocr.engine.ocrs",
    })
    .into_owned()
}

/// Availability of every concrete engine, using the saved settings for the
/// tesseract path.
pub fn engines() -> Vec<EngineStatus> {
    engines_with(&Settings::load_default().ocr)
}

pub fn engines_with(s: &OcrSettings) -> Vec<EngineStatus> {
    let native = platform::status();
    let tess = tesseract::status(&s.tesseract_path);
    #[cfg(feature = "ocrs")]
    let ocrs = ocrs_engine::status();
    #[cfg(not(feature = "ocrs"))]
    let ocrs = (false, t("ocr.note.ocrs_disabled").into_owned());
    vec![
        EngineStatus { engine: OcrEngine::Native, available: native.0, note: native.1 },
        EngineStatus { engine: OcrEngine::Tesseract, available: tess.0, note: tess.1 },
        EngineStatus { engine: OcrEngine::Ocrs, available: ocrs.0, note: ocrs.1 },
    ]
}

/// Languages the OS engine can recognise (BCP-47 tags); empty where there is
/// no native engine.
pub fn native_languages() -> Vec<String> {
    platform::languages()
}

/// Language packs installed for the tesseract CLI (tesseract codes such as `eng`).
pub fn tesseract_languages(s: &OcrSettings) -> Vec<String> {
    tesseract::installed_languages(&s.tesseract_path)
}
