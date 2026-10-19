//! Step capture end to end: record one manual shot and one click-triggered shot (a click is injected with
//! `SendInput` so the real mouse hook sees it), then export Markdown, HTML and an MP4 slideshow.
//!
//! Output goes to `target/smoke/snap-engine/steps/`.

use std::{path::PathBuf, sync::Arc, time::Duration};

use parking_lot::Mutex;
use snap_core::Settings;
use snap_engine::{
    Engine,
    steps::{self, Guide, StepRecorder, StepShot},
};

const OUT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/smoke/snap-engine/steps");

fn main() -> snap_core::Result<()> {
    tracing_subscriber::fmt().with_writer(std::io::stderr).with_env_filter("snap_engine=debug,warn").init();
    let out = PathBuf::from(OUT);
    std::fs::create_dir_all(&out)?;
    let mut settings = Settings::default();
    settings.capture.include_cursor = true;
    let engine = Engine::new(settings);

    let shots: Arc<Mutex<Vec<StepShot>>> = Arc::default();
    let sink = shots.clone();
    let recorder = StepRecorder::start(&engine, true, move |shot| sink.lock().push(shot))?;
    recorder.snap_now();
    std::thread::sleep(Duration::from_millis(800));
    click_on_a_window_title_bar();
    std::thread::sleep(Duration::from_millis(1500));
    recorder.stop();

    let shots = std::mem::take(&mut *shots.lock());
    println!("{} shots", shots.len());
    for (i, shot) in shots.iter().enumerate() {
        println!("  {}: {}x{} click {:?} caption {:?}", i + 1, shot.image.width(), shot.image.height(), shot.click, shot.caption);
    }
    let guide = Guide {
        title: "Open the settings window".into(),
        intro: "A short walkthrough recorded by the engine smoke test.".into(),
        steps: shots,
    };
    let md = steps::export_markdown(&guide, &out)?;
    println!("markdown: {}", md.display());
    steps::export_html(&guide, &out.join("guide.html"))?;
    println!("html: {} bytes", std::fs::metadata(out.join("guide.html"))?.len());
    let started = std::time::Instant::now();
    steps::export_video(&guide, &out.join("guide.mp4"), 2)?;
    println!("video: {} bytes in {:?}", std::fs::metadata(out.join("guide.mp4"))?.len(), started.elapsed());
    for (name, ms) in [("video-title.png", 1000), ("video-step1.png", 4000), ("video-step2.png", 6000)] {
        let frame = snap_record::edit::frame_at(&out.join("guide.mp4"), ms, 0)?;
        frame.save(out.join(name)).map_err(|e| snap_core::Error::Other(e.into()))?;
    }
    Ok(())
}

#[cfg(windows)]
fn click_on_a_window_title_bar() {
    use windows::Win32::UI::{
        Input::KeyboardAndMouse::{INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEINPUT, SendInput},
        WindowsAndMessaging::SetCursorPos,
    };
    let Some(window) = snap_capture::windows().ok().and_then(|ws| ws.into_iter().find(|w| !w.minimized && w.rect.w > 300)) else { return };
    let (x, y) = (window.rect.x + (window.rect.w / 2) as i32, window.rect.y + 12);
    println!("clicking at ({x}, {y}) on {:?}", window.title);
    let input = |flags| INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dwFlags: flags, ..Default::default() } } };
    // SAFETY: plain input injection with valid, fully initialised structures.
    unsafe {
        let _ = SetCursorPos(x, y);
        std::thread::sleep(Duration::from_millis(150));
        SendInput(&[input(MOUSEEVENTF_LEFTDOWN), input(MOUSEEVENTF_LEFTUP)], std::mem::size_of::<INPUT>() as i32);
    }
}

#[cfg(not(windows))]
fn click_on_a_window_title_bar() {}
