//! Image work: decoding uploads, thumbnails and the 1200x630 link-preview card.

use std::io::Cursor;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use anyhow::{Context, Result};
use image::{
    DynamicImage, ImageEncoder, ImageReader, Limits, Rgba, RgbaImage,
    codecs::{jpeg::JpegEncoder, png::PngEncoder},
    imageops::{self, FilterType},
};

pub const OG_WIDTH: u32 = 1200;
pub const OG_HEIGHT: u32 = 630;
const THUMB_SIDE: u32 = 480;

const BG_TOP: [u8; 3] = [0x0E, 0x0F, 0x13];
const BG_BOTTOM: [u8; 3] = [0x1B, 0x1E, 0x27];
const SURFACE: [u8; 3] = [0x1D, 0x20, 0x28];
const BORDER: [u8; 3] = [0x2C, 0x30, 0x3B];
const TEXT: [u8; 3] = [0xEC, 0xED, 0xEF];
const TEXT_DIM: [u8; 3] = [0x9A, 0xA0, 0xAC];
const ACCENT: [u8; 3] = [0x6B, 0x7F, 0xFF];

static INTER_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
static INTER_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");
static INTER_BOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-Bold.ttf");
static MONO: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Medium.ttf");

/// Decode an upload with allocation and dimension limits (first frame for animations).
pub fn decode(bytes: &[u8]) -> Result<RgbaImage> {
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(768 * 1024 * 1024);
    reader.limits(limits);
    Ok(reader.decode().context("decoding image")?.to_rgba8())
}

/// Flatten transparency onto white and encode a small JPEG for lists and previews.
pub fn thumbnail_jpeg(img: &RgbaImage) -> Result<Vec<u8>> {
    let (w, h) = fit(img.width(), img.height(), THUMB_SIDE, THUMB_SIDE);
    let small = if (w, h) == (img.width(), img.height()) { img.clone() } else { imageops::resize(img, w, h, FilterType::Triangle) };
    let mut rgb = image::RgbImage::new(w, h);
    for (x, y, p) in small.enumerate_pixels() {
        let a = f32::from(p[3]) / 255.0;
        let mix = |c: u8| (f32::from(c) * a + 255.0 * (1.0 - a)).round() as u8;
        rgb.put_pixel(x, y, image::Rgb([mix(p[0]), mix(p[1]), mix(p[2])]));
    }
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, 82).encode_image(&DynamicImage::ImageRgb8(rgb))?;
    Ok(out)
}

pub fn encode_png(img: &RgbaImage) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    PngEncoder::new(&mut out).write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgba8)?;
    Ok(out)
}

/// Largest size within `max_w` x `max_h` that keeps the aspect ratio (never upscales).
fn fit(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let scale = (max_w as f32 / w as f32).min(max_h as f32 / h as f32).min(1.0);
    (((w as f32 * scale).round() as u32).max(1), ((h as f32 * scale).round() as u32).max(1))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Hero {
    Image,
    Video,
    Locked,
    Brand,
}

pub struct Card<'a> {
    pub hero: Hero,
    pub image: Option<&'a RgbaImage>,
    pub title: &'a str,
    pub subtitle: &'a str,
}

fn blend(px: &mut Rgba<u8>, color: [u8; 3], alpha: f32) {
    let a = alpha.clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    for i in 0..3 {
        px[i] = (f32::from(px[i]) * (1.0 - a) + f32::from(color[i]) * a).round() as u8;
    }
    px[3] = 255;
}

/// Paint `color` where the signed distance function is negative, with a 1px anti-aliased edge.
fn paint_sdf(img: &mut RgbaImage, x0: i32, y0: i32, x1: i32, y1: i32, color: [u8; 3], sdf: impl Fn(f32, f32) -> f32) {
    for y in y0.max(0)..y1.min(img.height() as i32) {
        for x in x0.max(0)..x1.min(img.width() as i32) {
            let d = sdf(x as f32 + 0.5, y as f32 + 0.5);
            blend(img.get_pixel_mut(x as u32, y as u32), color, (0.5 - d).clamp(0.0, 1.0));
        }
    }
}

fn rounded_rect_sdf(x: f32, y: f32, w: f32, h: f32, r: f32) -> impl Fn(f32, f32) -> f32 {
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    move |px, py| {
        let (dx, dy) = ((px - cx).abs() - (w / 2.0 - r), (py - cy).abs() - (h / 2.0 - r));
        (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt() + dx.max(dy).min(0.0) - r
    }
}

fn fill_rounded(img: &mut RgbaImage, x: f32, y: f32, w: f32, h: f32, r: f32, color: [u8; 3]) {
    paint_sdf(img, x as i32 - 1, y as i32 - 1, (x + w) as i32 + 2, (y + h) as i32 + 2, color, rounded_rect_sdf(x, y, w, h, r));
}

fn background() -> RgbaImage {
    let mut img = RgbaImage::new(OG_WIDTH, OG_HEIGHT);
    for y in 0..OG_HEIGHT {
        let t = y as f32 / (OG_HEIGHT - 1) as f32;
        for x in 0..OG_WIDTH {
            let mut px = Rgba([0, 0, 0, 255]);
            for i in 0..3 {
                px[i] = (f32::from(BG_TOP[i]) * (1.0 - t) + f32::from(BG_BOTTOM[i]) * t).round() as u8;
            }
            // Soft accent glow in the top-right corner.
            let (dx, dy) = (x as f32 - 1120.0, y as f32 - 20.0);
            let glow = (1.0 - (dx * dx + dy * dy).sqrt() / 620.0).clamp(0.0, 1.0);
            blend(&mut px, ACCENT, glow * glow * 0.20);
            img.put_pixel(x, y, px);
        }
    }
    img
}

fn text_width(font: &FontRef, size: f32, text: &str) -> f32 {
    let scaled = font.as_scaled(PxScale::from(size));
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

/// Draw one line with its baseline at `y`; returns the advance width.
fn draw_text(img: &mut RgbaImage, font: &FontRef, size: f32, x: f32, y: f32, color: [u8; 3], text: &str) -> f32 {
    let scaled = font.as_scaled(PxScale::from(size));
    let mut cursor = x;
    let mut prev = None;
    for c in text.chars() {
        let mut glyph = scaled.scaled_glyph(c);
        if let Some(p) = prev {
            cursor += scaled.kern(p, glyph.id);
        }
        glyph.position = point(cursor, y);
        cursor += scaled.h_advance(glyph.id);
        prev = Some(glyph.id);
        if let Some(outlined) = font.outline_glyph(glyph) {
            let b = outlined.px_bounds();
            outlined.draw(|gx, gy, cov| {
                let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                if px >= 0 && py >= 0 && (px as u32) < img.width() && (py as u32) < img.height() {
                    blend(img.get_pixel_mut(px as u32, py as u32), color, cov);
                }
            });
        }
    }
    cursor - x
}

/// Shorten `s` with an ellipsis until it fits `max_w`; `force` appends one even if it already fits.
fn shorten(font: &FontRef, size: f32, s: &str, max_w: f32, force: bool) -> String {
    if !force && text_width(font, size, s) <= max_w {
        return s.to_string();
    }
    let mut chars: Vec<char> = s.chars().collect();
    loop {
        let candidate = chars.iter().collect::<String>().trim_end().to_string() + "\u{2026}";
        if text_width(font, size, &candidate) <= max_w || chars.is_empty() {
            return candidate;
        }
        chars.pop();
    }
}

/// Greedy word wrap into at most `max_lines`, ellipsising whatever does not fit.
fn wrap(font: &FontRef, size: f32, text: &str, max_w: f32, max_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    let mut overflow = false;
    for word in text.split_whitespace() {
        let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if line.is_empty() || text_width(font, size, &candidate) <= max_w {
            line = candidate;
            continue;
        }
        if lines.len() + 1 == max_lines {
            overflow = true;
            break;
        }
        lines.push(std::mem::replace(&mut line, word.to_string()));
    }
    if !line.is_empty() {
        lines.push(line);
    }
    let last = lines.len().saturating_sub(1);
    lines.into_iter().enumerate().map(|(i, l)| shorten(font, size, &l, max_w, overflow && i == last)).collect()
}

fn draw_logo(img: &mut RgbaImage, x: f32, y: f32, size: f32) {
    fill_rounded(img, x, y, size, size, size * 0.26, ACCENT);
    // Capture-bracket corners.
    let (m, arm, t) = (size * 0.24, size * 0.22, (size * 0.07).max(2.0));
    let white = [255, 255, 255];
    for (cx, cy, sx, sy) in [
        (x + m, y + m, 1.0, 1.0),
        (x + size - m, y + m, -1.0, 1.0),
        (x + m, y + size - m, 1.0, -1.0),
        (x + size - m, y + size - m, -1.0, -1.0),
    ] {
        let (hx, hy) = (if sx > 0.0 { cx } else { cx - arm }, if sy > 0.0 { cy } else { cy - t });
        fill_rounded(img, hx, hy, arm, t, t / 2.0, white);
        let (vx, vy) = (if sx > 0.0 { cx } else { cx - t }, if sy > 0.0 { cy } else { cy - arm });
        fill_rounded(img, vx, vy, t, arm, t / 2.0, white);
    }
}

fn draw_shadow(img: &mut RgbaImage, x: f32, y: f32, w: f32, h: f32, r: f32) {
    let pad = 60;
    let (lx, ly) = ((x as i32 - pad).max(0), (y as i32 - pad).max(0));
    let (lw, lh) = ((w as i32 + 2 * pad).min(OG_WIDTH as i32 - lx) as u32, (h as i32 + 2 * pad + 14).min(OG_HEIGHT as i32 - ly) as u32);
    let mut layer = RgbaImage::new(lw, lh);
    let sdf = rounded_rect_sdf(x - lx as f32, y - ly as f32 + 14.0, w, h, r);
    for py in 0..lh {
        for px in 0..lw {
            let a = (0.5 - sdf(px as f32 + 0.5, py as f32 + 0.5)).clamp(0.0, 1.0);
            layer.put_pixel(px, py, Rgba([0, 0, 0, (a * 255.0) as u8]));
        }
    }
    let blurred = imageops::blur(&layer, 18.0);
    for py in 0..lh {
        for px in 0..lw {
            let a = f32::from(blurred.get_pixel(px, py)[3]) / 255.0 * 0.55;
            blend(img.get_pixel_mut(lx as u32 + px, ly as u32 + py), [0, 0, 0], a);
        }
    }
}

fn draw_hero(img: &mut RgbaImage, card: &Card, area: (f32, f32, f32, f32)) {
    let (ax, ay, aw, ah) = area;
    match (card.hero, card.image) {
        (Hero::Image, Some(src)) => {
            let (w, h) = fit(src.width(), src.height(), aw as u32, ah as u32);
            // Upscale small captures only up to 2x so icons do not turn into blobs.
            let (w, h) = if w < aw as u32 && h < ah as u32 {
                let k = (aw / w as f32).min(ah / h as f32).min(2.0);
                ((w as f32 * k) as u32, (h as f32 * k) as u32)
            } else {
                (w, h)
            };
            let scaled = imageops::resize(src, w, h, FilterType::Lanczos3);
            let (x, y) = (ax + (aw - w as f32) / 2.0, ay + (ah - h as f32) / 2.0);
            draw_shadow(img, x, y, w as f32, h as f32, 14.0);
            fill_rounded(img, x - 1.0, y - 1.0, w as f32 + 2.0, h as f32 + 2.0, 15.0, BORDER);
            let sdf = rounded_rect_sdf(x, y, w as f32, h as f32, 14.0);
            for sy in 0..h {
                for sx in 0..w {
                    let (px, py) = (x as i32 + sx as i32, y as i32 + sy as i32);
                    let coverage = (0.5 - sdf(px as f32 + 0.5, py as f32 + 0.5)).clamp(0.0, 1.0);
                    let s = scaled.get_pixel(sx, sy);
                    // Transparent captures are composited over the card surface colour.
                    let mut base = Rgba([SURFACE[0], SURFACE[1], SURFACE[2], 255]);
                    blend(&mut base, [s[0], s[1], s[2]], f32::from(s[3]) / 255.0);
                    let dst = img.get_pixel_mut(px as u32, py as u32);
                    blend(dst, [base[0], base[1], base[2]], coverage);
                }
            }
        }
        _ => {
            draw_shadow(img, ax, ay, aw, ah, 14.0);
            fill_rounded(img, ax - 1.0, ay - 1.0, aw + 2.0, ah + 2.0, 15.0, BORDER);
            fill_rounded(img, ax, ay, aw, ah, 14.0, SURFACE);
            let (cx, cy) = (ax + aw / 2.0, ay + ah / 2.0);
            match card.hero {
                Hero::Video => {
                    let r = 62.0;
                    paint_sdf(img, (cx - r) as i32 - 2, (cy - r) as i32 - 2, (cx + r) as i32 + 2, (cy + r) as i32 + 2, ACCENT, |x, y| {
                        ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r
                    });
                    // Play triangle: edge distances of a right-pointing triangle.
                    let (p0, p1, p2) = ((cx - 16.0, cy - 28.0), (cx - 16.0, cy + 28.0), (cx + 30.0, cy));
                    paint_sdf(
                        img,
                        (cx - 40.0) as i32,
                        (cy - 40.0) as i32,
                        (cx + 40.0) as i32,
                        (cy + 40.0) as i32,
                        [255, 255, 255],
                        |x, y| triangle_sdf((x, y), p0, p1, p2),
                    );
                }
                Hero::Locked => {
                    let (bw, bh) = (92.0, 70.0);
                    let (bx, by) = (cx - bw / 2.0, cy - 8.0);
                    // Shackle: an outlined rounded rectangle whose lower half the body covers.
                    let shackle = rounded_rect_sdf(cx - 28.0, by - 46.0, 56.0, 70.0, 28.0);
                    paint_sdf(img, (cx - 36.0) as i32, (by - 52.0) as i32, (cx + 36.0) as i32, (by + 30.0) as i32, TEXT_DIM, |x, y| {
                        shackle(x, y).abs() - 5.0
                    });
                    fill_rounded(img, bx, by, bw, bh, 12.0, TEXT_DIM);
                    paint_sdf(img, (cx - 8.0) as i32, (by + 18.0) as i32, (cx + 8.0) as i32, (by + 52.0) as i32, SURFACE, |x, y| {
                        let dot = ((x - cx).powi(2) + (y - (by + 28.0)).powi(2)).sqrt() - 7.0;
                        let stem = rounded_rect_sdf(cx - 3.0, by + 30.0, 6.0, 20.0, 3.0)(x, y);
                        dot.min(stem)
                    });
                }
                _ => {
                    draw_logo(img, cx - 48.0, cy - 48.0, 96.0);
                }
            }
        }
    }
}

fn triangle_sdf(p: (f32, f32), a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> f32 {
    let edge = |p: (f32, f32), v0: (f32, f32), v1: (f32, f32)| {
        let (ex, ey) = (v1.0 - v0.0, v1.1 - v0.1);
        let (dx, dy) = (p.0 - v0.0, p.1 - v0.1);
        let t = ((dx * ex + dy * ey) / (ex * ex + ey * ey)).clamp(0.0, 1.0);
        let (qx, qy) = (dx - ex * t, dy - ey * t);
        (qx * qx + qy * qy).sqrt()
    };
    let d = edge(p, a, b).min(edge(p, b, c)).min(edge(p, c, a));
    let cross = |v0: (f32, f32), v1: (f32, f32)| (v1.0 - v0.0) * (p.1 - v0.1) - (v1.1 - v0.1) * (p.0 - v0.0);
    let (s1, s2, s3) = (cross(a, b), cross(b, c), cross(c, a));
    let inside = (s1 >= 0.0 && s2 >= 0.0 && s3 >= 0.0) || (s1 <= 0.0 && s2 <= 0.0 && s3 <= 0.0);
    if inside { -d } else { d }
}

/// Render the 1200x630 preview card.
pub fn render_card(card: &Card) -> Result<RgbaImage> {
    let regular = FontRef::try_from_slice(INTER_REGULAR)?;
    let semibold = FontRef::try_from_slice(INTER_SEMIBOLD)?;
    let bold = FontRef::try_from_slice(INTER_BOLD)?;
    let mono = FontRef::try_from_slice(MONO)?;

    let mut img = background();
    let margin = 64.0;
    let content_w = OG_WIDTH as f32 - 2.0 * margin;

    if card.hero == Hero::Brand {
        draw_logo(&mut img, margin, margin, 56.0);
        draw_text(&mut img, &bold, 34.0, margin + 76.0, margin + 40.0, TEXT, "Snapoleon");
        let lines = wrap(&semibold, 64.0, card.title, content_w, 3);
        for (i, line) in lines.iter().enumerate() {
            draw_text(&mut img, &semibold, 64.0, margin, 300.0 + i as f32 * 78.0, TEXT, line);
        }
        draw_text(&mut img, &regular, 28.0, margin, 560.0, TEXT_DIM, card.subtitle);
        return Ok(img);
    }

    draw_hero(&mut img, card, (margin, 44.0, content_w, 372.0));

    let lines = wrap(&semibold, 40.0, card.title, content_w, 2);
    for (i, line) in lines.iter().enumerate() {
        draw_text(&mut img, &semibold, 40.0, margin, 474.0 + i as f32 * 48.0, TEXT, line);
    }
    let footer_y = 586.0;
    let brand = "Snapoleon";
    let brand_w = text_width(&bold, 26.0, brand);
    let brand_x = OG_WIDTH as f32 - margin - brand_w;
    draw_logo(&mut img, brand_x - 46.0, footer_y - 28.0, 34.0);
    draw_text(&mut img, &bold, 26.0, brand_x, footer_y - 2.0, TEXT, brand);
    let sub_max = brand_x - 46.0 - margin - 24.0;
    let sub = wrap(&mono, 22.0, card.subtitle, sub_max, 1);
    if let Some(line) = sub.first() {
        draw_text(&mut img, &mono, 22.0, margin, footer_y - 4.0, TEXT_DIM, line);
    }
    Ok(img)
}
