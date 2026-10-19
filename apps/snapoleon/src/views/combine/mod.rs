//! Combine: arrange two or more images in a row, a column or a grid with a live preview, then
//! copy, save or edit the result. Images come from the library, files, the clipboard or drag and
//! drop (see `source`, which the diff view shares), and are reordered by dragging in the strip.

mod side;
pub(crate) mod source;
mod tray;
pub(crate) mod viewer;

use std::{any::Any, sync::Arc};

use egui::{Context, Frame, Key, Margin, Modifiers, Sense, TextureHandle, Vec2};
use image::RgbaImage;
use snap_core::{Capture, CaptureKind, Error, t, tf};
use snap_image::combine::{Align, Layout, combine};
use snap_ui::{
    Palette, icon, space,
    widgets::{Button, ButtonKind, empty_state_with, spinner},
};

use self::source::{Imported, Item, LibraryPage, LibraryPicker, Pick};
use super::{Chrome, OpenCx, View, ViewCx, editor::EditorInput, window_spec};
use crate::commands::Command;

pub const TAG: &str = "combine";

/// Images are added to the end of the strip.
const APPEND: usize = 0;

pub fn open(cx: &mut OpenCx, images: Vec<RgbaImage>) {
    let items = images.into_iter().enumerate().map(|(i, image)| Item::new(tf("combine.tile.name", &[("n", &(i + 1).to_string())]), image));
    let mut view = CombineView::new();
    view.add(items.collect());
    cx.open(window_spec(t("combine.title"), TAG, (1080.0, 720.0), (800.0, 520.0)), view);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Row,
    Column,
    Grid,
}

/// Everything the right panel edits; a change re-renders the preview.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Params {
    mode: Mode,
    /// Columns of the grid.
    cols: u32,
    gap: u32,
    align: Align,
    background: [u8; 4],
}

impl Default for Params {
    fn default() -> Self {
        Self { mode: Mode::Row, cols: 2, gap: 16, align: Align::Center, background: [247, 247, 245, 255] }
    }
}

impl Params {
    fn layout(&self) -> Layout {
        match self.mode {
            Mode::Row => Layout::Horizontal,
            Mode::Column => Layout::Vertical,
            Mode::Grid => Layout::Grid { cols: self.cols },
        }
    }
}

struct Tile {
    id: u64,
    name: String,
    image: Arc<RgbaImage>,
    thumb: RgbaImage,
    tex: Option<TextureHandle>,
}

/// The combined image and, once the UI has seen it, its texture.
struct Preview {
    image: Arc<RgbaImage>,
    tex: Option<TextureHandle>,
}

/// A finished render from the worker.
struct Rendered(RgbaImage);

struct CombineView {
    tiles: Vec<Tile>,
    next_id: u64,
    selected: Option<u64>,
    params: Params,
    preview: Option<Preview>,
    /// A render is running on the worker.
    rendering: bool,
    /// The inputs changed since the running (or last) render started.
    dirty: bool,
    viewer: viewer::Viewer,
    picker: LibraryPicker,
    /// The tile being dragged in the strip.
    dragging: Option<u64>,
}

impl CombineView {
    fn new() -> Self {
        Self {
            tiles: Vec::new(),
            next_id: 1,
            selected: None,
            params: Params::default(),
            preview: None,
            rendering: false,
            dirty: false,
            viewer: viewer::Viewer::default(),
            picker: LibraryPicker::default(),
            dragging: None,
        }
    }

    fn add(&mut self, items: Vec<Item>) {
        for item in items {
            self.tiles.push(Tile { id: self.next_id, name: item.name, image: Arc::new(item.image), thumb: item.thumb, tex: None });
            self.next_id += 1;
        }
        self.dirty = true;
    }

    fn remove(&mut self, id: u64) {
        self.tiles.retain(|t| t.id != id);
        if self.selected == Some(id) {
            self.selected = None;
        }
        self.dirty = true;
    }

    /// Move `id` so it ends up in front of the tile that is at `before` now (`len` = the end).
    fn move_tile(&mut self, id: u64, before: usize) {
        let Some(from) = self.tiles.iter().position(|t| t.id == id) else { return };
        let to = if before > from { before - 1 } else { before };
        if to != from {
            let tile = self.tiles.remove(from);
            self.tiles.insert(to, tile);
            self.dirty = true;
        }
    }

    fn can_export(&self) -> bool {
        self.tiles.len() >= 2 && self.preview.is_some()
    }

    fn start_render(&mut self, cx: &ViewCx) {
        if !self.dirty || self.rendering {
            return;
        }
        self.dirty = false;
        if self.tiles.is_empty() {
            self.preview = None;
            return;
        }
        self.rendering = true;
        let images: Vec<Arc<RgbaImage>> = self.tiles.iter().map(|t| t.image.clone()).collect();
        let params = self.params;
        cx.spawn(move || {
            let owned: Vec<RgbaImage> = images.iter().map(|i| (**i).clone()).collect();
            Rendered(combine(&owned, params.layout(), params.gap, params.align, params.background))
        });
    }

    fn import(&mut self, cx: &mut ViewCx, imported: Imported) {
        match imported.result {
            Ok(items) => {
                let first = self.next_id;
                self.add(items);
                self.selected = Some(first);
            }
            Err(Error::Cancelled) => {}
            Err(e) => cx.toast_error(&e),
        }
    }

    fn add_from(&mut self, cx: &ViewCx, pick: Pick) {
        match pick {
            Pick::Files => source::pick_files(cx, APPEND, true),
            Pick::Clipboard => source::paste(cx, APPEND),
            Pick::Library => self.picker.open(cx, APPEND, true),
            Pick::Latest => source::load_recent(cx, &[(APPEND, 0)]),
        }
    }

    fn shortcuts(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let dropped = source::dropped_images(ctx);
        if !dropped.is_empty() {
            source::load_files(cx, APPEND, dropped);
        }
        if ctx.egui_wants_keyboard_input() || self.picker.is_open() {
            return;
        }
        let (copy, save, paste, files, delete) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::COMMAND, Key::C),
                i.consume_key(Modifiers::COMMAND, Key::S),
                i.consume_key(Modifiers::COMMAND, Key::V) | i.events.iter().any(|e| matches!(e, egui::Event::Paste(_))),
                i.consume_key(Modifiers::COMMAND, Key::O),
                i.consume_key(Modifiers::NONE, Key::Delete) | i.consume_key(Modifiers::NONE, Key::Backspace),
            )
        });
        if copy && self.can_export() {
            self.copy(cx);
        }
        if save && self.can_export() {
            self.save(cx);
        }
        if paste {
            source::paste(cx, APPEND);
        }
        if files {
            source::pick_files(cx, APPEND, true);
        }
        if let (true, Some(id)) = (delete, self.selected) {
            self.remove(id);
        }
    }

    fn result(&self) -> Option<Arc<RgbaImage>> {
        self.preview.as_ref().map(|p| p.image.clone())
    }

    fn copy(&mut self, cx: &mut ViewCx) {
        if let Some(image) = self.result() {
            source::copy_image(cx, image);
        }
    }

    fn save(&mut self, cx: &mut ViewCx) {
        if let Some(image) = self.result() {
            source::save_image(cx, image);
        }
    }

    fn edit(&mut self, cx: &mut ViewCx) {
        let Some(image) = self.result() else { return };
        let mut capture = Capture::from_image(CaptureKind::File, (*image).clone());
        capture.context.window_title = Some(t("combine.title").into_owned());
        cx.push(Command::OpenEditor(EditorInput::from_capture(capture)));
    }

    fn body(&mut self, ui: &mut egui::Ui, cx: &mut ViewCx) {
        let p = Palette::of(ui);
        if self.tiles.is_empty() {
            self.empty(ui, cx);
            return;
        }
        egui::Panel::right("combine-side")
            .exact_size(312.0)
            .resizable(false)
            .frame(Frame::new().fill(p.surface).inner_margin(Margin::same(space::LG as i8)))
            .show(ui, |ui| self.side(ui, cx));
        egui::Panel::bottom("combine-tray")
            .exact_size(tray::HEIGHT)
            .resizable(false)
            .frame(Frame::new().fill(p.surface).inner_margin(Margin::symmetric(space::LG as i8, space::MD as i8)))
            .show(ui, |ui| self.tray(ui, cx));
        egui::CentralPanel::default().frame(Frame::new().fill(p.bg)).show(ui, |ui| self.canvas(ui));
    }

    fn empty(&mut self, ui: &mut egui::Ui, cx: &mut ViewCx) {
        ui.add_space(((ui.available_height() - 200.0) / 2.0).max(0.0));
        let mut chosen = None;
        empty_state_with(ui, icon::SQUARES_FOUR, &t("combine.empty.title"), &t("combine.empty.body"), |ui| {
            let resp = ui.add(Button::new(&t("combine.add_images")).icon(icon::PLUS).kind(ButtonKind::Primary));
            chosen = source::source_menu(ui, &resp, false);
        });
        if let Some(pick) = chosen {
            self.add_from(cx, pick);
        }
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        ui.allocate_rect(rect, Sense::hover());
        if let Some(preview) = &mut self.preview {
            let tex = preview.tex.get_or_insert_with(|| source::upload(ui.ctx(), "combine-preview", &preview.image));
            let px = Vec2::new(preview.image.width() as f32, preview.image.height() as f32);
            let placed = self.viewer.place(ui, rect, px);
            placed.paint(ui, tex);
            placed.paint_outline(ui);
            self.viewer.controls(ui, rect, px);
        } else {
            ui.put(egui::Rect::from_center_size(rect.center(), Vec2::splat(24.0)), spinner);
        }
    }
}

impl View for CombineView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        self.shortcuts(cx, ctx);
        self.start_render(cx);
        let title = t("combine.title");
        Chrome::new(&title).show(cx, ctx, |ui, cx| self.body(ui, cx));
        self.picker.show(ctx, cx);
        source::paint_drop_hint(ctx);
    }

    fn on_msg(&mut self, cx: &mut ViewCx, msg: Box<dyn Any + Send>) {
        let msg = match msg.downcast::<Imported>() {
            Ok(imported) => return self.import(cx, *imported),
            Err(msg) => msg,
        };
        let msg = match msg.downcast::<LibraryPage>() {
            Ok(page) => return self.picker.on_page(*page),
            Err(msg) => msg,
        };
        let msg = match msg.downcast::<Rendered>() {
            Ok(rendered) => {
                self.rendering = false;
                self.preview = (!self.tiles.is_empty()).then(|| Preview { image: Arc::new(rendered.0), tex: None });
                return;
            }
            Err(msg) => msg,
        };
        if let Ok(done) = msg.downcast::<source::Exported>() {
            source::exported(cx, *done);
        }
    }
}
