//! Step capture ("docs mode"): a session of screenshots with captions and
//! click points, exported as Markdown, a self-contained HTML guide, an MP4
//! slideshow, or rewritten into a how-to by the AI.

use std::path::{Path, PathBuf};

use snap_core::{CaptureContext, Point, Result, RgbaImage};

use crate::Engine;

#[derive(Debug, Clone)]
pub struct StepShot {
    pub image: RgbaImage,
    pub caption: String,
    /// Click point in image pixels, drawn as a ring in exports.
    pub click: Option<Point>,
    pub context: CaptureContext,
    pub taken_at_ms: i64,
}

#[derive(Debug, Clone, Default)]
pub struct Guide {
    pub title: String,
    pub intro: String,
    pub steps: Vec<StepShot>,
}

/// Writes `<dir>/<slug>.md` plus `<dir>/<slug>_images/NN.png`; returns the .md path.
pub fn export_markdown(guide: &Guide, dir: &Path) -> Result<PathBuf> {
    let _ = (guide, dir);
    Err(crate::todo())
}

/// One HTML file with images inlined as data URIs.
pub fn export_html(guide: &Guide, path: &Path) -> Result<()> {
    let _ = (guide, path);
    Err(crate::todo())
}

/// MP4 slideshow, `secs_per_step` per shot, captions burned in.
pub fn export_video(guide: &Guide, path: &Path, secs_per_step: u32) -> Result<()> {
    let _ = (guide, path, secs_per_step);
    Err(crate::todo())
}

/// Rewrite title, intro and captions with the AI.
pub fn ai_howto(engine: &Engine, guide: &mut Guide) -> Result<()> {
    let _ = (engine, guide);
    Err(crate::todo())
}

/// A running session. Captures automatically on each mouse click where the
/// platform allows (the shot is taken just before the click lands), and on
/// demand via [`StepRecorder::snap_now`].
pub struct StepRecorder {
    _priv: (),
}

impl StepRecorder {
    /// `on_shot` runs on a worker thread.
    pub fn start(engine: &Engine, auto_on_click: bool, on_shot: impl Fn(StepShot) + Send + Sync + 'static) -> Result<Self> {
        let _ = (engine, auto_on_click, on_shot);
        Err(crate::todo())
    }
    pub fn snap_now(&self) {}
    pub fn pause(&self, paused: bool) {
        let _ = paused;
    }
    pub fn stop(self) {}
}
