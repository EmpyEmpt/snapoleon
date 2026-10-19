//! Smoke test: lists monitors and windows, freezes the desktop, saves PNGs and prints the element chain
//! under the cursor.
//!
//! `cargo run -p snap-capture --example shot --release -- [out_dir] [delay_secs]`

use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use snap_capture::{
    CaptureOptions, capture_all, capture_monitor, capture_window, context_at, elements_at, foreground_window, freeze, monitor_label,
    monitors, role_label, warm_up, windows,
};
use snap_core::{CaptureKind, Rect};

fn main() -> snap_core::Result<()> {
    let mut args = std::env::args().skip(1);
    let out: PathBuf = args.next().map_or_else(|| "target/smoke/snap-capture".into(), PathBuf::from);
    let delay: u64 = args.next().and_then(|d| d.parse().ok()).unwrap_or(0);
    std::fs::create_dir_all(&out)?;
    warm_up();
    if delay > 0 {
        println!("waiting {delay}s so you can position the cursor...");
        thread::sleep(Duration::from_secs(delay));
    }

    println!("== monitors");
    for m in monitors()? {
        println!(
            "#{} {} id={} {:?} rect={:?} work={:?} scale={} primary={} hdr={} {}Hz",
            m.index,
            monitor_label(&m),
            m.id,
            m.name,
            m.rect,
            m.work_area,
            m.scale_factor,
            m.is_primary,
            m.hdr,
            m.refresh_hz
        );
    }

    println!("== windows (front to back)");
    let wins = windows()?;
    for w in wins.iter().take(15) {
        println!(
            "z={} id={:#x} pid={} [{}] {:?} rect={:?}{}",
            w.z,
            w.id,
            w.pid,
            w.app_name,
            w.title,
            w.rect,
            if w.minimized { " minimized" } else { "" }
        );
    }
    println!("foreground: {:?}", foreground_window().map(|w| (w.app_name, w.title)));

    println!("== freeze");
    let opts = CaptureOptions { include_cursor: true, ..Default::default() };
    for round in 0..3 {
        let t = Instant::now();
        let f = freeze(&opts)?;
        println!(
            "round {round}: {} ms for {} monitors, {} windows, bounds {:?}",
            t.elapsed().as_millis(),
            f.monitors.len(),
            f.windows.len(),
            f.bounds()
        );
        if round == 2 {
            for (m, img) in &f.monitors {
                let path = out.join(format!("monitor{}.png", m.index));
                img.save(&path).map_err(snap_core::Error::Image)?;
                println!("saved {} ({}x{})", path.display(), img.width(), img.height());
            }
            println!("pixel under cursor: {:?}", f.pixel(f.cursor));
            let r = Rect::new(f.cursor.x - 150, f.cursor.y - 100, 300, 200);
            let cap = f.to_capture(r, CaptureKind::Region);
            cap.image.save(out.join("region_at_cursor.png")).map_err(snap_core::Error::Image)?;
            println!("region capture context: {:?}", cap.context);
        }
    }

    println!("== captures");
    let t = Instant::now();
    let all = capture_all(&opts)?;
    println!(
        "capture_all {}x{} in {} ms tone_mapped={} ctx={:?}",
        all.image.width(),
        all.image.height(),
        t.elapsed().as_millis(),
        all.tone_mapped,
        all.context
    );
    all.image.save(out.join("all.png")).map_err(snap_core::Error::Image)?;
    let m0 = capture_monitor(0, &CaptureOptions::default())?;
    m0.image.save(out.join("monitor0_nocursor.png")).map_err(snap_core::Error::Image)?;

    // Prefer the biggest normal window so the result is easy to eyeball.
    let target = wins.iter().filter(|w| !w.minimized).max_by_key(|w| w.rect.area());
    if let Some(w) = target {
        for (name, shadow) in [("window", false), ("window_shadow", true)] {
            let t = Instant::now();
            let cap = capture_window(w.id, &CaptureOptions { window_shadow: shadow, ..Default::default() })?;
            println!(
                "{name}: {:?} -> {}x{} screen_rect={:?} in {} ms ctx={:?}",
                w.title,
                cap.image.width(),
                cap.image.height(),
                cap.screen_rect,
                t.elapsed().as_millis(),
                cap.context
            );
            cap.image.save(out.join(format!("{name}.png"))).map_err(snap_core::Error::Image)?;
        }
    }

    println!("== elements under cursor");
    let p = snap_capture::cursor_position();
    println!("cursor at {p:?}, context: {:?}", context_at(p));
    for attempt in 0..2 {
        let t = Instant::now();
        match elements_at(p) {
            Ok(chain) => {
                println!("attempt {attempt}: {} elements in {} ms", chain.len(), t.elapsed().as_millis());
                for e in &chain {
                    println!(
                        "  {:<12} [{}] {:?} {:?} win={:?}",
                        e.role,
                        role_label(&e.role),
                        e.name,
                        e.rect,
                        e.window_id.map(|w| format!("{w:#x}"))
                    );
                }
            }
            Err(e) => println!("attempt {attempt}: error {e}"),
        }
    }

    println!("== desktop icons");
    if let Some(guard) = snap_capture::hide_desktop_icons() {
        thread::sleep(Duration::from_millis(300));
        let hidden = capture_monitor(0, &CaptureOptions::default())?;
        hidden.image.save(out.join("monitor0_icons_hidden.png")).map_err(snap_core::Error::Image)?;
        drop(guard);
        println!("icons hidden for a capture and restored");
    } else {
        println!("no desktop icon list found");
    }
    Ok(())
}
