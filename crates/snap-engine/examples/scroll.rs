//! Scrolling capture. `scroll <window title>` scrolls a real window (the document area of the first window whose
//! title matches); `scroll sim` drives the same stitching loop over a synthetic page with a sticky header, which
//! needs no desktop. Results go to `target/smoke/snap-engine/scroll*.png`.

use std::sync::atomic::AtomicBool;

use snap_core::{Rect, RgbaImage, Settings, secrets::MemoryStore};
use snap_engine::{Engine, ScrollProgress, stitch_scroll};

const OUT: &str = "C:/Users/3mpti/snapoleon/target/smoke/snap-engine";

fn main() -> snap_core::Result<()> {
    tracing_subscriber::fmt().with_writer(std::io::stderr).with_env_filter("snap_engine=debug,warn").init();
    let arg = std::env::args().nth(1).expect("usage: scroll <window title> | scroll sim");
    let image = if arg == "sim" { simulate()? } else { real_window(&arg)? };
    println!("result {}x{}", image.width(), image.height());
    let name = if arg == "sim" { "scroll-sim.png" } else { "scroll.png" };
    image.save(format!("{OUT}/{name}")).map_err(|e| snap_core::Error::Other(e.into()))
}

fn real_window(title: &str) -> snap_core::Result<RgbaImage> {
    let engine = Engine::with_parts(Settings::default(), std::sync::Arc::new(MemoryStore::default()), None);
    let window =
        snap_capture::windows()?.into_iter().find(|w| w.title.to_lowercase().contains(&title.to_lowercase())).expect("no such window");
    // Skip the title bar and the status bar: only the document area should move.
    let r = window.rect;
    let area = Rect::new(r.x + 8, r.y + 110, r.w - 16, r.h.saturating_sub(160));
    println!("scrolling {:?} area {area:?}", window.title);
    let started = std::time::Instant::now();
    let cap =
        engine.scrolling(area, &AtomicBool::new(false), &|p: &ScrollProgress| println!("  frames {} height {}", p.frames, p.height))?;
    println!("done in {:?}", started.elapsed());
    Ok(cap.image)
}

/// A 5000 px page of distinct text lines seen through a 700 px viewport with a sticky 48 px header; one wheel
/// notch moves the content 37 px, like an app whose notch size the engine has to discover.
fn simulate() -> snap_core::Result<RgbaImage> {
    use snap_annotate::{Document, RenderOptions, ShapeKind, Style, TextBg};
    use snap_core::PointF;

    let (page_w, page_h, view_h, header, notch_px) = (900u32, 5000u32, 700u32, 48u32, 37u32);
    let mut doc = Document::new(RgbaImage::from_pixel(page_w, page_h, image::Rgba([252, 252, 253, 255])), 1.0);
    for i in 0..(page_h / 34) {
        let text = format!(
            "Row {i:03}  {}",
            ["amber", "birch", "cedar", "dune", "ember", "fjord", "grove"][(i * 5 % 7) as usize].repeat(1 + (i % 4) as usize)
        );
        doc.add(
            ShapeKind::Text {
                pos: PointF::new(24.0 + (i % 5) as f32 * 20.0, 10.0 + i as f32 * 34.0),
                text,
                size: 20.0,
                bold: i % 9 == 0,
                bg: TextBg::None,
                max_width: None,
            },
            Style::with_color([30, 34, 48, 255]),
        );
    }
    let page = snap_annotate::render(&doc, &RenderOptions { scale: 1.0, readable: false, beautify: false });
    page.save(format!("{OUT}/scroll-sim-page.png")).map_err(|e| snap_core::Error::Other(e.into()))?;

    let offset = std::cell::Cell::new(0u32);
    let grab = || {
        let mut view = RgbaImage::new(page_w, view_h);
        for y in 0..view_h {
            for x in 0..page_w {
                let px = if y < header { image::Rgba([32, 36, 48, 255]) } else { *page.get_pixel(x, (offset.get() + y).min(page_h - 1)) };
                view.put_pixel(x, y, px);
            }
        }
        Ok(view)
    };
    let scroll = |notches: i32| {
        offset.set((offset.get() + notches as u32 * notch_px).min(page_h - view_h));
        Ok(())
    };
    let stitched = stitch_scroll(view_h, grab, scroll, &AtomicBool::new(false), &|p: &ScrollProgress| {
        println!("  frames {} height {}", p.frames, p.height)
    })?;
    println!("page body height {} + header {header} expected about {}", page_h, page_h + header);
    Ok(stitched)
}
