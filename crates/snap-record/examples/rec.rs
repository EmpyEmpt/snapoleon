//! Smoke test: record the primary monitor to MP4 (3 s, system audio) and GIF (2 s),
//! probe both, trim the MP4 to 1 s and export it under 500 KB.
//!
//! `cargo run -p snap-record --example rec [--ffmpeg]`; without `--ffmpeg` only native backends are used.

use std::{path::PathBuf, time::Duration};

use snap_core::{
    Rect,
    settings::{EncoderPref, VideoFormat},
};
use snap_record::{RecordOptions, RecordTarget, edit};

fn options(target: RecordTarget, format: VideoFormat, fps: u32, output: PathBuf) -> RecordOptions {
    RecordOptions {
        target,
        format,
        fps,
        bitrate: 0,
        system_audio: format == VideoFormat::Mp4,
        microphone: false,
        mic_device: None,
        show_cursor: true,
        highlight_clicks: true,
        show_keystrokes: true,
        encoder: EncoderPref::Auto,
        max_gif_width: 800,
        output,
        ffmpeg: None,
        exclude_windows: Vec::new(),
    }
}

/// Wiggle the pointer so the capture has motion to encode.
fn wiggle(for_ms: u64) {
    #[cfg(windows)]
    {
        use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_millis(for_ms) {
            let t = start.elapsed().as_secs_f64() * 3.0;
            // SAFETY: plain Win32 call.
            let _ = unsafe { SetCursorPos(600 + (t.cos() * 300.0) as i32, 400 + (t.sin() * 200.0) as i32) };
            std::thread::sleep(Duration::from_millis(15));
        }
    }
    #[cfg(not(windows))]
    std::thread::sleep(Duration::from_millis(for_ms));
}

fn report(label: &str, path: &std::path::Path) -> snap_core::Result<edit::VideoInfo> {
    let info = edit::probe(path)?;
    let bytes = std::fs::metadata(path)?.len();
    println!(
        "{label}: {} bytes, {} ms, {}x{} @ {:.1} fps, audio={}",
        bytes, info.duration_ms, info.width, info.height, info.fps, info.has_audio
    );
    Ok(info)
}

/// An edit must keep the source's audio track.
fn check_audio(label: &str, source: &edit::VideoInfo, edited: &edit::VideoInfo) -> snap_core::Result<()> {
    if source.has_audio != edited.has_audio {
        return Err(snap_core::Error::Other(anyhow::anyhow!("{label}: audio={} but the source has audio={}", edited.has_audio, source.has_audio)));
    }
    Ok(())
}

#[cfg(windows)]
fn primary_monitor_index() -> usize {
    use windows_capture::monitor::Monitor;
    let primary = Monitor::primary().ok();
    Monitor::enumerate().ok().and_then(|all| all.iter().position(|m| Some(*m) == primary)).unwrap_or(0)
}

#[cfg(not(windows))]
fn primary_monitor_index() -> usize {
    0
}

/// Play a quiet test tone through the default output so loopback capture has something to hear.
fn play_tone(seconds: u32) -> Option<std::process::Child> {
    std::process::Command::new("C:/ffmpeg/bin/ffplay.exe")
        .args(["-nodisp", "-autoexit", "-loglevel", "quiet", "-f", "lavfi", "-i"])
        .arg(format!("sine=frequency=440:duration={seconds},volume=0.2"))
        .spawn()
        .ok()
}

fn main() -> snap_core::Result<()> {
    tracing_subscriber::fmt().with_env_filter("snap_record=debug").init();
    #[cfg(windows)]
    // SAFETY: first call in the process, before any window exists.
    unsafe {
        use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let with_ffmpeg = std::env::args().any(|a| a == "--ffmpeg");
    snap_record::set_ffmpeg_enabled(with_ffmpeg);

    let out = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/smoke/snap-record"));
    std::fs::create_dir_all(&out)?;
    let monitor = RecordTarget::Monitor(primary_monitor_index());
    println!("encoders: {:?}", snap_record::encoders());
    println!("microphones: {:?}", snap_record::mic_devices());

    // MP4, 3 s with system audio.
    let mp4 = out.join("screen.mp4");
    let mut mp4_opts = options(monitor.clone(), VideoFormat::Mp4, 30, mp4.clone());
    mp4_opts.microphone = std::env::args().any(|a| a == "--mic");
    let tone = std::env::args().any(|a| a == "--tone").then(|| play_tone(4)).flatten();
    let began = std::time::Instant::now();
    let rec = snap_record::start(mp4_opts)?;
    println!("start() took {} ms", began.elapsed().as_millis());
    wiggle(1500);
    rec.pause();
    std::thread::sleep(Duration::from_millis(500));
    println!("paused: elapsed {} ms, level {:.2}", rec.elapsed_ms(), rec.audio_level());
    rec.resume();
    wiggle(1500);
    let r = rec.stop()?;
    drop(tone);
    println!("mp4 recorded: {} ms, {}x{}, {} bytes", r.duration_ms, r.width, r.height, r.bytes);
    let mp4_info = report("mp4", &mp4)?;

    // GIF of a 1280x720 region, 2 s.
    let gif = out.join("region.gif");
    let rec = snap_record::start(options(RecordTarget::Region(Rect::new(100, 100, 1280, 720)), VideoFormat::Gif, 15, gif.clone()))?;
    wiggle(2000);
    let r = rec.stop()?;
    println!("gif recorded: {} ms, {}x{}, {} bytes", r.duration_ms, r.width, r.height, r.bytes);
    report("gif", &gif)?;

    // WebP too.
    let webp = out.join("region.webp");
    let rec = snap_record::start(options(RecordTarget::Region(Rect::new(100, 100, 1280, 720)), VideoFormat::Webp, 12, webp.clone()))?;
    wiggle(1500);
    let r = rec.stop()?;
    println!("webp recorded: {} ms, {}x{}, {} bytes", r.duration_ms, r.width, r.height, r.bytes);

    // Edit operations.
    let trimmed = out.join("trimmed.mp4");
    edit::trim(&mp4, &trimmed, 500, 1500)?;
    check_audio("trimmed", &mp4_info, &report("trimmed", &trimmed)?)?;

    let small = out.join("small.mp4");
    let size = edit::export(&mp4, &small, &edit::ExportOptions { max_bytes: Some(500 * 1024), ..Default::default() })?;
    println!("export <= 500 KB: {size} bytes");
    check_audio("small", &mp4_info, &report("small", &small)?)?;

    let thumb = edit::frame_at(&mp4, 1000, 640)?;
    thumb.save(out.join("frame.png")).map_err(snap_core::Error::Image)?;
    println!("frame_at: {}x{}", thumb.width(), thumb.height());

    let gif_from_mp4 = out.join("from_mp4.gif");
    let size = edit::export(
        &mp4,
        &gif_from_mp4,
        &edit::ExportOptions { format: VideoFormat::Gif, width: Some(480), fps: Some(10), ..Default::default() },
    )?;
    println!("mp4 -> gif: {size} bytes");

    let webp_from_mp4 = out.join("from_mp4.webp");
    let size = edit::export(
        &mp4,
        &webp_from_mp4,
        &edit::ExportOptions { format: VideoFormat::Webp, width: Some(480), fps: Some(8), range: Some((500, 2000)), ..Default::default() },
    )?;
    println!("mp4 -> webp: {size} bytes");

    // Cancelling removes the partial file.
    let cancelled = out.join("cancelled.mp4");
    let rec = snap_record::start(options(RecordTarget::Region(Rect::new(0, 0, 640, 360)), VideoFormat::Mp4, 30, cancelled.clone()))?;
    wiggle(300);
    rec.cancel();
    println!("cancelled file exists: {}", cancelled.exists());

    // Step-capture slideshow from three differently sized pictures.
    let slide = |w: u32, h: u32, base: [u8; 3]| {
        image::RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([base[0].wrapping_add((x * 255 / w) as u8), base[1].wrapping_add((y * 255 / h) as u8), base[2], 255])
        })
    };
    let slides =
        vec![(slide(1280, 720, [10, 20, 200]), 1200), (slide(800, 800, [200, 30, 30]), 1000), (slide(1920, 1080, [30, 160, 60]), 1200)];
    let show = out.join("slides.mp4");
    snap_record::slideshow::render(&slides, &show, 30)?;
    report("slideshow", &show)?;

    Ok(())
}
