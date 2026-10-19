//! Zoom and pan: the mapping between document pixels and the canvas.
//!
//! `zoom` is physical screen pixels per document pixel, so 1.0 shows the image pixel for pixel on any
//! display density. All "canvas" coordinates here are physical pixels relative to the canvas' top-left.

use egui::{Pos2, Rect, Vec2};
use snap_core::PointF;

pub const MIN_ZOOM: f32 = 0.02;
pub const MAX_ZOOM: f32 = 32.0;
/// Free space around a fitted image, in physical pixels.
const FIT_MARGIN: f32 = 32.0;
/// How much of the image must stay inside the canvas when panning, in physical pixels.
const KEEP_VISIBLE: f32 = 96.0;
/// Zoom steps for the buttons and keys.
const STEP: f32 = 1.25;

#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    pub zoom: f32,
    /// The document point shown at the canvas' top-left corner.
    pub offset: PointF,
    /// Keep the image fitted when the canvas is resized.
    pub fit: bool,
}

impl Viewport {
    pub fn new() -> Self {
        Self { zoom: 1.0, offset: PointF::default(), fit: true }
    }

    /// The zoom that shows the whole image in a canvas of `canvas` physical pixels (never above 100%).
    pub fn fit_zoom(canvas: Vec2, image: (u32, u32)) -> f32 {
        let (iw, ih) = (image.0.max(1) as f32, image.1.max(1) as f32);
        let avail = (canvas - Vec2::splat(FIT_MARGIN * 2.0)).max(Vec2::splat(32.0));
        (avail.x / iw).min(avail.y / ih).clamp(MIN_ZOOM, 1.0)
    }

    pub fn fit_now(&mut self, canvas: Vec2, image: (u32, u32)) {
        self.zoom = Self::fit_zoom(canvas, image);
        self.fit = true;
        self.clamp(canvas, image);
    }

    pub fn actual_size(&mut self, canvas: Vec2, image: (u32, u32)) {
        self.set_zoom(1.0, canvas / 2.0, canvas, image);
    }

    pub fn zoom_step(&mut self, direction: f32, canvas: Vec2, image: (u32, u32)) {
        let factor = if direction > 0.0 { STEP } else { 1.0 / STEP };
        self.set_zoom(self.zoom * factor, canvas / 2.0, canvas, image);
    }

    /// Zoom keeping the document point under `anchor` (canvas pixels) fixed.
    pub fn set_zoom(&mut self, zoom: f32, anchor: Vec2, canvas: Vec2, image: (u32, u32)) {
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let under = PointF::new(self.offset.x + anchor.x / self.zoom, self.offset.y + anchor.y / self.zoom);
        self.zoom = zoom;
        self.offset = PointF::new(under.x - anchor.x / zoom, under.y - anchor.y / zoom);
        self.fit = false;
        self.clamp(canvas, image);
    }

    /// Scroll the picture by `delta` canvas pixels.
    pub fn pan(&mut self, delta: Vec2, canvas: Vec2, image: (u32, u32)) {
        self.offset.x -= delta.x / self.zoom;
        self.offset.y -= delta.y / self.zoom;
        self.fit = false;
        self.clamp(canvas, image);
    }

    /// Re-fit after a resize when fitting, and keep the image reachable otherwise.
    pub fn update(&mut self, canvas: Vec2, image: (u32, u32)) {
        if self.fit {
            self.zoom = Self::fit_zoom(canvas, image);
        }
        self.clamp(canvas, image);
    }

    /// Centre the image along an axis where it is smaller than the canvas, and stop it leaving along the others.
    fn clamp(&mut self, canvas: Vec2, image: (u32, u32)) {
        let axis = |offset: f32, canvas: f32, image: u32| {
            let shown = image as f32 * self.zoom;
            if shown <= canvas {
                -(canvas - shown) / 2.0 / self.zoom
            } else {
                let keep = KEEP_VISIBLE.min(canvas / 2.0);
                let min = -(canvas - keep) / self.zoom;
                let max = image as f32 - keep / self.zoom;
                offset.clamp(min, max)
            }
        };
        self.offset = PointF::new(axis(self.offset.x, canvas.x, image.0), axis(self.offset.y, canvas.y, image.1));
    }

    /// Bring `rect` (document pixels) into view, panning only as far as needed.
    pub fn reveal(&mut self, rect: snap_core::RectF, canvas: Vec2, image: (u32, u32)) {
        let margin = 48.0 / self.zoom;
        let shift = |lo: f32, hi: f32, view_lo: f32, view_len: f32| {
            if lo - margin < view_lo {
                lo - margin - view_lo
            } else if hi + margin > view_lo + view_len {
                hi + margin - (view_lo + view_len)
            } else {
                0.0
            }
        };
        let dx = shift(rect.x, rect.right(), self.offset.x, canvas.x / self.zoom);
        let dy = shift(rect.y, rect.bottom(), self.offset.y, canvas.y / self.zoom);
        if dx != 0.0 || dy != 0.0 {
            self.offset.x += dx;
            self.offset.y += dy;
            self.fit = false;
            self.clamp(canvas, image);
        }
    }

    /// Document point -> screen position, for a canvas drawn at `rect` on a display with `ppp` pixels per point.
    pub fn to_screen(self, rect: Rect, ppp: f32, p: PointF) -> Pos2 {
        let k = self.zoom / ppp;
        Pos2::new(rect.left() + (p.x - self.offset.x) * k, rect.top() + (p.y - self.offset.y) * k)
    }

    /// Screen position -> document point.
    pub fn to_doc(self, rect: Rect, ppp: f32, pos: Pos2) -> PointF {
        let k = ppp / self.zoom;
        PointF::new(self.offset.x + (pos.x - rect.left()) * k, self.offset.y + (pos.y - rect.top()) * k)
    }

    /// Screen-space rectangle of a document rectangle.
    pub fn rect_to_screen(&self, rect: Rect, ppp: f32, r: snap_core::RectF) -> Rect {
        Rect::from_min_max(self.to_screen(rect, ppp, PointF::new(r.x, r.y)), self.to_screen(rect, ppp, PointF::new(r.right(), r.bottom())))
    }

    /// Document pixels covered by `points` screen points.
    pub fn doc_len(&self, ppp: f32, points: f32) -> f32 {
        points * ppp / self.zoom
    }

    /// Displayed zoom in percent as users think of it (100% = one image pixel per screen pixel).
    pub fn percent(&self) -> u32 {
        (self.zoom * 100.0).round() as u32
    }
}
