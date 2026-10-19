use std::fmt;

use image::RgbaImage;
use serde::{Deserialize, Serialize};

use crate::geom::Rect;

/// Sortable, collision-resistant id: `<ms-since-epoch base36>-<6 random base36>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CaptureId(pub String);

impl CaptureId {
    pub fn new() -> Self {
        use rand::Rng;
        let ms = crate::now_ms().max(0) as u64;
        let mut rng = rand::rng();
        let suffix: String = (0..6).map(|_| char::from_digit(rng.random_range(0..36), 36).unwrap()).collect();
        Self(format!("{}-{suffix}", to_base36(ms)))
    }
}

impl Default for CaptureId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for CaptureId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn to_base36(mut n: u64) -> String {
    let mut out = Vec::new();
    loop {
        out.push(char::from_digit((n % 36) as u32, 36).unwrap());
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.iter().rev().collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureKind {
    Region,
    Window,
    Element,
    Monitor,
    AllMonitors,
    Scrolling,
    Clipboard,
    File,
    Recording,
}

/// What was on screen when the capture happened. Feeds history search,
/// default file names, link titles and AI alt-text.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CaptureContext {
    /// Human app name ("Firefox", "Visual Studio Code").
    pub app_name: Option<String>,
    /// Executable path or bundle id.
    pub app_id: Option<String>,
    pub window_title: Option<String>,
    /// Active browser URL when it could be read via accessibility APIs.
    pub url: Option<String>,
    /// Accessible name/role of the captured element (element capture).
    pub element: Option<String>,
    pub monitor_name: Option<String>,
}

/// A captured still image plus everything known about where it came from.
#[derive(Debug, Clone)]
pub struct Capture {
    pub id: CaptureId,
    pub kind: CaptureKind,
    /// Straight (non-premultiplied) sRGB RGBA, physical pixels.
    pub image: RgbaImage,
    /// Where the image sat in virtual-desktop physical pixels.
    pub screen_rect: Rect,
    /// DPI scale of the monitor the capture (mostly) came from. 1.0 = 96 DPI.
    pub scale_factor: f32,
    pub context: CaptureContext,
    pub taken_at_ms: i64,
    /// True when an HDR source was tone-mapped to SDR.
    pub tone_mapped: bool,
}

impl Capture {
    pub fn new(kind: CaptureKind, image: RgbaImage, screen_rect: Rect, scale_factor: f32) -> Self {
        Self {
            id: CaptureId::new(),
            kind,
            image,
            screen_rect,
            scale_factor,
            context: CaptureContext::default(),
            taken_at_ms: crate::now_ms(),
            tone_mapped: false,
        }
    }

    pub fn from_image(kind: CaptureKind, image: RgbaImage) -> Self {
        let rect = Rect::new(0, 0, image.width(), image.height());
        Self::new(kind, image, rect, 1.0)
    }
}
