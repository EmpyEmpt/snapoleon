//! Minimal single-line text for window chrome (titles, addresses), set in the shared Inter face.

use std::sync::OnceLock;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use tiny_skia::Pixmap;

use snap_core::fonts::INTER_REGULAR;

fn font() -> &'static FontRef<'static> {
    static FONT: OnceLock<FontRef<'static>> = OnceLock::new();
    FONT.get_or_init(|| FontRef::try_from_slice(INTER_REGULAR).expect("embedded Inter parses"))
}

/// `size` is the em size in pixels.
fn scale(size: f32) -> PxScale {
    let f = font();
    PxScale::from(size * f.height_unscaled() / f.units_per_em().unwrap_or(2048.0))
}

pub(crate) fn width(text: &str, size: f32) -> f32 {
    let scaled = font().as_scaled(scale(size));
    let mut w = 0.0;
    let mut prev = None;
    for c in text.chars() {
        let id = scaled.glyph_id(c);
        if let Some(p) = prev {
            w += scaled.kern(p, id);
        }
        w += scaled.h_advance(id);
        prev = Some(id);
    }
    w
}

/// Shortens `text` with an ellipsis until it fits `max_w`.
pub(crate) fn ellipsize(text: &str, size: f32, max_w: f32) -> String {
    if width(text, size) <= max_w {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let candidate: String = chars.iter().collect::<String>().trim_end().to_string() + "\u{2026}";
        if width(&candidate, size) <= max_w {
            return candidate;
        }
    }
    String::new()
}

/// Draws `text` with its baseline at `y` starting at `x`; returns the advance width.
pub(crate) fn draw(pm: &mut Pixmap, text: &str, x: f32, y: f32, size: f32, color: [u8; 4]) -> f32 {
    let f = font();
    let scaled = f.as_scaled(scale(size));
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let mut caret = x;
    let mut prev = None;
    for c in text.chars() {
        let id = scaled.glyph_id(c);
        if let Some(p) = prev {
            caret += scaled.kern(p, id);
        }
        let glyph = id.with_scale_and_position(scaled.scale(), point(caret, y));
        if let Some(outlined) = f.outline_glyph(glyph) {
            let b = outlined.px_bounds();
            let data = pm.data_mut();
            outlined.draw(|gx, gy, cov| {
                let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                if px < 0 || py < 0 || px >= pw || py >= ph {
                    return;
                }
                let sa = cov * color[3] as f32 / 255.0;
                let i = (py as usize * pw as usize + px as usize) * 4;
                for k in 0..3 {
                    let src = color[k] as f32 * sa;
                    data[i + k] = (src + data[i + k] as f32 * (1.0 - sa)).round().min(255.0) as u8;
                }
                data[i + 3] = (sa * 255.0 + data[i + 3] as f32 * (1.0 - sa)).round().min(255.0) as u8;
            });
        }
        caret += scaled.h_advance(id);
        prev = Some(id);
    }
    caret - x
}
