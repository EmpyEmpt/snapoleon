//! Per-OS backends. Every function compiles everywhere; where no backend can
//! exist the result is `Error::Unsupported`.

use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Instant,
};

use parking_lot::Mutex;
use snap_core::{Point, Result};

use crate::{EncoderInfo, RecordTarget, audio, frame::Frame};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
pub(crate) mod windows;

#[cfg(windows)]
use self::windows as imp;
#[cfg(target_os = "linux")]
use linux as imp;
#[cfg(target_os = "macos")]
use macos as imp;
// Event taps replace the cross-platform listener on macOS only (see `input`).
#[cfg(target_os = "macos")]
pub(crate) use macos::{InputTaps, listen_input};

/// Where sources publish their newest frame.
#[derive(Default)]
pub(crate) struct FrameSlot(Mutex<Option<Arc<Frame>>>);

impl FrameSlot {
    pub fn set(&self, frame: Frame) {
        *self.0.lock() = Some(Arc::new(frame));
    }

    pub fn get(&self) -> Option<Arc<Frame>> {
        self.0.lock().clone()
    }
}

#[allow(dead_code)] // every backend reads a different subset of these fields
pub(crate) struct SourceSpec<'a> {
    pub target: &'a RecordTarget,
    pub show_cursor: bool,
    pub exclude_windows: &'a [u64],
    pub hide_desktop_icons: bool,
    pub system_audio: bool,
    pub fps: u32,
    pub ffmpeg: Option<&'a std::path::Path>,
}

/// Position of the recorded area's top-left pixel in virtual-desktop space (it moves for window targets).
pub(crate) type OriginFn = Arc<dyn Fn() -> Point + Send + Sync>;

/// A running capture. All frames it publishes are exactly [`FrameSource::size`] BGRA pixels.
pub(crate) trait FrameSource: Send {
    fn size(&self) -> (u32, u32);
    fn origin_fn(&self) -> OriginFn;
    /// Multiplier from global input coordinates to frame pixels.
    fn input_scale(&self) -> f32 {
        1.0
    }
    /// A platform specific audio feed (ScreenCaptureKit, PulseAudio). `None` means "use cpal".
    fn audio_source(&mut self, _kind: audio::Kind, _device: Option<&str>, _stop: Arc<AtomicBool>) -> Option<Result<audio::Source>> {
        None
    }
    fn stop(self: Box<Self>);
}

pub(crate) fn open_source(spec: &SourceSpec, slot: Arc<FrameSlot>) -> Result<Box<dyn FrameSource>> {
    imp::open_source(spec, slot)
}

pub(crate) fn native_mp4_sink(spec: &crate::sink::SinkSpec) -> Result<Box<dyn crate::sink::Sink>> {
    imp::native_mp4_sink(spec)
}

pub(crate) fn cursor_position() -> Option<Point> {
    imp::cursor_position()
}

/// Microphone names where the OS has a better source of truth than cpal.
pub(crate) fn mic_devices() -> Option<Vec<String>> {
    imp::mic_devices()
}

pub(crate) fn native_encoders() -> Vec<EncoderInfo> {
    imp::native_encoders()
}

/// Sleep until `deadline`, as precisely as the OS allows.
pub(crate) fn sleep_until(deadline: Instant) {
    imp::sleep_until(deadline)
}

pub(crate) fn open_decoder(path: &std::path::Path, with_audio: bool) -> Result<Box<dyn crate::decode::Decoder>> {
    imp::open_decoder(path, with_audio)
}
