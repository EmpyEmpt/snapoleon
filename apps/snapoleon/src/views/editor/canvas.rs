//! The canvas: the picture and its annotations as GPU textures, zoom and pan, selection and the pointer
//! behaviour of every tool, and the on-canvas overlays (handles, crop, caret).
//!
//! The document is drawn by `snap_annotate::render_view` into two textures at the size of the visible
//! area: the base image (cached while the view stands still) and the annotations (re-rendered when the
//! document changes), so dragging a shape never re-samples the screenshot.

use std::time::Instant;

use egui::{
    Color32, ColorImage, Context, CornerRadius, CursorIcon, Event, Key, Mesh, Painter, PointerButton, Pos2, Rect, Response, Role, Sense,
    Shape as Paint, Stroke, StrokeKind, TextureHandle, TextureOptions, Ui, Vec2, WidgetInfo, epaint::TextureId,
};
use snap_annotate::{Document, Handle, Shape, ShapeKind, View, bounds, drag, handles, hit_test, render_view};
use snap_core::{PointF, RectF};
use snap_ui::{Palette, palette};

use super::{
    create::{self, Build, DRAG_THRESHOLD},
    crop::Crop,
    session::Session,
    text_edit::{self, TextEdit},
    tools::{Tool, ToolSet},
    viewport::Viewport,
};

/// Distance in screen points within which a handle grabs the pointer.
const HANDLE_GRAB: f32 = 9.0;
/// Distance in screen points within which a shape counts as under the pointer.
const SHAPE_GRAB: f32 = 5.0;

/// What the canvas is doing with the pointer.
enum Gesture {
    None,
    Pan,
    Marquee {
        start: PointF,
        base: Vec<u64>,
    },
    Move {
        last: PointF,
        started: bool,
    },
    Handle {
        id: u64,
        handle: Handle,
        last: PointF,
        started: bool,
    },
    Create {
        tool: Tool,
        start: PointF,
        id: Option<u64>,
        moved: bool,
        /// How far from a shape (document pixels) a click still counts as on it.
        grab: f32,
    },
    Crop {
        last: PointF,
    },
    /// Dragging inside the text being edited, to select.
    TextSelect,
}

/// What a rendered layer was made for; the layer is drawn again when this changes.
#[derive(Clone, Copy, PartialEq)]
struct LayerKey {
    size: (u32, u32),
    zoom: u32,
    offset: (u32, u32),
    /// The document revision, for the annotation layer only.
    rev: u64,
}

struct Layer {
    texture: Option<TextureHandle>,
    key: Option<LayerKey>,
}

impl Layer {
    fn new() -> Self {
        Self { texture: None, key: None }
    }

    fn upload(&mut self, ctx: &Context, name: &str, image: ColorImage) {
        match &mut self.texture {
            Some(texture) => texture.set(image, TextureOptions::NEAREST),
            None => self.texture = Some(ctx.load_texture(name, image, TextureOptions::NEAREST)),
        }
    }
}

pub struct Canvas {
    base_doc: Option<Document>,
    base: Layer,
    notes: Layer,
    checker: Option<(bool, TextureHandle)>,
    gesture: Gesture,
    pub text: Option<TextEdit>,
    pub crop: Crop,
    hover: Option<u64>,
    /// A rectangle to flag on the canvas (the OCR line the user clicked), in document pixels.
    pub highlight: Option<RectF>,
    /// Size of the canvas in physical pixels.
    pub phys: Vec2,
    pub pointer_doc: Option<PointF>,
}

/// What the canvas needs from the editor each frame.
pub struct CanvasCx<'a> {
    pub s: &'a mut Session,
    pub view: &'a mut Viewport,
    pub tool: Tool,
    pub tools: &'a ToolSet,
    pub step_start: u32,
    pub ui_scale: f32,
}

impl Canvas {
    pub fn new() -> Self {
        Self {
            base_doc: None,
            base: Layer::new(),
            notes: Layer::new(),
            checker: None,
            gesture: Gesture::None,
            text: None,
            crop: Crop::new(),
            hover: None,
            highlight: None,
            phys: Vec2::ZERO,
            pointer_doc: None,
        }
    }

    pub fn editing_text(&self) -> bool {
        self.text.is_some()
    }

    /// Leave text editing, dropping a text that stayed empty.
    pub fn commit_text(&mut self, s: &mut Session) {
        let Some(edit) = self.text.take() else { return };
        let empty = s.doc.get(edit.id).is_some_and(|sh| matches!(sh.kind, ShapeKind::Text { .. }) && text_of(sh).trim().is_empty());
        if empty {
            s.doc.remove(edit.id);
            s.selection.retain(|id| *id != edit.id);
            if edit.checkpointed {
                s.touch();
            }
        }
    }

    /// Start editing the text of shape `id` with the caret at `caret` (`None` = at the end).
    ///
    /// `fresh_at` is the list position of a text shape made for this edit, which stays out of undo until its
    /// text first changes. `checkpointed` says the shape's creation already has an undo step.
    pub fn begin_text(&mut self, s: &Session, id: u64, caret: Option<usize>, fresh_at: Option<usize>, checkpointed: bool) {
        let Some(geom) = s.doc.get(id).and_then(text_edit::geom) else { return };
        let end = geom.text.chars().count();
        let mut edit = TextEdit::new(id, caret.unwrap_or(end), fresh_at);
        edit.checkpointed = checkpointed;
        self.text = Some(edit);
    }

    /// Bring the canvas to a state where a different tool can take over.
    pub fn tool_changed(&mut self, s: &mut Session, tool: Tool) {
        self.commit_text(s);
        self.gesture = Gesture::None;
        if tool == Tool::Crop {
            self.crop.begin(s);
        } else {
            self.crop.cancel();
        }
    }

    pub fn show(&mut self, ui: &mut Ui, cx: &mut CanvasCx) -> Response {
        let ctx = ui.ctx().clone();
        let ppp = ctx.pixels_per_point();
        let p = palette(&ctx);
        let avail = ui.available_rect_before_wrap();
        let (_, response) = ui.allocate_exact_size(avail.size(), Sense::click_and_drag());
        response.widget_info(|| WidgetInfo::labeled(Role::Canvas, true, snap_core::t("editor.a11y.canvas")));

        // Align the canvas to physical pixels so that the textures map 1:1 onto the screen.
        let min = Pos2::new((avail.min.x * ppp).round() / ppp, (avail.min.y * ppp).round() / ppp);
        // Textures cannot exceed what the GPU takes; a window that large simply leaves its far edge empty.
        let limit = ctx.input(|i| i.max_texture_side) as f32;
        let phys = Vec2::new((avail.width() * ppp).floor().clamp(1.0, limit), (avail.height() * ppp).floor().clamp(1.0, limit));
        let rect = Rect::from_min_size(min, phys / ppp);
        self.phys = phys;
        let image = cx.s.doc.canvas_size();
        cx.view.update(phys, image);

        self.keyboard_text(&ctx, cx);
        self.zoom_and_pan(&ctx, &response, rect, ppp, cx);
        self.pointer(&ctx, &response, rect, ppp, cx);
        self.pointer_doc = response.hover_pos().map(|pos| cx.view.to_doc(rect, ppp, pos));

        self.render_layers(&ctx, cx);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, p.bg);
        self.paint_image(&ctx, &painter, rect, ppp, cx, &p);
        self.paint_overlays(&painter, rect, ppp, cx, &p);
        self.set_cursor(&ctx, &response, rect, ppp, cx);

        response
    }

    // ---------------------------------------------------------------------------------------------
    // Rendering
    // ---------------------------------------------------------------------------------------------

    fn render_layers(&mut self, ctx: &Context, cx: &CanvasCx) {
        let (w, h) = (self.phys.x as u32, self.phys.y as u32);
        let view = View { offset: cx.view.offset, zoom: cx.view.zoom, size: (w, h) };
        let key = LayerKey { size: (w, h), zoom: view.zoom.to_bits(), offset: (view.offset.x.to_bits(), view.offset.y.to_bits()), rev: 0 };
        let started = Instant::now();

        if self.base.key != Some(key) {
            let base_doc = self.base_doc.get_or_insert_with(|| {
                let mut d = cx.s.doc.clone();
                d.shapes.clear();
                d.crop = None;
                d.beautify = None;
                d
            });
            let pm = render_view(base_doc, &view, true);
            self.base.upload(ctx, "editor-base", ColorImage::from_rgba_premultiplied([w as usize, h as usize], pm.data()));
            self.base.key = Some(key);
        }

        let notes_key = LayerKey { rev: cx.s.rev + 1, ..key };
        if self.notes.key != Some(notes_key) {
            if cx.s.doc.shapes.iter().any(|s| !s.hidden) {
                let pm = render_view(&cx.s.doc, &view, false);
                self.notes.upload(ctx, "editor-notes", ColorImage::from_rgba_premultiplied([w as usize, h as usize], pm.data()));
            } else {
                self.notes.texture = None;
            }
            self.notes.key = Some(notes_key);
        }
        let took = started.elapsed();
        if took.as_millis() > 12 {
            tracing::debug!("editor canvas render took {took:?} at {w}x{h}");
        }
    }

    fn paint_image(&mut self, ctx: &Context, painter: &Painter, rect: Rect, ppp: f32, cx: &CanvasCx, p: &Palette) {
        let (iw, ih) = cx.s.doc.canvas_size();
        let image = cx.view.rect_to_screen(rect, ppp, RectF::new(0.0, 0.0, iw as f32, ih as f32));
        let visible = image.intersect(rect);
        if visible.is_positive() {
            let checker = self.checker_texture(ctx, p);
            let cell = 16.0 / ppp;
            let uv = Rect::from_min_max(
                Pos2::new((visible.min.x - image.min.x) / cell, (visible.min.y - image.min.y) / cell),
                Pos2::new((visible.max.x - image.min.x) / cell, (visible.max.y - image.min.y) / cell),
            );
            let mut mesh = Mesh::with_texture(checker);
            mesh.add_rect_with_uv(visible, uv, Color32::WHITE);
            painter.add(mesh);
        }
        let full = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        for layer in [&self.base, &self.notes] {
            if let Some(texture) = &layer.texture {
                painter.image(texture.id(), rect, full, Color32::WHITE);
            }
        }
        // The hairline sits just outside the picture, as a frame, and never over its pixels.
        let frame = Rect::from_min_max(image.min - Vec2::splat(1.0 / ppp), image.max + Vec2::splat(1.0 / ppp));
        painter.rect_stroke(frame, 0.0, Stroke::new(1.0 / ppp, p.border_strong()), StrokeKind::Inside);
    }

    /// The 2x2 texture that tiles into the transparency checkerboard.
    fn checker_texture(&mut self, ctx: &Context, p: &Palette) -> TextureId {
        if self.checker.as_ref().is_none_or(|(dark, _)| *dark != p.dark) {
            let (a, b) = if p.dark {
                (Color32::from_rgb(0x2A, 0x2D, 0x37), Color32::from_rgb(0x21, 0x24, 0x2D))
            } else {
                (Color32::WHITE, Color32::from_rgb(0xE9, 0xE9, 0xE4))
            };
            let image = ColorImage::new([2, 2], vec![a, b, b, a]);
            let handle = ctx.load_texture("editor-checker", image, TextureOptions::NEAREST_REPEAT);
            self.checker = Some((p.dark, handle));
        }
        self.checker.as_ref().map(|(_, h)| h.id()).unwrap_or_default()
    }

    // ---------------------------------------------------------------------------------------------
    // Zoom and pan
    // ---------------------------------------------------------------------------------------------

    fn zoom_and_pan(&mut self, ctx: &Context, response: &Response, rect: Rect, ppp: f32, cx: &mut CanvasCx) {
        if !response.hovered() {
            return;
        }
        let image = cx.s.doc.canvas_size();
        let (zoom, scroll, pos) = ctx.input(|i| (i.zoom_delta(), i.smooth_scroll_delta, i.pointer.hover_pos()));
        if (zoom - 1.0).abs() > f32::EPSILON {
            let anchor = pos.map_or(self.phys / 2.0, |p| (p - rect.min) * ppp);
            cx.view.set_zoom(cx.view.zoom * zoom, anchor, self.phys, image);
        } else if scroll != Vec2::ZERO {
            cx.view.pan(scroll * ppp, self.phys, image);
        }
    }

    // ---------------------------------------------------------------------------------------------
    // Pointer
    // ---------------------------------------------------------------------------------------------

    fn pointer(&mut self, ctx: &Context, response: &Response, rect: Rect, ppp: f32, cx: &mut CanvasCx) {
        let (pressed, down, released, middle_pressed, middle_down, space, shift, pos) = ctx.input(|i| {
            (
                i.pointer.button_pressed(PointerButton::Primary),
                i.pointer.primary_down(),
                i.pointer.primary_released(),
                i.pointer.button_pressed(PointerButton::Middle),
                i.pointer.middle_down(),
                i.key_down(Key::Space),
                i.modifiers.shift,
                i.pointer.latest_pos(),
            )
        });
        let image = cx.s.doc.canvas_size();
        let hovered = response.hovered();

        // Panning wins over everything else while it lasts.
        if matches!(self.gesture, Gesture::Pan) {
            if (down || middle_down) && !released {
                let delta = ctx.input(|i| i.pointer.delta());
                cx.view.pan(delta * ppp, self.phys, image);
            } else {
                self.gesture = Gesture::None;
            }
            return;
        }
        if hovered && (middle_pressed || (pressed && space)) && matches!(self.gesture, Gesture::None) {
            self.commit_text(cx.s);
            self.gesture = Gesture::Pan;
            return;
        }

        let Some(pos) = pos else {
            if !down && !matches!(self.gesture, Gesture::None) {
                self.release(ctx, shift, false, cx);
            }
            return;
        };
        let doc = cx.view.to_doc(rect, ppp, pos);
        // A button released outside the window (or focus lost mid-drag) never reports its release.
        let lost = !down && !released && !pressed && !matches!(self.gesture, Gesture::None);

        if pressed && hovered && matches!(self.gesture, Gesture::None) {
            self.press(rect, ppp, pos, doc, shift, cx);
        }
        if down || released {
            self.drag_to(rect, ppp, pos, doc, shift, cx);
        }
        if released || lost {
            let double = released && response.double_clicked();
            self.release(ctx, shift, double, cx);
        }
    }

    fn press(&mut self, rect: Rect, ppp: f32, pos: Pos2, doc: PointF, shift: bool, cx: &mut CanvasCx) {
        let tol = cx.view.doc_len(ppp, SHAPE_GRAB);

        if let Some(edit) = &self.text {
            let inside = cx.s.doc.get(edit.id).is_some_and(|sh| text_hit(sh, doc, tol));
            if inside {
                self.place_caret(doc, shift, cx.s);
                self.gesture = Gesture::TextSelect;
                return;
            }
            self.commit_text(cx.s);
        }

        if cx.tool == Tool::Crop {
            let near = self.crop_handle_at(rect, ppp, pos, cx.view);
            self.crop.press(doc, near, cx.s.doc.canvas_size());
            self.gesture = Gesture::Crop { last: doc };
            return;
        }

        if let Some((id, handle)) = self.handle_at(rect, ppp, pos, cx) {
            self.gesture = Gesture::Handle { id, handle, last: doc, started: false };
            return;
        }

        if cx.tool == Tool::Select {
            match hit_test(&cx.s.doc, doc, tol) {
                Some(id) => {
                    if shift {
                        cx.s.toggle_selected(id);
                    } else if !cx.s.is_selected(id) {
                        cx.s.select_only(id);
                    }
                    self.gesture = Gesture::Move { last: doc, started: false };
                }
                None => {
                    let base = if shift { cx.s.selection.clone() } else { Vec::new() };
                    if !shift {
                        cx.s.selection.clear();
                    }
                    self.gesture = Gesture::Marquee { start: doc, base };
                }
            }
        } else {
            // A new shape starts inside the exported picture: one placed in the empty margin would be cut from the output.
            let start = clamp_to(export_area(&cx.s.doc), doc);
            self.gesture = Gesture::Create { tool: cx.tool, start, id: None, moved: false, grab: tol };
        }
    }

    fn drag_to(&mut self, rect: Rect, ppp: f32, pos: Pos2, doc: PointF, shift: bool, cx: &mut CanvasCx) {
        let ui_scale = cx.ui_scale;
        let image = cx.s.doc.canvas_size();
        match &mut self.gesture {
            Gesture::None | Gesture::Pan => {}
            Gesture::Marquee { start, base } => {
                let area = RectF::from_corners(*start, doc);
                let mut selection = base.clone();
                for shape in cx.s.doc.shapes.iter().filter(|s| !s.hidden) {
                    if intersects(bounds(shape), area) && !selection.contains(&shape.id) {
                        selection.push(shape.id);
                    }
                }
                cx.s.selection = selection;
            }
            Gesture::Move { last, started } => {
                let delta = PointF::new(doc.x - last.x, doc.y - last.y);
                if delta.x != 0.0 || delta.y != 0.0 {
                    if !*started {
                        *started = true;
                        cx.s.checkpoint();
                    }
                    for id in cx.s.selection.clone() {
                        if let Some(shape) = cx.s.doc.get_mut(id) {
                            drag(shape, Handle::Body, delta);
                        }
                    }
                    *last = doc;
                    cx.s.touch();
                }
            }
            Gesture::Handle { id, handle, last, started } => {
                let delta = PointF::new(doc.x - last.x, doc.y - last.y);
                if delta.x != 0.0 || delta.y != 0.0 {
                    if !*started {
                        *started = true;
                        cx.s.checkpoint();
                    }
                    if let Some(shape) = cx.s.doc.get_mut(*id) {
                        drag(shape, *handle, delta);
                    }
                    *last = doc;
                    cx.s.touch();
                }
            }
            Gesture::Crop { last } => {
                let delta = PointF::new(doc.x - last.x, doc.y - last.y);
                self.crop.drag_to(doc, delta, image);
                *last = doc;
            }
            Gesture::TextSelect => self.place_caret(doc, true, cx.s),
            Gesture::Create { tool, start, id, moved, .. } => {
                let travelled = cx.view.to_screen(rect, ppp, *start).distance(pos);
                if !*moved && travelled < DRAG_THRESHOLD {
                    return;
                }
                *moved = true;
                let build = Build { tools: cx.tools, scale: ui_scale, image, shift };
                let previous = id.and_then(|i| cx.s.doc.get(i)).map(|s| s.kind.clone());
                let doc = clamp_to(export_area(&cx.s.doc), doc);
                let Some(kind) = create::dragged(*tool, *start, doc, &build, previous.as_ref()) else { return };
                match id {
                    Some(existing) => {
                        if let Some(shape) = cx.s.doc.get_mut(*existing) {
                            shape.kind = kind;
                        }
                        cx.s.touch();
                    }
                    None => {
                        let style = cx.tools.styles.get(tool).copied().unwrap_or_default();
                        let new_id = if *tool == Tool::Text {
                            // Held back from undo until the first character is typed.
                            cx.s.doc.add(kind, style)
                        } else {
                            cx.s.checkpoint();
                            cx.s.add(kind, style)
                        };
                        cx.s.select_only(new_id);
                        *id = Some(new_id);
                    }
                }
            }
        }
    }

    fn release(&mut self, ctx: &Context, shift: bool, double: bool, cx: &mut CanvasCx) {
        let gesture = std::mem::replace(&mut self.gesture, Gesture::None);
        match gesture {
            Gesture::Crop { .. } => self.crop.release(),
            Gesture::Move { started: false, .. } if double => {
                if let Some(id) = cx.s.selection.first().copied().filter(|_| cx.s.selection.len() == 1) {
                    self.begin_text(cx.s, id, None, None, false);
                    if let Some(edit) = &mut self.text {
                        edit.moved_at = ctx.input(|i| i.time);
                    }
                }
            }
            Gesture::Create { tool, id, moved, start, grab } => {
                let build = Build { tools: cx.tools, scale: cx.ui_scale, image: cx.s.doc.canvas_size(), shift };
                if moved {
                    if let Some(id) = id {
                        cx.s.select_only(id);
                        if create::edits_text(tool) {
                            let fresh_at = (tool == Tool::Text).then(|| cx.s.doc.shapes.iter().position(|s| s.id == id)).flatten();
                            self.begin_text(cx.s, id, None, fresh_at, tool == Tool::Callout);
                        }
                    }
                } else if create::has_click_action(tool) {
                    self.click_create(tool, start, grab, &build, cx);
                }
            }
            _ => {}
        }
    }

    /// A press and release without travel, with a tool that places something on click.
    fn click_create(&mut self, tool: Tool, at: PointF, grab: f32, build: &Build, cx: &mut CanvasCx) {
        // Clicking existing text with a text tool edits it instead of stacking a new one on top.
        if create::edits_text(tool) {
            let hit = hit_test(&cx.s.doc, at, grab).filter(|id| cx.s.doc.get(*id).is_some_and(|s| text_edit::geom(s).is_some()));
            if let Some(id) = hit {
                cx.s.select_only(id);
                self.begin_text(cx.s, id, None, None, false);
                return;
            }
        }
        let number = if cx.s.doc.shapes.iter().any(|s| matches!(s.kind, ShapeKind::Step { .. })) {
            cx.s.doc.next_step_number()
        } else {
            cx.step_start
        };
        let Some((kind, style)) = create::clicked(tool, at, number, build) else { return };
        if tool == Tool::Text {
            let id = cx.s.doc.add(kind, style);
            cx.s.select_only(id);
            let fresh_at = cx.s.doc.shapes.iter().position(|s| s.id == id);
            self.begin_text(cx.s, id, Some(0), fresh_at, false);
        } else {
            cx.s.checkpoint();
            let id = cx.s.add(kind, style);
            cx.s.select_only(id);
            if tool == Tool::Callout {
                self.begin_text(cx.s, id, None, None, true);
            }
        }
    }

    /// Put the caret where the pointer is, extending the selection when `extend`.
    fn place_caret(&mut self, doc: PointF, extend: bool, s: &Session) {
        let Some(edit) = &mut self.text else { return };
        let Some(shape) = s.doc.get(edit.id) else { return };
        let Some(geom) = text_edit::geom(shape) else { return };
        let lines = text_edit::visual_lines(geom.text, &geom.metrics);
        let row = (((doc.y - geom.origin.y) / geom.metrics.pitch()).floor().max(0.0) as usize).min(lines.len() - 1);
        let idx = text_edit::index_at(geom.text, &geom.metrics, &lines, row, doc.x - geom.origin.x);
        edit.caret = idx;
        if !extend {
            edit.anchor = idx;
        }
    }

    fn handle_at(&self, rect: Rect, ppp: f32, pos: Pos2, cx: &CanvasCx) -> Option<(u64, Handle)> {
        let shape = cx.s.single()?;
        handles(shape)
            .into_iter()
            .map(|(h, p)| (h, cx.view.to_screen(rect, ppp, p).distance(pos)))
            .filter(|(_, d)| *d <= HANDLE_GRAB)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(h, _)| (shape.id, h))
    }

    fn crop_handle_at(&self, rect: Rect, ppp: f32, pos: Pos2, view: &Viewport) -> Option<Handle> {
        self.crop
            .handle_points()
            .into_iter()
            .map(|(h, p)| (h, view.to_screen(rect, ppp, p).distance(pos)))
            .filter(|(_, d)| *d <= HANDLE_GRAB)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(h, _)| h)
    }

    // ---------------------------------------------------------------------------------------------
    // Text editing keys
    // ---------------------------------------------------------------------------------------------

    fn keyboard_text(&mut self, ctx: &Context, cx: &mut CanvasCx) {
        let Some(edit) = &mut self.text else { return };
        let id = edit.id;
        let Some((mut text, metrics)) = cx.s.doc.get(id).and_then(text_edit::geom).map(|g| (g.text.to_string(), g.metrics)) else {
            self.text = None;
            return;
        };
        let events: Vec<Event> = ctx.input(|i| i.events.clone());
        if events.is_empty() {
            return;
        }
        let outcome = edit.apply(&events, &mut text, &metrics);
        if outcome.changed {
            checkpoint_text(edit, cx.s);
            if let Some(target) = cx.s.doc.get_mut(id).and_then(text_edit::text_mut) {
                *target = text;
            }
            cx.s.touch();
        }
        if outcome.moved || outcome.changed {
            edit.moved_at = ctx.input(|i| i.time);
        }
        if outcome.finish {
            self.commit_text(cx.s);
        }
    }

    // ---------------------------------------------------------------------------------------------
    // Overlays
    // ---------------------------------------------------------------------------------------------

    fn paint_overlays(&mut self, painter: &Painter, rect: Rect, ppp: f32, cx: &mut CanvasCx, p: &Palette) {
        let view = &*cx.view;
        let (iw, ih) = cx.s.doc.canvas_size();
        let crisp = |v: f32| (v * ppp).round() / ppp;
        let line = 1.0_f32;

        // Everything outside the export crop is dimmed.
        let crop = if self.crop.active { Some(self.crop.rect) } else { cx.s.doc.crop };
        if let Some(c) = crop {
            let image = view.rect_to_screen(rect, ppp, RectF::new(0.0, 0.0, iw as f32, ih as f32));
            let hole = view.rect_to_screen(rect, ppp, c);
            let dim = Color32::from_black_alpha(if self.crop.active { 150 } else { 110 });
            for part in [
                Rect::from_min_max(image.min, Pos2::new(image.max.x, hole.min.y)),
                Rect::from_min_max(Pos2::new(image.min.x, hole.max.y), image.max),
                Rect::from_min_max(Pos2::new(image.min.x, hole.min.y), Pos2::new(hole.min.x, hole.max.y)),
                Rect::from_min_max(Pos2::new(hole.max.x, hole.min.y), Pos2::new(image.max.x, hole.max.y)),
            ] {
                if part.is_positive() {
                    painter.rect_filled(part, 0.0, dim);
                }
            }
            if !self.crop.active {
                painter.rect_stroke(hole, 0.0, Stroke::new(line, p.accent.gamma_multiply(0.7)), StrokeKind::Outside);
            }
        }

        if self.crop.active {
            self.paint_crop(painter, rect, ppp, view, p);
        }

        if let Some(h) = self.highlight {
            let r = view.rect_to_screen(rect, ppp, h).expand(2.0);
            painter.rect_filled(r, 3.0, p.accent.gamma_multiply(0.22));
            painter.rect_stroke(r, 3.0, Stroke::new(1.5, p.accent), StrokeKind::Outside);
        }

        if cx.tool == Tool::Select && matches!(self.gesture, Gesture::None) {
            self.hover =
                self.pointer_doc.and_then(|d| hit_test(&cx.s.doc, d, view.doc_len(ppp, SHAPE_GRAB))).filter(|id| !cx.s.is_selected(*id));
        } else {
            self.hover = None;
        }
        if let Some(shape) = self.hover.and_then(|id| cx.s.doc.get(id)) {
            let r = view.rect_to_screen(rect, ppp, bounds(shape)).expand(1.0);
            painter.rect_stroke(r, 2.0, Stroke::new(line, p.accent.gamma_multiply(0.55)), StrokeKind::Outside);
        }

        let selected: Vec<&Shape> = cx.s.selected_shapes().collect();
        for shape in &selected {
            let r = view.rect_to_screen(rect, ppp, bounds(shape));
            let r = Rect::from_min_max(Pos2::new(crisp(r.min.x), crisp(r.min.y)), Pos2::new(crisp(r.max.x), crisp(r.max.y)));
            painter.rect_stroke(
                r,
                0.0,
                Stroke::new(line, p.accent.gamma_multiply(if selected.len() > 1 { 0.6 } else { 0.9 })),
                StrokeKind::Outside,
            );
        }
        if selected.len() > 1
            && let Some(b) = cx.s.selection_bounds()
        {
            let r = view.rect_to_screen(rect, ppp, b).expand(4.0);
            let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
            painter.extend(Paint::dashed_line(&corners, Stroke::new(line, p.accent), 5.0, 4.0));
        }
        if let Some(shape) = cx.s.single() {
            for (handle, at) in handles(shape) {
                paint_handle(painter, view.to_screen(rect, ppp, at), handle, p, ppp);
            }
        }

        if let Gesture::Marquee { start, .. } = &self.gesture
            && let Some(now) = self.pointer_doc
        {
            let r = view.rect_to_screen(rect, ppp, RectF::from_corners(*start, now));
            painter.rect_filled(r, 0.0, p.accent.gamma_multiply(0.1));
            painter.rect_stroke(r, 0.0, Stroke::new(line, p.accent), StrokeKind::Inside);
        }

        self.paint_caret(painter, rect, ppp, cx, p);
        paint_outside_note(painter, rect, cx, p);
    }

    fn paint_crop(&self, painter: &Painter, rect: Rect, ppp: f32, view: &Viewport, p: &Palette) {
        let r = view.rect_to_screen(rect, ppp, self.crop.rect);
        let white = Stroke::new(1.0, Color32::from_white_alpha(230));
        painter.rect_stroke(r, 0.0, white, StrokeKind::Outside);
        painter.rect_stroke(r, 0.0, Stroke::new(1.0, p.accent), StrokeKind::Inside);
        let faint = Stroke::new(1.0, Color32::from_white_alpha(70));
        for i in 1..3 {
            let t = i as f32 / 3.0;
            painter.vline(r.left() + r.width() * t, r.y_range(), faint);
            painter.hline(r.x_range(), r.top() + r.height() * t, faint);
        }
        for (handle, at) in self.crop.handle_points() {
            paint_handle(painter, view.to_screen(rect, ppp, at), handle, p, ppp);
        }
        // The size, as a mono pill above the corner (inside when there is no room).
        let label = format!("{} × {}", self.crop.rect.w.round() as i64, self.crop.rect.h.round() as i64);
        let galley = painter.layout_no_wrap(label, snap_ui::text::mono().resolve(&painter.ctx().global_style()), p.text);
        let size = galley.size() + Vec2::new(16.0, 8.0);
        let mut top_left = Pos2::new(r.left(), r.top() - size.y - 6.0);
        if top_left.y < rect.top() + 4.0 {
            top_left.y = r.top() + 6.0;
            top_left.x = r.left() + 6.0;
        }
        let pill = Rect::from_min_size(top_left, size);
        painter.rect_filled(pill, size.y / 2.0, p.elevated);
        painter.rect_stroke(pill, size.y / 2.0, Stroke::new(1.0, p.border), StrokeKind::Inside);
        painter.galley(pill.center() - galley.size() / 2.0, galley, p.text);
    }

    fn paint_caret(&self, painter: &Painter, rect: Rect, ppp: f32, cx: &CanvasCx, p: &Palette) {
        let Some(edit) = &self.text else { return };
        let Some(shape) = cx.s.doc.get(edit.id) else { return };
        let Some(geom) = text_edit::geom(shape) else { return };
        let view = &*cx.view;
        let m = geom.metrics;
        let lines = text_edit::visual_lines(geom.text, &m);
        let at = |idx: usize| {
            let (line, x) = text_edit::caret_xy(geom.text, &m, &lines, idx);
            PointF::new(geom.origin.x + x, geom.origin.y + line as f32 * m.pitch())
        };
        if let Some((a, b)) = edit.selection() {
            let (la, lb) = (text_edit::caret_xy(geom.text, &m, &lines, a).0, text_edit::caret_xy(geom.text, &m, &lines, b).0);
            for (line, &(start, end)) in lines.iter().enumerate().take(lb + 1).skip(la) {
                let from = if line == la { at(a) } else { at(start) };
                let to = if line == lb { at(b) } else { at(end) };
                let r = RectF::new(from.x, from.y, (to.x - from.x).max(m.size * 0.25), m.pitch());
                painter.rect_filled(view.rect_to_screen(rect, ppp, r), 0.0, p.accent.gamma_multiply(0.3));
            }
        }
        let caret = at(edit.caret);
        let top = view.to_screen(rect, ppp, PointF::new(caret.x, caret.y + m.size * 0.08));
        let bottom = view.to_screen(rect, ppp, PointF::new(caret.x, caret.y + m.size * 1.18));
        let blink_on = painter.ctx().input(|i| ((i.time - edit.moved_at) * 2.0) as i64 % 2 == 0);
        painter.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        if blink_on {
            let x = (top.x * ppp).round() / ppp;
            painter.line_segment([Pos2::new(x, top.y), Pos2::new(x, bottom.y)], Stroke::new(1.5, p.accent));
        }
    }

    fn set_cursor(&self, ctx: &Context, response: &Response, rect: Rect, ppp: f32, cx: &CanvasCx) {
        if matches!(self.gesture, Gesture::Pan) {
            ctx.set_cursor_icon(CursorIcon::Grabbing);
            return;
        }
        if !response.hovered() && matches!(self.gesture, Gesture::None) {
            return;
        }
        if ctx.input(|i| i.key_down(Key::Space)) {
            ctx.set_cursor_icon(CursorIcon::Grab);
            return;
        }
        let pos = ctx.input(|i| i.pointer.latest_pos());
        let near = pos.and_then(|pos| {
            if cx.tool == Tool::Crop {
                self.crop_handle_at(rect, ppp, pos, cx.view)
            } else {
                self.handle_at(rect, ppp, pos, cx).map(|(_, h)| h)
            }
        });
        let gesture_handle = match &self.gesture {
            Gesture::Handle { handle, .. } => Some(*handle),
            _ => None,
        };
        if let Some(handle) = near.or(gesture_handle) {
            ctx.set_cursor_icon(match handle {
                Handle::Corner(0) | Handle::Corner(2) => CursorIcon::ResizeNwSe,
                Handle::Corner(_) => CursorIcon::ResizeNeSw,
                Handle::Edge(0) | Handle::Edge(2) => CursorIcon::ResizeVertical,
                Handle::Edge(_) => CursorIcon::ResizeHorizontal,
                _ => CursorIcon::PointingHand,
            });
            return;
        }
        let over_edited = self.text.as_ref().is_some_and(|edit| {
            let tol = cx.view.doc_len(ppp, SHAPE_GRAB);
            self.pointer_doc.zip(cx.s.doc.get(edit.id)).is_some_and(|(d, sh)| text_hit(sh, d, tol))
        });
        ctx.set_cursor_icon(match cx.tool {
            _ if over_edited => CursorIcon::Text,
            Tool::Select if self.hover.is_some() || matches!(self.gesture, Gesture::Move { .. }) => CursorIcon::Move,
            Tool::Select => CursorIcon::Default,
            Tool::Crop if self.pointer_doc.is_some_and(|d| self.crop.rect.contains(d)) => CursorIcon::Move,
            Tool::Text => CursorIcon::Text,
            _ => CursorIcon::Crosshair,
        });
    }
}

/// Take the undo step for the first change of a text session. A text made for this session is lifted out
/// of the list while the snapshot is taken, so that undo returns to before it existed.
fn checkpoint_text(edit: &mut TextEdit, s: &mut Session) {
    if edit.checkpointed {
        return;
    }
    edit.checkpointed = true;
    match edit.fresh_at.take() {
        Some(at) if s.doc.shapes.get(at).is_some_and(|sh| sh.id == edit.id) => {
            let shape = s.doc.shapes.remove(at);
            s.checkpoint();
            s.doc.shapes.insert(at, shape);
        }
        _ => s.checkpoint(),
    }
}

/// Whether `p` is on the text of `shape` (its box, with a little slack).
fn text_hit(shape: &Shape, p: PointF, tol: f32) -> bool {
    let b = bounds(shape);
    p.x >= b.x - tol && p.x <= b.right() + tol && p.y >= b.y - tol && p.y <= b.bottom() + tol
}

fn text_of(shape: &Shape) -> &str {
    match &shape.kind {
        ShapeKind::Text { text, .. } | ShapeKind::Callout { text, .. } => text,
        _ => "",
    }
}

/// The part of the picture that is exported (the crop, else the whole image), in document pixels.
fn export_area(doc: &Document) -> RectF {
    let r = doc.crop_rect();
    RectF::new(r.x as f32, r.y as f32, r.w as f32, r.h as f32)
}

fn clamp_to(area: RectF, p: PointF) -> PointF {
    PointF::new(p.x.clamp(area.x, area.right().max(area.x)), p.y.clamp(area.y, area.bottom().max(area.y)))
}

/// A note at the bottom of the canvas while an annotation reaches past the exported picture: that part is not saved.
fn paint_outside_note(painter: &Painter, rect: Rect, cx: &CanvasCx, p: &Palette) {
    let area = export_area(&cx.s.doc);
    // A stroke's width of slack: an outline drawn along the edge is not worth a warning.
    let outside = cx.s.doc.shapes.iter().filter(|s| !s.hidden).any(|s| {
        let (b, slack) = (bounds(s), s.style.width.max(1.0));
        b.x < area.x - slack || b.y < area.y - slack || b.right() > area.right() + slack || b.bottom() > area.bottom() + slack
    });
    if !outside {
        return;
    }
    let style = snap_ui::text::caption_style().resolve(&painter.ctx().global_style());
    let galley = painter.layout_no_wrap(snap_core::t("editor.canvas.outside").into_owned(), style, p.text);
    let size = galley.size() + Vec2::new(20.0, 10.0);
    let pill = Rect::from_center_size(Pos2::new(rect.center().x, rect.bottom() - 12.0 - size.y / 2.0), size);
    painter.rect_filled(pill, size.y / 2.0, p.elevated);
    painter.rect_stroke(pill, size.y / 2.0, Stroke::new(1.0, p.border), StrokeKind::Inside);
    painter.galley(pill.center() - galley.size() / 2.0, galley, p.text);
}

fn intersects(a: RectF, b: RectF) -> bool {
    a.x < b.right() && a.right() > b.x && a.y < b.bottom() && a.bottom() > b.y
}

/// The 8 pt white square (or round) grip with an accent border from DESIGN.md.
fn paint_handle(painter: &Painter, at: Pos2, handle: Handle, p: &Palette, ppp: f32) {
    let at = Pos2::new((at.x * ppp).round() / ppp, (at.y * ppp).round() / ppp);
    let stroke = Stroke::new(1.5, p.accent);
    match handle {
        Handle::Start | Handle::End | Handle::Ctrl | Handle::Tail => {
            painter.circle_filled(at, 5.0, Color32::WHITE);
            painter.circle_stroke(at, 5.0, stroke);
        }
        _ => {
            let r = Rect::from_center_size(at, Vec2::splat(8.0));
            painter.rect_filled(r, CornerRadius::same(2), Color32::WHITE);
            painter.rect_stroke(r, CornerRadius::same(2), stroke, StrokeKind::Inside);
        }
    }
}
