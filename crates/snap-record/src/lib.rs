//! Screen recording to MP4/GIF/WebP with audio and input overlays, plus a small
//! video toolbox (probe, frame grab, trim, export-to-size, slideshow).
//!
//! Pipeline: a per-OS frame source publishes the newest picture, a pacer ticks at
//! the target frame rate (repeating or skipping pictures to keep time), draws
//! click/keystroke overlays, and hands frames to an encoder sink together with
//! the mixed audio. Pausing freezes the clock, so the output has no gap.
//!
//! `SNAP_RECORD_BACKEND=ffmpeg` forces the ffmpeg encoder instead of the native one (diagnostics).
//!
//! The process should be per-monitor DPI aware (`snap_platform::dpi::init()`) so
//! that rectangles and input coordinates are physical pixels.

use std::path::PathBuf;

pub use snap_core::settings::{EncoderPref, VideoFormat};
use snap_core::{Rect, Result};

mod audio;
mod clock;
mod decode;
pub mod edit;
mod ffdec;
mod ffmpeg;
mod frame;
mod gifdec;
mod gifenc;
mod input;
mod overlay;
mod platform;
mod recorder;
mod sink;
pub mod slideshow;
mod webpenc;

pub use ffmpeg::{ffmpeg_path, set_ffmpeg_enabled, set_ffmpeg_path};
pub use recorder::Recorder;

/// What to record. Coordinates are physical pixels in virtual-desktop space.
#[derive(Debug, Clone, PartialEq)]
pub enum RecordTarget {
    Region(Rect),
    /// Index in the OS monitor enumeration order (same as `snap_capture::monitors()`).
    Monitor(usize),
    /// Native window id (`HWND` on Windows).
    Window(u64),
}

#[derive(Debug, Clone)]
pub struct RecordOptions {
    pub target: RecordTarget,
    pub format: VideoFormat,
    /// Frames per second (GIF/WebP are limited to 50).
    pub fps: u32,
    /// Video bits per second; 0 picks one from the resolution.
    pub bitrate: u32,
    pub system_audio: bool,
    pub microphone: bool,
    /// Input device name from [`mic_devices`]; `None` = default.
    pub mic_device: Option<String>,
    pub show_cursor: bool,
    /// Cursor halo and click ripples.
    pub highlight_clicks: bool,
    pub show_keystrokes: bool,
    pub encoder: EncoderPref,
    /// Width limit for GIF and animated WebP output.
    pub max_gif_width: u32,
    pub output: PathBuf,
    /// ffmpeg binary for this recording; `None` = configured path, next to the exe, then PATH.
    pub ffmpeg: Option<PathBuf>,
    /// Native window ids (of this process) to leave out of the capture, e.g. the recorder HUD.
    pub exclude_windows: Vec<u64>,
    /// Leave Finder's desktop icons out of the capture (macOS). Elsewhere the caller hides them on screen for the
    /// whole recording through `snap_capture::hide_desktop_icons`.
    pub hide_desktop_icons: bool,
}

/// The finished file.
#[derive(Debug, Clone, PartialEq)]
pub struct Recording {
    pub path: PathBuf,
    pub duration_ms: u64,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub format: VideoFormat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderInfo {
    pub name: String,
    pub hardware: bool,
}

/// Start recording. Returns once frames are flowing and the encoder is ready.
pub fn start(o: RecordOptions) -> Result<Recorder> {
    recorder::start(o)
}

/// H.264 encoders available natively and through ffmpeg (hardware first).
pub fn encoders() -> Vec<EncoderInfo> {
    let mut all = platform::native_encoders();
    if let Some(ff) = ffmpeg::ffmpeg_path() {
        all.extend(ffmpeg::working_encoders(&ff).iter().map(|e| EncoderInfo { name: format!("ffmpeg: {}", e.name), hardware: e.hardware }));
    }
    all.sort_by_key(|e| !e.hardware);
    all
}

/// Names of input (microphone) devices, the default first.
pub fn mic_devices() -> Vec<String> {
    platform::mic_devices().unwrap_or_else(audio::input_device_names)
}
