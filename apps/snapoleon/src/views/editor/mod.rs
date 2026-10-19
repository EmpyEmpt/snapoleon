//! The annotation editor: tool rail, context bar, canvas, right panel and status bar around a
//! `snap_annotate::Document`. See docs/DESIGN.md "Editor".
//!
//! `EditorInput` and `open` are the entry points the rest of the app uses; everything else is internal.

use std::{
    any::Any,
    path::{Path, PathBuf},
};

use crossbeam_channel::Sender;
use egui::{Context, Event, Frame, Key, Margin, Modifiers, Panel as SidePanel, Stroke, Ui, UiBuilder, Vec2};
use image::RgbaImage;
use snap_annotate::{DocMeta, Document};
use snap_core::{Capture, CaptureKind, PointF, Rect, Result, Settings, t, tf};
use snap_engine::Annotated;
use snap_ui::{
    icon, palette, space, text,
    widgets::{
        Button, ButtonKind, icon_button, tool_button, tooltip,
        util::{Motion, anim},
    },
};

use super::{Chrome, OpenCx, View, ViewCx, window_spec};

mod actions;
mod canvas;
mod create;
mod crop;
mod demo;
mod options;
mod output;
mod panel;
mod previews;
mod session;
mod text_edit;
mod tools;
mod viewport;

use canvas::{Canvas, CanvasCx};
use output::{ExportParams, Msg};
use panel::{Panel, PanelCx, Tab};
use previews::{Previews, Wants};
use session::{Order, Session};
use tools::{Tool, ToolSet};
use viewport::Viewport;

pub const TAG: &str = "editor";

/// Everything the editor opens with.
pub struct EditorInput {
    pub capture: Capture,
    /// An existing annotation document (a re-opened `.snap` project).
    pub document: Option<Document>,
    /// Where the project is saved; `None` until the first "Save project".
    pub project_path: Option<PathBuf>,
    /// Set when a pipeline's `Annotate` step is waiting for the result. The editor sends exactly
    /// once when it closes: `Some(Annotated)` when the user finished, `None` when they discarded.
    /// Dropping the sender without sending counts as discarding.
    pub reply: Option<Sender<Option<Annotated>>>,
}

impl EditorInput {
    pub fn from_capture(capture: Capture) -> Self {
        Self { capture, document: None, project_path: None, reply: None }
    }

    /// An image file, or a `.snap` project.
    pub fn from_file(path: &Path) -> Result<Self> {
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case(snap_core::PROJECT_EXT)) {
            return Self::from_project(path);
        }
        let image = snap_image::encode::load(path)?;
        let mut capture = Capture::from_image(CaptureKind::File, image);
        capture.context.window_title = path.file_name().map(|n| n.to_string_lossy().into_owned());
        Ok(Self::from_capture(capture))
    }

    pub fn from_project(path: &Path) -> Result<Self> {
        let document = snap_annotate::load_project(path)?;
        let (w, h) = document.canvas_size();
        let mut capture = Capture::new(CaptureKind::File, (*document.base).clone(), Rect::new(0, 0, w, h), document.scale_factor);
        if let Some(id) = &document.meta.capture_id {
            capture.id = id.clone();
        }
        capture.context = document.meta.context.clone();
        capture.taken_at_ms = document.meta.created_ms;
        Ok(Self { capture, document: Some(document), project_path: Some(path.to_path_buf()), reply: None })
    }
}

pub fn open(cx: &mut OpenCx, input: EditorInput) {
    let mut view = EditorView::new(input, cx.settings);
    // A project from the library belongs to an entry with a picture: saving updates both.
    if view.project.is_some() {
        view.output_file =
            cx.engine.history().and_then(|h| h.get(&view.meta.id).ok().flatten()).and_then(|e| e.file).filter(|f| f.exists());
    }
    cx.open(window_spec(t("editor.title"), TAG, (1200.0, 800.0), (820.0, 540.0)), view);
}

/// Things the user can ask for, from buttons, menus and keys.
#[derive(Clone, Debug, PartialEq)]
enum Act {
    Copy,
    Save,
    SaveAs,
    Upload,
    Pin,
    Done,
    Undo,
    Redo,
    Paste,
    PasteImage,
    CopyShapes,
    CutShapes,
    PasteShapes,
    PasteText(String),
    Duplicate,
    Delete,
    SelectAll,
    Reorder(OrderKind),
    Zoom(ZoomKind),
    TogglePanel,
    Tool(Tool),
    Escape,
    Nudge(i32, i32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum OrderKind {
    Forward,
    Backward,
    Front,
    Back,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ZoomKind {
    Fit,
    Actual,
    In,
    Out,
}

/// Which outputs are being made right now.
#[derive(Default)]
struct Working {
    copy: bool,
    save: bool,
    upload: bool,
    done: bool,
}

/// What the title bar draws and needs to know.
struct Bar {
    can_undo: bool,
    can_redo: bool,
    reply: bool,
    working: [bool; 4],
}

pub struct EditorView {
    s: Session,
    /// The capture's context for naming and history; its image is a 1x1 stand-in, the pixels live in the document.
    meta: Capture,
    project: Option<PathBuf>,
    /// The picture the last save wrote (the library entry's file); saving again replaces it in place.
    output_file: Option<PathBuf>,
    /// The document revision `output_file` shows; an upload sends the file as is while the document still is that.
    output_rev: u64,
    /// The document revision the running output was started from.
    export_rev: u64,
    /// The toast showing the progress of a long step of an output.
    progress_toast: Option<u64>,
    reply: Option<Sender<Option<Annotated>>>,
    tool: Tool,
    tools: ToolSet,
    /// Display density of the capture; default sizes of strokes and text are multiplied by it.
    ui_scale: f32,
    view: Viewport,
    canvas: Canvas,
    panel: Panel,
    previews: Previews,
    /// The document revision of the last copy, save, upload or finish.
    saved_rev: u64,
    working: Working,
    searching: bool,
    /// The user was asked about unsaved changes and answered "close anyway".
    confirm_close: bool,
    discard: bool,
    closing: bool,
    /// What copying shapes put on the system clipboard, to tell our own shapes from foreign text on paste.
    shape_clip_text: String,
    /// Tool, width and font size at open, to store only what the user changed.
    opened_with: (Tool, f32, f32),
    started: bool,
}

impl EditorView {
    fn new(input: EditorInput, settings: &Settings) -> Self {
        let EditorInput { mut capture, document, project_path, reply } = input;
        let image = std::mem::replace(&mut capture.image, RgbaImage::new(1, 1));
        let ui_scale = if document.is_some() { 1.0 } else { capture.scale_factor.clamp(1.0, 4.0) };
        let doc = document.unwrap_or_else(|| {
            let mut doc = Document::new(image, capture.scale_factor);
            doc.meta = DocMeta { capture_id: Some(capture.id.clone()), context: capture.context.clone(), created_ms: capture.taken_at_ms };
            doc
        });
        let tools = ToolSet::new(settings, ui_scale);
        let tool = match Tool::from_id(&settings.editor.last_tool) {
            Some(Tool::Crop) | None => Tool::Arrow,
            Some(tool) => tool,
        };
        let opened_with = (tool, settings.editor.stroke_width, settings.editor.font_size);
        let mut view = Self {
            s: Session::new(doc),
            meta: capture,
            project: project_path,
            output_file: None,
            output_rev: 0,
            export_rev: 0,
            progress_toast: None,
            reply,
            tool,
            tools,
            ui_scale,
            view: Viewport::new(),
            canvas: Canvas::new(),
            panel: Panel::new(true),
            previews: Previews::new(),
            saved_rev: 0,
            working: Working::default(),
            searching: false,
            confirm_close: false,
            discard: false,
            closing: false,
            shape_clip_text: String::new(),
            opened_with,
            started: false,
        };
        demo::apply(&mut view);
        view
    }

    fn dirty(&self) -> bool {
        self.s.rev != self.saved_rev
    }

    fn title(&self) -> String {
        match self.output_file.as_deref().or(self.project.as_deref()).and_then(|p| p.file_stem()) {
            Some(name) => name.to_string_lossy().into_owned(),
            None => t("editor.title").into_owned(),
        }
    }

    /// Switch tools, finishing whatever the old one was doing.
    fn set_tool(&mut self, tool: Tool) {
        if tool == self.tool {
            return;
        }
        self.tool = tool;
        self.canvas.tool_changed(&mut self.s, tool);
        if tool != Tool::Select {
            // Drawing tools start from a clean slate; the select tool keeps the selection.
            self.s.selection.clear();
        }
    }

    /// The document point in the middle of what is on screen.
    fn visible_center(&self) -> PointF {
        let half = self.canvas.phys / 2.0 / self.view.zoom;
        let (w, h) = self.s.doc.canvas_size();
        PointF::new((self.view.offset.x + half.x).clamp(0.0, w as f32), (self.view.offset.y + half.y).clamp(0.0, h as f32))
    }

    fn zoom(&mut self, kind: ZoomKind) {
        let (image, canvas) = (self.s.doc.canvas_size(), self.canvas.phys);
        match kind {
            ZoomKind::Fit => self.view.fit_now(canvas, image),
            ZoomKind::Actual => self.view.actual_size(canvas, image),
            ZoomKind::In => self.view.zoom_step(1.0, canvas, image),
            ZoomKind::Out => self.view.zoom_step(-1.0, canvas, image),
        }
    }

    fn act(&mut self, cx: &mut ViewCx, act: Act) {
        tracing::debug!("editor: {act:?}");
        match act {
            Act::Copy => self.copy(cx),
            Act::Save => self.save(cx),
            Act::SaveAs => self.save_as(cx),
            Act::Upload => self.upload(cx),
            Act::Pin => self.pin(cx),
            Act::Done => self.done(cx),
            Act::Paste => match snap_platform::clipboard::get_text() {
                Some(text) if self.s.has_clipboard() && text == self.shape_clip_text => self.s.paste_shapes(16.0 * self.ui_scale),
                _ => self.paste_image(cx),
            },
            Act::PasteImage => self.paste_image(cx),
            Act::Undo => {
                self.canvas.commit_text(&mut self.s);
                self.s.undo();
                self.after_history();
            }
            Act::Redo => {
                self.canvas.commit_text(&mut self.s);
                self.s.redo();
                self.after_history();
            }
            Act::CopyShapes => {
                let count = self.s.copy_selection();
                let text: Vec<String> = self.s.selected_shapes().filter_map(|s| text_edit::geom(s).map(|g| g.text.to_string())).collect();
                self.shape_clip_text = if text.iter().all(|t| t.is_empty()) {
                    tf("editor.clipboard.shapes", &[("n", &count.to_string())])
                } else {
                    text.join("\n")
                };
                let _ = snap_platform::clipboard::set_text(&self.shape_clip_text);
            }
            Act::CutShapes => {
                self.act(cx, Act::CopyShapes);
                self.s.delete_selection();
            }
            Act::PasteShapes => self.s.paste_shapes(16.0 * self.ui_scale),
            Act::PasteText(text) => self.paste_text(cx, text),
            Act::Duplicate => self.s.duplicate(16.0 * self.ui_scale),
            Act::Delete => {
                self.s.delete_selection();
            }
            Act::SelectAll => self.s.select_all(),
            Act::Reorder(kind) => self.s.reorder(match kind {
                OrderKind::Forward => Order::Forward,
                OrderKind::Backward => Order::Backward,
                OrderKind::Front => Order::Front,
                OrderKind::Back => Order::Back,
            }),
            Act::Zoom(kind) => self.zoom(kind),
            Act::TogglePanel => self.panel.open = !self.panel.open,
            Act::Tool(tool) => self.set_tool(tool),
            Act::Escape => {
                if self.tool != Tool::Crop && !self.s.selection.is_empty() {
                    self.s.selection.clear();
                } else {
                    self.set_tool(Tool::Select);
                }
            }
            Act::Nudge(dx, dy) => self.s.nudge(PointF::new(dx as f32, dy as f32)),
        }
    }

    fn after_history(&mut self) {
        if self.tool == Tool::Crop {
            self.canvas.crop.begin(&self.s);
        }
    }

    /// Ctrl+V with text: our own copied shapes, a clipboard picture, or text to place as a note.
    fn paste_text(&mut self, cx: &mut ViewCx, text: String) {
        if self.s.has_clipboard() && text == self.shape_clip_text {
            self.act(cx, Act::PasteShapes);
        } else if let Some(image) = snap_platform::clipboard::get_image() {
            self.add_pasted_image(image);
        } else {
            self.canvas.commit_text(&mut self.s);
            let at = self.visible_center();
            let style = self.tools.styles.get(&Tool::Text).copied().unwrap_or_default();
            let x = &self.tools.extras;
            let kind = snap_annotate::ShapeKind::Text { pos: at, text, size: x.font_size, bold: x.bold, bg: x.text_bg, max_width: None };
            self.s.checkpoint();
            let id = self.s.add(kind, style);
            self.s.select_only(id);
        }
    }

    /// Keys that work anywhere in the window.
    fn shortcuts(&mut self, ctx: &Context, acts: &mut Vec<Act>) {
        if self.confirm_close {
            return;
        }
        let typing = self.canvas.editing_text() || ctx.egui_wants_keyboard_input();
        if typing {
            return;
        }
        let selection = !self.s.selection.is_empty();
        let reply = self.reply.is_some();
        let events: Vec<Event> = ctx.input(|i| i.events.clone());
        for event in events {
            match event {
                Event::Copy => acts.push(if selection { Act::CopyShapes } else { Act::Copy }),
                Event::Cut if selection => acts.push(Act::CutShapes),
                Event::Paste(text) => acts.push(Act::PasteText(text)),
                Event::PasteImage(_) => acts.push(Act::PasteImage),
                _ => {}
            }
        }
        let cmd = Modifiers::COMMAND;
        let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let none = Modifiers::NONE;
        let shift = ctx.input(|i| i.modifiers.shift);
        let step = if shift { 10 } else { 1 };
        // Most specific first: Ctrl+Shift+S must not be taken as Ctrl+S.
        let mut keys: Vec<(Modifiers, Key, Act)> = vec![
            (cmd_shift, Key::Z, Act::Redo),
            (cmd, Key::Z, Act::Undo),
            (cmd, Key::Y, Act::Redo),
            (cmd_shift, Key::S, Act::SaveAs),
            (cmd, Key::S, Act::Save),
            (cmd, Key::D, Act::Duplicate),
            (cmd, Key::A, Act::SelectAll),
            (cmd, Key::Num0, Act::Zoom(ZoomKind::Fit)),
            (cmd, Key::Num1, Act::Zoom(ZoomKind::Actual)),
            (cmd, Key::Equals, Act::Zoom(ZoomKind::In)),
            (cmd, Key::Plus, Act::Zoom(ZoomKind::In)),
            (cmd, Key::Minus, Act::Zoom(ZoomKind::Out)),
            (cmd_shift, Key::CloseBracket, Act::Reorder(OrderKind::Front)),
            (cmd_shift, Key::OpenBracket, Act::Reorder(OrderKind::Back)),
            (cmd, Key::CloseBracket, Act::Reorder(OrderKind::Forward)),
            (cmd, Key::OpenBracket, Act::Reorder(OrderKind::Backward)),
            (cmd, Key::Backslash, Act::TogglePanel),
            (none, Key::Escape, Act::Escape),
        ];
        if reply {
            keys.push((cmd, Key::Enter, Act::Done));
            keys.push((none, Key::Enter, Act::Done));
        }
        if selection {
            keys.push((none, Key::Delete, Act::Delete));
            keys.push((none, Key::Backspace, Act::Delete));
            keys.push((none, Key::ArrowLeft, Act::Nudge(-step, 0)));
            keys.push((none, Key::ArrowRight, Act::Nudge(step, 0)));
            keys.push((none, Key::ArrowUp, Act::Nudge(0, -step)));
            keys.push((none, Key::ArrowDown, Act::Nudge(0, step)));
        }
        keys.extend(Tool::GROUPS.iter().flat_map(|g| g.iter()).map(|tool| (none, tool.key(), Act::Tool(*tool))));
        ctx.input_mut(|i| {
            for (modifiers, key, act) in keys {
                if i.consume_key(modifiers, key) {
                    acts.push(act);
                }
            }
        });
    }

    fn title_actions(ui: &mut Ui, bar: &Bar, acts: &mut Vec<Act>) {
        ui.spacing_mut().interact_size.y = 28.0;
        ui.spacing_mut().item_spacing.x = 4.0;
        let [copy, save, upload, done] = bar.working;
        ui.add_enabled_ui(bar.can_undo, |ui| {
            if icon_button(ui, icon::ARROW_U_UP_LEFT, &t("editor.undo"), Some("Ctrl+Z")).clicked() {
                acts.push(Act::Undo);
            }
        });
        ui.add_enabled_ui(bar.can_redo, |ui| {
            if icon_button(ui, icon::ARROW_U_UP_RIGHT, &t("editor.redo"), Some("Ctrl+Shift+Z")).clicked() {
                acts.push(Act::Redo);
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if bar.reply {
                let label = t("common.done");
                let resp = ui.add_enabled(!done, Button::new(&label).kind(ButtonKind::Primary).icon(icon::CHECK));
                if resp.clicked() {
                    acts.push(Act::Done);
                }
                tooltip(resp, &t("editor.done_tip"), Some("Enter"));
                ui.add_enabled_ui(!copy, |ui| {
                    if icon_button(ui, icon::COPY, &t("editor.copy_tip"), Some("Ctrl+C")).clicked() {
                        acts.push(Act::Copy);
                    }
                });
            } else {
                let label = t("common.copy");
                let resp = ui.add_enabled(!copy, Button::new(&label).kind(ButtonKind::Primary).icon(icon::COPY));
                if resp.clicked() {
                    acts.push(Act::Copy);
                }
                tooltip(resp, &t("editor.copy_tip"), Some("Ctrl+C"));
            }
            ui.add_space(space::XS);
            ui.add_enabled_ui(!save, |ui| {
                if icon_button(ui, icon::FLOPPY_DISK, &t("editor.save_tip"), Some("Ctrl+S")).clicked() {
                    acts.push(Act::Save);
                }
                if icon_button(ui, icon::FILE_ARROW_DOWN, &t("editor.save_as_tip"), Some("Ctrl+Shift+S")).clicked() {
                    acts.push(Act::SaveAs);
                }
            });
            ui.add_enabled_ui(!upload, |ui| {
                if icon_button(ui, icon::CLOUD_ARROW_UP, &t("editor.upload_tip"), None).clicked() {
                    acts.push(Act::Upload);
                }
            });
            if icon_button(ui, icon::PUSH_PIN, &t("editor.pin_tip"), None).clicked() {
                acts.push(Act::Pin);
            }
        });
    }

    fn rail(&mut self, ui: &mut Ui, acts: &mut Vec<Act>) {
        let p = palette(ui.ctx());
        ui.spacing_mut().item_spacing = Vec2::new(0.0, 2.0);
        egui::ScrollArea::vertical().scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
            for (i, group) in Tool::GROUPS.iter().enumerate() {
                if i > 0 {
                    ui.add_space(4.0);
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
                    ui.painter().hline(rect.shrink2(Vec2::new(8.0, 0.0)).x_range(), rect.center().y, Stroke::new(1.0, p.border));
                    ui.add_space(4.0);
                }
                for tool in group.iter() {
                    if tool_button(ui, tool.icon(), &tool.label(), Some(tool.hotkey()), self.tool == *tool).clicked() {
                        acts.push(Act::Tool(*tool));
                    }
                }
            }
            ui.add_space(8.0);
            if tool_button(ui, icon::IMAGE, &t("editor.tool.image"), Some("Ctrl+V"), false).clicked() {
                acts.push(Act::PasteImage);
            }
        });
    }

    fn status_bar(&mut self, ui: &mut Ui, acts: &mut Vec<Act>, params: ExportParams) {
        ui.spacing_mut().interact_size.y = 24.0;
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if icon_button(ui, icon::MINUS, &t("editor.zoom_out"), Some("Ctrl+-")).clicked() {
                acts.push(Act::Zoom(ZoomKind::Out));
            }
            let zoom = format!("{}%", self.view.percent());
            let resp = ui.add(Button::new(&zoom).kind(ButtonKind::Ghost).min_width(52.0));
            if resp.clicked() {
                acts.push(Act::Zoom(ZoomKind::Actual));
            }
            tooltip(resp, &t("editor.zoom_actual"), Some("Ctrl+1"));
            if icon_button(ui, icon::PLUS, &t("editor.zoom_in"), Some("Ctrl++")).clicked() {
                acts.push(Act::Zoom(ZoomKind::In));
            }
            if icon_button(ui, icon::CORNERS_OUT, &t("editor.zoom_fit"), Some("Ctrl+0")).clicked() {
                acts.push(Act::Zoom(ZoomKind::Fit));
            }
            ui.add_space(space::SM);
            if let Some(at) = self.canvas.pointer_doc {
                let (w, h) = self.s.doc.canvas_size();
                if at.x >= 0.0 && at.y >= 0.0 && at.x < w as f32 && at.y < h as f32 {
                    ui.label(text::mono_text(format!("{}, {}", at.x as i32, at.y as i32)).color(palette(ui.ctx()).text_faint));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = space::SM;
                let open = self.panel.open;
                let tip = t(if open { "editor.panel_hide" } else { "editor.panel_show" });
                if icon_button(ui, icon::SIDEBAR_SIMPLE, &tip, Some("Ctrl+\\")).clicked() {
                    acts.push(Act::TogglePanel);
                }
                ui.add_space(space::SM);
                let dim = palette(ui.ctx()).text_dim;
                let crop = self.s.doc.crop_rect();
                let (w, h) = self.s.doc.canvas_size();
                let export = match self.previews.latest().filter(|_| self.previews.is_current(self.s.rev, params)) {
                    Some(e) => {
                        let size = format!("{} × {}", e.size.0, e.size.1);
                        match e.bytes {
                            Some(b) => format!("{size}  ·  ~{}", panel::bytes_label(b)),
                            None => size,
                        }
                    }
                    None => "…".to_string(),
                };
                ui.label(text::mono_text(export).color(dim));
                ui.label(text::caption(t("editor.status.export")));
                ui.add_space(space::MD);
                let source =
                    if crop.w != w || crop.h != h { format!("{} × {}  ({w} × {h})", crop.w, crop.h) } else { format!("{w} × {h}") };
                ui.label(text::mono_text(source).color(dim));
            });
        });
    }

    /// Ask what to do with unsaved changes when the window is being closed.
    fn close_prompt(&mut self, ctx: &Context, cx: &mut ViewCx) {
        if !self.confirm_close {
            return;
        }
        let p = palette(ctx);
        let mut keep = false;
        let mut discard = false;
        let modal = egui::Modal::new(egui::Id::unique("editor-close")).backdrop_color(p.scrim).frame(
            Frame::new()
                .fill(p.elevated)
                .stroke(Stroke::new(1.0, p.border))
                .corner_radius(snap_ui::radius::CARD)
                .shadow(p.shadow())
                .inner_margin(Margin::same(20)),
        );
        let response = modal.show(ctx, |ui| {
            ui.set_width(340.0);
            ui.spacing_mut().item_spacing = Vec2::new(space::SM, space::SM);
            ui.label(text::title(t("editor.close.title")));
            ui.label(text::dim(t(if self.reply.is_some() { "editor.close.body_pipeline" } else { "editor.close.body" })));
            ui.add_space(space::SM);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(Button::new(&t("editor.close.discard")).kind(ButtonKind::Danger)).clicked() {
                    discard = true;
                }
                if ui.add(Button::new(&t("editor.close.keep"))).clicked() {
                    keep = true;
                }
            });
        });
        if response.should_close() {
            keep = true;
        }
        if discard {
            self.discard = true;
            self.confirm_close = false;
            cx.close();
        } else if keep {
            self.confirm_close = false;
        }
    }

    fn finish(&mut self, done: Option<Annotated>) {
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(done);
        }
    }

    /// Store the tool and sizes the user settled on, so the next editor starts there.
    fn remember(&self, cx: &mut ViewCx) {
        let (tool, width, font) = self.opened_with;
        let width_now = (self.tools.styles.get(&Tool::Arrow).map_or(width, |s| s.width) / self.ui_scale).round();
        let font_now = (self.tools.extras.font_size / self.ui_scale).round();
        let tool_now = if self.tool == Tool::Crop { Tool::Select } else { self.tool };
        if tool_now != tool || (width_now - width).abs() >= 1.0 || (font_now - font).abs() >= 1.0 {
            cx.update_settings(move |s| {
                s.editor.last_tool = tool_now.id().to_string();
                s.editor.stroke_width = width_now.clamp(1.0, 40.0);
                s.editor.font_size = font_now.clamp(8.0, 200.0);
            });
        }
    }

    fn body(&mut self, ui: &mut Ui, cx: &mut ViewCx, acts: &mut Vec<Act>) {
        let p = palette(ui.ctx());
        let params = ExportParams::from_settings(cx.settings);

        SidePanel::bottom("editor-status")
            .exact_size(28.0)
            .resizable(false)
            .frame(Frame::new().fill(p.surface).inner_margin(Margin::symmetric(8, 0)))
            .show(ui, |ui| self.status_bar(ui, acts, params));

        SidePanel::left("editor-rail")
            .exact_size(44.0)
            .resizable(false)
            .frame(Frame::new().fill(p.surface_2).inner_margin(Margin::symmetric(4, 8)))
            .show(ui, |ui| self.rail(ui, acts));

        let mut reveal = None;
        let slide = anim(ui.ctx(), egui::Id::unique("editor-panel-slide"), self.panel.open, Motion::SLIDE);
        if slide > 0.0 {
            // The panel keeps its full layout while it slides: only its width, and so what is visible, changes.
            let frame = Panel::frame(ui);
            SidePanel::right("editor-panel").exact_size((panel::WIDTH * slide).round()).resizable(false).frame(frame).show(ui, |ui| {
                let outer = ui.max_rect();
                let full = egui::Rect::from_min_size(outer.min, Vec2::new(panel::WIDTH - 2.0 * panel::MARGIN, outer.height()));
                let mut inner = ui.new_child(UiBuilder::new().max_rect(full));
                inner.set_clip_rect(outer);
                let mut pcx = PanelCx { s: &mut self.s, vcx: cx, previews: &mut self.previews };
                reveal = self.panel.ui(&mut inner, &mut pcx).reveal;
            });
        }
        if let Some(rect) = reveal {
            self.view.reveal(rect, self.canvas.phys, self.s.doc.canvas_size());
        }

        let mut request = None;
        SidePanel::top("editor-options").frame(Frame::new()).resizable(false).show(ui, |ui| {
            ui.spacing_mut().interact_size.y = 28.0;
            let mut ocx = options::OptionsCx {
                s: &mut self.s,
                tools: &mut self.tools,
                tool: self.tool,
                canvas: &mut self.canvas,
                ui_scale: self.ui_scale,
                searching: self.searching,
                ai: cx.settings.ai.enabled,
            };
            request = options::show(ui, &mut ocx);
        });
        if let Some(options::Request::AutoRedact { smart }) = request {
            self.auto_redact(cx, smart);
        }

        self.canvas.highlight = self.panel.highlight();
        egui::CentralPanel::default().frame(Frame::new()).show(ui, |ui| {
            let mut ccx = CanvasCx {
                s: &mut self.s,
                view: &mut self.view,
                tool: self.tool,
                tools: &self.tools,
                step_start: cx.settings.editor.step_badge_start.max(1),
                ui_scale: self.ui_scale,
            };
            let response = self.canvas.show(ui, &mut ccx);
            response.context_menu(|ui| Self::canvas_menu(ui, &self.s, acts));
        });
    }

    fn canvas_menu(ui: &mut Ui, s: &Session, acts: &mut Vec<Act>) {
        let has_selection = !s.selection.is_empty();
        let mut item = |ui: &mut Ui, label: String, hotkey: &str, enabled: bool, act: Act| {
            let resp = ui.add_enabled(enabled, egui::Button::new(label).shortcut_text(hotkey));
            if resp.clicked() {
                acts.push(act);
                ui.close();
            }
        };
        item(ui, t("editor.menu.duplicate").into_owned(), "Ctrl+D", has_selection, Act::Duplicate);
        item(ui, t("editor.menu.copy").into_owned(), "Ctrl+C", has_selection, Act::CopyShapes);
        item(ui, t("editor.menu.cut").into_owned(), "Ctrl+X", has_selection, Act::CutShapes);
        item(ui, t("editor.menu.paste").into_owned(), "Ctrl+V", true, Act::Paste);
        ui.separator();
        item(ui, t("editor.menu.front").into_owned(), "Ctrl+Shift+]", has_selection, Act::Reorder(OrderKind::Front));
        item(ui, t("editor.menu.forward").into_owned(), "Ctrl+]", has_selection, Act::Reorder(OrderKind::Forward));
        item(ui, t("editor.menu.backward").into_owned(), "Ctrl+[", has_selection, Act::Reorder(OrderKind::Backward));
        item(ui, t("editor.menu.back").into_owned(), "Ctrl+Shift+[", has_selection, Act::Reorder(OrderKind::Back));
        ui.separator();
        item(ui, t("editor.menu.delete").into_owned(), "Del", has_selection, Act::Delete);
    }
}

impl View for EditorView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        if !std::mem::replace(&mut self.started, true) {
            demo::first_frame(self, cx);
        }
        let mut acts: Vec<Act> = Vec::new();
        self.shortcuts(ctx, &mut acts);

        let bar = Bar {
            can_undo: self.s.undo.can_undo(),
            can_redo: self.s.undo.can_redo(),
            reply: self.reply.is_some(),
            working: [self.working.copy, self.working.save, self.working.upload, self.working.done],
        };
        let title = self.title();
        let mut bar_acts: Vec<Act> = Vec::new();
        Chrome::new(&title).center(|ui| Self::title_actions(ui, &bar, &mut bar_acts)).show(cx, ctx, |ui, cx| self.body(ui, cx, &mut acts));
        acts.extend(bar_acts);
        for act in acts {
            self.act(cx, act);
        }

        let params = ExportParams::from_settings(cx.settings);
        let tab = self.panel.tab;
        let panel_open = self.panel.open;
        let thumbs = panel_open && tab == Tab::Beautify;
        let bytes = panel_open && tab == Tab::Export || self.previews.latest().is_some_and(|e| e.bytes.is_some());
        self.previews.poll(ctx, &self.s, cx, Wants { params, bytes, thumbs, fast: thumbs });

        self.close_prompt(ctx, cx);
    }

    fn close_requested(&mut self, _cx: &mut ViewCx) -> bool {
        if self.closing || self.discard || !self.dirty() {
            return true;
        }
        self.confirm_close = true;
        false
    }

    fn closed(&mut self, cx: &mut ViewCx) {
        self.finish(None);
        self.remember(cx);
    }

    fn on_msg(&mut self, cx: &mut ViewCx, msg: Box<dyn Any + Send>) {
        if let Ok(msg) = msg.downcast::<Msg>() {
            self.handle(cx, *msg);
        }
    }
}
