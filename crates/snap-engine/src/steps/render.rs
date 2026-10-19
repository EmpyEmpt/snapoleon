//! Drawing for step-capture exports: click rings, caption bars and the title card of the slideshow.

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use snap_core::{Point, RgbaImage};
use tiny_skia::{Color, FillRule, Paint, Path, PathBuilder, Pixmap, Stroke, Transform};

use snap_core::fonts::{INTER_BOLD, INTER_REGULAR, INTER_SEMIBOLD};

/// Imperial blue from the design tokens, readable on both light and dark screenshots.
const ACCENT: [u8; 3] = [0x3D, 0x5A, 0xFE];
const SCRIM: [u8; 3] = [0x0E, 0x0F, 0x13];
const TEXT: [u8; 4] = [0xEC, 0xED, 0xEF, 0xFF];
const TEXT_DIM: [u8; 4] = [0x9A, 0xA0, 0xAC, 0xFF];
/// Bézier constant for a quarter circle.
const KAPPA: f32 = 0.552_284_8;

fn font(data: &'static [u8]) -> FontRef<'static> {
    FontRef::try_from_slice(data).expect("the bundled Inter font is valid")
}

// --- click ring ---------------------------------------------------------------------------------------------------

/// `img` with a ring around the click point; the image is returned unchanged without a click.
pub(super) fn with_click_ring(img: &RgbaImage, click: Option<Point>) -> RgbaImage {
    let mut out = img.clone();
    if let Some(at) = click {
        draw_click_ring(&mut out, at);
    }
    out
}

fn draw_click_ring(img: &mut RgbaImage, at: Point) {
    let reference = img.width().min(img.height()) as f32;
    let radius = (reference * 0.028).clamp(16.0, 44.0);
    let stroke = (radius * 0.16).max(3.0);
    let pad = radius + stroke * 2.0 + 4.0;
    let side = (pad * 2.0).ceil() as u32;
    let Some(mut layer) = Pixmap::new(side, side) else { return };
    let centre = side as f32 / 2.0;
    let accent = |a: f32| Color::from_rgba8(ACCENT[0], ACCENT[1], ACCENT[2], (a * 255.0) as u8);
    let white = |a: f32| Color::from_rgba8(255, 255, 255, (a * 255.0) as u8);
    if let Some(disc) = PathBuilder::from_circle(centre, centre, radius) {
        fill_path(&mut layer, &disc, accent(0.2));
        stroke_path(&mut layer, &disc, white(0.95), stroke + 4.0);
        stroke_path(&mut layer, &disc, accent(1.0), stroke);
    }
    if let Some(dot) = PathBuilder::from_circle(centre, centre, stroke * 0.9) {
        stroke_path(&mut layer, &dot, white(0.95), 3.0);
        fill_path(&mut layer, &dot, accent(1.0));
    }
    overlay(img, &layer, at.x - centre as i32, at.y - centre as i32);
}

// --- slides -----------------------------------------------------------------------------------------------------

/// A screenshot slide: the picture with a rounded caption bar and a numbered badge along the bottom.
pub(super) fn caption_slide(img: &RgbaImage, caption: &str, number: usize) -> RgbaImage {
    let mut out = img.clone();
    let (w, h) = (out.width() as f32, out.height() as f32);
    let (regular, bold) = (font(INTER_SEMIBOLD), font(INTER_BOLD));
    let size = (h * 0.034).clamp(16.0, 54.0);
    let (pad_x, pad_y) = (size * 0.9, size * 0.7);
    let badge = size * 1.7;
    let gap = size * 0.8;
    let text_max = (w * 0.84 - 2.0 * pad_x - badge - gap).max(size * 4.0);
    let lines = wrap(&regular, size, caption, text_max);
    let line_h = size * 1.3;
    let text_w = lines.iter().map(|l| text_width(&regular, size, l)).fold(0.0, f32::max);
    let bar_w = (2.0 * pad_x + badge + gap + text_w).min(w - 8.0);
    let bar_h = (2.0 * pad_y + (lines.len() as f32 * line_h).max(badge * 0.8)).min(h - 8.0);
    let (bar_x, bar_y) = ((w - bar_w) / 2.0, h - bar_h - (h * 0.04).max(12.0));

    let pad = 2.0;
    if let Some(mut layer) = Pixmap::new((bar_w + 2.0 * pad).ceil() as u32, (bar_h + 2.0 * pad).ceil() as u32) {
        if let Some(bar) = rounded_rect(pad, pad, bar_w, bar_h, (bar_h / 2.0).min(size * 1.2)) {
            fill_path(&mut layer, &bar, Color::from_rgba8(SCRIM[0], SCRIM[1], SCRIM[2], 232));
        }
        if let Some(disc) = PathBuilder::from_circle(pad + pad_x * 0.55 + badge / 2.0, pad + bar_h / 2.0, badge / 2.0) {
            fill_path(&mut layer, &disc, Color::from_rgba8(ACCENT[0], ACCENT[1], ACCENT[2], 255));
        }
        overlay(&mut out, &layer, (bar_x - pad) as i32, (bar_y - pad) as i32);
    }
    let badge_cx = bar_x + pad_x * 0.55 + badge / 2.0;
    let number = number.to_string();
    let badge_size = badge * 0.5;
    draw_text(
        &mut out,
        &bold,
        badge_size,
        badge_cx - text_width(&bold, badge_size, &number) / 2.0,
        bar_y + bar_h / 2.0 + badge_size * 0.36,
        [255; 4],
        &number,
    );
    let text_x = bar_x + pad_x * 0.55 + badge + gap;
    let first_baseline = bar_y + (bar_h - lines.len() as f32 * line_h) / 2.0 + size * 1.0;
    for (i, line) in lines.iter().enumerate() {
        draw_text(&mut out, &regular, size, text_x, first_baseline + i as f32 * line_h, TEXT, line);
    }
    out
}

/// Opening slide of the video: headline and intro on the app's dark background.
pub(super) fn title_card(w: u32, h: u32, title: &str, intro: &str) -> RgbaImage {
    let mut out = RgbaImage::from_pixel(w, h, image::Rgba([SCRIM[0], SCRIM[1], SCRIM[2], 255]));
    let (bold, regular) = (font(INTER_BOLD), font(INTER_REGULAR));
    let margin = w as f32 * 0.09;
    let max_w = w as f32 - 2.0 * margin;
    let title_size = (w as f32 * 0.052).clamp(28.0, 96.0);
    let intro_size = title_size * 0.42;
    let title_lines = wrap(&bold, title_size, title, max_w);
    let intro_lines = wrap(&regular, intro_size, intro, max_w);
    let (title_h, intro_h) = (title_lines.len() as f32 * title_size * 1.18, intro_lines.len() as f32 * intro_size * 1.5);
    let gap = if intro_lines.is_empty() { 0.0 } else { title_size * 0.5 };
    let top = (h as f32 - title_h - gap - intro_h) / 2.0;

    // A short accent bar above the headline, the one flourish.
    if let Some(mut layer) = Pixmap::new((title_size * 1.6).ceil() as u32, (title_size * 0.14).ceil().max(4.0) as u32)
        && let Some(bar) = rounded_rect(0.0, 0.0, layer.width() as f32, layer.height() as f32, layer.height() as f32 / 2.0)
    {
        fill_path(&mut layer, &bar, Color::from_rgba8(ACCENT[0], ACCENT[1], ACCENT[2], 255));
        overlay(&mut out, &layer, margin as i32, (top - title_size * 0.6) as i32);
    }
    for (i, line) in title_lines.iter().enumerate() {
        draw_text(&mut out, &bold, title_size, margin, top + title_size * 0.95 + i as f32 * title_size * 1.18, TEXT, line);
    }
    let intro_top = top + title_h + gap;
    for (i, line) in intro_lines.iter().enumerate() {
        draw_text(&mut out, &regular, intro_size, margin, intro_top + intro_size * 1.1 + i as f32 * intro_size * 1.5, TEXT_DIM, line);
    }
    out
}

// --- primitives ---------------------------------------------------------------------------------------------------

fn paint(color: Color) -> Paint<'static> {
    let mut paint = Paint { anti_alias: true, ..Paint::default() };
    paint.set_color(color);
    paint
}

fn fill_path(layer: &mut Pixmap, path: &Path, color: Color) {
    layer.fill_path(path, &paint(color), FillRule::Winding, Transform::identity(), None);
}

fn stroke_path(layer: &mut Pixmap, path: &Path, color: Color, width: f32) {
    layer.stroke_path(path, &paint(color), &Stroke { width, ..Stroke::default() }, Transform::identity(), None);
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let k = r * KAPPA;
    let (x1, y1) = (x + w, y + h);
    let mut p = PathBuilder::new();
    p.move_to(x + r, y);
    p.line_to(x1 - r, y);
    p.cubic_to(x1 - r + k, y, x1, y + r - k, x1, y + r);
    p.line_to(x1, y1 - r);
    p.cubic_to(x1, y1 - r + k, x1 - r + k, y1, x1 - r, y1);
    p.line_to(x + r, y1);
    p.cubic_to(x + r - k, y1, x, y1 - r + k, x, y1 - r);
    p.line_to(x, y + r);
    p.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    p.close();
    p.finish()
}

/// Source-over a premultiplied layer onto a straight-alpha image at (`ox`, `oy`), clipped to the image.
fn overlay(img: &mut RgbaImage, layer: &Pixmap, ox: i32, oy: i32) {
    let (lw, lh) = (layer.width() as i32, layer.height() as i32);
    for ly in 0..lh {
        let y = oy + ly;
        if y < 0 || y >= img.height() as i32 {
            continue;
        }
        for lx in 0..lw {
            let x = ox + lx;
            if x < 0 || x >= img.width() as i32 {
                continue;
            }
            let src = layer.pixel(lx as u32, ly as u32).expect("inside the layer");
            if src.alpha() == 0 {
                continue;
            }
            let dst = img.get_pixel_mut(x as u32, y as u32);
            let (sa, da) = (f32::from(src.alpha()) / 255.0, f32::from(dst[3]) / 255.0);
            let out_a = sa + da * (1.0 - sa);
            let channel = |s: u8, d: u8| {
                let premultiplied = f32::from(s) / 255.0 + f32::from(d) / 255.0 * da * (1.0 - sa);
                ((premultiplied / out_a) * 255.0).round().clamp(0.0, 255.0) as u8
            };
            *dst = image::Rgba([
                channel(src.red(), dst[0]),
                channel(src.green(), dst[1]),
                channel(src.blue(), dst[2]),
                (out_a * 255.0).round() as u8,
            ]);
        }
    }
}

fn text_width(font: &FontRef, size: f32, text: &str) -> f32 {
    let scaled = font.as_scaled(PxScale::from(size));
    let mut width = 0.0;
    let mut previous = None;
    for c in text.chars() {
        let id = scaled.glyph_id(c);
        if let Some(prev) = previous {
            width += scaled.kern(prev, id);
        }
        width += scaled.h_advance(id);
        previous = Some(id);
    }
    width
}

/// Greedy word wrap; words longer than a line are split by character.
fn wrap(font: &FontRef, size: f32, text: &str, max_w: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if text_width(font, size, &candidate) <= max_w {
            line = candidate;
            continue;
        }
        if !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        for c in word.chars() {
            if text_width(font, size, &format!("{line}{c}")) > max_w && !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            line.push(c);
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Draw `text` with its baseline at `baseline`, blending onto the image.
fn draw_text(img: &mut RgbaImage, font: &FontRef, size: f32, x: f32, baseline: f32, color: [u8; 4], text: &str) {
    let scaled = font.as_scaled(PxScale::from(size));
    let mut pen = x;
    let mut previous = None;
    for c in text.chars() {
        let id = scaled.glyph_id(c);
        if let Some(prev) = previous {
            pen += scaled.kern(prev, id);
        }
        let glyph = id.with_scale_and_position(PxScale::from(size), point(pen, baseline));
        pen += scaled.h_advance(id);
        previous = Some(id);
        let Some(outline) = font.outline_glyph(glyph) else { continue };
        let bounds = outline.px_bounds();
        outline.draw(|gx, gy, coverage| {
            let (px, py) = (bounds.min.x as i32 + gx as i32, bounds.min.y as i32 + gy as i32);
            if px < 0 || py < 0 || px >= img.width() as i32 || py >= img.height() as i32 {
                return;
            }
            let a = coverage * f32::from(color[3]) / 255.0;
            let dst = img.get_pixel_mut(px as u32, py as u32);
            for i in 0..3 {
                dst[i] = (f32::from(color[i]) * a + f32::from(dst[i]) * (1.0 - a)).round() as u8;
            }
        });
    }
}
