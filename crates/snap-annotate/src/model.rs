//! The annotation document: a base image plus a list of vector shapes.

use std::collections::VecDeque;
use std::sync::Arc;

use image::RgbaImage;
use serde::{Deserialize, Serialize};
use snap_core::settings::{BeautifyPreset, RedactStyle};
use snap_core::{Capture, CaptureContext, CaptureId, PointF, RectF, Rect};

use crate::cache::ImageCache;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Style {
    /// Primary color: lines, outlines, text, badge and bubble fills.
    pub stroke: [u8; 4],
    /// Interior fill for rectangles, ellipses and bubbles. Arrows and step badges use it as
    /// a halo/ring color instead (typically white).
    pub fill: Option<[u8; 4]>,
    /// Line width in document pixels.
    pub width: f32,
    pub opacity: f32,
    pub shadow: bool,
    pub dashed: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self { stroke: [255, 59, 48, 255], fill: None, width: 4.0, opacity: 1.0, shadow: false, dashed: false }
    }
}

impl Style {
    pub fn with_color(color: [u8; 4]) -> Self {
        Self { stroke: color, ..Self::default() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArrowHead {
    #[default]
    Triangle,
    Open,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextBg {
    #[default]
    None,
    /// Rounded padded box in the fill color (stroke color when there is no fill).
    Solid,
    /// Speech bubble with a small tail.
    Bubble,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stamp {
    Check,
    Cross,
    Question,
    Warning,
    Star,
    Heart,
    ThumbsUp,
    Info,
}

impl Stamp {
    pub const ALL: [Stamp; 8] =
        [Stamp::Check, Stamp::Cross, Stamp::Question, Stamp::Warning, Stamp::Star, Stamp::Heart, Stamp::ThumbsUp, Stamp::Info];

    /// The color a freshly placed stamp of this kind should get.
    pub fn default_color(self) -> [u8; 4] {
        match self {
            Stamp::Check => [52, 199, 89, 255],
            Stamp::Cross => [255, 59, 48, 255],
            Stamp::Question => [10, 132, 255, 255],
            Stamp::Warning => [255, 159, 10, 255],
            Stamp::Star => [255, 204, 0, 255],
            Stamp::Heart => [255, 55, 95, 255],
            Stamp::ThumbsUp => [10, 132, 255, 255],
            Stamp::Info => [90, 200, 250, 255],
        }
    }

    pub fn label(self) -> String {
        let key = match self {
            Stamp::Check => "annotate.stamp.check",
            Stamp::Cross => "annotate.stamp.cross",
            Stamp::Question => "annotate.stamp.question",
            Stamp::Warning => "annotate.stamp.warning",
            Stamp::Star => "annotate.stamp.star",
            Stamp::Heart => "annotate.stamp.heart",
            Stamp::ThumbsUp => "annotate.stamp.thumbs_up",
            Stamp::Info => "annotate.stamp.info",
        };
        snap_core::t(key).into_owned()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ShapeKind {
    Arrow {
        from: PointF,
        to: PointF,
        /// Quadratic control point; `None` = straight.
        ctrl: Option<PointF>,
        head: ArrowHead,
        double: bool,
    },
    Line {
        from: PointF,
        to: PointF,
    },
    Rect {
        rect: RectF,
        radius: f32,
    },
    Ellipse {
        rect: RectF,
    },
    Pen {
        points: Vec<PointF>,
    },
    Highlighter {
        points: Vec<PointF>,
    },
    Text {
        /// Top-left of the text itself; a background grows outwards from here.
        pos: PointF,
        text: String,
        size: f32,
        bold: bool,
        bg: TextBg,
        max_width: Option<f32>,
    },
    Callout {
        rect: RectF,
        tail: PointF,
        text: String,
        size: f32,
    },
    Step {
        center: PointF,
        number: u32,
        radius: f32,
    },
    Redact {
        rect: RectF,
        style: RedactStyle,
    },
    Spotlight {
        rect: RectF,
        ellipse: bool,
    },
    Magnifier {
        source: RectF,
        center: PointF,
        radius: f32,
    },
    Image {
        rect: RectF,
        /// Stored as `assets/<shape id>.png` inside `.snap` files, not in `doc.json`.
        #[serde(skip, default = "blank_image")]
        image: Arc<RgbaImage>,
    },
    Stamp {
        center: PointF,
        size: f32,
        glyph: Stamp,
    },
    Measure {
        from: PointF,
        to: PointF,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub id: u64,
    pub kind: ShapeKind,
    pub style: Style,
    pub hidden: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DocMeta {
    pub capture_id: Option<CaptureId>,
    pub context: CaptureContext,
    pub created_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Document {
    /// Full-resolution capture, never modified by annotations.
    #[serde(skip, default = "blank_image")]
    pub base: Arc<RgbaImage>,
    pub scale_factor: f32,
    /// Export crop in base-image pixels.
    pub crop: Option<RectF>,
    pub shapes: Vec<Shape>,
    pub beautify: Option<BeautifyPreset>,
    pub meta: DocMeta,
    next_id: u64,
    #[serde(skip)]
    pub(crate) cache: Arc<ImageCache>,
}

pub(crate) fn blank_image() -> Arc<RgbaImage> {
    Arc::new(RgbaImage::new(1, 1))
}

impl Document {
    pub fn new(base: RgbaImage, scale_factor: f32) -> Self {
        Self {
            base: Arc::new(base),
            scale_factor,
            crop: None,
            shapes: Vec::new(),
            beautify: None,
            meta: DocMeta { created_ms: snap_core::now_ms(), ..DocMeta::default() },
            next_id: 1,
            cache: Arc::default(),
        }
    }

    pub fn from_capture(c: &Capture) -> Self {
        let mut doc = Self::new(c.image.clone(), c.scale_factor);
        doc.meta = DocMeta { capture_id: Some(c.id.clone()), context: c.context.clone(), created_ms: c.taken_at_ms };
        doc
    }

    /// Size of the document space: the base image, regardless of any export crop.
    pub fn canvas_size(&self) -> (u32, u32) {
        (self.base.width(), self.base.height())
    }

    /// The crop in whole base pixels, clamped to the image; the full image when unset or empty.
    pub fn crop_rect(&self) -> Rect {
        let (bw, bh) = self.canvas_size();
        let full = Rect::new(0, 0, bw, bh);
        let Some(c) = self.crop else { return full };
        let x0 = c.x.round().clamp(0.0, bw as f32) as i32;
        let y0 = c.y.round().clamp(0.0, bh as f32) as i32;
        let x1 = c.right().round().clamp(0.0, bw as f32) as i32;
        let y1 = c.bottom().round().clamp(0.0, bh as f32) as i32;
        if x1 <= x0 || y1 <= y0 { full } else { Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32) }
    }

    /// Pixel size of [`crate::render`] output before beautify, at `scale`.
    pub fn export_size(&self, scale: f32) -> (u32, u32) {
        let c = self.crop_rect();
        crate::render::scaled_size(c.w, c.h, scale)
    }

    /// Next free step-badge number (one more than the highest in use).
    pub fn next_step_number(&self) -> u32 {
        self.shapes
            .iter()
            .filter_map(|s| match s.kind {
                ShapeKind::Step { number, .. } => Some(number),
                _ => None,
            })
            .max()
            .map_or(1, |n| n + 1)
    }

    pub fn add(&mut self, kind: ShapeKind, style: Style) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.shapes.push(Shape { id, kind, style, hidden: false });
        id
    }

    pub fn get(&self, id: u64) -> Option<&Shape> {
        self.shapes.iter().find(|s| s.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Shape> {
        self.shapes.iter_mut().find(|s| s.id == id)
    }

    pub fn remove(&mut self, id: u64) {
        self.shapes.retain(|s| s.id != id);
    }

    pub fn bring_to_front(&mut self, id: u64) {
        if let Some(i) = self.shapes.iter().position(|s| s.id == id) {
            let s = self.shapes.remove(i);
            self.shapes.push(s);
        }
    }

    pub fn send_to_back(&mut self, id: u64) {
        if let Some(i) = self.shapes.iter().position(|s| s.id == id) {
            let s = self.shapes.remove(i);
            self.shapes.insert(0, s);
        }
    }

    /// One step towards the front.
    pub fn raise(&mut self, id: u64) {
        if let Some(i) = self.shapes.iter().position(|s| s.id == id)
            && i + 1 < self.shapes.len()
        {
            self.shapes.swap(i, i + 1);
        }
    }

    /// One step towards the back.
    pub fn lower(&mut self, id: u64) {
        if let Some(i) = self.shapes.iter().position(|s| s.id == id)
            && i > 0
        {
            self.shapes.swap(i, i - 1);
        }
    }

    /// Makes sure freshly added ids cannot collide with loaded shapes.
    pub(crate) fn fix_next_id(&mut self) {
        let max = self.shapes.iter().map(|s| s.id).max().unwrap_or(0);
        self.next_id = self.next_id.max(max + 1);
    }
}

const MAX_UNDO: usize = 200;

#[derive(Clone, Debug)]
struct Snapshot {
    shapes: Vec<Shape>,
    crop: Option<RectF>,
    beautify: Option<BeautifyPreset>,
}

impl Snapshot {
    fn of(doc: &Document) -> Self {
        Self { shapes: doc.shapes.clone(), crop: doc.crop, beautify: doc.beautify.clone() }
    }

    fn restore(self, doc: &mut Document) {
        doc.shapes = self.shapes;
        doc.crop = self.crop;
        doc.beautify = self.beautify;
    }
}

/// Snapshot-based undo/redo of shapes, crop and beautify. Call [`Undo::checkpoint`] *before*
/// every user-visible change; snapshots are cheap because the base image and pasted images are `Arc`s.
#[derive(Debug, Default)]
pub struct Undo {
    past: VecDeque<Snapshot>,
    future: Vec<Snapshot>,
}

impl Undo {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn checkpoint(&mut self, doc: &Document) {
        if self.past.len() == MAX_UNDO {
            self.past.pop_front();
        }
        self.past.push_back(Snapshot::of(doc));
        self.future.clear();
    }

    pub fn undo(&mut self, doc: &mut Document) -> bool {
        let Some(prev) = self.past.pop_back() else { return false };
        self.future.push(Snapshot::of(doc));
        prev.restore(doc);
        true
    }

    pub fn redo(&mut self, doc: &mut Document) -> bool {
        let Some(next) = self.future.pop() else { return false };
        self.past.push_back(Snapshot::of(doc));
        next.restore(doc);
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    pub fn clear(&mut self) {
        self.past.clear();
        self.future.clear();
    }
}
