//! Diff: compare two images (a "before" and an "after") as highlighted changes, an overlay, side by
//! side, a difference map or a swipe slider, with the share of changed pixels and their bounding
//! box. The result can be copied, saved or opened in the editor. Image sources and the zoomable
//! viewer come from the combine view.

mod canvas;
mod panel;

use std::{any::Any, sync::Arc};

use egui::{Context, Frame, Key, Margin, Modifiers, TextureHandle};
use image::RgbaImage;
use snap_core::{Capture, CaptureKind, Error, t};
use snap_image::diff::{DiffMode, DiffStats, diff};
use snap_ui::{Palette, space};

use super::{Chrome, OpenCx, View, ViewCx, editor::EditorInput, window_spec};
use crate::{
    commands::Command,
    views::combine::{
        source::{self, Imported, Item, LibraryPage, LibraryPicker, Pick},
        viewer::Viewer,
    },
};

pub const TAG: &str = "diff";

/// Slot 0 is the "before" image, slot 1 the "after" image.
const BEFORE: usize = 0;
const AFTER: usize = 1;

pub fn open(cx: &mut OpenCx, a: Option<RgbaImage>, b: Option<RgbaImage>) {
    let mut view = DiffView::new();
    for (slot, image) in [(BEFORE, a), (AFTER, b)] {
        if let Some(image) = image {
            view.set(slot, Item::new(t("diff.capture").into_owned(), image, 1.0));
        }
    }
    cx.open(window_spec(t("diff.title"), TAG, (1120.0, 800.0), (820.0, 560.0)), view);
}

fn slot_key(slot: usize) -> &'static str {
    if slot == BEFORE { "diff.before" } else { "diff.after" }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Highlight,
    Overlay,
    SideBySide,
    Difference,
    Swipe,
}

/// Everything that changes the computed result.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Params {
    mode: Mode,
    /// Opacity of the "after" image in the overlay mode, 0..=1.
    opacity: f32,
    threshold: u8,
}

impl Params {
    fn diff_mode(&self) -> DiffMode {
        match self.mode {
            // Swipe draws the two images itself; the highlight run only provides the statistics.
            Mode::Highlight | Mode::Swipe => DiffMode::Highlight,
            Mode::Overlay => DiffMode::Overlay { opacity: self.opacity },
            Mode::SideBySide => DiffMode::SideBySide,
            Mode::Difference => DiffMode::Difference,
        }
    }
}

struct Slot {
    name: String,
    image: Arc<RgbaImage>,
    scale: f32,
    thumb: RgbaImage,
    thumb_tex: Option<TextureHandle>,
    /// Full-size texture, only needed by the swipe mode.
    tex: Option<TextureHandle>,
}

struct Outcome {
    image: Arc<RgbaImage>,
    stats: DiffStats,
    tex: Option<TextureHandle>,
}

/// A finished comparison from the worker.
struct Computed {
    image: RgbaImage,
    stats: DiffStats,
}

struct DiffView {
    slots: [Option<Slot>; 2],
    params: Params,
    outcome: Option<Outcome>,
    /// A comparison is running on the worker.
    computing: bool,
    /// The inputs changed since the running (or last) comparison started.
    dirty: bool,
    viewer: Viewer,
    picker: LibraryPicker,
    /// Position of the swipe divider, 0..=1 of the image width.
    swipe: f32,
}

impl DiffView {
    fn new() -> Self {
        Self {
            slots: [None, None],
            params: Params { mode: Mode::Highlight, opacity: 0.5, threshold: 4 },
            outcome: None,
            computing: false,
            dirty: false,
            viewer: Viewer::default(),
            picker: LibraryPicker::default(),
            swipe: 0.5,
        }
    }

    fn set(&mut self, slot: usize, item: Item) {
        self.slots[slot] =
            Some(Slot { name: item.name, image: Arc::new(item.image), scale: item.scale, thumb: item.thumb, thumb_tex: None, tex: None });
        self.outcome = None;
        self.dirty = true;
    }

    fn swap(&mut self) {
        self.slots.swap(BEFORE, AFTER);
        self.dirty = true;
    }

    fn pair(&self) -> Option<(&Slot, &Slot)> {
        Some((self.slots[BEFORE].as_ref()?, self.slots[AFTER].as_ref()?))
    }

    fn can_export(&self) -> bool {
        self.pair().is_some() && (self.params.mode == Mode::Swipe || self.outcome.is_some())
    }

    fn start_compute(&mut self, cx: &ViewCx) {
        if !self.dirty || self.computing {
            return;
        }
        self.dirty = false;
        let Some((a, b)) = self.pair().map(|(a, b)| (a.image.clone(), b.image.clone())) else {
            self.outcome = None;
            return;
        };
        self.computing = true;
        let params = self.params;
        cx.spawn(move || {
            let (image, stats) = diff(&a, &b, params.diff_mode(), params.threshold);
            Computed { image, stats }
        });
    }

    fn import(&mut self, cx: &mut ViewCx, imported: Imported) {
        match imported.result {
            Ok(items) => {
                // Several files fill this slot and then the other one.
                let slots = [imported.slot, 1 - imported.slot];
                for (slot, item) in slots.into_iter().zip(items) {
                    self.set(slot, item);
                }
            }
            Err(Error::Cancelled) => {}
            Err(e) => cx.toast_error(&e),
        }
    }

    fn choose(&mut self, cx: &ViewCx, slot: usize, pick: Pick) {
        match pick {
            Pick::Files => source::pick_files(cx, slot, false),
            Pick::Clipboard => source::paste(cx, slot),
            Pick::Library => self.picker.open(cx, slot, false),
            Pick::Latest => source::load_recent(cx, &[(slot, 0)]),
        }
    }

    fn shortcuts(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let dropped = source::dropped_images(ctx);
        if !dropped.is_empty() {
            let slot = if self.slots[BEFORE].is_none() { BEFORE } else { AFTER };
            source::load_files(cx, slot, dropped);
        }
        if ctx.egui_wants_keyboard_input() || self.picker.is_open() {
            return;
        }
        let (copy, save) = ctx.input_mut(|i| (i.consume_key(Modifiers::COMMAND, Key::C), i.consume_key(Modifiers::COMMAND, Key::S)));
        if copy && self.can_export() {
            self.copy(cx);
        }
        if save && self.can_export() {
            self.save(cx);
        }
    }

    /// The image the export buttons act on: the computed map, or the split composite in swipe mode.
    fn export_image(&self) -> Option<Arc<RgbaImage>> {
        if self.params.mode == Mode::Swipe {
            let (a, b) = self.pair()?;
            return Some(Arc::new(canvas::compose_swipe(&a.image, &b.image, self.swipe)));
        }
        self.outcome.as_ref().map(|o| o.image.clone())
    }

    fn copy(&mut self, cx: &mut ViewCx) {
        if let (Some(image), Some((a, b))) = (self.export_image(), self.pair()) {
            source::copy_image(cx, image, source::combined_scale([a.scale, b.scale]));
        }
    }

    fn save(&mut self, cx: &mut ViewCx) {
        if let Some(image) = self.export_image() {
            source::save_image(cx, image);
        }
    }

    fn edit(&mut self, cx: &mut ViewCx) {
        let Some(image) = self.export_image() else { return };
        let mut capture = Capture::from_image(CaptureKind::File, (*image).clone());
        capture.context.window_title = Some(t("diff.title").into_owned());
        cx.push(Command::OpenEditor(EditorInput::from_capture(capture)));
    }

    fn body(&mut self, ui: &mut egui::Ui, cx: &mut ViewCx) {
        let p = Palette::of(ui);
        egui::Panel::right("diff-side")
            .exact_size(312.0)
            .resizable(false)
            .frame(Frame::new().fill(p.surface).inner_margin(Margin::same(space::LG as i8)))
            .show(ui, |ui| self.side(ui, cx));
        egui::CentralPanel::default().frame(Frame::new().fill(p.bg)).show(ui, |ui| self.canvas(ui, cx));
    }
}

impl View for DiffView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        self.shortcuts(cx, ctx);
        self.start_compute(cx);
        let title = t("diff.title");
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
        let msg = match msg.downcast::<Computed>() {
            Ok(done) => {
                self.computing = false;
                self.outcome = self.pair().map(|_| Outcome { image: Arc::new(done.image), stats: done.stats, tex: None });
                return;
            }
            Err(msg) => msg,
        };
        if let Ok(done) = msg.downcast::<source::Exported>() {
            source::exported(cx, *done);
        }
    }
}
