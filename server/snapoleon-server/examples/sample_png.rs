//! Writes a synthetic "screenshot" PNG for smoke-testing uploads:
//! `cargo run -p snapoleon-server --example sample_png -- out.png [width height]`

use image::{Rgba, RgbaImage};

fn rect(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, c: [u8; 4]) {
    for py in y..(y + h).min(img.height()) {
        for px in x..(x + w).min(img.width()) {
            img.put_pixel(px, py, Rgba(c));
        }
    }
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "sample.png".into());
    let w: u32 = args.next().and_then(|v| v.parse().ok()).unwrap_or(1600);
    let h: u32 = args.next().and_then(|v| v.parse().ok()).unwrap_or(900);

    let mut img = RgbaImage::from_pixel(w, h, Rgba([245, 246, 250, 255]));
    rect(&mut img, 0, 0, w, 48, [34, 38, 52, 255]);
    for (i, c) in [[255, 95, 86, 255], [255, 189, 46, 255], [39, 201, 63, 255]]
        .into_iter()
        .enumerate()
    {
        rect(&mut img, 18 + i as u32 * 26, 17, 14, 14, c);
    }
    rect(&mut img, 0, 48, 260, h - 48, [255, 255, 255, 255]);
    for i in 0..12 {
        rect(
            &mut img,
            24,
            80 + i * 44,
            160 + (i * 37) % 60,
            14,
            [200, 204, 216, 255],
        );
    }
    for row in 0..8 {
        rect(
            &mut img,
            300,
            90 + row * 96,
            w - 360,
            72,
            [255, 255, 255, 255],
        );
        rect(
            &mut img,
            324,
            110 + row * 96,
            280 + (row * 53) % 200,
            14,
            [107, 127, 255, 255],
        );
        rect(
            &mut img,
            324,
            138 + row * 96,
            w - 480 - (row * 91) % 300,
            10,
            [190, 194, 208, 255],
        );
    }
    img.save(&out)?;
    println!("wrote {out} ({w}x{h})");
    Ok(())
}
