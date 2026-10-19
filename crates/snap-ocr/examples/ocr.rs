//! Smoke test: draws English/Russian text, a table and a QR code, then runs
//! OCR, table extraction and code decoding on it.
//!
//! `cargo run -p snap-ocr --example ocr [-- <image.png>]`
//! Output goes to target/smoke/snap-ocr/.

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use image::{Rgba, RgbaImage};
use snap_core::settings::OcrSettings;
use snap_ocr::{codes, engines, native_languages, recognize};

const OUT: &str = "C:/Users/3mpti/snapoleon/target/smoke/snap-ocr";

fn draw_text(img: &mut RgbaImage, font: &FontRef, size: f32, x: f32, baseline: f32, text: &str) {
    let scaled = font.as_scaled(PxScale::from(size));
    let mut pen = x;
    let mut prev = None;
    for c in text.chars() {
        let id = scaled.glyph_id(c);
        if let Some(p) = prev {
            pen += scaled.kern(p, id);
        }
        let glyph = id.with_scale_and_position(PxScale::from(size), ab_glyph::point(pen, baseline));
        if let Some(outlined) = font.outline_glyph(glyph) {
            let b = outlined.px_bounds();
            outlined.draw(|gx, gy, cov| {
                let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                if px >= 0 && py >= 0 && (px as u32) < img.width() && (py as u32) < img.height() {
                    let v = (255.0 * (1.0 - cov)) as u8;
                    let p = img.get_pixel_mut(px as u32, py as u32);
                    let v = p[0].min(v);
                    *p = Rgba([v, v, v, 255]);
                }
            });
        }
        pen += scaled.h_advance(id);
        prev = Some(id);
    }
}

fn sample() -> RgbaImage {
    let font = FontRef::try_from_slice(include_bytes!("../../../assets/fonts/Inter-Regular.ttf")).expect("font");
    let mut img = RgbaImage::from_pixel(1000, 640, Rgba([255, 255, 255, 255]));
    draw_text(&mut img, &font, 30.0, 40.0, 60.0, "Snapoleon screenshot OCR test");
    draw_text(&mut img, &font, 20.0, 40.0, 110.0, "The quick brown fox jumps over the lazy dog.");
    draw_text(&mut img, &font, 20.0, 40.0, 142.0, "Съешь же ещё этих мягких французских булок, да выпей чаю.");
    draw_text(&mut img, &font, 11.0, 40.0, 172.0, "Tiny UI label text needs the upscale pass: Параметры, 0123456789");

    let rows = [["Name", "Qty", "Price"], ["Apples", "12", "3.50"], ["Бананы", "7", "1.20"], ["Cherries", "30", "9.99"]];
    for (r, row) in rows.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            draw_text(&mut img, &font, 20.0, 40.0 + c as f32 * 220.0, 240.0 + r as f32 * 36.0, cell);
        }
    }

    draw_text(&mut img, &font, 18.0, 40.0, 440.0, "First paragraph keeps going on this line");
    draw_text(&mut img, &font, 18.0, 40.0, 464.0, "and wraps onto a second line right here.");
    draw_text(&mut img, &font, 18.0, 40.0, 530.0, "Второй абзац отделён пустой строкой");
    draw_text(&mut img, &font, 18.0, 40.0, 554.0, "и тоже занимает две строки подряд.");

    let qr = codes::qr("https://snapoleon.app/s/abc123", 5);
    image::imageops::overlay(&mut img, &qr, 700, 380);
    img
}

fn main() -> anyhow::Result<()> {
    std::fs::create_dir_all(OUT)?;
    let img = match std::env::args().nth(1) {
        Some(path) => image::open(path)?.to_rgba8(),
        None => sample(),
    };
    img.save(format!("{OUT}/input.png"))?;

    println!("engines:");
    for e in engines() {
        println!("  {:?}: available={} ({})", e.engine, e.available, e.note);
    }
    println!("native languages: {:?}", native_languages());

    let started = std::time::Instant::now();
    let result = recognize(&img, &OcrSettings::default())?;
    println!("\nengine={} language={:?} lines={} in {:?}\n", result.engine, result.language, result.lines.len(), started.elapsed());
    println!("--- text ---\n{}\n", result.text());
    match result.to_markdown_table() {
        Some(md) => println!("--- markdown table ---\n{md}\n"),
        None => println!("--- no table detected ---\n"),
    }
    if let Some(tsv) = result.to_tsv() {
        std::fs::write(format!("{OUT}/table.tsv"), tsv)?;
    }
    std::fs::write(format!("{OUT}/result.json"), serde_json::to_string_pretty(&result)?)?;

    println!("--- codes ---");
    for c in codes::decode(&img) {
        println!("{} {:?} at {:?}", c.format, c.text, c.rect);
    }
    let qr = codes::qr("Привет, Snapoleon! https://example.com/?q=1", 4);
    qr.save(format!("{OUT}/qr.png"))?;
    println!("standalone qr decode: {:?}", codes::decode(&qr).iter().map(|c| c.text.as_str()).collect::<Vec<_>>());
    Ok(())
}
