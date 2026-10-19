//! Dev harness hooks (`SNAPOLEON_DEV=editor`): draw sample annotations and open a chosen state so that
//! screenshots show the editor the way it looks in use. Nothing here runs outside the harness.
//!
//! - `SNAPOLEON_DEV_EDITOR_DEMO=1`: add sample shapes.
//! - `SNAPOLEON_DEV_EDITOR_TOOL=<id>`: the active tool (`arrow`, `text`, `crop`, ...).
//! - `SNAPOLEON_DEV_EDITOR_SELECT=last|all`: select shapes (with the select tool).
//! - `SNAPOLEON_DEV_EDITOR_TAB=beautify|text|export`: the panel tab; `SNAPOLEON_DEV_EDITOR_PANEL=0` hides it.
//! - `SNAPOLEON_DEV_EDITOR_BEAUTIFY=1`: turn on the first beautify preset.
//! - `SNAPOLEON_DEV_EDITOR_EDIT_TEXT=1`: start editing the first text shape.
//! - `SNAPOLEON_DEV_EDITOR_RUN=ocr|redact`: run text recognition or auto-redact on the first frame.

use snap_annotate::{ArrowHead, ShapeKind, Stamp, Style, TextBg};
use snap_core::{PointF, RectF, settings::RedactStyle};

use super::{EditorView, panel::Tab, tools::Tool};
use crate::views::ViewCx;

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// Set the editor up as the environment asks. Does nothing outside `SNAPOLEON_DEV=editor`.
pub fn apply(view: &mut EditorView) {
    if var("SNAPOLEON_DEV").as_deref() != Some("editor") {
        return;
    }
    if var("SNAPOLEON_DEV_EDITOR_DEMO").is_some() {
        sample_shapes(view);
    }
    if var("SNAPOLEON_DEV_EDITOR_BEAUTIFY").is_some() {
        view.s.doc.beautify = Some(snap_core::settings::default_beautify_presets().remove(0));
        view.s.touch();
    }
    if let Some(tab) = var("SNAPOLEON_DEV_EDITOR_TAB").as_deref().and_then(Tab::from_name) {
        view.panel.tab = tab;
    }
    if var("SNAPOLEON_DEV_EDITOR_PANEL").as_deref() == Some("0") {
        view.panel.open = false;
    }
    if let Some(tool) = var("SNAPOLEON_DEV_EDITOR_TOOL").as_deref().and_then(Tool::from_id) {
        view.set_tool(tool);
    }
    match var("SNAPOLEON_DEV_EDITOR_SELECT").as_deref() {
        Some("all") => {
            view.set_tool(Tool::Select);
            view.s.select_all();
        }
        Some("last") => {
            view.set_tool(Tool::Select);
            if let Some(id) = view.s.doc.shapes.last().map(|s| s.id) {
                view.s.select_only(id);
            }
        }
        _ => {}
    }
    if var("SNAPOLEON_DEV_EDITOR_EDIT_TEXT").is_some()
        && let Some(id) = view.s.doc.shapes.iter().find(|s| matches!(s.kind, ShapeKind::Text { .. })).map(|s| s.id)
    {
        view.s.select_only(id);
        view.canvas.begin_text(&view.s, id, None, None, false);
    }
}

/// Work that needs the window context, run once on the first frame.
pub fn first_frame(view: &mut EditorView, cx: &mut ViewCx) {
    if var("SNAPOLEON_DEV").as_deref() != Some("editor") {
        return;
    }
    match var("SNAPOLEON_DEV_EDITOR_RUN").as_deref() {
        Some("redact") => view.auto_redact(cx),
        Some("ocr") => view.panel.run_ocr(&view.s, cx),
        _ => {}
    }
}

fn sample_shapes(view: &mut EditorView) {
    let (w, h) = view.s.doc.canvas_size();
    let (w, h) = (w as f32, h as f32);
    let k = view.ui_scale;
    let at = |fx: f32, fy: f32| PointF::new(w * fx, h * fy);
    let styles = &view.tools.styles;
    let style = |tool: Tool| styles.get(&tool).copied().unwrap_or_default();
    let s = &mut view.s;

    s.add(
        ShapeKind::Rect { rect: RectF::new(w * 0.10, h * 0.16, w * 0.30, h * 0.20), radius: 10.0 * k },
        Style { stroke: [255, 59, 48, 255], ..style(Tool::Rect) },
    );
    s.add(
        ShapeKind::Arrow { from: at(0.44, 0.30), to: at(0.53, 0.40), ctrl: Some(at(0.50, 0.28)), head: ArrowHead::Triangle, double: false },
        style(Tool::Arrow),
    );
    s.add(
        ShapeKind::Text {
            pos: at(0.43, 0.20),
            text: "Check this panel".into(),
            size: 30.0 * k,
            bold: true,
            bg: TextBg::Solid,
            max_width: None,
        },
        Style { stroke: [10, 132, 255, 255], ..style(Tool::Text) },
    );
    s.add(
        ShapeKind::Ellipse { rect: RectF::new(w * 0.58, h * 0.14, w * 0.14, h * 0.18) },
        Style { stroke: [52, 199, 89, 255], width: 5.0 * k, ..style(Tool::Ellipse) },
    );
    for (n, (fx, fy)) in [(0.16, 0.50), (0.30, 0.56), (0.44, 0.62)].into_iter().enumerate() {
        s.add(ShapeKind::Step { center: at(fx, fy), number: n as u32 + 1, radius: 18.0 * k }, style(Tool::Step));
    }
    s.add(
        ShapeKind::Redact { rect: RectF::new(w * 0.60, h * 0.50, w * 0.22, h * 0.07), style: RedactStyle::Pixelate },
        style(Tool::Redact),
    );
    s.add(
        ShapeKind::Highlighter {
            points: (0..=24).map(|i| PointF::new(w * (0.10 + 0.012 * i as f32), h * 0.74 + (i as f32 * 0.7).sin() * 3.0 * k)).collect(),
        },
        style(Tool::Highlighter),
    );
    s.add(
        ShapeKind::Callout {
            rect: RectF::new(w * 0.62, h * 0.66, w * 0.20, h * 0.12),
            tail: at(0.60, 0.60),
            text: "Looks fine now".into(),
            size: 24.0 * k,
        },
        Style { stroke: [255, 149, 0, 255], ..style(Tool::Callout) },
    );
    s.add(
        ShapeKind::Stamp { center: at(0.88, 0.20), size: 64.0 * k, glyph: Stamp::Check },
        Style { stroke: Stamp::Check.default_color(), ..style(Tool::Stamp) },
    );
    s.add(ShapeKind::Measure { from: at(0.10, 0.88), to: at(0.34, 0.88) }, style(Tool::Measure));
    s.touch();
    s.select_only(s.doc.shapes[1].id);
}
