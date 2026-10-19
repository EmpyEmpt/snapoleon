//! The Library: every capture in history as a searchable, filterable grid with a detail drawer.
//!
//! History queries, thumbnail decoding and every file or database action run on worker threads and come back
//! through [`Msg`]; the UI thread only paints. The grid is virtualized (only visible cards are laid out) and
//! history is paged in 200 rows at a time, so a 10 000 entry library scrolls like a small one.

use std::{
    any::Any,
    collections::HashSet,
    time::{Duration, Instant},
};

use egui::{ColorImage, Context, Key, Modifiers, Rect, TextureHandle, TextureOptions, Ui, UiBuilder, Vec2};
use snap_core::{Error, t};
use snap_history::Entry;
use snap_ui::widgets::{chip, search_field, util};

use super::{Chrome, OpenCx, View, ViewCx, window_spec};
use crate::commands::Command;

mod actions;
mod card;
mod drawer;
mod format;
mod grid;
pub(crate) mod menu;
pub(crate) mod paint;
mod query;
mod thumbs;

use query::{Filter, KindFilter};
use thumbs::Thumbs;

pub const TAG: &str = "library";

/// How often a visible window checks for captures written by another process (the headless CLI).
/// In-app changes arrive at once as `events::HistoryChanged`.
const POLL: Duration = Duration::from_secs(15);
/// Typing pauses this long before a search runs.
const DEBOUNCE: Duration = Duration::from_millis(250);
/// Longest side of the drawer preview, in pixels.
const PREVIEW_EDGE: u32 = 1600;

pub fn open(cx: &mut OpenCx) {
    if cx.focus_tagged(TAG) {
        return;
    }
    cx.open(window_spec(t("library.title"), TAG, (1100.0, 720.0), (760.0, 520.0)), LibraryView::new());
}

/// Things you can do to one or more captures; the card, the drawer, menus and the keyboard all speak this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Act {
    Edit,
    Copy,
    Pin,
    Favorite,
    Upload,
    Reveal,
    CopyFile,
    CopyPath,
    Recognize,
    /// Ask for confirmation, then delete (the confirmation lives in the drawer or the selection bar).
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Load {
    /// A new query: replaces the list and scrolls to the top.
    Reset,
    /// The same query again, keeping as many rows as are loaded (new captures landed).
    Refresh,
    /// The next page of the current query.
    More,
}

/// What worker threads report back.
enum Msg {
    Page {
        generation: u64,
        mode: Load,
        result: Result<query::Page, Error>,
    },
    Apps(Vec<(String, u32)>),
    Poll {
        count: u64,
        newest: Option<String>,
    },
    Preview {
        id: String,
        image: Option<ColorImage>,
        file_ok: bool,
    },
    /// Entries re-read from history after a change made elsewhere (an upload added a link).
    Entries(Vec<Entry>),
    Done(Result<actions::Outcome, Error>),
    Uploaded {
        ids: Vec<String>,
        result: Result<actions::Uploaded, Error>,
    },
    Recognized {
        id: String,
        result: Result<String, Error>,
    },
    /// A database write finished; the screen already shows its result.
    Written,
    /// How far the running upload is, 0 to 1.
    UploadProgress(f32),
    /// A database write failed; the optimistic change on screen is reloaded.
    WriteFailed(Error),
}

struct Preview {
    id: String,
    tex: Option<TextureHandle>,
    file_ok: bool,
}

#[derive(Clone, Copy)]
enum MenuKind {
    /// The actions of one capture.
    Entry,
    /// "Copy link as" for the link at this index of the selected capture.
    Link(usize),
}

struct OpenMenu {
    kind: MenuKind,
    /// The capture the menu belongs to.
    id: String,
    anchor: menu::Anchor,
}

pub struct LibraryView {
    started: bool,
    entries: Vec<Entry>,
    /// Database row where the next page starts.
    next_offset: u32,
    more: bool,
    loading: bool,
    generation: u64,

    filter: Filter,
    search: String,
    search_edited: Option<Instant>,
    applied_search: String,
    focus_search: bool,
    apps: Vec<(String, u32)>,

    /// (rows, newest id) as of the last poll; a change means new captures landed.
    signature: Option<(u64, Option<String>)>,
    last_poll: Instant,
    polling: bool,

    selected: HashSet<String>,
    /// Keyboard cursor and range-select anchor, as capture ids.
    cursor: Option<String>,
    anchor: Option<String>,
    scroll_to: Option<usize>,
    scroll_top: bool,
    /// Grid shape from the last frame, for keyboard navigation.
    cols: usize,
    visible_rows: usize,

    thumbs: Thumbs,
    decoded: Vec<thumbs::Loaded>,
    preview: Option<Preview>,
    preview_requested: Option<String>,
    decoded_preview: Option<(String, Option<ColorImage>, bool)>,

    drawer: drawer::State,
    /// The capture the drawer last showed, so it can slide out with its content intact.
    drawer_entry: Option<Entry>,
    quick_look: bool,
    bulk_confirm: bool,
    menu: Option<OpenMenu>,
    recognizing: HashSet<String>,
    uploading: HashSet<String>,
    /// The toast that shows how far the running upload is.
    upload_toast: Option<u64>,
}

impl LibraryView {
    fn new() -> Self {
        Self {
            started: false,
            entries: Vec::new(),
            next_offset: 0,
            more: false,
            loading: true,
            generation: 0,
            filter: Filter::default(),
            search: String::new(),
            search_edited: None,
            applied_search: String::new(),
            focus_search: false,
            apps: Vec::new(),
            signature: None,
            last_poll: Instant::now(),
            polling: false,
            selected: HashSet::new(),
            cursor: None,
            anchor: None,
            scroll_to: None,
            scroll_top: false,
            cols: 1,
            visible_rows: 3,
            thumbs: Thumbs::new(),
            decoded: Vec::new(),
            preview: None,
            preview_requested: None,
            decoded_preview: None,
            drawer: drawer::State::default(),
            drawer_entry: None,
            quick_look: false,
            bulk_confirm: false,
            menu: None,
            recognizing: HashSet::new(),
            uploading: HashSet::new(),
            upload_toast: None,
        }
    }

    /// The one selected capture, whose details the drawer shows.
    fn subject(&self) -> Option<&Entry> {
        let mut ids = self.selected.iter();
        let (id, None) = (ids.next()?, ids.next()) else { return None };
        self.entries.iter().find(|e| e.id.0 == *id)
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.id.0 == id)
    }

    fn selected_entries(&self) -> Vec<Entry> {
        self.entries.iter().filter(|e| self.selected.contains(&e.id.0)).cloned().collect()
    }

    // --- loading -----------------------------------------------------------------------------------------------

    fn load(&mut self, cx: &mut ViewCx, mode: Load) {
        let Some(history) = cx.engine.history() else {
            self.loading = false;
            return;
        };
        let (offset, want) = match mode {
            Load::Reset => (0, query::PAGE as usize),
            Load::Refresh => (0, self.entries.len().clamp(query::PAGE as usize, 2000)),
            Load::More => (self.next_offset, query::PAGE as usize),
        };
        if mode != Load::More {
            self.generation += 1;
        }
        self.loading = true;
        let (generation, filter, text) = (self.generation, self.filter.clone(), self.applied_search.clone());
        cx.spawn(move || Msg::Page { generation, mode, result: query::fetch(&history, &filter, &text, offset, want) });
    }

    fn fetch_apps(&self, cx: &ViewCx) {
        if let Some(history) = cx.engine.history() {
            cx.spawn(move || Msg::Apps(history.apps().unwrap_or_default()));
        }
    }

    fn poll(&mut self, cx: &ViewCx) {
        let Some(history) = cx.engine.history() else { return };
        self.polling = true;
        self.last_poll = Instant::now();
        cx.spawn(move || {
            let count = history.count().unwrap_or(0);
            let newest = history
                .search(&snap_history::Query { limit: 1, ..Default::default() })
                .ok()
                .and_then(|rows| rows.first().map(|e| e.id.0.clone()));
            Msg::Poll { count, newest }
        });
    }

    fn page_arrived(&mut self, cx: &mut ViewCx, generation: u64, mode: Load, result: Result<query::Page, Error>) {
        if generation != self.generation {
            return;
        }
        self.loading = false;
        let page = match result {
            Ok(page) => page,
            Err(e) => {
                // Without this the grid would ask for the next page again on every frame.
                self.more = false;
                return cx.toast_error(&e);
            }
        };
        self.next_offset = page.consumed;
        self.more = page.more;
        match mode {
            Load::More => {
                let known: HashSet<String> = self.entries.iter().map(|e| e.id.0.clone()).collect();
                self.entries.extend(page.entries.into_iter().filter(|e| !known.contains(&e.id.0)));
            }
            Load::Reset | Load::Refresh => {
                self.entries = page.entries;
                let alive: HashSet<&str> = self.entries.iter().map(|e| e.id.0.as_str()).collect();
                self.selected.retain(|id| alive.contains(id.as_str()));
                if self.cursor.as_deref().is_some_and(|c| !alive.contains(c)) {
                    self.cursor = None;
                }
                self.scroll_top = mode == Load::Reset;
            }
        }
    }

    // --- selection ---------------------------------------------------------------------------------------------

    fn select_only(&mut self, id: &str) {
        self.selected.clear();
        self.selected.insert(id.to_owned());
        self.cursor = Some(id.to_owned());
        self.anchor = Some(id.to_owned());
        self.drawer.confirm_delete = false;
        self.bulk_confirm = false;
    }

    fn clear_selection(&mut self) {
        self.selected.clear();
        self.cursor = None;
        self.anchor = None;
        self.quick_look = false;
        self.drawer.confirm_delete = false;
        self.bulk_confirm = false;
    }

    fn click(&mut self, id: &str, mods: Modifiers) {
        self.drawer.confirm_delete = false;
        self.bulk_confirm = false;
        if mods.shift
            && let (Some(a), Some(b)) = (self.anchor.as_deref().and_then(|a| self.index_of(a)), self.index_of(id))
        {
            if !mods.command {
                self.selected.clear();
            }
            let (lo, hi) = (a.min(b), a.max(b));
            self.selected.extend(self.entries[lo..=hi].iter().map(|e| e.id.0.clone()));
            self.cursor = Some(id.to_owned());
            return;
        }
        if mods.command {
            if !self.selected.remove(id) {
                self.selected.insert(id.to_owned());
            }
            self.cursor = Some(id.to_owned());
            self.anchor = Some(id.to_owned());
        } else {
            self.select_only(id);
        }
    }

    /// Move the keyboard cursor to entry `index`, selecting it (or extending the selection with Shift).
    fn move_cursor(&mut self, index: usize, extend: bool) {
        let Some(id) = self.entries.get(index).map(|e| e.id.0.clone()) else { return };
        if extend {
            self.click(&id, Modifiers::SHIFT);
        } else {
            self.select_only(&id);
        }
        self.cursor = Some(id);
        self.scroll_to = Some(index);
    }

    fn keyboard(&mut self, cx: &mut ViewCx, ctx: &Context) {
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::F)) {
            self.focus_search = true;
            return;
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let pressed = |mods: Modifiers, key: Key| ctx.input_mut(|i| i.consume_key(mods, key));
        let n = self.entries.len();
        let cols = self.cols.max(1);
        let current = self.cursor.as_deref().and_then(|c| self.index_of(c));

        let arrows = [(Key::ArrowLeft, -1_isize), (Key::ArrowRight, 1), (Key::ArrowUp, -(cols as isize)), (Key::ArrowDown, cols as isize)];
        for (key, step) in arrows {
            for extend in [true, false] {
                if pressed(if extend { Modifiers::SHIFT } else { Modifiers::NONE }, key) && n > 0 {
                    let target = match current {
                        None => 0,
                        Some(cur) => {
                            let next = cur as isize + step;
                            // Down from a short last row lands on the last card instead of doing nothing.
                            if next >= n as isize && step as usize == cols && cur / cols < (n - 1) / cols {
                                n - 1
                            } else {
                                next.clamp(0, n as isize - 1) as usize
                            }
                        }
                    };
                    self.move_cursor(target, extend);
                }
            }
        }
        let page = cols * self.visible_rows.max(1);
        if n > 0 {
            if pressed(Modifiers::NONE, Key::Home) {
                self.move_cursor(0, false);
            }
            if pressed(Modifiers::NONE, Key::End) {
                self.move_cursor(n - 1, false);
            }
            if pressed(Modifiers::NONE, Key::PageDown) {
                self.move_cursor((current.unwrap_or(0) + page).min(n - 1), false);
            }
            if pressed(Modifiers::NONE, Key::PageUp) {
                self.move_cursor(current.unwrap_or(0).saturating_sub(page), false);
            }
        }
        if pressed(Modifiers::COMMAND, Key::A) {
            self.selected = self.entries.iter().map(|e| e.id.0.clone()).collect();
        }
        if self.selected.is_empty() {
            return;
        }
        let confirming = self.drawer.confirm_delete || self.bulk_confirm;
        if pressed(Modifiers::NONE, Key::Enter) {
            if confirming {
                self.delete_selected(cx);
            } else if let Some(entry) = self.cursor.as_deref().and_then(|c| self.index_of(c)).map(|i| self.entries[i].clone()) {
                self.run(cx, Act::Edit, vec![entry]);
            }
        }
        if pressed(Modifiers::NONE, Key::Delete) || pressed(Modifiers::NONE, Key::Backspace) {
            if self.selected.len() == 1 {
                self.drawer.confirm_delete = true;
            } else {
                self.bulk_confirm = true;
            }
        }
        if pressed(Modifiers::NONE, Key::Space) && self.selected.len() == 1 {
            self.quick_look = !self.quick_look;
        }
        if self.menu.is_none() && pressed(Modifiers::NONE, Key::Escape) {
            if self.quick_look {
                self.quick_look = false;
            } else if confirming {
                self.drawer.confirm_delete = false;
                self.bulk_confirm = false;
            } else {
                self.clear_selection();
            }
        }
        let targets = self.selected_entries();
        for (mods, key, act) in [
            (Modifiers::COMMAND, Key::C, Act::Copy),
            (Modifiers::NONE, Key::E, Act::Edit),
            (Modifiers::NONE, Key::P, Act::Pin),
            (Modifiers::NONE, Key::F, Act::Favorite),
            (Modifiers::NONE, Key::U, Act::Upload),
        ] {
            if pressed(mods, key) {
                self.run(cx, act, targets.clone());
            }
        }
    }

    // --- per-frame ---------------------------------------------------------------------------------------------

    fn begin_frame(&mut self, cx: &mut ViewCx, ctx: &Context) {
        if !self.started {
            self.started = true;
            self.thumbs.start(&cx.handle());
            self.load(cx, Load::Reset);
            self.fetch_apps(cx);
        }
        self.thumbs.begin_frame();
        for loaded in std::mem::take(&mut self.decoded) {
            self.thumbs.loaded(ctx, loaded);
        }
        if let Some((id, image, file_ok)) = self.decoded_preview.take() {
            let tex = image.map(|image| ctx.load_texture(format!("library-preview-{id}"), image, TextureOptions::LINEAR));
            self.preview = Some(Preview { id, tex, file_ok });
        }

        if let Some(at) = self.search_edited {
            let left = DEBOUNCE.saturating_sub(at.elapsed());
            if left.is_zero() {
                self.search_edited = None;
                let text = self.search.trim().to_owned();
                if text != self.applied_search {
                    self.applied_search = text;
                    self.clear_selection();
                    self.load(cx, Load::Reset);
                }
            } else {
                ctx.request_repaint_after(left);
            }
        }

        ctx.request_repaint_after(POLL);
        let minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
        if self.last_poll.elapsed() >= POLL && !self.polling && !minimized {
            self.poll(cx);
        }
        self.keyboard(cx, ctx);
        self.sync_preview(cx);
    }

    /// Keep the drawer preview in step with the selected capture.
    fn sync_preview(&mut self, cx: &mut ViewCx) {
        let Some(entry) = self.subject().cloned() else { return };
        if self.preview_requested.as_deref() == Some(entry.id.0.as_str()) {
            return;
        }
        self.preview_requested = Some(entry.id.0.clone());
        self.preview = None;
        let video = entry.kind == snap_core::CaptureKind::Recording;
        cx.spawn(move || {
            let file_ok = entry.file.as_ref().is_some_and(|f| f.exists());
            let image = entry.file.as_deref().filter(|_| file_ok).and_then(|f| actions::load_preview(f, video));
            Msg::Preview { id: entry.id.0, image, file_ok }
        });
    }

    fn filter_bar(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        let mut filter = self.filter.clone();
        egui::Frame::new().inner_margin(egui::Margin::symmetric(grid::PAD as i8, 0)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            // Chips are 26 px; without this each row would be padded to the 32 px control height.
            ui.spacing_mut().interact_size.y = 26.0;
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
                let kinds = [
                    (KindFilter::All, "library.filter.all"),
                    (KindFilter::Screenshots, "library.filter.screenshots"),
                    (KindFilter::Recordings, "library.filter.recordings"),
                    (KindFilter::Favorites, "library.filter.favorites"),
                ];
                for (kind, key) in kinds {
                    if chip(ui, &t(key), filter.kind == kind).clicked() {
                        filter.kind = kind;
                    }
                }
                separator(ui);
                let ranges = [
                    (query::DateRange::Today, "library.filter.today"),
                    (query::DateRange::Week, "library.filter.week"),
                    (query::DateRange::Month, "library.filter.month"),
                ];
                for (range, key) in ranges {
                    let on = filter.range == Some(range);
                    if chip(ui, &t(key), on).clicked() {
                        filter.range = if on { None } else { Some(range) };
                    }
                }
            });
            if !self.apps.is_empty() {
                self.app_chips(ui, &mut filter);
            }
        });
        if filter != self.filter {
            self.filter = filter;
            self.clear_selection();
            self.load(cx, Load::Reset);
        }
    }

    /// The most used apps as chips on one line; the line scrolls sideways and fades out where it continues.
    fn app_chips(&self, ui: &mut Ui, filter: &mut Filter) {
        let p = snap_ui::palette(ui.ctx());
        let out = ui
            .scope(|ui| {
                ui.style_mut().always_scroll_the_only_direction = true;
                egui::ScrollArea::horizontal()
                    .id_salt("library-apps")
                    .auto_shrink([false, true])
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            for (app, count) in self.apps.iter().take(12) {
                                let on = filter.app.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(app));
                                if chip(ui, &format!("{app} · {count}"), on).clicked() {
                                    filter.app = if on { None } else { Some(app.clone()) };
                                }
                            }
                        });
                    })
            })
            .inner;
        let (rect, offset) = (out.inner_rect, out.state.clamped_offset().x);
        let (more_right, more_left) = (out.content_size.x - offset - rect.width() > 1.0, offset > 1.0);
        for (visible, right) in [(more_right, true), (more_left, false)] {
            if visible {
                paint::edge_fade(ui.painter(), rect, p.bg, right);
            }
        }
    }

    fn body(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        let ctx = ui.ctx().clone();
        let full = ui.available_rect_before_wrap();
        let subject = self.subject().cloned();
        if let Some(entry) = &subject {
            self.drawer_entry = Some(entry.clone());
        }
        let open = util::anim_value(&ctx, ui.scope_id().with("library-drawer-open"), f32::from(subject.is_some()), util::Motion::SLIDE);
        let drawer_w = (drawer::WIDTH * open).round();
        if drawer_w < 1.0 && subject.is_none() {
            self.drawer_entry = None;
        }
        let pane_rect = Rect::from_min_max(full.min, egui::pos2(full.max.x - drawer_w, full.max.y));
        ui.allocate_rect(full, egui::Sense::hover());

        let mut pane = ui.new_child(UiBuilder::new().max_rect(pane_rect).layout(egui::Layout::top_down(egui::Align::Min)));
        pane.set_clip_rect(pane_rect);
        pane.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
        pane.add_space(12.0);
        self.filter_bar(&mut pane, cx);
        self.grid(&mut pane, cx);

        if drawer_w >= 1.0
            && let Some(entry) = self.drawer_entry.clone()
        {
            let rect = Rect::from_min_size(egui::pos2(pane_rect.max.x, full.min.y), Vec2::new(drawer::WIDTH, full.height()));
            let mut side = ui.new_child(UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)));
            side.set_clip_rect(rect.intersect(full));
            self.show_drawer(&mut side, cx, entry);
        }
    }

    fn show_drawer(&mut self, ui: &mut Ui, cx: &mut ViewCx, entry: Entry) {
        let preview = self.preview.as_ref().filter(|p| p.id == entry.id.0);
        let file_ok = preview.is_none_or(|p| p.file_ok);
        let loading = preview.is_none();
        let tex = preview.and_then(|p| p.tex.clone()).or_else(|| self.thumbs.get(&entry).cloned());
        let props = drawer::Props {
            entry: &entry,
            preview: tex.as_ref(),
            loading,
            file_ok,
            now_ms: snap_core::now_ms(),
            recognizing: self.recognizing.contains(&entry.id.0),
            uploading: self.uploading.contains(&entry.id.0),
        };
        let out = drawer::show(ui, &mut self.drawer, &props);
        for event in out {
            self.drawer_event(cx, &entry, event);
        }
    }

    fn drawer_event(&mut self, cx: &mut ViewCx, entry: &Entry, event: drawer::Out) {
        use drawer::Out;
        match event {
            Out::Close => self.clear_selection(),
            Out::Act(act) => self.run(cx, act, vec![entry.clone()]),
            Out::QuickLook => self.quick_look = true,
            Out::OpenUrl(url) => cx.push(Command::OpenUrl(url)),
            Out::CopyText(text) => {
                cx.push(Command::CopyText(text));
                cx.toast(snap_ui::ToastKind::Success, t("common.copied"));
            }
            Out::ShowText => {
                let text = entry.text.clone().unwrap_or_default();
                cx.push(Command::ShowText { title: paint::title_of(entry), text });
            }
            Out::LinkMenu(index, rect) => self.toggle_menu(MenuKind::Link(index), &entry.id.0, menu::Anchor::Widget(rect)),
            Out::MoreMenu(rect) => self.toggle_menu(MenuKind::Entry, &entry.id.0, menu::Anchor::Widget(rect)),
            Out::SetTags(tags) => self.set_tags(cx, &entry.id.0, tags),
            Out::Delete => self.delete_selected(cx),
        }
    }

    fn toggle_menu(&mut self, kind: MenuKind, id: &str, anchor: menu::Anchor) {
        let same = self.menu.as_ref().is_some_and(|m| m.id == id && std::mem::discriminant(&m.kind) == std::mem::discriminant(&kind));
        self.menu = if same { None } else { Some(OpenMenu { kind, id: id.to_owned(), anchor }) };
    }
}

fn separator(ui: &mut Ui) {
    let p = snap_ui::palette(ui.ctx());
    ui.add_space(2.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(1.0, 18.0), egui::Sense::hover());
    ui.painter().vline(rect.center().x, rect.y_range(), egui::Stroke::new(1.0, p.border));
    ui.add_space(2.0);
}

impl View for LibraryView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        self.begin_frame(cx, ctx);

        let title = t("library.title");
        let hint = t("library.search.hint");
        let mut search = std::mem::take(&mut self.search);
        let focus_search = std::mem::take(&mut self.focus_search);
        let mut edited = false;
        Chrome::new(&title)
            .center(|ui| {
                let free = ui.available_width();
                let width = free.min(560.0);
                ui.add_space(((free - width) / 2.0).max(0.0));
                ui.allocate_ui(Vec2::new(width, snap_ui::CONTROL_HEIGHT), |ui| {
                    let response = search_field(ui, &mut search, &hint);
                    edited = response.changed();
                    if focus_search {
                        response.request_focus();
                    }
                });
            })
            .show(cx, ctx, |ui, cx| self.body(ui, cx));
        self.search = search;
        if edited {
            self.search_edited = Some(Instant::now());
        }

        self.overlays(cx, ctx);
        self.thumbs.end_frame();
    }

    fn on_msg(&mut self, cx: &mut ViewCx, msg: Box<dyn Any + Send>) {
        if msg.is::<crate::events::HistoryChanged>() {
            // A newer generation supersedes any page still in flight.
            self.load(cx, Load::Refresh);
            self.fetch_apps(cx);
            return;
        }
        let msg = match msg.downcast::<thumbs::Loaded>() {
            Ok(loaded) => {
                self.decoded.push(*loaded);
                return;
            }
            Err(msg) => msg,
        };
        let Ok(msg) = msg.downcast::<Msg>() else { return };
        self.message(cx, *msg);
    }
}
