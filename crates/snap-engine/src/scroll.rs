//! Scrolling capture: scroll the content under a rectangle with the mouse wheel and stitch the frames.

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use snap_capture::CaptureOptions;
use snap_core::{Capture, CaptureKind, Error, Point, Rect, Result, RgbaImage, t};
use snap_image::stitch::{PushOutcome, Stitcher};

use crate::{Engine, ScrollProgress};

/// Stitched pages stop growing here.
const MAX_HEIGHT: u32 = 30_000;
/// Safety net against a page that keeps producing "new" rows (infinite feeds, animations).
const MAX_FRAMES: u32 = 400;
/// Time for the content to settle after a wheel event (smooth scrolling, lazy-loaded images).
const SETTLE: Duration = Duration::from_millis(220);
/// Wheel notches for the first scroll, before we know how far a notch moves this particular content.
const PROBE_NOTCHES: i32 = 2;
const MAX_NOTCHES: i32 = 40;
const PREVIEW_HEIGHT: u32 = 1600;
/// Smallest area worth scrolling.
const MIN_SIZE: (u32, u32) = (40, 120);

pub(crate) fn scrolling(engine: &Engine, rect: Rect, cancel: &AtomicBool, on_progress: &dyn Fn(&ScrollProgress)) -> Result<Capture> {
    if rect.w < MIN_SIZE.0 || rect.h < MIN_SIZE.1 {
        return Err(Error::invalid(t("engine.error.scroll_too_small")));
    }
    // A cursor sprite in every frame would stamp itself into the stitched page.
    let opts = CaptureOptions { include_cursor: false, ..engine.capture_options() };
    let first = snap_capture::capture_rect(rect, &opts)?;
    let centre = Point::new(rect.x + (rect.w / 2) as i32, rect.y + (rect.h / 2) as i32);
    let mut first_frame = Some(first.image.clone());
    let image = stitch_scroll(
        rect.h,
        || match first_frame.take() {
            Some(frame) => Ok(frame),
            None => Ok(snap_capture::capture_rect(rect, &opts)?.image),
        },
        |notches| {
            snap_capture::scroll_at(centre, notches)?;
            std::thread::sleep(SETTLE);
            Ok(())
        },
        cancel,
        on_progress,
    )?;
    let screen_rect = Rect::new(rect.x, rect.y, rect.w, image.height());
    let mut cap = Capture::new(CaptureKind::Scrolling, image, screen_rect, first.scale_factor);
    cap.context = first.context;
    cap.tone_mapped = first.tone_mapped;
    Ok(cap)
}

/// The scroll-and-stitch loop behind [`Engine::scrolling`](crate::Engine::scrolling), independent of the screen:
/// `grab` returns the current viewport (the first call must return the unscrolled view), `scroll(n)` moves the
/// content down by `n` wheel notches and returns once it has settled. Stops when the content stops moving, on
/// `cancel`, after too many unmatched frames, or at the height cap.
pub fn stitch_scroll(
    viewport_h: u32,
    mut grab: impl FnMut() -> Result<RgbaImage>,
    mut scroll: impl FnMut(i32) -> Result<()>,
    cancel: &AtomicBool,
    on_progress: &dyn Fn(&ScrollProgress),
) -> Result<RgbaImage> {
    let mut stitcher = Stitcher::new();
    stitcher.push(&grab()?);
    let report = |stitcher: &Stitcher, frames: u32| {
        on_progress(&ScrollProgress { preview: stitcher.preview(PREVIEW_HEIGHT), height: stitcher.height(), frames });
    };
    report(&stitcher, 1);

    // Aim for a third of the viewport per scroll: plenty of overlap for the stitcher, few frames overall.
    let target_px = viewport_h as f32 / 3.0;
    let mut notches = PROBE_NOTCHES;
    let mut calibrate = true;
    let (mut frames, mut still, mut lost) = (1u32, 0u32, 0u32);

    while !cancel.load(Ordering::Relaxed) && frames < MAX_FRAMES && stitcher.height() < MAX_HEIGHT {
        match scroll(notches) {
            Ok(()) => {}
            Err(e) if frames == 1 => return Err(e),
            Err(e) => {
                tracing::warn!("scrolling stopped: {e}");
                break;
            }
        }
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let frame = grab()?;
        frames += 1;
        let outcome = stitcher.push(&frame);
        tracing::debug!("scrolled {notches} notches: {outcome:?}");
        match outcome {
            PushOutcome::Appended { rows } => {
                (still, lost) = (0, 0);
                if calibrate {
                    let px_per_notch = rows as f32 / notches as f32;
                    notches = ((target_px / px_per_notch.max(1.0)).round() as i32).clamp(1, MAX_NOTCHES);
                }
                calibrate = true;
                report(&stitcher, frames);
            }
            PushOutcome::NoChange => {
                // The content may simply not have moved yet; the next scroll then moves twice as far,
                // which says nothing about the size of a single notch.
                calibrate = false;
                still += 1;
                if still >= 2 {
                    break;
                }
            }
            PushOutcome::NoOverlap => {
                lost += 1;
                if lost >= 3 {
                    break;
                }
                notches = (notches / 2).max(1);
            }
        }
    }
    Ok(stitcher.finish())
}
