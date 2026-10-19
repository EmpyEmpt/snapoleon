//! Windows.Media.Ocr through the `windows` crate.

use std::sync::Once;

use anyhow::anyhow;
use snap_core::{Error, RectF, Result, RgbaImage, t, tf};
use windows::{
    Globalization::Language,
    Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap},
    Media::Ocr::OcrEngine,
    Storage::Streams::DataWriter,
    Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize},
};

use crate::{OcrLine, OcrResult, OcrWord};

/// Engines tried per call when several languages are configured.
const MAX_ENGINES: usize = 3;

fn win(e: windows::core::Error) -> Error {
    Error::Other(anyhow!("Windows.Media.Ocr: {e}"))
}

fn tag(l: &Language) -> String {
    l.LanguageTag().map(|t| t.to_string()).unwrap_or_default()
}

fn available() -> Result<Vec<Language>> {
    Ok(OcrEngine::AvailableRecognizerLanguages().map_err(win)?.into_iter().collect())
}

pub(crate) fn languages() -> Vec<String> {
    // WinRT objects must be released on the thread that created them, so only strings leave it.
    std::thread::scope(|s| {
        s.spawn(|| {
            init_apartment()?;
            Ok::<_, Error>(available()?.iter().map(tag).collect::<Vec<_>>())
        })
        .join()
    })
    .ok()
    .and_then(Result::ok)
    .unwrap_or_default()
}

pub(crate) fn status() -> (bool, String) {
    let langs = languages();
    if langs.is_empty() {
        (false, t("ocr.error.no_language").into_owned())
    } else {
        (true, tf("ocr.note.native_windows", &[("langs", &langs.join(", "))]))
    }
}

pub(crate) fn recognize(img: &RgbaImage, wanted: &[String]) -> Result<OcrResult> {
    std::thread::scope(|s| s.spawn(|| recognize_on_thread(img, wanted)).join())
        .map_err(|_| Error::Other(anyhow!("Windows.Media.Ocr worker panicked")))?
}

/// WinRT calls need an initialised multithreaded apartment, and the WinRT
/// factories are cached process-wide. Each call runs on its own short-lived
/// thread, so one parked thread keeps the MTA (and those caches) alive for
/// the life of the process; without it the second call would hit freed factories.
fn init_apartment() -> Result<()> {
    static KEEPER: Once = Once::new();
    KEEPER.call_once(|| {
        let (ready, wait) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("snap-ocr-mta".into()).spawn(move || {
            let init = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
            let _ = ready.send(init.is_ok());
            if init.is_ok() {
                loop {
                    std::thread::park();
                }
            }
        });
        if spawned.is_ok() {
            let _ = wait.recv();
        }
    });
    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.map_err(win)
}

fn recognize_on_thread(img: &RgbaImage, wanted: &[String]) -> Result<OcrResult> {
    init_apartment()?;
    let max = OcrEngine::MaxImageDimension().map_err(win)?;
    if img.width() > max || img.height() > max {
        return Err(Error::invalid(tf("ocr.error.too_large", &[("max", &max.to_string())])));
    }
    let engines = pick_engines(wanted)?;
    let bitmap = to_bitmap(img)?;

    let results = engines.iter().map(|e| run(e, &bitmap)).collect::<Result<Vec<_>>>()?;
    Ok(crate::run::merge(results))
}

/// Engines for the requested BCP-47 tags (exact tag, then same primary
/// language). With none requested, or none installed, the user-profile
/// language comes first, followed by the other installed packs.
fn pick_engines(wanted: &[String]) -> Result<Vec<OcrEngine>> {
    let installed = available()?;
    let primary = |s: &str| s.split('-').next().unwrap_or(s).to_ascii_lowercase();

    let mut order: Vec<Language> = Vec::new();
    let push = |order: &mut Vec<Language>, l: &Language| {
        if !order.iter().any(|o| tag(o) == tag(l)) {
            order.push(l.clone());
        }
    };
    for want in wanted {
        let found = installed
            .iter()
            .find(|l| tag(l).eq_ignore_ascii_case(want))
            .or_else(|| installed.iter().find(|l| primary(&tag(l)) == primary(want)));
        if let Some(l) = found {
            push(&mut order, l);
        }
    }
    if order.is_empty() {
        if !wanted.is_empty() {
            tracing::warn!("none of the requested OCR languages {wanted:?} is installed, using defaults");
        }
        if let Some(profile) = OcrEngine::TryCreateFromUserProfileLanguages().ok().and_then(|e| e.RecognizerLanguage().ok()) {
            push(&mut order, &profile);
        }
        for l in &installed {
            push(&mut order, l);
        }
    }

    let engines: Vec<OcrEngine> = order.iter().filter_map(|l| OcrEngine::TryCreateFromLanguage(l).ok()).take(MAX_ENGINES).collect();
    if engines.is_empty() {
        return Err(Error::NotFound(t("ocr.error.no_language").into_owned()));
    }
    Ok(engines)
}

/// BGRA8 bitmap with transparency flattened onto white (the engine ignores alpha).
fn to_bitmap(img: &RgbaImage) -> Result<SoftwareBitmap> {
    let mut bgra = Vec::with_capacity(img.as_raw().len());
    for p in img.pixels() {
        let a = p[3] as u32;
        let over_white = |c: u8| ((c as u32 * a + 255 * (255 - a)) / 255) as u8;
        bgra.extend_from_slice(&[over_white(p[2]), over_white(p[1]), over_white(p[0]), 255]);
    }
    let writer = DataWriter::new().map_err(win)?;
    writer.WriteBytes(&bgra).map_err(win)?;
    let buffer = writer.DetachBuffer().map_err(win)?;
    SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, img.width() as i32, img.height() as i32).map_err(win)
}

fn run(engine: &OcrEngine, bitmap: &SoftwareBitmap) -> Result<OcrResult> {
    let recognized = engine.RecognizeAsync(bitmap).map_err(win)?.join().map_err(win)?;
    let mut lines = Vec::new();
    for line in recognized.Lines().map_err(win)? {
        let mut words = Vec::new();
        for w in line.Words().map_err(win)? {
            let r = w.BoundingRect().map_err(win)?;
            words.push(OcrWord {
                text: w.Text().map_err(win)?.to_string(),
                rect: RectF::new(r.X, r.Y, r.Width, r.Height),
                confidence: 1.0,
            });
        }
        let Some(rect) = words.iter().map(|w| w.rect).reduce(union) else { continue };
        lines.push(OcrLine { text: line.Text().map_err(win)?.to_string(), rect, words });
    }
    let language = engine.RecognizerLanguage().ok().map(|l| tag(&l));
    Ok(OcrResult { lines, language, engine: "windows".into() })
}

fn union(a: RectF, b: RectF) -> RectF {
    let (x0, y0) = (a.x.min(b.x), a.y.min(b.y));
    RectF::new(x0, y0, a.right().max(b.right()) - x0, a.bottom().max(b.bottom()) - y0)
}
