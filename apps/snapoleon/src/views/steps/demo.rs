//! Demo data for the dev harness. `SNAPOLEON_DEV_DEMO=1` makes the HUD record a simulated session
//! from real crops of the screen; `SNAPOLEON_DEV_DEMO=guide` opens the guide editor on a finished
//! demo guide. The real engine recorder is used otherwise.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use image::{RgbaImage, imageops};
use snap_capture::CaptureOptions;
use snap_core::{CaptureContext, Point};
use snap_engine::steps::{Guide, StepShot};

use crate::views::ViewHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Demo {
    Session,
    Guide,
}

pub fn mode() -> Option<Demo> {
    let value = std::env::var("SNAPOLEON_DEV_DEMO").ok()?;
    Some(if value == "guide" { Demo::Guide } else { Demo::Session })
}

const CAPTIONS: [(&str, &str, &str); 6] = [
    ("Open the project settings", "Firefox", "Snapoleon: screenshots without the weight"),
    ("Click \u{201c}Pipelines\u{201d} in the sidebar", "Snapoleon", "Settings"),
    ("Press \u{201c}Add pipeline\u{201d}", "Snapoleon", "Settings"),
    ("Name the pipeline and pick its steps", "Visual Studio Code", "main.rs - snapoleon"),
    ("Choose where the result should go", "Telegram", "Design review"),
    ("Press Save to finish", "Notion", "Release checklist"),
];

fn screen() -> Option<RgbaImage> {
    let monitor = snap_capture::monitors().ok()?.into_iter().find(|m| m.is_primary)?;
    snap_capture::capture_monitor(monitor.index, &CaptureOptions::default()).ok().map(|c| c.image)
}

/// The `i`-th demo step: a crop of the screen, a caption and a click point inside the crop.
fn shot(screen: &RgbaImage, i: usize) -> StepShot {
    let (w, h) = screen.dimensions();
    let (cw, ch) = (w.min(1280), h.min(720));
    let x = (i as u32 * 173) % (w - cw + 1);
    let y = (i as u32 * 97) % (h - ch + 1);
    let (caption, app, title) = CAPTIONS[i % CAPTIONS.len()];
    StepShot {
        image: imageops::crop_imm(screen, x, y, cw, ch).to_image(),
        caption: caption.to_owned(),
        click: Some(Point::new((cw as f32 * (0.28 + 0.09 * (i % 5) as f32)) as i32, (ch as f32 * (0.3 + 0.07 * (i % 4) as f32)) as i32)),
        context: CaptureContext { app_name: Some(app.into()), window_title: Some(title.into()), ..CaptureContext::default() },
        taken_at_ms: snap_core::now_ms(),
    }
}

/// A finished guide of six steps.
pub fn guide() -> Guide {
    let steps = screen().map(|s| (0..CAPTIONS.len()).map(|i| shot(&s, i)).collect()).unwrap_or_default();
    Guide {
        title: "Set up a pipeline".into(),
        intro: "Pipelines run the same steps after every capture. This guide builds one that annotates, saves and uploads.".into(),
        steps,
    }
}

/// A simulated recorder: one step every second until stopped.
pub struct Feed {
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
}

impl Feed {
    pub fn start(handle: ViewHandle) -> Self {
        let (stop, paused) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let (stop_flag, pause_flag) = (stop.clone(), paused.clone());
        std::thread::spawn(move || {
            let Some(screen) = screen() else { return };
            let mut i = 0;
            while !stop_flag.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(900));
                if !pause_flag.load(Ordering::Relaxed) {
                    handle.send(super::session::Shot::new(shot(&screen, i)));
                    i += 1;
                }
            }
        });
        Self { stop, paused }
    }

    pub fn pause(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
