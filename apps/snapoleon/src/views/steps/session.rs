//! A running capture session: the engine's recorder, or the simulated one of the dev harness.

use image::RgbaImage;
use snap_core::Result;
use snap_engine::steps::{StepRecorder, StepShot};
use snap_image::resize::thumbnail;

use super::demo;
use crate::views::ViewCx;

/// Longest side of the thumbnails kept next to each shot.
pub const THUMB_SIDE: u32 = 720;

/// One captured step as it reaches the HUD: the shot and a small copy made off the UI thread.
pub struct Shot {
    pub shot: StepShot,
    pub thumb: RgbaImage,
}

impl Shot {
    pub fn new(shot: StepShot) -> Self {
        let thumb = thumbnail(&shot.image, THUMB_SIDE);
        Self { shot, thumb }
    }
}

pub enum Session {
    Live(StepRecorder),
    Demo(demo::Feed),
}

impl Session {
    /// Start capturing; every shot arrives as a [`Shot`] message to the calling window.
    pub fn start(cx: &ViewCx) -> Result<Self> {
        let handle = cx.handle();
        if demo::mode().is_some() {
            return Ok(Self::Demo(demo::Feed::start(handle)));
        }
        StepRecorder::start(cx.engine, true, move |shot| handle.send(Shot::new(shot))).map(Self::Live)
    }

    pub fn snap_now(&self) {
        if let Self::Live(recorder) = self {
            recorder.snap_now();
        }
    }

    pub fn pause(&self, paused: bool) {
        match self {
            Self::Live(recorder) => recorder.pause(paused),
            Self::Demo(feed) => feed.pause(paused),
        }
    }

    pub fn stop(self) {
        match self {
            Self::Live(recorder) => recorder.stop(),
            Self::Demo(feed) => feed.stop(),
        }
    }
}
