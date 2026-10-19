//! The selection rectangle and the drag that edits it. Pure logic in virtual-desktop pixels: the
//! view feeds it pointer positions and modifier state, it hands back the rectangle to show.

use egui::{CursorIcon, Pos2, pos2};
use snap_capture::Frozen;
use snap_core::{CaptureKind, Rect as PxRect};

use super::space::rect_f;

/// A selection edge snaps to a window edge this close (physical pixels).
const SNAP_PX: f32 = 6.0;
/// A press that moves less than this (physical pixels) is a click.
const CLICK_SLOP: f32 = 4.0;
/// How far from a handle's centre it still grabs, in points.
pub const GRAB_PT: f32 = 9.0;
/// Half-width of the grabbable strip along an edge, in points.
const BAND_PT: f32 = 4.0;

/// Which edges a drag moves. All four = moving the whole rectangle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Edges {
    pub l: bool,
    pub t: bool,
    pub r: bool,
    pub b: bool,
}

impl Edges {
    pub const ALL: Edges = Edges::new(true, true, true, true);

    const fn new(l: bool, t: bool, r: bool, b: bool) -> Self {
        Self { l, t, r, b }
    }

    fn is_corner(self) -> bool {
        (self.l ^ self.r) && (self.t ^ self.b)
    }

    pub fn cursor(self) -> CursorIcon {
        match (self.l, self.t, self.r, self.b) {
            (true, true, false, false) | (false, false, true, true) => CursorIcon::ResizeNwSe,
            (false, true, true, false) | (true, false, false, true) => CursorIcon::ResizeNeSw,
            (true, false, false, false) | (false, false, true, false) => CursorIcon::ResizeHorizontal,
            (false, true, false, false) | (false, false, false, true) => CursorIcon::ResizeVertical,
            _ => CursorIcon::Move,
        }
    }
}

/// What the pointer is over on an existing selection.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    Handle(Edges),
    Inside,
}

/// Window and monitor edges that selection edges snap to.
pub struct SnapLines {
    xs: Vec<i32>,
    ys: Vec<i32>,
}

impl SnapLines {
    pub fn new(frozen: &Frozen) -> Self {
        let rects = frozen.monitors.iter().map(|(m, _)| m.rect).chain(frozen.windows.iter().filter(|w| !w.minimized).map(|w| w.rect));
        let (mut xs, mut ys) = (Vec::new(), Vec::new());
        for r in rects {
            xs.extend([r.x, r.right()]);
            ys.extend([r.y, r.bottom()]);
        }
        for list in [&mut xs, &mut ys] {
            list.sort_unstable();
            list.dedup();
        }
        Self { xs, ys }
    }

    fn nearest(list: &[i32], v: f32) -> Option<f32> {
        let i = list.partition_point(|&x| (x as f32) < v);
        let below = i.checked_sub(1).and_then(|k| list.get(k));
        [below, list.get(i)]
            .into_iter()
            .flatten()
            .map(|&x| x as f32)
            .filter(|x| (x - v).abs() <= SNAP_PX)
            .min_by(|a, b| (a - v).abs().total_cmp(&(b - v).abs()))
    }

    fn x(&self, v: f32) -> Option<f32> {
        Self::nearest(&self.xs, v)
    }

    fn y(&self, v: f32) -> Option<f32> {
        Self::nearest(&self.ys, v)
    }
}

/// What constrains a drag this frame.
pub struct Limits<'a> {
    pub bounds: PxRect,
    pub lines: &'a SnapLines,
    /// Shift: square when creating, keep the aspect ratio when resizing by a corner.
    pub square: bool,
    /// Edges snap to window edges (off while Alt is held, or when the setting is off).
    pub snap: bool,
    /// Space: the drag moves the whole rectangle instead of its edges.
    pub shift_all: bool,
}

struct Drag {
    /// Left, top, right, bottom as the pointer dictates, before snapping and clamping.
    raw: [f32; 4],
    edges: Edges,
    last: Pos2,
    press: Pos2,
    ratio: f32,
    moved: bool,
    creating: bool,
}

/// How a press ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Release {
    /// A press that created nothing: the caller decides what a click on this spot means.
    Click,
    Dragged,
}

pub struct Selection {
    pub rect: Option<PxRect>,
    /// `Window` or `Element` while the rectangle is exactly a snapped target, else `Region`.
    pub kind: CaptureKind,
    drag: Option<Drag>,
}

impl Selection {
    pub fn new() -> Self {
        Self { rect: None, kind: CaptureKind::Region, drag: None }
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// A new rectangle is being dragged out (as opposed to resizing or moving one).
    pub fn is_creating(&self) -> bool {
        self.drag.as_ref().is_some_and(|d| d.creating)
    }

    /// The edges being dragged, for the cursor icon.
    pub fn dragging_edges(&self) -> Option<Edges> {
        self.drag.as_ref().map(|d| d.edges)
    }

    pub fn set(&mut self, rect: PxRect, kind: CaptureKind) {
        self.rect = Some(rect);
        self.kind = kind;
    }

    pub fn clear(&mut self) {
        *self = Self::new();
    }

    /// What `at` (virtual-desktop pixels) grabs on the current selection. `ppp` scales the hit areas so they
    /// stay the same size on screen at any DPI.
    pub fn hit(&self, at: Pos2, ppp: f32) -> Option<Hit> {
        let r = rect_f(self.rect?);
        let (grab, band) = (GRAB_PT * ppp, BAND_PT * ppp);
        let mut best: Option<(f32, Edges)> = None;
        let mut consider = |p: Pos2, e: Edges| {
            let d = (at - p).abs().max_elem();
            if d <= grab && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, e));
            }
        };
        consider(r.left_top(), Edges::new(true, true, false, false));
        consider(r.right_top(), Edges::new(false, true, true, false));
        consider(r.left_bottom(), Edges::new(true, false, false, true));
        consider(r.right_bottom(), Edges::new(false, false, true, true));
        // Midpoint handles only exist when the side is long enough not to crowd the corners.
        if r.width() >= 3.0 * grab {
            consider(pos2(r.center().x, r.top()), Edges::new(false, true, false, false));
            consider(pos2(r.center().x, r.bottom()), Edges::new(false, false, false, true));
        }
        if r.height() >= 3.0 * grab {
            consider(pos2(r.left(), r.center().y), Edges::new(true, false, false, false));
            consider(pos2(r.right(), r.center().y), Edges::new(false, false, true, false));
        }
        if let Some((_, edges)) = best {
            return Some(Hit::Handle(edges));
        }
        let (dl, dr) = ((at.x - r.left()).abs(), (at.x - r.right()).abs());
        let (dt, db) = ((at.y - r.top()).abs(), (at.y - r.bottom()).abs());
        let nearest = dl.min(dr).min(dt).min(db);
        if nearest <= band && r.expand(band).contains(at) {
            return Some(Hit::Handle(side(dl, dr, dt, db, nearest)));
        }
        r.contains(at).then_some(Hit::Inside)
    }

    /// Start a drag at `at`: on a handle, inside the rectangle (move), or on empty space (new rectangle).
    pub fn press(&mut self, at: Pos2, hit: Option<Hit>) {
        let (raw, edges, ratio, creating) = match (hit, self.rect) {
            (Some(hit), Some(rect)) => {
                let r = rect_f(rect);
                let edges = match hit {
                    Hit::Handle(edges) => edges,
                    Hit::Inside => Edges::ALL,
                };
                let ratio = if r.height() > 0.0 { r.width() / r.height() } else { 1.0 };
                ([r.min.x, r.min.y, r.max.x, r.max.y], edges, ratio, false)
            }
            _ => {
                self.rect = None;
                self.kind = CaptureKind::Region;
                ([at.x, at.y, at.x, at.y], Edges::new(false, false, true, true), 1.0, true)
            }
        };
        self.drag = Some(Drag { raw, edges, last: at, press: at, ratio, moved: false, creating });
    }

    /// Follow the pointer to `to`.
    pub fn drag(&mut self, to: Pos2, lim: &Limits) {
        let Some(d) = self.drag.as_mut() else { return };
        let delta = to - d.last;
        d.last = to;
        if (to - d.press).length() > CLICK_SLOP {
            d.moved = true;
        }
        let edges = if lim.shift_all { Edges::ALL } else { d.edges };
        for (value, active, by) in [(0, edges.l, delta.x), (1, edges.t, delta.y), (2, edges.r, delta.x), (3, edges.b, delta.y)] {
            if active {
                d.raw[value] += by;
            }
        }
        // Dragging an edge past the opposite one flips which edge is which.
        if d.raw[0] > d.raw[2] {
            d.raw.swap(0, 2);
            std::mem::swap(&mut d.edges.l, &mut d.edges.r);
        }
        if d.raw[1] > d.raw[3] {
            d.raw.swap(1, 3);
            std::mem::swap(&mut d.edges.t, &mut d.edges.b);
        }
        if d.moved {
            self.kind = CaptureKind::Region;
        }
        if d.moved || !d.creating {
            self.rect = Some(finalize(d, lim));
        }
    }

    /// Finish the drag.
    pub fn release(&mut self) -> Option<Release> {
        let d = self.drag.take()?;
        Some(if d.creating && !d.moved { Release::Click } else { Release::Dragged })
    }

    /// Move the rectangle by whole pixels, staying inside `bounds`.
    pub fn nudge(&mut self, dx: i32, dy: i32, bounds: PxRect) {
        let Some(r) = self.rect.as_mut() else { return };
        r.x = (r.x + dx).clamp(bounds.x, (bounds.right() - r.w as i32).max(bounds.x));
        r.y = (r.y + dy).clamp(bounds.y, (bounds.bottom() - r.h as i32).max(bounds.y));
        self.kind = CaptureKind::Region;
    }

    /// Grow or shrink the right and bottom edges by whole pixels.
    pub fn grow(&mut self, dw: i32, dh: i32, bounds: PxRect) {
        let Some(r) = self.rect.as_mut() else { return };
        r.w = (r.w as i32 + dw).clamp(1, (bounds.right() - r.x).max(1)) as u32;
        r.h = (r.h as i32 + dh).clamp(1, (bounds.bottom() - r.y).max(1)) as u32;
        self.kind = CaptureKind::Region;
    }
}

/// The single edge a strip hit belongs to (exactly one, so ties never produce a corner).
fn side(dl: f32, dr: f32, dt: f32, db: f32, nearest: f32) -> Edges {
    if dl == nearest {
        Edges::new(true, false, false, false)
    } else if dr == nearest {
        Edges::new(false, false, true, false)
    } else if dt == nearest {
        Edges::new(false, true, false, false)
    } else {
        debug_assert!(db == nearest);
        Edges::new(false, false, false, true)
    }
}

/// Turn the pointer-driven edges into the rectangle to show: aspect lock, snapping, clamping, whole pixels.
fn finalize(d: &Drag, lim: &Limits) -> PxRect {
    let [mut l, mut t, mut r, mut b] = d.raw;
    let moving = d.edges == Edges::ALL;
    if lim.square && d.edges.is_corner() {
        let (w, h) = (r - l, b - t);
        let nw = w.max(h * d.ratio);
        let nh = nw / d.ratio;
        if d.edges.r {
            r = l + nw;
        } else {
            l = r - nw;
        }
        if d.edges.b {
            b = t + nh;
        } else {
            t = b - nh;
        }
    } else if lim.snap {
        if moving {
            // Whichever of the two opposite edges is nearer to a line pulls the whole rectangle onto it.
            let shift = |a: Option<f32>, b: Option<f32>, va: f32, vb: f32| match (a, b) {
                (Some(a), Some(b)) => Some(if (a - va).abs() <= (b - vb).abs() { a - va } else { b - vb }),
                (Some(a), None) => Some(a - va),
                (None, Some(b)) => Some(b - vb),
                (None, None) => None,
            };
            if let Some(dx) = shift(lim.lines.x(l), lim.lines.x(r), l, r) {
                (l, r) = (l + dx, r + dx);
            }
            if let Some(dy) = shift(lim.lines.y(t), lim.lines.y(b), t, b) {
                (t, b) = (t + dy, b + dy);
            }
        } else {
            if d.edges.l {
                l = lim.lines.x(l).unwrap_or(l);
            }
            if d.edges.r {
                r = lim.lines.x(r).unwrap_or(r);
            }
            if d.edges.t {
                t = lim.lines.y(t).unwrap_or(t);
            }
            if d.edges.b {
                b = lim.lines.y(b).unwrap_or(b);
            }
        }
    }

    let bounds = rect_f(lim.bounds);
    if moving {
        let (w, h) = ((r - l).round(), (b - t).round());
        let x = l.round().clamp(bounds.min.x, (bounds.max.x - w).max(bounds.min.x));
        let y = t.round().clamp(bounds.min.y, (bounds.max.y - h).max(bounds.min.y));
        return PxRect::new(x as i32, y as i32, w.max(0.0) as u32, h.max(0.0) as u32);
    }
    let (l, r) = (l.round().clamp(bounds.min.x, bounds.max.x), r.round().clamp(bounds.min.x, bounds.max.x));
    let (t, b) = (t.round().clamp(bounds.min.y, bounds.max.y), b.round().clamp(bounds.min.y, bounds.max.y));
    PxRect::new(l as i32, t as i32, (r - l).max(0.0) as u32, (b - t).max(0.0) as u32)
}
