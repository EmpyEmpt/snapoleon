//! A stand-in for `Engine::scrolling` in the dev harness (`SNAPOLEON_DEV_DEMO=1`): "scrolls" down
//! the real screen below the area, so the panel shows live progress without a scrollable window.

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use snap_capture::CaptureOptions;
use snap_core::{Capture, CaptureKind, Error, Rect, Result};
use snap_engine::ScrollProgress;
use snap_image::resize::fit;

pub fn enabled() -> bool {
    std::env::var_os("SNAPOLEON_DEV_DEMO").is_some()
}

pub fn run(rect: Rect, cancel: &AtomicBool, on_progress: &dyn Fn(&ScrollProgress)) -> Result<Capture> {
    let monitor = snap_capture::monitors()?.into_iter().find(|m| m.is_primary).ok_or_else(|| Error::NotFound("monitor".into()))?;
    let screen = snap_capture::capture_monitor(monitor.index, &CaptureOptions::default())?.image;
    let (x, y) = (rect.x.max(0) as u32, rect.y.max(0) as u32);
    let width = rect.w.min(screen.width().saturating_sub(x)).max(1);
    let limit = screen.height().saturating_sub(y).max(1);
    let mut height = rect.h.min(limit);
    let mut frames = 1;
    loop {
        let page = image::imageops::crop_imm(&screen, x, y, width, height).to_image();
        on_progress(&ScrollProgress { preview: fit(&page, None, Some(1600), false), height, frames });
        if cancel.load(Ordering::Relaxed) || height >= limit {
            return Ok(Capture::new(CaptureKind::Scrolling, page, rect, 1.0));
        }
        std::thread::sleep(Duration::from_millis(500));
        height = (height + rect.h / 12).min(limit);
        frames += 1;
    }
}
