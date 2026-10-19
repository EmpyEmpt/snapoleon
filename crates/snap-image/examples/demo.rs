//! Smoke test: exercises every snap-image module on this machine and writes PNGs to
//! `target/smoke/snap-image/`. Run: `cargo run -p snap-image --example demo --release`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use image::{Rgba, RgbaImage};
use snap_core::settings::{Background, BeautifyPreset, FrameStyle, ImageFormat, RedactStyle};
use snap_core::{CaptureContext, Rect};
use snap_image::combine::{Align, Layout, combine};
use snap_image::diff::{DiffMode, diff};
use snap_image::stitch::{PushOutcome, Stitcher};
use snap_image::{beautify, color, effects, encode, resize};

static INTER: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");

fn out_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/smoke/snap-image");
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn save(dir: &Path, name: &str, img: &RgbaImage) {
    let bytes = encode::encode(img, ImageFormat::Png, 100, false).unwrap();
    std::fs::write(dir.join(name), bytes).unwrap();
}

// ------------------------------------------------------------------ drawing

fn rect(img: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32, c: [u8; 4]) {
    for yy in y.max(0)..(y + h).min(img.height() as i32) {
        for xx in x.max(0)..(x + w).min(img.width() as i32) {
            img.put_pixel(xx as u32, yy as u32, Rgba(c));
        }
    }
}

/// Text with `size` = em px, baseline at `y`.
fn text(img: &mut RgbaImage, s: &str, x: f32, y: f32, size: f32, c: [u8; 4]) {
    let font = FontRef::try_from_slice(INTER).unwrap();
    let scale = PxScale::from(size * font.height_unscaled() / font.units_per_em().unwrap());
    let scaled = font.as_scaled(scale);
    let mut caret = x;
    for ch in s.chars() {
        let id = scaled.glyph_id(ch);
        let glyph = id.with_scale_and_position(scale, point(caret, y));
        if let Some(o) = font.outline_glyph(glyph) {
            let b = o.px_bounds();
            o.draw(|gx, gy, cov| {
                let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                if px >= 0 && py >= 0 && (px as u32) < img.width() && (py as u32) < img.height() {
                    let dst = img.get_pixel_mut(px as u32, py as u32);
                    for (d, s) in dst.0.iter_mut().zip(c).take(3) {
                        *d = (s as f32 * cov + *d as f32 * (1.0 - cov)).round() as u8;
                    }
                }
            });
        }
        caret += scaled.h_advance(id);
    }
}

const LOREM: &[&str] = &[
    "Snapoleon captures regions, windows and whole monitors in a single keystroke.",
    "Thin 1px strokes and 11px text are what usually break when an image is shrunk.",
    "Select a rectangle, press Enter, and the result lands on the clipboard instantly.",
    "Pipelines run after every capture: redact, resize, upload, copy the link, notify.",
    "History keeps every capture searchable by the text recognised inside the image.",
    "Scrolling capture stitches long pages and ignores sticky headers and footers.",
];

/// Dense settings-like screen: light content area, dark sidebar, table, code block.
fn synth_ui() -> RgbaImage {
    let (w, h) = (1200, 760);
    let mut img = RgbaImage::from_pixel(w, h, Rgba([250, 250, 252, 255]));
    rect(&mut img, 0, 0, 220, h as i32, [30, 32, 38, 255]);
    for (i, label) in ["Overview", "Captures", "Pipelines", "Destinations", "Hotkeys", "History", "Privacy", "Advanced"].iter().enumerate() {
        let y = 70 + i as i32 * 34;
        if i == 1 {
            rect(&mut img, 10, y - 20, 200, 28, [58, 62, 74, 255]);
        }
        text(&mut img, label, 28.0, y as f32, 13.0, if i == 1 { [255, 255, 255, 255] } else { [170, 176, 190, 255] });
    }
    text(&mut img, "SNAPOLEON", 28.0, 36.0, 12.0, [120, 130, 255, 255]);

    text(&mut img, "Capture settings", 252.0, 52.0, 24.0, [24, 26, 32, 255]);
    text(&mut img, "Choose how screenshots are taken, named and stored on disk.", 252.0, 78.0, 12.0, [110, 116, 130, 255]);
    for (i, line) in LOREM.iter().enumerate() {
        text(&mut img, line, 252.0, 120.0 + i as f32 * 20.0, 11.0, [52, 56, 66, 255]);
    }

    // Table with 1px rules.
    let top = 270;
    rect(&mut img, 252, top, 900, 28, [236, 238, 244, 255]);
    for (i, head) in ["Name", "Format", "Quality", "Size", "Modified"].iter().enumerate() {
        text(&mut img, head, 264.0 + i as f32 * 180.0, top as f32 + 19.0, 11.0, [70, 76, 92, 255]);
    }
    for r in 0..9 {
        let y = top + 28 + r * 30;
        rect(&mut img, 252, y + 29, 900, 1, [222, 225, 232, 255]);
        let cells = [format!("Screenshot {:04}.png", 1041 + r), "PNG".into(), format!("{}%", 100 - r * 3), format!("{} KB", 212 + r * 37), format!("2026-10-{:02}", 10 + r)];
        for (i, cell) in cells.iter().enumerate() {
            text(&mut img, cell, 264.0 + i as f32 * 180.0, y as f32 + 19.0, 11.0, [40, 44, 54, 255]);
        }
    }
    // Buttons.
    rect(&mut img, 252, 610, 110, 34, [61, 90, 254, 255]);
    text(&mut img, "Save changes", 268.0, 632.0, 12.0, [255, 255, 255, 255]);
    rect(&mut img, 374, 610, 90, 34, [255, 255, 255, 255]);
    rect(&mut img, 374, 610, 90, 1, [200, 204, 214, 255]);
    rect(&mut img, 374, 643, 90, 1, [200, 204, 214, 255]);
    rect(&mut img, 374, 610, 1, 34, [200, 204, 214, 255]);
    rect(&mut img, 463, 610, 1, 34, [200, 204, 214, 255]);
    text(&mut img, "Cancel", 398.0, 632.0, 12.0, [40, 44, 54, 255]);
    // Dark code block.
    rect(&mut img, 252, 666, 900, 70, [24, 26, 32, 255]);
    for (i, code) in ["$ snapoleon capture --region 0,0,1280,720 --pipeline share", "> saved C:\\Users\\me\\Pictures\\Screenshots\\Screenshot 1050.png (214 KB)"].iter().enumerate() {
        text(&mut img, code, 268.0, 692.0 + i as f32 * 22.0, 12.0, [190, 230, 190, 255]);
    }
    img
}

/// Tall article page for the stitch test, without sticky chrome.
fn synth_page(w: u32, h: u32) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(w, h, Rgba([255, 255, 255, 255]));
    let mut y = 90;
    let mut n = 1;
    while y < h as i32 - 60 {
        text(&mut img, &format!("{n}. Section heading number {n}"), 60.0, y as f32 + 20.0, 20.0, [20, 24, 36, 255]);
        y += 36;
        for line in 0..4 {
            text(&mut img, &format!("{} (paragraph {n}, line {})", LOREM[(n + line) % LOREM.len()], line + 1), 60.0, y as f32 + 14.0, 13.0, [60, 64, 76, 255]);
            y += 24;
        }
        let shade = ((n * 37) % 160) as u8;
        rect(&mut img, 60, y + 6, 520, 90, [60 + shade / 2, 120, 200 - shade / 2, 255]);
        rect(&mut img, 600, y + 6, 300, 90, [230, 232, 238, 255]);
        y += 120;
        n += 1;
    }
    img
}

/// Which fixed furniture a simulated browser viewport has.
#[derive(Clone, Copy)]
struct Chrome {
    header: bool,
    footer: bool,
    scrollbar: bool,
    sidebar: bool,
}

const HEADER: u32 = 56;
const FOOTER: u32 = 40;
const SIDEBAR: u32 = 170;

/// One viewport of `page` starting at `scroll`, with optional sticky header/footer, a fixed
/// textured sidebar and a scrollbar whose thumb moves with the scroll position.
fn viewport(page: &RgbaImage, scroll: u32, vh: u32, ch: Chrome) -> RgbaImage {
    let (w, _) = page.dimensions();
    let mut frame = RgbaImage::new(w, vh);
    for y in 0..vh {
        for x in 0..w {
            frame.put_pixel(x, y, *page.get_pixel(x, (scroll + y).min(page.height() - 1)));
        }
    }
    if ch.sidebar {
        rect(&mut frame, 0, 0, SIDEBAR as i32, vh as i32, [244, 245, 250, 255]);
        for i in 0..(vh / 26) {
            text(&mut frame, &format!("Nav item {}", i + 1), 16.0, 80.0 + i as f32 * 26.0, 12.0, [70, 76, 96, 255]);
        }
    }
    if ch.header {
        rect(&mut frame, 0, 0, w as i32, HEADER as i32, [32, 36, 48, 255]);
        text(&mut frame, "Docs  /  Scrolling capture", 24.0, 35.0, 15.0, [240, 242, 250, 255]);
    }
    if ch.footer {
        rect(&mut frame, 0, (vh - FOOTER) as i32, w as i32, FOOTER as i32, [240, 241, 246, 255]);
        text(&mut frame, "Sticky footer - cookie settings", 24.0, vh as f32 - 15.0, 12.0, [90, 96, 112, 255]);
    }
    if ch.scrollbar {
        rect(&mut frame, w as i32 - 14, 0, 14, vh as i32, [232, 232, 236, 255]);
        let thumb = (scroll as f32 / page.height() as f32 * vh as f32) as i32;
        rect(&mut frame, w as i32 - 11, thumb, 8, 120, [150, 154, 166, 255]);
    }
    frame
}

// -------------------------------------------------------------------- stages

fn zoom_pair(a: &RgbaImage, b: &RgbaImage, crop: Rect, k: u32) -> RgbaImage {
    let up = |img: &RgbaImage| {
        let c = image::imageops::crop_imm(img, crop.x as u32, crop.y as u32, crop.w, crop.h).to_image();
        image::imageops::resize(&c, c.width() * k, c.height() * k, image::imageops::FilterType::Nearest)
    };
    combine(&[up(a), up(b)], Layout::Vertical, 8, Align::Start, [255, 0, 255, 255])
}

fn downscale_stage(dir: &Path, name: &str, src: &RgbaImage) {
    for (tag, scale) in [("50", 0.5f32), ("33", 0.33)] {
        let (w, h) = ((src.width() as f32 * scale) as u32, (src.height() as f32 * scale) as u32);
        let t = Instant::now();
        let plain = resize::resize(src, w, h);
        let t_plain = t.elapsed();
        let t = Instant::now();
        let readable = resize::readable_downscale(src, w, h);
        let t_read = t.elapsed();
        println!("{name} x{scale}: {w}x{h}  plain {t_plain:?}  readable {t_read:?}");
        save(dir, &format!("{name}_{tag}_plain.png"), &plain);
        save(dir, &format!("{name}_{tag}_readable.png"), &readable);
        let crop = Rect::new(0, 0, (w / 2).min(360), (h / 2).min(220));
        save(dir, &format!("{name}_{tag}_zoom.png"), &zoom_pair(&plain, &readable, crop, 3));
    }
}

fn beautify_stage(dir: &Path, ui: &RgbaImage) {
    let shot = resize::resize(ui, 900, 570);
    let ctx = CaptureContext {
        window_title: Some("Capture settings - Snapoleon".into()),
        url: Some("https://snapoleon.app/docs/capture-settings?tab=general".into()),
        ..CaptureContext::default()
    };
    let styles = [
        (FrameStyle::None, "none"),
        (FrameStyle::MacWindow, "mac_light"),
        (FrameStyle::MacWindowDark, "mac_dark"),
        (FrameStyle::Browser, "browser_light"),
        (FrameStyle::BrowserDark, "browser_dark"),
    ];
    for (frame, name) in styles {
        let preset = BeautifyPreset { frame, ..BeautifyPreset::default() };
        let t = Instant::now();
        let out = beautify::apply_with(&shot, &preset, &ctx);
        println!("beautify {name}: {}x{} in {:?}", out.width(), out.height(), t.elapsed());
        save(dir, &format!("beautify_{name}.png"), &out);
    }
    // Phone: a portrait crop of the UI.
    let portrait = image::imageops::crop_imm(ui, 220, 0, 420, 760).to_image();
    for (preset, name) in [
        (BeautifyPreset { frame: FrameStyle::Phone, background: Background::Gradient { from: "#FF9A62".into(), to: "#E8467C".into(), angle_deg: 120.0 }, ..BeautifyPreset::default() }, "phone"),
        (BeautifyPreset { frame: FrameStyle::Phone, background: Background::Transparent, shadow: 0, padding: 0, ..BeautifyPreset::default() }, "phone_transparent"),
    ] {
        save(dir, &format!("beautify_{name}.png"), &beautify::apply(&portrait, &preset));
    }
    let landscape = beautify::apply(&resize::resize(ui, 640, 400), &BeautifyPreset { frame: FrameStyle::Phone, ..BeautifyPreset::default() });
    save(dir, "beautify_phone_landscape.png", &landscape);

    // Every background kind and the shipped presets.
    for (i, preset) in snap_core::settings::default_beautify_presets().iter().enumerate() {
        save(dir, &format!("preset_{i}_{}.png", preset.name.to_lowercase().replace([' ', ':'], "_")), &beautify::apply_with(&shot, preset, &ctx));
    }
    let solid = BeautifyPreset { background: Background::Solid { color: "#0F172A".into() }, aspect: Some((1, 1)), corner_radius: 20, padding: 40, shadow: 40, ..BeautifyPreset::default() };
    save(dir, "beautify_square_solid.png", &beautify::apply(&shot, &solid));
    let transparent = BeautifyPreset { background: Background::Transparent, ..BeautifyPreset::default() };
    save(dir, "beautify_transparent_bg.png", &beautify::apply(&shot, &transparent));
    let img_bg = BeautifyPreset { background: Background::Image { path: dir.join("beautify_square_solid.png").display().to_string() }, ..BeautifyPreset::default() };
    save(dir, "beautify_image_bg.png", &beautify::apply(&shot, &img_bg));
}

/// Scrolls a synthetic page in `steps` (cycled) px increments, stitches, and reports how far
/// the result is from the page's own pixels in the area that must match.
fn stitch_case(dir: &Path, name: &str, page: &RgbaImage, vh: u32, steps: &[u32], ch: Chrome, keep_slices: bool) {
    let mut stitcher = Stitcher::new();
    let mut slices = Vec::new();
    let max_scroll = page.height() - vh;
    let (mut scroll, mut i) = (0u32, 0usize);
    let mut outcomes = Vec::new();
    loop {
        let frame = viewport(page, scroll.min(max_scroll), vh, ch);
        let outcome = stitcher.push(&frame);
        outcomes.push(outcome);
        if slices.len() < 3 {
            slices.push(frame);
        }
        if scroll >= max_scroll || outcome == PushOutcome::NoChange {
            break;
        }
        scroll += steps[i % steps.len()];
        i += 1;
    }
    // At the end of the page a real capture keeps scrolling and gets identical frames.
    let again = stitcher.push(&viewport(page, max_scroll, vh, ch));
    let summary: Vec<String> = outcomes
        .iter()
        .map(|o| match o {
            PushOutcome::Appended { rows } => format!("+{rows}"),
            PushOutcome::NoChange => "=".into(),
            PushOutcome::NoOverlap => "!".into(),
        })
        .collect();
    let preview = stitcher.preview(900);
    let result = stitcher.finish();

    // Compare the page body: below the sticky header, above the footer, clear of sidebar/scrollbar.
    let (x0, x1) = (if ch.sidebar { SIDEBAR } else { 0 }, page.width() - if ch.scrollbar { 14 } else { 0 });
    let (y0, y1) = (if ch.header { HEADER } else { 0 }, page.height() - if ch.footer { FOOTER } else { 0 });
    let size_ok = result.height() == page.height();
    let crop = |img: &RgbaImage| image::imageops::crop_imm(img, x0, y0, x1 - x0, y1.min(img.height()) - y0).to_image();
    let (_, stats) = diff(&crop(&result), &crop(page), DiffMode::Highlight, 0);
    println!(
        "stitch[{name}] pushes {} then {again:?}; result {}x{} (page {}x{}) height_ok={size_ok}; body mismatch {} px",
        summary.join(" "),
        result.width(),
        result.height(),
        page.width(),
        page.height(),
        stats.changed_pixels
    );
    save(dir, &format!("stitch_{name}.png"), &resize::fit(&result, None, Some(1400), false));
    if keep_slices {
        for (n, s) in slices.iter().enumerate() {
            save(dir, &format!("stitch_{name}_slice{}.png", n + 1), s);
        }
        save(dir, &format!("stitch_{name}_preview.png"), &preview);
    }
}

fn stitch_stage(dir: &Path) {
    let page = synth_page(1200, 2600);
    let all = Chrome { header: true, footer: true, scrollbar: true, sidebar: false };
    stitch_case(dir, "sticky", &page, 700, &[480], all, true);
    stitch_case(dir, "plain", &page, 700, &[300, 450, 120, 610], Chrome { header: false, footer: false, scrollbar: false, sidebar: false }, false);
    stitch_case(dir, "big_steps", &page, 700, &[480, 520], all, false);
    stitch_case(dir, "tiny_steps", &page, 700, &[37, 58, 21], all, false);
    stitch_case(dir, "sidebar", &page, 700, &[400, 520], Chrome { sidebar: true, ..all }, false);
    let sparse = {
        let mut p = RgbaImage::from_pixel(1200, 2600, Rgba([255, 255, 255, 255]));
        for k in 0..6 {
            text(&mut p, &format!("Isolated paragraph {k}: sparse pages have long blank gaps."), 60.0, 160.0 + k as f32 * 420.0, 16.0, [30, 30, 40, 255]);
            rect(&mut p, 60, 200 + k * 420, 300, 4, [200, 60, 60, 255]);
        }
        p
    };
    stitch_case(dir, "sparse", &sparse, 700, &[330], all, false);

    // Real captured content: two screenshots stacked into one long page.
    let smoke = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/smoke/snap-capture");
    if let (Ok(a), Ok(b)) = (encode::load(&smoke.join("window.png")), encode::load(&smoke.join("monitor0.png"))) {
        let b = resize::resize(&b, a.width(), b.height() * a.width() / b.width());
        let page = combine(&[a, b], Layout::Vertical, 0, Align::Start, [0, 0, 0, 255]);
        stitch_case(dir, "real", &page, 640, &[260, 310], Chrome { header: false, footer: false, scrollbar: false, sidebar: false }, false);
    }
}

fn effects_stage(dir: &Path, ui: &RgbaImage) {
    let mut shot = image::imageops::crop_imm(ui, 220, 0, 700, 300).to_image();
    let mut variants = Vec::new();
    for style in [RedactStyle::Pixelate, RedactStyle::Blur, RedactStyle::Solid] {
        let mut v = shot.clone();
        effects::redact(&mut v, Rect::new(30, 40, 300, 26), style);
        effects::redact(&mut v, Rect::new(30, 100, 420, 20), style);
        variants.push(v);
    }
    save(dir, "redact_styles.png", &combine(&variants, Layout::Vertical, 6, Align::Start, [255, 0, 255, 255]));

    effects::dim_except(&mut shot, &[Rect::new(20, 90, 400, 90)], 0.65);
    save(dir, "spotlight.png", &shot);

    let mut rounded = image::imageops::crop_imm(ui, 220, 40, 300, 160).to_image();
    effects::round_corners(&mut rounded, 24);
    save(dir, "rounded.png", &effects::drop_shadow(&rounded, 24, (0, 10), [0, 0, 0, 120]));

    let a = ui.clone();
    let mut b = ui.clone();
    effects::fill(&mut b, Rect::new(260, 300, 200, 60), [255, 120, 0, 255]);
    effects::pixelate(&mut b, Rect::new(700, 400, 160, 90), 10);
    for (mode, name) in [
        (DiffMode::Highlight, "highlight"),
        (DiffMode::Overlay { opacity: 0.5 }, "overlay"),
        (DiffMode::SideBySide, "side_by_side"),
        (DiffMode::Difference, "difference"),
    ] {
        let (img, stats) = diff(&a, &b, mode, 4);
        println!("diff {name}: {} px changed, bbox {:?}", stats.changed_pixels, stats.bbox);
        save(dir, &format!("diff_{name}.png"), &resize::fit(&img, Some(1000), None, false));
    }
    let grid = combine(&[resize::resize(ui, 300, 190), resize::resize(ui, 200, 126), resize::resize(ui, 260, 164), resize::resize(ui, 180, 114)], Layout::Grid { cols: 2 }, 12, Align::Center, [20, 20, 24, 255]);
    save(dir, "combine_grid.png", &grid);
}

fn encode_stage(dir: &Path, ui: &RgbaImage) {
    for fmt in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::Webp] {
        let plain = encode::encode(ui, fmt, 85, false).unwrap().len();
        let t = Instant::now();
        let opt = encode::encode(ui, fmt, 85, true).unwrap().len();
        println!("encode {fmt:?}: plain {plain} B, optimized {opt} B ({:?})", t.elapsed());
        let target = plain / 3;
        let t = Instant::now();
        match encode::encode_to_size(ui, fmt, target) {
            Ok((bytes, fit)) => {
                let back = encode::decode(&bytes).unwrap();
                println!("  to_size({target}) -> {} B scale {:.3} q {} ({}x{}) in {:?}", bytes.len(), fit.scale, fit.quality, back.width(), back.height(), t.elapsed());
            }
            Err(e) => println!("  to_size({target}) failed: {e}"),
        }
    }
    // JPEG tagged with EXIF orientation 6 (rotate 90 CW) must come back rotated.
    let jpeg = encode::encode(&image::imageops::crop_imm(ui, 0, 0, 400, 100).to_image(), ImageFormat::Jpeg, 90, false).unwrap();
    let exif: [u8; 36] = [
        0xFF, 0xE1, 0x00, 0x22, b'E', b'x', b'i', b'f', 0, 0, b'M', b'M', 0, 42, 0, 0, 0, 8, 0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0, 0, 0, 0, 0,
    ];
    let tagged = [&jpeg[..2], &exif[..], &jpeg[2..]].concat();
    let upright = encode::decode(&tagged).unwrap();
    println!("exif orientation 6: 400x100 jpeg decodes as {}x{} (expect 100x400)", upright.width(), upright.height());

    let path = dir.join("saved.webp");
    let n = encode::save(ui, &path, ImageFormat::Webp, 80, false).unwrap();
    let loaded = encode::load(&path).unwrap();
    println!("save/load {} -> {n} B, {}x{}, format {:?}", path.display(), loaded.width(), loaded.height(), encode::format_from_path(&path));

    for c in [[61, 90, 254, 255], [255, 255, 255, 255], [30, 32, 38, 128]] {
        println!(
            "color {} | {} | {} | {} | {}",
            color::hex(c),
            color::rgb(c),
            color::hsl(c),
            color::hsv(c),
            color::oklch(c)
        );
    }
    println!(
        "contrast black/white {:.2}, accent/white {:.2} ({:?}), avg(ui sidebar) {:?}",
        color::contrast_ratio([0, 0, 0, 255], [255, 255, 255, 255]),
        color::contrast_ratio([61, 90, 254, 255], [255, 255, 255, 255]),
        color::wcag_level(color::contrast_ratio([61, 90, 254, 255], [255, 255, 255, 255])),
        color::average(ui, Rect::new(0, 0, 200, 700)),
    );
}

fn main() {
    let dir = out_dir();
    let ui = synth_ui();
    save(&dir, "synthetic_ui.png", &ui);
    downscale_stage(&dir, "synthetic", &ui);

    let smoke = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/smoke/snap-capture/window.png");
    if let Ok(real) = encode::load(&smoke) {
        println!("real screenshot {}x{}", real.width(), real.height());
        downscale_stage(&dir, "real", &real);
    }

    beautify_stage(&dir, &ui);
    stitch_stage(&dir);
    effects_stage(&dir, &ui);
    encode_stage(&dir, &ui);
    println!("outputs in {}", dir.display());
}
