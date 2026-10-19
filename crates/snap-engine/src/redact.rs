//! OCR-driven detection and covering of personal data.

use snap_core::{
    Result, RgbaImage,
    settings::{RedactKind, RedactStyle},
};
use snap_redact::LineWords;

use crate::{Engine, Hit};

pub(crate) fn find_redactions(engine: &Engine, img: &RgbaImage, kinds: &[RedactKind]) -> Result<Vec<Hit>> {
    let settings = engine.settings();
    let kinds = if kinds.is_empty() { &settings.privacy.redact_kinds[..] } else { kinds };
    if kinds.is_empty() {
        return Ok(Vec::new());
    }
    let lines: Vec<LineWords> = engine
        .ocr(img)?
        .lines
        .into_iter()
        .map(|line| LineWords { words: line.words.into_iter().map(|w| (w.text, w.rect)).collect() })
        .collect();
    Ok(snap_redact::locate(&lines, kinds))
}

/// Detect (`kinds` empty = settings) and cover every hit destructively. `style` defaults to the privacy setting.
pub(crate) fn auto_redact(engine: &Engine, img: &mut RgbaImage, kinds: &[RedactKind], style: Option<RedactStyle>) -> Result<Vec<Hit>> {
    let style = style.unwrap_or(engine.settings().privacy.redact_style);
    let hits = find_redactions(engine, img, kinds)?;
    for hit in &hits {
        snap_image::effects::redact(img, hit.rect.round(), style);
    }
    Ok(hits)
}
