//! End-to-end smoke run of the engine on this machine, headless:
//!
//! 1. capture a monitor -> auto-redact -> beautify -> save -> OCR -> history,
//! 2. a synthetic screenshot full of personal data -> auto-redact -> save (checks that redaction really fires),
//! 3. searching the history for what was just recorded.
//!
//! Output goes to `target/smoke/snap-engine/`; history and settings are private to this run.

use std::{path::PathBuf, sync::Arc, time::Instant};

use snap_core::{
    Capture, CaptureKind, Settings,
    pipeline::Step,
    secrets::MemoryStore,
    settings::{ImageFormat, RedactKind},
};
use snap_engine::{CaptureRequest, Engine, HeadlessUi, History, PipelineUi};
use snap_history::Query;

const OUT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/smoke/snap-engine");

fn main() -> snap_core::Result<()> {
    tracing_subscriber::fmt().with_writer(std::io::stderr).with_env_filter("snap_engine=debug,warn").init();
    let out = PathBuf::from(OUT);
    std::fs::create_dir_all(&out)?;

    let mut settings = Settings::default();
    settings.capture.save_dir = out.join("saved").display().to_string();
    settings.capture.format = ImageFormat::Png;
    settings.privacy.redact_kinds = RedactKind::ALL.to_vec();
    let history = Arc::new(History::open(&out.join("history.sqlite"))?);
    let engine = Engine::with_parts(settings, Arc::new(MemoryStore::default()), Some(history.clone()));
    let ui = HeadlessUi::default();

    // 1. A real monitor through the whole pipeline.
    let started = Instant::now();
    let cap = engine.capture(&CaptureRequest::Monitor(0))?;
    println!("captured {}x{} in {:?}", cap.image.width(), cap.image.height(), started.elapsed());
    let steps = [
        Step::AutoRedact { kinds: vec![], style: None },
        Step::Beautify { preset: "Graphite".into() },
        Step::Save { dir: None, template: Some("engine-monitor {HH}.{mm}.{ss}".into()), format: None },
        Step::Ocr { copy: false, as_markdown_table: false },
        Step::Notify,
    ];
    let started = Instant::now();
    let outcome = engine.run_steps(cap, &steps, &ui)?;
    println!("pipeline took {:?}", started.elapsed());
    println!("  file: {:?}", outcome.file);
    println!("  history id: {:?}", outcome.id);
    println!("  text: {} chars", outcome.text.as_deref().map_or(0, str::len));

    // 2. Personal data on purpose.
    let synthetic = personal_data_screenshot();
    synthetic.save(out.join("synthetic-before.png")).map_err(|e| snap_core::Error::Other(e.into()))?;
    let cap = Capture::from_image(CaptureKind::Clipboard, synthetic.clone());
    let hits = engine.find_redactions(&synthetic, &[])?;
    println!("find_redactions: {} hits", hits.len());
    for hit in &hits {
        println!("  {:?} {:?} at ({:.0}, {:.0})", hit.kind, hit.text, hit.rect.x, hit.rect.y);
    }
    let outcome = engine.run_steps(
        cap,
        &[
            Step::AutoRedact { kinds: vec![], style: None },
            Step::Save { dir: None, template: Some("engine-synthetic".into()), format: None },
        ],
        &ui,
    )?;
    println!("  redacted copy: {:?}", outcome.file);

    // 3. Search what we just stored.
    let found = history.search(&Query { text: "Monitor".into(), limit: 10, ..Query::default() })?;
    println!("history holds {} entries; {} match \"engine\"", history.count()?, found.len());
    // Let the background text indexing finish before the process exits.
    std::thread::sleep(std::time::Duration::from_secs(3));
    for entry in history.search(&Query { limit: 5, ..Query::default() })? {
        println!("  {} {:?} text: {} chars", entry.id, entry.kind, entry.text.as_deref().map_or(0, str::len));
    }
    let _ = &ui as &dyn PipelineUi;
    Ok(())
}

/// A white page with the kinds of data the redactor knows, drawn with the annotation renderer.
fn personal_data_screenshot() -> snap_core::RgbaImage {
    use snap_annotate::{Document, RenderOptions, ShapeKind, Style, TextBg};
    use snap_core::PointF;

    let base = snap_core::RgbaImage::from_pixel(1100, 520, image::Rgba([250, 250, 252, 255]));
    let mut doc = Document::new(base, 1.0);
    let lines = [
        "Contact: anna.petrova@example.com",
        "Phone: +7 912 345-67-89",
        "Payment: 5555 5555 5555 4444",
        "IBAN: GB82 WEST 1234 5698 7654 32",
        "Token: ghp_16C7e42F292c6912E7710c838347Ae178B4a",
        "Server: 192.168.10.24",
        "Nothing secret about this line",
    ];
    for (i, line) in lines.iter().enumerate() {
        let kind = ShapeKind::Text {
            pos: PointF::new(40.0, 30.0 + i as f32 * 64.0),
            text: (*line).into(),
            size: 34.0,
            bold: false,
            bg: TextBg::None,
            max_width: None,
        };
        doc.add(kind, Style::with_color([20, 22, 30, 255]));
    }
    snap_annotate::render(&doc, &RenderOptions { scale: 1.0, readable: false, beautify: false })
}
