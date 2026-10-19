//! Writes the brand assets: `assets/brand/snapoleon.svg` and `snapoleon-<size>.png`, plus tray icon previews
//! and a contact sheet under `target/smoke/snap-ui/`.
//!
//!     cargo run -p snap-ui --example brand_export

use std::{fs, path::PathBuf};

use image::{Rgba, RgbaImage, imageops};

const SIZES: [u32; 8] = [16, 32, 48, 64, 128, 256, 512, 1024];

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn main() -> std::io::Result<()> {
    let brand = workspace().join("assets/brand");
    let smoke = workspace().join("target/smoke/snap-ui");
    fs::create_dir_all(&brand)?;
    fs::create_dir_all(&smoke)?;

    fs::write(brand.join("snapoleon.svg"), snap_ui::app_icon_svg())?;
    for size in SIZES {
        let path = brand.join(format!("snapoleon-{size}.png"));
        snap_ui::app_icon_rgba(size).save(&path).expect("write icon png");
        println!("{}", path.display());
    }

    // Contact sheet: app icon and tray variants on dark and light backgrounds, at several sizes.
    let cell = 140;
    let mut sheet = RgbaImage::from_pixel(cell * 6, cell * 2, Rgba([0x0E, 0x0F, 0x13, 255]));
    for y in cell..cell * 2 {
        for x in 0..sheet.width() {
            sheet.put_pixel(x, y, Rgba([0xF7, 0xF7, 0xF5, 255]));
        }
    }
    let mut x = 10;
    for size in [128u32, 64, 32, 16] {
        imageops::overlay(&mut sheet, &snap_ui::app_icon_rgba(size), x as i64, 6);
        imageops::overlay(&mut sheet, &snap_ui::app_icon_rgba(size), x as i64, (cell + 6) as i64);
        x += size + 12;
    }
    let mut x = 10 + cell * 3;
    for size in [64u32, 32, 16] {
        for (row, ink) in [(0, [255, 255, 255, 255]), (1, [24, 24, 28, 255])] {
            let y = (row * cell + 6) as i64;
            imageops::overlay(&mut sheet, &snap_ui::tray_icon_rgba_colored(size, false, ink), x as i64, y);
            imageops::overlay(&mut sheet, &snap_ui::tray_icon_rgba_colored(size, true, ink), (x + size + 8) as i64, y);
        }
        x += (size + 8) * 2 + 8;
    }
    sheet.save(smoke.join("brand-sheet.png")).expect("write sheet");

    let tray = snap_ui::tray_icon_rgba(32, true);
    println!("tray icon {}x{}", tray.width(), tray.height());
    Ok(())
}
