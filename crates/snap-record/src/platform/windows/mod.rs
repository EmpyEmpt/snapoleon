//! Windows backend: Windows.Graphics.Capture frames, Media Foundation encoding/decoding.

use std::time::{Duration, Instant};

use snap_core::Point;
use windows::Win32::{
    Foundation::POINT,
    Media::{timeBeginPeriod, timeEndPeriod},
    UI::WindowsAndMessaging::GetCursorPos,
};

mod capture;
pub(crate) mod mf;

pub(crate) use capture::open_source;
pub(crate) use mf::{native_encoders, native_mp4_sink, open_decoder};

pub(crate) fn mic_devices() -> Option<Vec<String>> {
    None
}

pub(crate) fn cursor_position() -> Option<Point> {
    let mut p = POINT::default();
    // SAFETY: plain out-parameter call.
    unsafe { GetCursorPos(&mut p) }.ok().map(|()| Point::new(p.x, p.y))
}

/// Raises the system timer resolution to 1 ms while alive so pacing is accurate.
pub(crate) struct TimerResolution;

impl TimerResolution {
    pub fn raise() -> Self {
        // SAFETY: paired with `timeEndPeriod` in Drop.
        unsafe { timeBeginPeriod(1) };
        Self
    }
}

impl Drop for TimerResolution {
    fn drop(&mut self) {
        // SAFETY: matches the `timeBeginPeriod(1)` in `raise`.
        unsafe { timeEndPeriod(1) };
    }
}

pub(crate) fn sleep_until(deadline: Instant) {
    loop {
        let Some(left) = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) else { return };
        if left > Duration::from_millis(2) {
            std::thread::sleep(left - Duration::from_millis(1));
        } else {
            std::thread::yield_now();
        }
    }
}
