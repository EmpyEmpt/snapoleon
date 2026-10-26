//! Where the images of combine and diff come from: a file dialog, the clipboard, dropped files and
//! the library. Everything slow runs on a worker thread and comes back as an [`Imported`] message
//! to the window's `on_msg`. Shared by both views.

use std::{borrow::Cow, path::PathBuf, sync::Arc};

use egui::{
    Align, Color32, Context, Id, Layout, Margin, Rect, Response, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Ui, Vec2,
};
use image::RgbaImage;
use snap_core::{CaptureKind, Error, Result, settings::ImageFormat, t, tf};
use snap_history::{Entry, Query};
use snap_ui::{
    Palette, ToastKind, icon, space, text,
    widgets::{empty_state, icon_button, primary_button, secondary_button, spinner, util},
};

use crate::{commands::Command, views::ViewCx};

/// The widest texture a preview uploads; bigger images are shrunk for display only.
const MAX_TEXTURE_SIDE: u32 = 4096;
const THUMB_SIDE: u32 = 320;
/// Width of the add-image menu.
const MENU_WIDTH: f32 = 208.0;
const IMAGE_EXTENSIONS: [&str; 7] = ["png", "jpg", "jpeg", "webp", "bmp", "gif", "tiff"];

pub struct Item {
    pub name: String,
    pub image: RgbaImage,
    /// Pixels per point, as far as is known (1 when nothing says).
    pub scale: f32,
    /// Small version for strips and cards, made on the worker.
    pub thumb: RgbaImage,
}

impl Item {
    pub fn new(name: String, image: RgbaImage, scale: f32) -> Self {
        let thumb = snap_image::resize::thumbnail(&image, THUMB_SIDE);
        Self { name, image, scale, thumb }
    }
}

/// Pixels per point of a picture made from images of these scales, laid out at their pixel size: the smallest, so
/// none of them pastes smaller than it was.
pub fn combined_scale(scales: impl IntoIterator<Item = f32>) -> f32 {
    scales.into_iter().reduce(f32::min).unwrap_or(1.0)
}

/// Images that arrived for `slot` (what asked for them is the view's business), or why none did.
pub struct Imported {
    pub slot: usize,
    pub result: Result<Vec<Item>>,
}

/// Upload an image for display. Nearest when magnified so zoomed-in pixels stay crisp.
pub fn upload(ctx: &Context, name: &str, image: &RgbaImage) -> TextureHandle {
    let shown = if image.width() > MAX_TEXTURE_SIDE || image.height() > MAX_TEXTURE_SIDE {
        Cow::Owned(snap_image::resize::fit(image, Some(MAX_TEXTURE_SIDE), Some(MAX_TEXTURE_SIDE), false))
    } else {
        Cow::Borrowed(image)
    };
    let size = [shown.width() as usize, shown.height() as usize];
    let options = TextureOptions { magnification: egui::TextureFilter::Nearest, ..TextureOptions::LINEAR };
    ctx.load_texture(name, egui::ColorImage::from_rgba_unmultiplied(size, shown.as_raw()), options)
}

fn send_loaded(cx: &ViewCx, slot: usize, load: impl FnOnce() -> Result<Vec<Item>> + Send + 'static) {
    cx.spawn(move || Imported { slot, result: load() });
}

/// Open the file dialog; `multi` allows several files.
pub fn pick_files(cx: &ViewCx, slot: usize, multi: bool) {
    send_loaded(cx, slot, move || {
        let dialog = rfd::FileDialog::new().set_title(t("combine.source.dialog")).add_filter(t("combine.source.images"), &IMAGE_EXTENSIONS);
        let paths = if multi { dialog.pick_files().unwrap_or_default() } else { dialog.pick_file().into_iter().collect() };
        if paths.is_empty() { Err(Error::Cancelled) } else { load_paths(unknown_monitor(paths)) }
    });
}

pub fn paste(cx: &ViewCx, slot: usize) {
    send_loaded(cx, slot, || {
        let image = snap_platform::clipboard::get_image().ok_or_else(|| Error::invalid(t("app.clipboard.no_image")))?;
        Ok(vec![Item::new(t("combine.source.clipboard_name").into_owned(), image, 1.0)])
    });
}

pub fn load_files(cx: &ViewCx, slot: usize, paths: Vec<PathBuf>) {
    send_loaded(cx, slot, move || load_paths(unknown_monitor(paths)));
}

/// Files that did not come from the library, so the monitor they were taken on is not known.
fn unknown_monitor(paths: Vec<PathBuf>) -> Vec<(PathBuf, Option<String>)> {
    paths.into_iter().map(|p| (p, None)).collect()
}

/// Load the `nth` newest still image of the library (0 = the latest capture) for each `(slot, nth)`.
pub fn load_recent(cx: &ViewCx, picks: &[(usize, usize)]) {
    for &(slot, nth) in picks {
        let history = cx.engine.history();
        send_loaded(cx, slot, move || {
            let files = history.map(|h| recent_files(&h, nth + 1)).unwrap_or_default();
            let file = files.into_iter().nth(nth).ok_or_else(|| Error::invalid(t("combine.library.empty_title")))?;
            load_paths(vec![file])
        });
    }
}

/// Paths of the newest still images that still exist, newest first, with the monitor each was taken on.
fn recent_files(history: &snap_history::History, count: usize) -> Vec<(PathBuf, Option<String>)> {
    let query = Query { limit: (count * 2 + 8) as u32, ..Query::default() };
    let entries = history.search(&query).unwrap_or_default();
    entries
        .into_iter()
        .filter(|e| e.kind != CaptureKind::Recording)
        .filter_map(|e| Some((e.file.filter(|f| f.exists())?, e.context.monitor_name)))
        .take(count)
        .collect()
}

/// Decode every readable file (with the monitor it was taken on, when the library knows); fails only when none could
/// be read.
fn load_paths(files: Vec<(PathBuf, Option<String>)>) -> Result<Vec<Item>> {
    let mut items = Vec::new();
    let mut first_error = None;
    for (path, monitor) in files {
        match snap_engine::load_saved_image(&path, monitor.as_deref()) {
            Ok((image, scale)) => {
                items.push(Item::new(path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), image, scale))
            }
            Err(e) => {
                tracing::warn!("cannot load {}: {e}", path.display());
                first_error.get_or_insert(e);
            }
        }
    }
    match (items.is_empty(), first_error) {
        (true, Some(e)) => Err(e),
        _ => Ok(items),
    }
}

/// Image files dropped on the window this frame.
pub fn dropped_images(ctx: &Context) -> Vec<PathBuf> {
    ctx.input(|i| {
        i.raw
            .dropped_files
            .iter()
            .map(|f| f.path().to_path_buf())
            .filter(|p| p.extension().is_some_and(|e| IMAGE_EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x))))
            .collect()
    })
}

/// Dim the window and say where to let go while files hover over it.
pub fn paint_drop_hint(ctx: &Context) {
    if ctx.input(|i| i.raw.hovered_files.is_empty()) {
        return;
    }
    let p = snap_ui::palette(ctx);
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::unique("drop-hint")));
    painter.rect_filled(screen, 0.0, p.scrim);
    painter.rect_stroke(screen.shrink(12.0), 14.0, Stroke::new(2.0, p.accent), StrokeKind::Inside);
    let galley =
        painter.layout_no_wrap(t("combine.source.drop").into_owned(), text::title_style().resolve(&ctx.global_style()), Color32::WHITE);
    painter.galley(screen.center() - galley.size() / 2.0, galley, Color32::WHITE);
}

/// One thumbnail card of the library sheet.
struct Card {
    path: PathBuf,
    /// The monitor the capture was taken on, for its scale.
    monitor: Option<String>,
    label: String,
    size: (u32, u32),
    thumb: Option<RgbaImage>,
    tex: Option<TextureHandle>,
    selected: bool,
}

/// What the worker found; delivered to `on_msg` and handed to [`LibraryPicker::on_page`].
pub struct LibraryPage(Vec<Card>);

enum State {
    Closed,
    Loading,
    Ready(Vec<Card>),
}

/// A sheet over the window that lists recent library images with thumbnails.
pub struct LibraryPicker {
    state: State,
    slot: usize,
    multi: bool,
}

impl Default for LibraryPicker {
    fn default() -> Self {
        Self { state: State::Closed, slot: 0, multi: false }
    }
}

impl LibraryPicker {
    pub fn is_open(&self) -> bool {
        !matches!(self.state, State::Closed)
    }

    pub fn open(&mut self, cx: &ViewCx, slot: usize, multi: bool) {
        self.state = State::Loading;
        self.slot = slot;
        self.multi = multi;
        let history = cx.engine.history();
        cx.spawn(move || LibraryPage(history.map(|h| recent_cards(&h)).unwrap_or_default()));
    }

    pub fn on_page(&mut self, page: LibraryPage) {
        if self.is_open() {
            self.state = State::Ready(page.0);
        }
    }

    /// Draw the sheet. Confirming starts loading the chosen files for the slot given to `open`.
    pub fn show(&mut self, ctx: &Context, cx: &ViewCx) {
        if !self.is_open() {
            return;
        }
        let p = snap_ui::palette(ctx);
        let frame = egui::Frame::new()
            .fill(p.elevated)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(14.0)
            .inner_margin(Margin::same(space::LG as i8))
            .shadow(p.shadow());
        let mut confirm = false;
        let mut cancel = false;
        let multi = self.multi;
        let state = &mut self.state;
        let modal = egui::Modal::new(Id::unique("library-sheet")).backdrop_color(p.scrim).frame(frame).show(ctx, |ui| {
            ui.set_width((ctx.content_rect().width() - 64.0).clamp(360.0, 680.0));
            ui.spacing_mut().item_spacing = Vec2::splat(space::SM);
            ui.horizontal(|ui| {
                ui.label(text::title(t("combine.library.title")));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if icon_button(ui, icon::X, &t("common.close"), Some("Esc")).clicked() {
                        cancel = true;
                    }
                });
            });
            match state {
                State::Closed => {}
                State::Loading => {
                    ui.add_space(96.0);
                    ui.vertical_centered(spinner);
                    ui.add_space(96.0);
                }
                State::Ready(cards) if cards.is_empty() => {
                    ui.add_space(space::XL);
                    empty_state(ui, icon::IMAGES, &t("combine.library.empty_title"), &t("combine.library.empty_body"));
                    ui.add_space(space::XL);
                }
                State::Ready(cards) => {
                    if grid(ui, cards, multi) && !multi {
                        confirm = true;
                    }
                    ui.add_space(space::XS);
                    ui.horizontal(|ui| {
                        let chosen = cards.iter().filter(|c| c.selected).count();
                        ui.label(text::caption(tf("combine.library.selected", &[("n", &chosen.to_string())])));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.add_enabled_ui(chosen > 0, |ui| {
                                if primary_button(ui, &t("combine.library.add")).clicked() {
                                    confirm = true;
                                }
                            });
                            if secondary_button(ui, &t("common.cancel")).clicked() {
                                cancel = true;
                            }
                        });
                    });
                }
            }
        });
        if modal.should_close() {
            cancel = true;
        }
        if confirm {
            if let State::Ready(cards) = std::mem::replace(&mut self.state, State::Closed) {
                let files: Vec<_> = cards.into_iter().filter(|c| c.selected).map(|c| (c.path, c.monitor)).collect();
                send_loaded(cx, self.slot, move || load_paths(files));
            }
        } else if cancel {
            self.state = State::Closed;
        }
    }
}

/// The newest still images that still exist on disk.
fn recent_cards(history: &snap_history::History) -> Vec<Card> {
    let entries = history.search(&Query { limit: 60, ..Query::default() }).unwrap_or_default();
    entries
        .into_iter()
        .filter(|e| e.kind != CaptureKind::Recording)
        .filter_map(|e| {
            let path = e.file.clone().filter(|f| f.exists())?;
            let thumb = e.thumb.as_deref().and_then(|t| snap_image::encode::load(t).ok());
            Some(Card {
                label: card_label(&e, &path),
                size: (e.width, e.height),
                path,
                monitor: e.context.monitor_name.clone(),
                thumb,
                tex: None,
                selected: false,
            })
        })
        .collect()
}

fn card_label(entry: &Entry, path: &std::path::Path) -> String {
    entry
        .context
        .window_title
        .clone()
        .or_else(|| entry.context.app_name.clone())
        .or_else(|| path.file_stem().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default()
}

const COLUMNS: usize = 4;
const CARD_GAP: f32 = 12.0;

/// The card grid; returns true when a card was clicked (the caller decides what that means).
fn grid(ui: &mut egui::Ui, cards: &mut [Card], multi: bool) -> bool {
    let p = Palette::of(ui);
    let width = ui.available_width();
    let cell_w = (width - CARD_GAP * (COLUMNS as f32 - 1.0)) / COLUMNS as f32;
    let thumb_h = (cell_w * 10.0 / 16.0).round();
    let mut hit = None;
    egui::ScrollArea::vertical().max_height(360.0).auto_shrink([false, true]).show(ui, |ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(CARD_GAP);
        egui::Grid::new("library-grid").num_columns(COLUMNS).spacing(Vec2::splat(CARD_GAP)).show(ui, |ui| {
            for (i, card) in cards.iter_mut().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(cell_w, thumb_h + 22.0), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, ui.is_enabled(), card.selected, &card.label));
                let hover = util::hover_anim(ui, &resp);
                let thumb = Rect::from_min_size(rect.min, Vec2::new(cell_w, thumb_h));
                if ui.is_rect_visible(rect) {
                    if card.tex.is_none() {
                        card.tex = card.thumb.as_ref().map(|t| upload(ui.ctx(), "library-card", t));
                    }
                    let painter = ui.painter();
                    painter.rect_filled(thumb, 8.0, p.surface_2);
                    if let Some(tex) = &card.tex {
                        let image = fit_inside(thumb.shrink(1.0), tex.size_vec2());
                        painter.image(tex.id(), image, Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    painter.rect_stroke(
                        thumb,
                        8.0,
                        Stroke::new(1.0, util::lerp_color(p.border, p.border_strong(), hover)),
                        StrokeKind::Inside,
                    );
                    if card.selected {
                        painter.rect_stroke(thumb.expand(2.0), 10.0, Stroke::new(2.0, p.accent), StrokeKind::Outside);
                        let dot = egui::Pos2::new(thumb.right() - 14.0, thumb.top() + 14.0);
                        painter.circle_filled(dot, 10.0, p.accent);
                        util::paint_icon(ui, dot, icon::CHECK, 13.0, p.on_accent);
                    }
                    let label = util::layout(ui, &card.label, &text::caption_style(), p.text_dim);
                    let clipped =
                        ui.painter().with_clip_rect(Rect::from_min_size(egui::pos2(rect.left(), thumb.bottom()), Vec2::new(cell_w, 22.0)));
                    clipped.galley(util::snap(ui, egui::pos2(rect.left(), thumb.bottom() + 5.0)), label, p.text_dim);
                }
                let tip = format!(
                    "{}
{} × {}",
                    card.label, card.size.0, card.size.1
                );
                if resp.on_hover_text(tip).clicked() {
                    hit = Some(i);
                }
                if (i + 1) % COLUMNS == 0 {
                    ui.end_row();
                }
            }
        });
    });
    let Some(i) = hit else { return false };
    let was_selected = cards[i].selected;
    if !multi {
        cards.iter_mut().for_each(|c| c.selected = false);
    }
    cards[i].selected = !multi || !was_selected;
    true
}

/// The largest rect of the image's aspect ratio that fits in `bounds`, centered.
pub fn fit_inside(bounds: Rect, image: Vec2) -> Rect {
    let scale = (bounds.width() / image.x).min(bounds.height() / image.y);
    Rect::from_center_size(bounds.center(), image * scale)
}

/// An entry of the "add image" menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    Files,
    Clipboard,
    Library,
    /// The newest capture in the library.
    Latest,
}

/// The menu that opens from `resp`: files, clipboard, library and, with `latest`, the newest capture.
/// Returns the choice.
pub fn source_menu(ui: &Ui, resp: &Response, latest: bool) -> Option<Pick> {
    let p = Palette::of(ui);
    let frame = egui::Frame::new()
        .fill(p.elevated)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(10.0)
        .inner_margin(Margin::same(4))
        .shadow(p.shadow());
    let style = ui.style().clone();
    let mut picked = None;
    // Up to four 32 px rows, 2 px apart, in a 4 px frame: it fits anywhere, but must not open sideways.
    let (popup, _) = snap_ui::widgets::popup::fitted(egui::Popup::menu(resp), 4.0, 4.0 * 34.0 - 2.0 + 10.0);
    popup
        .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
        .frame(frame)
        .width(MENU_WIDTH)
        .style(move |s: &mut egui::Style| {
            *s = (*style).clone();
            s.spacing.item_spacing.y = 2.0;
        })
        .show(|ui| {
            ui.set_width(MENU_WIDTH - 8.0);
            let entries = [
                (Pick::Files, icon::FOLDER_OPEN, "combine.source.files"),
                (Pick::Clipboard, icon::CLIPBOARD, "combine.source.clipboard"),
                (Pick::Library, icon::IMAGES, "combine.source.library"),
                (Pick::Latest, icon::CLOCK_COUNTER_CLOCKWISE, "combine.source.latest"),
            ];
            for (pick, glyph, label) in entries.into_iter().filter(|(pick, ..)| latest || *pick != Pick::Latest) {
                if menu_item(ui, glyph, &t(label)).clicked() {
                    picked = Some(pick);
                }
            }
        });
    picked
}

fn menu_item(ui: &mut Ui, glyph: &str, label: &str) -> Response {
    let p = Palette::of(ui);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::MenuItem, ui.is_enabled(), label));
    if ui.is_rect_visible(rect) {
        let hover = util::hover_anim(ui, &resp);
        ui.painter().rect_filled(rect, 6.0, p.hover().gamma_multiply(hover));
        util::paint_icon(ui, egui::pos2(rect.left() + 18.0, rect.center().y), glyph, 16.0, util::lerp_color(p.text_dim, p.text, hover));
        let galley = util::layout(ui, label, &text::body_style(), p.text);
        ui.painter().galley(util::snap(ui, egui::pos2(rect.left() + 36.0, rect.center().y - galley.size().y / 2.0)), galley, p.text);
    }
    resp
}

/// A finished export from a worker; hand it to [`exported`].
pub enum Exported {
    Copied(Result<()>),
    /// `None` when the user dismissed the dialog.
    Saved(Result<Option<PathBuf>>),
}

/// `scale` is the image's pixels per point ([`combined_scale`] of its sources).
pub fn copy_image(cx: &ViewCx, image: Arc<RgbaImage>, scale: f32) {
    cx.spawn(move || Exported::Copied(snap_platform::clipboard::set_image_scaled(&image, scale)));
}

/// Ask where to save (starting in the configured folder with the configured name), then encode by
/// the chosen file's extension. `scale` is as for [`copy_image`].
pub fn save_image(cx: &ViewCx, image: Arc<RgbaImage>, scale: f32) {
    let capture = &cx.settings.capture;
    let dir = snap_core::paths::save_dir(&capture.save_dir);
    let name = snap_core::naming::expand(&capture.filename_template, &snap_core::naming::NameContext::now());
    let (jpeg, webp, optimize) = (capture.jpeg_quality, capture.webp_quality, capture.optimize_png);
    cx.spawn(move || {
        let dialog = rfd::FileDialog::new()
            .set_directory(dir)
            .set_file_name(format!("{name}.png"))
            .add_filter("PNG", &["png"])
            .add_filter("JPEG", &["jpg", "jpeg"])
            .add_filter("WebP", &["webp"]);
        Exported::Saved(dialog.save_file().map_or(Ok(None), |path| {
            let format = snap_image::encode::format_from_path(&path).unwrap_or(ImageFormat::Png);
            let quality = match format {
                ImageFormat::Jpeg => jpeg,
                ImageFormat::Webp => webp,
                ImageFormat::Png => 100,
            };
            snap_image::encode::save(&image, &path, format, quality, optimize, Some(scale)).map(|_| Some(path))
        }))
    });
}

/// Toast for a finished export: "Copied", "Saved <name>" with a Show in folder button, or the error.
pub fn exported(cx: &mut ViewCx, done: Exported) {
    match done {
        Exported::Copied(Ok(())) => cx.toast(ToastKind::Success, t("common.copied")),
        Exported::Saved(Ok(Some(path))) => {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            cx.toast_action(
                ToastKind::Success,
                tf("combine.saved", &[("name", &name)]),
                t("app.notice.show_in_folder"),
                Command::Reveal(path),
            );
        }
        Exported::Saved(Ok(None)) => {}
        Exported::Copied(Err(e)) | Exported::Saved(Err(e)) => cx.toast_error(&e),
    }
}

/// `1234567` -> `1,234,567`; the separator comes from the caller's locale.
pub fn group_digits(n: u64, sep: &str) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 * sep.len());
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push_str(sep);
        }
        out.push(c);
    }
    out
}
