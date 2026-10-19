//! The crop tool's working rectangle and the maths for dragging it.

use snap_annotate::{Handle, Shape, ShapeKind, Style, drag, handles};
use snap_core::{PointF, RectF, t};

use super::session::Session;

/// Smallest crop, in image pixels.
const MIN_SIDE: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Aspect {
    Free,
    Original,
    Square,
    FourThree,
    SixteenNine,
    ThreeTwo,
}

impl Aspect {
    pub const ALL: [Aspect; 6] = [Aspect::Free, Aspect::Original, Aspect::Square, Aspect::FourThree, Aspect::SixteenNine, Aspect::ThreeTwo];

    pub fn label(self) -> String {
        match self {
            Aspect::Free => t("editor.crop.free").into_owned(),
            Aspect::Original => t("editor.crop.original").into_owned(),
            Aspect::Square => "1:1".into(),
            Aspect::FourThree => "4:3".into(),
            Aspect::SixteenNine => "16:9".into(),
            Aspect::ThreeTwo => "3:2".into(),
        }
    }

    /// Width over height, `None` for a free crop.
    fn ratio(self, image: (u32, u32)) -> Option<f32> {
        match self {
            Aspect::Free => None,
            Aspect::Original => Some(image.0 as f32 / image.1.max(1) as f32),
            Aspect::Square => Some(1.0),
            Aspect::FourThree => Some(4.0 / 3.0),
            Aspect::SixteenNine => Some(16.0 / 9.0),
            Aspect::ThreeTwo => Some(3.0 / 2.0),
        }
    }
}

enum Drag {
    Handle(Handle),
    /// Moving the whole rectangle.
    Move,
    /// Dragging out a new rectangle from this corner.
    New(PointF),
}

pub struct Crop {
    pub active: bool,
    /// The rectangle being edited, in image pixels.
    pub rect: RectF,
    pub aspect: Aspect,
    drag: Option<Drag>,
}

impl Crop {
    pub fn new() -> Self {
        Self { active: false, rect: RectF::default(), aspect: Aspect::Free, drag: None }
    }

    /// Start editing from the document's current crop (the whole image when there is none).
    pub fn begin(&mut self, s: &Session) {
        let r = s.doc.crop_rect();
        self.rect = r.to_f();
        self.active = true;
        self.drag = None;
    }

    pub fn cancel(&mut self) {
        self.active = false;
        self.drag = None;
    }

    /// Commit the rectangle as the document's export crop. The whole image clears the crop.
    pub fn apply(&mut self, s: &mut Session) {
        let (w, h) = s.doc.canvas_size();
        let r = clamp_to(self.rect, w, h);
        let full = r.x <= 0.5 && r.y <= 0.5 && (r.w - w as f32).abs() <= 1.0 && (r.h - h as f32).abs() <= 1.0;
        let crop = (!full).then_some(RectF::new(r.x.round(), r.y.round(), r.w.round(), r.h.round()));
        if crop != s.doc.crop {
            s.checkpoint();
            s.doc.crop = crop;
            s.touch();
        }
        self.cancel();
    }

    pub fn reset(&mut self, s: &mut Session) {
        if s.doc.crop.is_some() {
            s.checkpoint();
            s.doc.crop = None;
            s.touch();
        }
        self.cancel();
    }

    /// Where the 8 resize handles are.
    pub fn handle_points(&self) -> Vec<(Handle, PointF)> {
        handles(&probe(self.rect))
    }

    /// A press at `p`. `near` is the handle under the pointer, if any. Inside a crop that already is the
    /// whole image there is nothing to move, so the press starts a new rectangle.
    pub fn press(&mut self, p: PointF, near: Option<Handle>, image: (u32, u32)) {
        let whole = self.rect.w >= image.0 as f32 - 1.0 && self.rect.h >= image.1 as f32 - 1.0;
        self.drag = Some(match near {
            Some(h) => Drag::Handle(h),
            None if self.rect.contains(p) && !whole => Drag::Move,
            None => Drag::New(p),
        });
    }

    /// The pointer moved by `delta` and is now at `p`.
    pub fn drag_to(&mut self, p: PointF, delta: PointF, image: (u32, u32)) {
        let (w, h) = image;
        let ratio = self.aspect.ratio(image);
        match self.drag {
            Some(Drag::Move) => {
                let mut r = self.rect;
                r.x = (r.x + delta.x).clamp(0.0, (w as f32 - r.w).max(0.0));
                r.y = (r.y + delta.y).clamp(0.0, (h as f32 - r.h).max(0.0));
                self.rect = r;
            }
            Some(Drag::Handle(handle)) => {
                let mut shape = probe(self.rect);
                drag(&mut shape, handle, delta);
                let mut r = match shape.kind {
                    ShapeKind::Rect { rect, .. } => rect,
                    _ => return,
                };
                if let Some(ratio) = ratio {
                    r = lock_ratio(r, self.rect, handle, ratio);
                }
                let inside = r.x >= -0.5 && r.y >= -0.5 && r.right() <= w as f32 + 0.5 && r.bottom() <= h as f32 + 0.5;
                if ratio.is_none() {
                    r = clamp_to(r, w, h);
                } else if !inside {
                    return;
                }
                if r.w >= MIN_SIDE && r.h >= MIN_SIDE {
                    self.rect = r;
                }
            }
            Some(Drag::New(start)) => {
                let end = PointF::new(p.x.clamp(0.0, w as f32), p.y.clamp(0.0, h as f32));
                let start = PointF::new(start.x.clamp(0.0, w as f32), start.y.clamp(0.0, h as f32));
                let mut r = RectF::from_corners(start, end);
                if let Some(ratio) = ratio {
                    let side_w = r.w.max(r.h * ratio);
                    let side_h = side_w / ratio;
                    let x = if end.x < start.x { start.x - side_w } else { start.x };
                    let y = if end.y < start.y { start.y - side_h } else { start.y };
                    let fitted = RectF::new(x, y, side_w, side_h);
                    if fitted.x >= 0.0 && fitted.y >= 0.0 && fitted.right() <= w as f32 && fitted.bottom() <= h as f32 {
                        r = fitted;
                    } else {
                        return;
                    }
                }
                if r.w >= 1.0 && r.h >= 1.0 {
                    self.rect = r;
                }
            }
            None => {}
        }
    }

    pub fn release(&mut self) {
        if self.rect.w < MIN_SIDE || self.rect.h < MIN_SIDE {
            self.rect = RectF::new(self.rect.x, self.rect.y, self.rect.w.max(MIN_SIDE), self.rect.h.max(MIN_SIDE));
        }
        self.drag = None;
    }

    /// Re-fit the rectangle when the aspect option changes.
    pub fn set_aspect(&mut self, aspect: Aspect, image: (u32, u32)) {
        self.aspect = aspect;
        let Some(ratio) = aspect.ratio(image) else { return };
        let c = self.rect.center();
        let (mut w, mut h) = (self.rect.w, self.rect.w / ratio);
        if h > self.rect.h {
            (w, h) = (self.rect.h * ratio, self.rect.h);
        }
        let x = (c.x - w / 2.0).clamp(0.0, (image.0 as f32 - w).max(0.0));
        let y = (c.y - h / 2.0).clamp(0.0, (image.1 as f32 - h).max(0.0));
        self.rect = RectF::new(x, y, w, h);
    }
}

/// A throwaway rectangle shape, so that the annotation crate's handle maths can resize the crop.
fn probe(rect: RectF) -> Shape {
    Shape { id: 0, kind: ShapeKind::Rect { rect, radius: 0.0 }, style: Style::default(), hidden: false }
}

fn clamp_to(r: RectF, w: u32, h: u32) -> RectF {
    let (w, h) = (w as f32, h as f32);
    let x0 = r.x.clamp(0.0, w);
    let y0 = r.y.clamp(0.0, h);
    let x1 = r.right().clamp(0.0, w);
    let y1 = r.bottom().clamp(0.0, h);
    RectF::new(x0, y0, x1 - x0, y1 - y0)
}

/// Adjust a resized rectangle to `ratio` (width over height), keeping the side opposite the handle in place.
fn lock_ratio(resized: RectF, before: RectF, handle: Handle, ratio: f32) -> RectF {
    match handle {
        Handle::Corner(i) => {
            let w = resized.w.max(MIN_SIDE);
            let h = w / ratio;
            let x = if matches!(i, 0 | 3) { before.right() - w } else { before.x };
            let y = if matches!(i, 0 | 1) { before.bottom() - h } else { before.y };
            RectF::new(x, y, w, h)
        }
        Handle::Edge(i) => {
            let c = before.center();
            if matches!(i, 1 | 3) {
                let h = resized.w / ratio;
                let x = if i == 3 { before.right() - resized.w } else { before.x };
                RectF::new(x, c.y - h / 2.0, resized.w, h)
            } else {
                let w = resized.h * ratio;
                let y = if i == 0 { before.bottom() - resized.h } else { before.y };
                RectF::new(c.x - w / 2.0, y, w, resized.h)
            }
        }
        _ => resized,
    }
}
