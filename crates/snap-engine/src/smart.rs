//! AI-assisted extras: redaction suggestions on top of pattern detection, and titles for file names and links.

use std::sync::Mutex;

use snap_core::{Capture, CaptureId, Result, RgbaImage, settings::RedactKind};
use snap_redact::LineWords;

use crate::{Engine, Hit};

/// Share of the smaller box an AI hit must cover of a pattern hit to count as the same finding.
const SAME_FINDING: f32 = 0.5;

impl Engine {
    /// [`find_redactions`](Engine::find_redactions) plus what the AI endpoint flags in the recognised text, such as
    /// names and addresses that no pattern matches. The AI part is best effort: when the endpoint fails, the pattern
    /// hits are returned alone. Sends the recognised text to the configured endpoint.
    pub fn find_redactions_smart(&self, img: &RgbaImage, kinds: &[RedactKind]) -> Result<Vec<Hit>> {
        let settings = self.settings();
        let kinds = if kinds.is_empty() { &settings.privacy.redact_kinds[..] } else { kinds };
        let lines: Vec<LineWords> = self
            .ocr(img)?
            .lines
            .into_iter()
            .map(|line| LineWords { words: line.words.into_iter().map(|w| (w.text, w.rect)).collect() })
            .collect();
        let mut hits = snap_redact::locate(&lines, kinds);
        let ai = self.ai();
        if !ai.available() {
            return Ok(hits);
        }
        let text = lines.iter().map(|l| l.words.iter().map(|w| w.0.as_str()).collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join("\n");
        match ai.suggest_redactions(&text) {
            Ok(needles) => {
                let known = hits.clone();
                hits.extend(
                    snap_redact::locate_substrings(&lines, &needles)
                        .into_iter()
                        .filter(|h| !known.iter().any(|k| overlap(k.rect, h.rect) > SAME_FINDING)),
                );
            }
            Err(e) => tracing::warn!("AI redaction suggestions failed, keeping pattern hits only: {e}"),
        }
        Ok(hits)
    }
}

/// Share of the smaller rectangle covered by the other.
fn overlap(a: snap_core::RectF, b: snap_core::RectF) -> f32 {
    let w = (a.right().min(b.right()) - a.x.max(b.x)).max(0.0);
    let h = (a.bottom().min(b.bottom()) - a.y.max(b.y)).max(0.0);
    let smaller = (a.w * a.h).min(b.w * b.h);
    if smaller <= 0.0 { 0.0 } else { w * h / smaller }
}

/// The AI title of `cap` when "AI names" is on and the endpoint answers, else `None`. A save followed by an upload of
/// the same capture asks the model once.
pub(crate) fn title(engine: &Engine, img: &RgbaImage, cap: &Capture) -> Option<String> {
    static LAST: Mutex<Option<(CaptureId, String)>> = Mutex::new(None);
    if !engine.settings().ai.smart_names {
        return None;
    }
    if let Some((id, title)) = LAST.lock().ok()?.as_ref()
        && *id == cap.id
    {
        return Some(title.clone());
    }
    let ai = engine.ai();
    if !ai.available() {
        return None;
    }
    match ai.title(Some(img), None, &cap.context) {
        Ok(title) => {
            if let Ok(mut last) = LAST.lock() {
                *last = Some((cap.id.clone(), title.clone()));
            }
            Some(title)
        }
        Err(e) => {
            tracing::warn!("AI title failed, using the template name: {e}");
            None
        }
    }
}
