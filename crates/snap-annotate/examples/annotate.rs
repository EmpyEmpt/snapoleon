//! Smoke test: builds documents with every shape kind in both styles, renders them at several
//! scales, times the live view and round-trips a `.snap` project.
//!
//! Output goes to `target/smoke/snap-annotate/`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use image::{Rgba, RgbaImage};
use snap_annotate::*;
use snap_core::settings::{BeautifyPreset, RedactStyle};
use snap_core::{PointF, RectF};

const OUT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/smoke/snap-annotate");

fn p(x: f32, y: f32) -> PointF {
    PointF::new(x, y)
}

fn r(x: f32, y: f32, w: f32, h: f32) -> RectF {
    RectF::new(x, y, w, h)
}

fn base_image() -> RgbaImage {
    for name in ["window.png", "monitor0_nocursor.png"] {
        let path = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/smoke/snap-capture")).join(name);
        if let Ok(img) = image::open(&path) {
            println!("base image: {}", path.display());
            return img.to_rgba8();
        }
    }
    println!("base image: synthetic");
    synthetic(1600, 1000)
}

/// A UI-like picture: light page, header, sidebar, text lines and cards.
fn synthetic(w: u32, h: u32) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(w, h, Rgba([246, 247, 250, 255]));
    let mut rect = |x: u32, y: u32, rw: u32, rh: u32, c: [u8; 4]| {
        for yy in y..(y + rh).min(h) {
            for xx in x..(x + rw).min(w) {
                img.put_pixel(xx, yy, Rgba(c));
            }
        }
    };
    rect(0, 0, w, 56, [32, 36, 48, 255]);
    rect(0, 56, 220, h, [236, 238, 244, 255]);
    for i in 0..14 {
        rect(240, 90 + i * 34, 300 + (i * 97) % 600, 10, [150, 156, 170, 255]);
    }
    for c in 0..3 {
        rect(240 + c * 360, 600, 330, 220, [255, 255, 255, 255]);
        rect(256 + c * 360, 620, 120, 14, [70, 120, 220, 255]);
    }
    img
}

fn logo(size: u32) -> RgbaImage {
    RgbaImage::from_fn(size, size, |x, y| {
        let (dx, dy) = (x as f32 - size as f32 / 2.0, y as f32 - size as f32 / 2.0);
        let d = (dx * dx + dy * dy).sqrt() / (size as f32 / 2.0);
        if d > 1.0 {
            return Rgba([0, 0, 0, 0]);
        }
        let t = (x + y) as f32 / (2.0 * size as f32);
        Rgba([(94.0 + 120.0 * t) as u8, (92.0 + 40.0 * t) as u8, 230, 255])
    })
}

fn save(img: &RgbaImage, name: &str) {
    let path = Path::new(OUT).join(name);
    img.save(&path).expect("write png");
    println!("wrote {} ({}x{})", path.display(), img.width(), img.height());
}

fn style(color: [u8; 4], width: f32) -> Style {
    Style { stroke: color, width, ..Style::default() }
}

fn shadowed(mut s: Style) -> Style {
    s.shadow = true;
    s
}

const RED: [u8; 4] = [255, 59, 48, 255];
const BLUE: [u8; 4] = [10, 132, 255, 255];
const GREEN: [u8; 4] = [52, 199, 89, 255];
const YELLOW: [u8; 4] = [255, 214, 10, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];
const DARK: [u8; 4] = [28, 28, 30, 255];

/// Everything except spotlight (which would dim the whole picture), laid out for a base of `w` x `h`.
fn populate(doc: &mut Document, w: f32, h: f32) {
    let (fx, fy) = (w / 1600.0, h / 1000.0);
    let x = |v: f32| v * fx;
    let y = |v: f32| v * fy;

    // Arrows: plain, outlined+shadow, curved, open head, double, thick, thin.
    doc.add(
        ShapeKind::Arrow { from: p(x(60.0), y(70.0)), to: p(x(260.0), y(150.0)), ctrl: None, head: ArrowHead::Triangle, double: false },
        style(RED, 4.0),
    );
    doc.add(
        ShapeKind::Arrow {
            from: p(x(60.0), y(190.0)),
            to: p(x(300.0), y(290.0)),
            ctrl: Some(p(x(80.0), y(300.0))),
            head: ArrowHead::Triangle,
            double: false,
        },
        Style { fill: Some(WHITE), shadow: true, ..style(BLUE, 5.0) },
    );
    doc.add(
        ShapeKind::Arrow { from: p(x(60.0), y(330.0)), to: p(x(300.0), y(380.0)), ctrl: None, head: ArrowHead::Open, double: false },
        style(GREEN, 5.0),
    );
    doc.add(
        ShapeKind::Arrow {
            from: p(x(60.0), y(430.0)),
            to: p(x(300.0), y(470.0)),
            ctrl: Some(p(x(180.0), y(380.0))),
            head: ArrowHead::Triangle,
            double: true,
        },
        shadowed(style(RED, 4.0)),
    );
    doc.add(
        ShapeKind::Arrow { from: p(x(60.0), y(520.0)), to: p(x(300.0), y(540.0)), ctrl: None, head: ArrowHead::Triangle, double: false },
        style(DARK, 10.0),
    );
    doc.add(
        ShapeKind::Arrow { from: p(x(60.0), y(580.0)), to: p(x(300.0), y(560.0)), ctrl: None, head: ArrowHead::None, double: false },
        style(YELLOW, 6.0),
    );
    doc.add(
        ShapeKind::Arrow { from: p(x(60.0), y(610.0)), to: p(x(130.0), y(640.0)), ctrl: None, head: ArrowHead::Triangle, double: false },
        style(RED, 3.0),
    );

    // Line, rectangles, ellipse.
    doc.add(ShapeKind::Line { from: p(x(360.0), y(60.0)), to: p(x(600.0), y(100.0)) }, Style { dashed: true, ..style(RED, 3.0) });
    doc.add(ShapeKind::Rect { rect: r(x(360.0), y(130.0), x(220.0), y(120.0)), radius: 10.0 }, style(RED, 4.0));
    doc.add(
        ShapeKind::Rect { rect: r(x(620.0), y(130.0), x(220.0), y(120.0)), radius: 0.0 },
        Style { fill: Some([255, 214, 10, 70]), shadow: true, ..style(YELLOW, 3.0) },
    );
    doc.add(ShapeKind::Ellipse { rect: r(x(360.0), y(290.0), x(220.0), y(120.0)) }, style(BLUE, 4.0));
    doc.add(
        ShapeKind::Ellipse { rect: r(x(620.0), y(290.0), x(220.0), y(120.0)) },
        shadowed(Style { fill: Some([10, 132, 255, 60]), ..style(BLUE, 3.0) }),
    );

    // Pen and highlighter.
    let wave: Vec<PointF> = (0..40).map(|i| p(x(360.0 + i as f32 * 12.0), y(470.0) + (i as f32 * 0.55).sin() * 22.0)).collect();
    doc.add(ShapeKind::Pen { points: wave }, style(RED, 4.0));
    let wave2: Vec<PointF> = (0..14).map(|i| p(x(360.0 + i as f32 * 8.0), y(540.0) + (i as f32 * 0.6).sin() * 18.0)).collect();
    doc.add(ShapeKind::Pen { points: wave2 }, shadowed(style(BLUE, 6.0)));
    let hl = vec![p(x(360.0), y(610.0)), p(x(520.0), y(606.0)), p(x(700.0), y(614.0)), p(x(840.0), y(608.0))];
    doc.add(ShapeKind::Highlighter { points: hl }, style(YELLOW, 8.0));

    // Text in every flavour; Cyrillic included.
    let pos = |a: f32, b: f32| p(x(a), y(b));
    doc.add(
        ShapeKind::Text { pos: pos(900.0, 70.0), text: "Plain label".into(), size: 28.0, bold: true, bg: TextBg::None, max_width: None },
        style(RED, 4.0),
    );
    doc.add(
        ShapeKind::Text {
            pos: pos(900.0, 130.0),
            text: "Solid background".into(),
            size: 24.0,
            bold: true,
            bg: TextBg::Solid,
            max_width: None,
        },
        Style { fill: Some(RED), ..style(RED, 4.0) },
    );
    doc.add(
        ShapeKind::Text { pos: pos(900.0, 200.0), text: "Light solid".into(), size: 24.0, bold: false, bg: TextBg::Solid, max_width: None },
        Style { fill: Some(YELLOW), shadow: true, ..style(RED, 4.0) },
    );
    doc.add(
        ShapeKind::Text {
            pos: pos(900.0, 280.0),
            text: "Speech bubble".into(),
            size: 24.0,
            bold: true,
            bg: TextBg::Bubble,
            max_width: None,
        },
        shadowed(Style { fill: Some(BLUE), ..style(BLUE, 4.0) }),
    );
    doc.add(
        ShapeKind::Text {
            pos: pos(900.0, 380.0),
            text: "Привет, мир! Это проверка переноса длинного русского текста по ширине.".into(),
            size: 22.0,
            bold: false,
            bg: TextBg::Bubble,
            max_width: Some(x(330.0)),
        },
        Style { fill: Some(DARK), ..style(DARK, 4.0) },
    );
    doc.add(
        ShapeKind::Callout {
            rect: r(x(900.0), y(540.0), x(300.0), y(90.0)),
            tail: pos(840.0, 690.0),
            text: "Нажмите здесь, чтобы продолжить".into(),
            size: 20.0,
        },
        shadowed(Style { fill: Some(GREEN), ..style(GREEN, 4.0) }),
    );

    // Step badges (plain, with ring + shadow, two-digit).
    doc.add(ShapeKind::Step { center: pos(1320.0, 90.0), number: 1, radius: 18.0 }, style(RED, 4.0));
    doc.add(
        ShapeKind::Step { center: pos(1390.0, 90.0), number: 2, radius: 18.0 },
        shadowed(Style { fill: Some(WHITE), ..style(BLUE, 4.0) }),
    );
    doc.add(ShapeKind::Step { center: pos(1460.0, 90.0), number: 12, radius: 18.0 }, style(YELLOW, 4.0));
    doc.add(ShapeKind::Step { center: pos(1530.0, 90.0), number: 3, radius: 26.0 }, shadowed(style(GREEN, 4.0)));

    // Redactions.
    doc.add(ShapeKind::Redact { rect: r(x(1250.0), y(160.0), x(260.0), y(34.0)), style: RedactStyle::Pixelate }, Style::default());
    doc.add(ShapeKind::Redact { rect: r(x(1250.0), y(210.0), x(260.0), y(34.0)), style: RedactStyle::Solid }, Style::default());
    doc.add(ShapeKind::Redact { rect: r(x(1250.0), y(260.0), x(260.0), y(34.0)), style: RedactStyle::Blur }, Style::default());

    // Magnifier, pasted image, stamps, measure.
    doc.add(
        ShapeKind::Magnifier { source: r(x(24.0), y(790.0), x(150.0), y(36.0)), center: pos(580.0, 880.0), radius: 80.0 },
        Style::default(),
    );
    doc.add(ShapeKind::Image { rect: r(x(1250.0), y(580.0), x(110.0), y(110.0)), image: Arc::new(logo(220)) }, shadowed(Style::default()));
    for (i, glyph) in Stamp::ALL.iter().enumerate() {
        let c = p(x(900.0 + i as f32 * 78.0), y(800.0));
        doc.add(
            ShapeKind::Stamp { center: c, size: 54.0, glyph: *glyph },
            Style { shadow: i % 2 == 0, ..style(glyph.default_color(), 4.0) },
        );
    }
    doc.add(ShapeKind::Measure { from: pos(360.0, 760.0), to: pos(605.0, 760.0) }, style(RED, 4.0));
    doc.add(ShapeKind::Measure { from: pos(380.0, 810.0), to: pos(480.0, 900.0) }, shadowed(style(BLUE, 4.0)));
}

fn main() {
    std::fs::create_dir_all(OUT).expect("create output dir");
    let base = base_image();
    let (w, h) = (base.width() as f32, base.height() as f32);

    let mut doc = Document::new(base.clone(), 1.0);
    populate(&mut doc, w, h);

    let t = Instant::now();
    let full = render(&doc, &RenderOptions::default());
    println!("render 1.0: {:?}", t.elapsed());
    save(&full, "gallery_1.0.png");

    let light_base = synthetic(base.width(), base.height());
    let mut light = Document::new(light_base, 1.0);
    populate(&mut light, w, h);
    light.add(ShapeKind::Redact { rect: r(240.0, 90.0, 300.0, 12.0), style: RedactStyle::Pixelate }, Style::default());
    save(&render(&light, &RenderOptions::default()), "gallery_light_1.0.png");

    let half = RenderOptions { scale: 0.5, readable: true, beautify: true };
    save(&render(&doc, &half), "gallery_0.5_readable.png");
    save(&render(&doc, &RenderOptions { readable: false, ..half }), "gallery_0.5_plain.png");
    save(&render(&doc, &RenderOptions { scale: 2.0, ..RenderOptions::default() }), "gallery_2.0.png");

    // Crop + beautify.
    let mut cropped = doc.clone();
    cropped.crop = Some(r(w * 0.2, h * 0.05, w * 0.6, h * 0.5));
    cropped.beautify = Some(BeautifyPreset::default());
    save(&render(&cropped, &RenderOptions::default()), "gallery_crop_beautify.png");

    // Spotlight on its own (it dims everything else).
    let mut spot = Document::new(base.clone(), 1.0);
    spot.add(ShapeKind::Spotlight { rect: r(w * 0.1, h * 0.1, w * 0.3, h * 0.2), ellipse: false }, Style::default());
    spot.add(ShapeKind::Spotlight { rect: r(w * 0.45, h * 0.3, w * 0.25, h * 0.3), ellipse: true }, Style::default());
    spot.add(
        ShapeKind::Arrow { from: p(w * 0.8, h * 0.8), to: p(w * 0.58, h * 0.55), ctrl: None, head: ArrowHead::Triangle, double: false },
        Style { fill: Some(WHITE), ..style(YELLOW, 5.0) },
    );
    save(&render(&spot, &RenderOptions::default()), "spotlight.png");

    // Live view: timing and a zoomed-in crop of the same document.
    let view = View { offset: p(0.0, 0.0), zoom: 0.9, size: (1600, 1000) };
    let _ = render_view(&doc, &view, true);
    let t = Instant::now();
    let runs = 20;
    for i in 0..runs {
        let v = View { offset: p(i as f32 * 0.5, 0.0), ..view };
        std::hint::black_box(render_view(&doc, &v, true));
    }
    println!("render_view 1600x1000 (zoom 0.9, all shapes): {:?} per frame", t.elapsed() / runs);
    let t = Instant::now();
    for _ in 0..runs {
        std::hint::black_box(render_view(&doc, &view, false));
    }
    println!("render_view without base: {:?} per frame", t.elapsed() / runs);
    let mut bare = Document::new(base.clone(), 1.0);
    bare.add(ShapeKind::Rect { rect: r(100.0, 100.0, 300.0, 200.0), radius: 6.0 }, style(RED, 4.0));
    let t = Instant::now();
    for i in 0..runs {
        std::hint::black_box(render_view(&bare, &View { offset: p(i as f32, 0.0), ..view }, true));
    }
    println!("render_view typical doc (one rect): {:?} per frame", t.elapsed() / runs);

    for (i, shape) in doc.shapes.iter().enumerate() {
        let mut one = Document::new(base.clone(), 1.0);
        one.add(shape.kind.clone(), shape.style);
        let t = Instant::now();
        for _ in 0..5 {
            std::hint::black_box(render_view(&one, &view, false));
        }
        let each = t.elapsed() / 5;
        if each.as_micros() > 600 {
            println!("  slow shape #{i} {:?}: {each:?}", std::mem::discriminant(&shape.kind));
        }
    }

    let save_pm = |pm: &tiny_skia::Pixmap, name: &str| {
        let img = RgbaImage::from_raw(pm.width(), pm.height(), pm.clone().take_demultiplied()).expect("pixmap size");
        save(&img, name);
    };
    save_pm(&render_view(&doc, &view, true), "view_0.9.png");
    save_pm(&render_view(&doc, &View { offset: p(w * 0.05, h * 0.05), zoom: 0.5, size: (1000, 700) }, true), "view_0.5.png");
    save_pm(&render_view(&doc, &View { offset: p(60.0, 50.0), zoom: 3.0, size: (1000, 700) }, true), "view_3.0.png");
    save_pm(&render_view(&light, &View { offset: p(890.0, 770.0), zoom: 3.0, size: (1200, 400) }, true), "detail_stamps.png");
    save_pm(&render_view(&light, &View { offset: p(890.0, 60.0), zoom: 2.5, size: (1200, 800) }, true), "detail_text.png");
    save_pm(&render_view(&light, &View { offset: p(40.0, 50.0), zoom: 3.0, size: (1200, 800) }, true), "detail_arrows.png");
    save_pm(&render_view(&doc, &View { offset: p(60.0, 50.0), zoom: 8.0, size: (800, 400) }, true), "view_8.0.png");

    // Hit testing, handles, undo.
    let mut undo = Undo::new();
    for shape in &doc.shapes {
        let probe = match handles(shape).first() {
            Some((_, at)) => *at,
            None => bounds(shape).center(),
        };
        let found = hit_test(&doc, probe, 6.0);
        println!("hit shape {:>2} ({:?}) at ({:.0}, {:.0}): {:?}", shape.id, std::mem::discriminant(&shape.kind), probe.x, probe.y, found);
    }
    let first = &doc.shapes[0];
    let ShapeKind::Arrow { from, to, .. } = first.kind else { unreachable!("first shape is an arrow") };
    let mid = p((from.x + to.x) / 2.0, (from.y + to.y) / 2.0);
    assert_eq!(hit_test(&doc, mid, 3.0), Some(first.id), "midpoint of the first arrow must hit it");
    assert_eq!(hit_test(&doc, p(mid.x, mid.y + 40.0), 3.0), None, "far from the arrow must miss");
    // Every handle of every shape must survive dragging in any direction.
    for shape in &doc.shapes {
        for (handle, _) in handles(shape) {
            for delta in [p(3.0, 2.0), p(-80.0, -60.0), p(500.0, 400.0)] {
                let mut copy = shape.clone();
                drag(&mut copy, handle, delta);
                let b = bounds(&copy);
                assert!([b.x, b.y, b.w, b.h].iter().all(|v| v.is_finite()), "{handle:?} on shape {} gave {b:?}", shape.id);
            }
        }
    }
    let arrow = doc.shapes[0].id;
    undo.checkpoint(&doc);
    let shape = doc.get_mut(arrow).expect("shape exists");
    for (handle, at) in handles(shape) {
        drag(shape, handle, p(2.0, 3.0));
        println!("  handle {handle:?} at ({:.1}, {:.1})", at.x, at.y);
    }
    println!("bounds after drag: {:?}", bounds(doc.get(arrow).expect("shape exists")));
    assert!(undo.undo(&mut doc), "undo restores the checkpoint");
    assert!(undo.redo(&mut doc), "redo reapplies");
    assert!(undo.undo(&mut doc));
    let (tw, th) = text_layout("Привет, мир! Wrap me please", 20.0, false, Some(120.0));
    println!("text_layout -> {tw:.1} x {th:.1}");

    // Project round trip.
    let proj = Path::new(OUT).join("gallery.snap");
    let t = Instant::now();
    save_project(&doc, &proj).expect("save project");
    println!("save_project: {:?}, {} bytes", t.elapsed(), std::fs::metadata(&proj).map(|m| m.len()).unwrap_or(0));
    let t = Instant::now();
    let loaded = load_project(&proj).expect("load project");
    println!("load_project: {:?}", t.elapsed());
    let same = doc.shapes == loaded.shapes && *doc.base == *loaded.base && doc.meta == loaded.meta;
    println!("round trip: {} shapes, identical = {same}", loaded.shapes.len());
    assert!(same, "project round trip must be lossless");
    let mut loaded = loaded;
    let new_id = loaded.add(ShapeKind::Step { center: p(5.0, 5.0), number: 1, radius: 10.0 }, Style::default());
    assert!(doc.shapes.iter().all(|s| s.id != new_id), "ids stay unique after load");
}
