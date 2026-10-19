//! The document being edited: shapes, selection, undo and the shape clipboard.

use std::{sync::Arc, time::Instant};

use image::RgbaImage;
use snap_annotate::{Document, Handle, Shape, ShapeKind, Style, Undo, bounds, drag};
use snap_core::{PointF, RectF};

/// Where a shape moves in the stacking order.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Order {
    Forward,
    Backward,
    Front,
    Back,
}

/// A change this soon after a change with the same key shares its undo step (slider drags, arrow-key nudges).
const COALESCE_MS: u128 = 700;

pub struct Session {
    pub doc: Document,
    pub undo: Undo,
    pub selection: Vec<u64>,
    /// Bumped by every change to the document; drives re-rendering and the unsaved-changes guard.
    pub rev: u64,
    shapes_clipboard: Vec<Shape>,
    pastes: i32,
    coalesce: Option<(&'static str, Instant)>,
}

impl Session {
    pub fn new(doc: Document) -> Self {
        Self { doc, undo: Undo::new(), selection: Vec::new(), rev: 0, shapes_clipboard: Vec::new(), pastes: 0, coalesce: None }
    }

    /// Tell the renderer and the dirty tracking that the document changed.
    pub fn touch(&mut self) {
        self.rev += 1;
    }

    /// Snapshot for undo. Call before every user-visible change.
    pub fn checkpoint(&mut self) {
        self.coalesce = None;
        self.undo.checkpoint(&self.doc);
    }

    /// Like [`checkpoint`](Self::checkpoint), but a burst of changes with the same `key` makes one undo step.
    pub fn checkpoint_coalesced(&mut self, key: &'static str) {
        let now = Instant::now();
        if let Some((k, at)) = self.coalesce
            && k == key
            && now.duration_since(at).as_millis() < COALESCE_MS
        {
            self.coalesce = Some((key, now));
            return;
        }
        self.undo.checkpoint(&self.doc);
        self.coalesce = Some((key, now));
    }

    pub fn undo(&mut self) -> bool {
        self.coalesce = None;
        let done = self.undo.undo(&mut self.doc);
        self.after_history(done)
    }

    pub fn redo(&mut self) -> bool {
        self.coalesce = None;
        let done = self.undo.redo(&mut self.doc);
        self.after_history(done)
    }

    fn after_history(&mut self, done: bool) -> bool {
        if done {
            let doc = &self.doc;
            self.selection.retain(|id| doc.get(*id).is_some());
            self.touch();
        }
        done
    }

    pub fn add(&mut self, kind: ShapeKind, style: Style) -> u64 {
        self.touch();
        self.doc.add(kind, style)
    }

    pub fn is_selected(&self, id: u64) -> bool {
        self.selection.contains(&id)
    }

    pub fn select_only(&mut self, id: u64) {
        self.selection = vec![id];
    }

    pub fn toggle_selected(&mut self, id: u64) {
        match self.selection.iter().position(|s| *s == id) {
            Some(i) => {
                self.selection.remove(i);
            }
            None => self.selection.push(id),
        }
    }

    pub fn select_all(&mut self) {
        self.selection = self.doc.shapes.iter().filter(|s| !s.hidden).map(|s| s.id).collect();
    }

    pub fn selected_shapes(&self) -> impl Iterator<Item = &Shape> {
        self.selection.iter().filter_map(|id| self.doc.get(*id))
    }

    /// The only selected shape, when exactly one is selected.
    pub fn single(&self) -> Option<&Shape> {
        match self.selection.as_slice() {
            [id] => self.doc.get(*id),
            _ => None,
        }
    }

    /// Union of the visual extents of the selected shapes, in document pixels.
    pub fn selection_bounds(&self) -> Option<RectF> {
        self.selected_shapes().map(bounds).reduce(RectF::union)
    }

    pub fn delete_selection(&mut self) -> bool {
        if self.selection.is_empty() {
            return false;
        }
        self.checkpoint();
        for id in std::mem::take(&mut self.selection) {
            self.doc.remove(id);
        }
        self.touch();
        true
    }

    /// Move the selected shapes by `delta` document pixels.
    pub fn nudge(&mut self, delta: PointF) {
        if self.selection.is_empty() {
            return;
        }
        self.checkpoint_coalesced("nudge");
        for id in self.selection.clone() {
            if let Some(shape) = self.doc.get_mut(id) {
                drag(shape, Handle::Body, delta);
            }
        }
        self.touch();
    }

    /// Duplicate the selection a little to the lower right and select the copies.
    pub fn duplicate(&mut self, offset: f32) {
        let copies: Vec<Shape> = self.selected_shapes().cloned().collect();
        self.insert_copies(copies, offset);
    }

    /// Remember the selected shapes for [`paste_shapes`](Self::paste_shapes). Returns how many were copied.
    pub fn copy_selection(&mut self) -> usize {
        self.shapes_clipboard = self.selected_shapes().cloned().collect();
        self.pastes = 0;
        self.shapes_clipboard.len()
    }

    pub fn has_clipboard(&self) -> bool {
        !self.shapes_clipboard.is_empty()
    }

    pub fn paste_shapes(&mut self, step: f32) {
        self.pastes += 1;
        let copies = self.shapes_clipboard.clone();
        self.insert_copies(copies, step * self.pastes as f32);
    }

    fn insert_copies(&mut self, copies: Vec<Shape>, offset: f32) {
        if copies.is_empty() {
            return;
        }
        self.checkpoint();
        let mut ids = Vec::with_capacity(copies.len());
        for mut shape in copies {
            drag(&mut shape, Handle::Body, PointF::new(offset, offset));
            ids.push(self.doc.add(shape.kind, shape.style));
        }
        self.selection = ids;
        self.touch();
    }

    pub fn reorder(&mut self, order: Order) {
        if self.selection.is_empty() {
            return;
        }
        self.checkpoint();
        // Raise/lower walk the topmost-first / bottom-first so a multi-selection keeps its relative order.
        let mut ids: Vec<u64> = self.doc.shapes.iter().filter(|s| self.selection.contains(&s.id)).map(|s| s.id).collect();
        if matches!(order, Order::Forward | Order::Front) {
            ids.reverse();
        }
        for id in ids {
            match order {
                Order::Forward => self.doc.raise(id),
                Order::Backward => self.doc.lower(id),
                Order::Front => self.doc.bring_to_front(id),
                Order::Back => self.doc.send_to_back(id),
            }
        }
        self.touch();
    }

    /// Place a pasted image in the middle of `around`, shrunk to fit inside `limit`.
    pub fn add_image(&mut self, image: RgbaImage, around: PointF, limit: (f32, f32), style: Style) -> u64 {
        let (w, h) = (image.width() as f32, image.height() as f32);
        let k = (limit.0 / w).min(limit.1 / h).min(1.0);
        let (w, h) = (w * k, h * k);
        let rect = RectF::new(around.x - w / 2.0, around.y - h / 2.0, w, h);
        self.checkpoint();
        let id = self.add(ShapeKind::Image { rect, image: Arc::new(image) }, style);
        self.select_only(id);
        id
    }
}
