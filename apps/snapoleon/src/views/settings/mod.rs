//! The settings window: a sidebar of pages, a search box that filters rows across all pages, and the pages
//! themselves. Changes apply immediately: every page edits a draft copy of the settings and the window hands
//! the changed sections to the app a moment after the last edit (at once for anything but typing).

use std::{
    any::Any,
    time::{Duration, Instant},
};

use egui::{Align, Context, Frame, Key, Layout, Margin, Modifiers, ScrollArea, Ui, vec2};
use snap_core::{Settings, policy::Policy, settings::BeautifyPreset, t, tf};
use snap_ui::{
    icon, palette, space, text,
    widgets::{empty_state, nav_item, search_field, util},
};

use super::{Chrome, OpenCx, View, ViewCx, window_spec};
use crate::commands::Command;

mod advanced;
mod capture;
mod destinations;
mod general;
mod history;
pub(crate) mod hotkeys;
mod kit;
mod links;
mod pins;
mod pipelines;
mod privacy;
mod recording;
mod text_ai;

pub use kit::ensure_save_dir;
use kit::{Env, Msg, Pick, Probe, Rows};

pub const TAG: &str = "settings";

/// How long typing must pause before a text edit is saved.
const TYPING_PAUSE: Duration = Duration::from_millis(450);
/// The widest the page column grows on a big window.
const COLUMN_WIDTH: f32 = 720.0;

/// A settings page. Deep links (`snapoleon settings`, the hotkey conflict toast, the tray) name one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingsPage {
    General,
    Capture,
    Hotkeys,
    Pipelines,
    Destinations,
    Links,
    Recording,
    TextAi,
    Privacy,
    History,
    Pins,
    Advanced,
}

impl SettingsPage {
    pub const ALL: [SettingsPage; 12] = [
        SettingsPage::General,
        SettingsPage::Capture,
        SettingsPage::Hotkeys,
        SettingsPage::Pipelines,
        SettingsPage::Destinations,
        SettingsPage::Links,
        SettingsPage::Recording,
        SettingsPage::TextAi,
        SettingsPage::Privacy,
        SettingsPage::History,
        SettingsPage::Pins,
        SettingsPage::Advanced,
    ];

    /// Stable key for the dev harness (`SNAPOLEON_DEV_PAGE`) and deep links.
    pub fn key(self) -> &'static str {
        match self {
            SettingsPage::General => "general",
            SettingsPage::Capture => "capture",
            SettingsPage::Hotkeys => "hotkeys",
            SettingsPage::Pipelines => "pipelines",
            SettingsPage::Destinations => "destinations",
            SettingsPage::Links => "links",
            SettingsPage::Recording => "recording",
            SettingsPage::TextAi => "text_ai",
            SettingsPage::Privacy => "privacy",
            SettingsPage::History => "history",
            SettingsPage::Pins => "pins",
            SettingsPage::Advanced => "advanced",
        }
    }

    pub fn from_key(key: &str) -> Option<SettingsPage> {
        SettingsPage::ALL.into_iter().find(|p| p.key() == key)
    }

    fn icon(self) -> &'static str {
        match self {
            SettingsPage::General => icon::GEAR,
            SettingsPage::Capture => icon::CAMERA,
            SettingsPage::Hotkeys => icon::KEYBOARD,
            SettingsPage::Pipelines => icon::FLOW_ARROW,
            SettingsPage::Destinations => icon::CLOUD_ARROW_UP,
            SettingsPage::Links => icon::LINK,
            SettingsPage::Recording => icon::VIDEO_CAMERA,
            SettingsPage::TextAi => icon::TEXT_AA,
            SettingsPage::Privacy => icon::SHIELD_CHECK,
            SettingsPage::History => icon::CLOCK_COUNTER_CLOCKWISE,
            SettingsPage::Pins => icon::PUSH_PIN,
            SettingsPage::Advanced => icon::WRENCH,
        }
    }

    fn title(self) -> String {
        t(&format!("settings.page.{}.title", self.key())).into_owned()
    }

    fn subtitle(self) -> String {
        t(&format!("settings.page.{}.desc", self.key())).into_owned()
    }
}

/// The localised name of a pipeline step.
/// A pipeline's name in the current language.
pub fn pipeline_label(pipeline: &snap_core::pipeline::Pipeline) -> String {
    pipelines::display_name(pipeline)
}

pub fn step_label(step: &snap_core::pipeline::Step) -> String {
    pipelines::step_title(step)
}

/// Opens the window, or brings it forward and switches to `page` when it is already open.
/// An existing window receives the page as `on_msg(Box<SettingsPage>)`.
pub fn open(cx: &mut OpenCx, page: Option<SettingsPage>) {
    if let Some(&win) = cx.shell.windows_with_tag(TAG).first() {
        cx.focus_tagged(TAG);
        if let Some(page) = page {
            cx.post(win, page);
        }
        return;
    }
    let view = SettingsView::new(cx.settings, page.unwrap_or(SettingsPage::General));
    // 600 tall keeps the whole sidebar in view.
    cx.open(window_spec(t("settings.title"), TAG, (920.0, 680.0), (720.0, 600.0)), view);
}

/// Per-page state that must survive between frames.
#[derive(Default)]
struct Pages {
    general: general::State,
    hotkeys: hotkeys::State,
    pipelines: pipelines::State,
    destinations: destinations::State,
    links: links::State,
    text_ai: text_ai::State,
    history: history::State,
    advanced: advanced::State,
}

impl Pages {
    /// Leave every drill-in view: a pipeline's steps, a destination's editor, the destination picker.
    fn reset_drill(&mut self) {
        self.pipelines.reset();
        self.destinations.reset();
    }

    /// The drill-in view `page` shows, if any.
    fn drill(&self, page: SettingsPage) -> Option<String> {
        match page {
            SettingsPage::Pipelines => self.pipelines.view_key(),
            SettingsPage::Destinations => self.destinations.view_key(),
            _ => None,
        }
    }
}

struct SettingsView {
    page: SettingsPage,
    search: String,
    search_edit: Option<egui::Id>,
    /// What the pages edit.
    draft: Settings,
    /// The last settings snapshot the app handed us.
    seen: Settings,
    /// What the app has been told so far.
    committed: Settings,
    /// The draft as of the previous frame, to notice edits.
    previous: Settings,
    last_edit: Instant,
    env: Env,
    probed: bool,
    pages: Pages,
    /// Start the content at its top on the next frame.
    scroll_top: bool,
    /// The drill-in view shown last frame, to notice when one opens or closes.
    drill: Option<String>,
    /// A text field had the keyboard last frame (egui has already taken it away when Escape arrives).
    typing: bool,
}

impl SettingsView {
    fn new(settings: &Settings, page: SettingsPage) -> Self {
        Self {
            page,
            search: String::new(),
            search_edit: None,
            draft: settings.clone(),
            seen: settings.clone(),
            committed: settings.clone(),
            previous: settings.clone(),
            last_edit: Instant::now(),
            env: Env { policy: Policy::load(), probe: None, portable: snap_core::paths::is_portable() },
            probed: false,
            pages: Pages::default(),
            scroll_top: false,
            drill: None,
            typing: false,
        }
    }

    /// Show `page` from its top. As in the macOS System Settings sidebar, choosing a page, also the one already on
    /// screen, leaves whatever sub-page it was in.
    fn go(&mut self, page: SettingsPage) {
        self.pages.reset_drill();
        self.page = page;
        self.search.clear();
        self.scroll_top = true;
    }

    /// Escape (outside a text field or an open list) and Cmd+[ step back out of a drill-in view.
    fn keyboard_back(&mut self, ctx: &Context) {
        if self.pages.drill(self.page).is_none() || !self.search.trim().is_empty() {
            return;
        }
        let back = ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::OpenBracket))
            || (!self.typing && !egui::Popup::is_any_open(ctx) && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)));
        if back {
            self.pages.reset_drill();
            self.scroll_top = true;
        }
    }

    /// Adopt the settings the app re-applied (policy, normalisation) when nothing is waiting to be saved.
    fn adopt(&mut self, snapshot: &Settings) {
        if *snapshot == self.seen {
            return;
        }
        self.seen = snapshot.clone();
        if self.draft == self.committed {
            self.draft = snapshot.clone();
            self.committed = snapshot.clone();
            self.previous = snapshot.clone();
        }
    }

    /// Start the slow probes (devices, encoders, OCR engines) on a worker thread.
    fn probe(&mut self, cx: &mut ViewCx) {
        self.probed = true;
        let ocr = self.draft.ocr.clone();
        let history = cx.engine.history();
        cx.spawn(move || Msg::Probed(Probe::gather(&ocr, history.and_then(|h| h.count().ok()))));
    }

    /// Hand edits to the app. Typing is saved after a pause, everything else straight away.
    fn commit(&mut self, cx: &mut ViewCx, ctx: &Context) {
        if self.draft == self.committed {
            return;
        }
        if self.draft != self.previous {
            self.previous = self.draft.clone();
            self.last_edit = Instant::now();
        }
        let quiet = self.last_edit.elapsed();
        if ctx.egui_wants_keyboard_input() && quiet < TYPING_PAUSE {
            ctx.request_repaint_after(TYPING_PAUSE - quiet);
            return;
        }
        self.flush(cx);
    }

    /// Send every edit not yet sent.
    fn flush(&mut self, cx: &mut ViewCx) {
        if self.draft == self.committed {
            return;
        }
        let changed = changed_sections(&self.committed, &self.draft);
        let draft = self.draft.clone();
        cx.update_settings(move |s| copy_sections(&changed, &draft, s));
        self.committed = self.draft.clone();
    }

    fn apply_pick(&mut self, target: Pick, path: Option<std::path::PathBuf>) {
        let Some(path) = path else { return };
        let value = path.display().to_string();
        match target {
            Pick::SaveDir => self.draft.capture.save_dir = value,
            Pick::FfmpegPath => self.draft.recording.ffmpeg_path = value,
            Pick::TesseractPath => {
                self.draft.ocr.tesseract_path = value;
                self.pages.text_ai.reprobe = true;
            }
            Pick::StepDir { pipeline, step } => {
                if let Some(snap_core::pipeline::Step::Save { dir, .. }) =
                    self.draft.pipelines.iter_mut().find(|p| p.id == pipeline).and_then(|p| p.steps.get_mut(step))
                {
                    *dir = Some(value);
                }
            }
            Pick::BackgroundImage { preset } => {
                if let Some(BeautifyPreset { background, .. }) = self.draft.beautify_presets.get_mut(preset) {
                    *background = snap_core::settings::Background::Image { path: value };
                }
            }
            Pick::LocalDir { .. } => {}
        }
    }

    fn nav(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = space::XS;
        let resp = search_field(ui, &mut self.search, &t("settings.search"));
        self.search_edit = Some(resp.id.with("edit"));
        ui.add_space(space::SM);
        let searching = !self.search.trim().is_empty();
        let current = self.page;
        let out = ScrollArea::vertical().id_salt("settings-nav").auto_shrink([false, false]).show(ui, |ui| {
            let mut picked = None;
            for page in SettingsPage::ALL {
                if nav_item(ui, page.icon(), &page.title(), !searching && current == page).clicked() {
                    picked = Some(page);
                }
            }
            picked
        });
        if let Some(page) = out.inner {
            self.go(page);
        }
        // In a short window the list scrolls; fade the cut edges so it does not look like it simply ends.
        let (rect, offset) = (out.inner_rect, out.state.clamped_offset().y);
        let surface = palette(ui.ctx()).surface;
        if offset > 1.0 {
            util::scroll_fade(ui.painter(), rect, surface, false);
        }
        if out.content_size.y - offset - rect.height() > 1.0 {
            util::scroll_fade(ui.painter(), rect, surface, true);
        }
    }

    fn content(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        let drill = self.pages.drill(self.page);
        if drill != self.drill {
            // A sub-page opened or closed: show it from its top, not at the offset the list had.
            self.drill = drill.clone();
            self.scroll_top = true;
        }
        let searching = !self.search.trim().is_empty();
        // Each page and sub-page keeps its own scroll position.
        let key = if searching { "search" } else { self.page.key() };
        let mut area = ScrollArea::vertical().id_salt(("settings-content", key, drill)).auto_shrink([false, false]);
        if std::mem::take(&mut self.scroll_top) {
            area = area.vertical_scroll_offset(0.0);
        }
        let Self { page, search, draft, env, pages, .. } = self;
        area.show(ui, |ui| {
            let width = (ui.available_width() - 2.0 * space::XL).clamp(280.0, COLUMN_WIDTH);
            ui.vertical_centered(|ui| {
                ui.add_space(space::XL);
                ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::top_down(Align::Min), |ui| {
                    ui.spacing_mut().item_spacing = vec2(space::SM, space::SM);
                    let query = search.trim().to_owned();
                    let mut rc = Rows::new(&query, &*env);
                    if rc.searching() {
                        search_results(ui, &mut rc, pages, cx, draft, &query);
                    } else {
                        kit::page_header(ui, &page.title(), &page.subtitle());
                        show_page(ui, &mut rc, *page, pages, cx, draft);
                    }
                });
                ui.add_space(space::XXL);
            });
        });
    }
}

fn show_page(ui: &mut Ui, rc: &mut Rows, page: SettingsPage, pages: &mut Pages, cx: &mut ViewCx, draft: &mut Settings) {
    match page {
        SettingsPage::General => general::show(ui, rc, &mut pages.general, cx, draft),
        SettingsPage::Capture => capture::show(ui, rc, cx, draft),
        SettingsPage::Hotkeys => hotkeys::show(ui, rc, &mut pages.hotkeys, cx, draft),
        SettingsPage::Pipelines => pipelines::show(ui, rc, &mut pages.pipelines, cx, draft),
        SettingsPage::Destinations => destinations::show(ui, rc, &mut pages.destinations, cx, draft),
        SettingsPage::Links => links::show(ui, rc, &mut pages.links, cx, &mut draft.links),
        SettingsPage::Recording => recording::show(ui, rc, cx, &mut draft.recording),
        SettingsPage::TextAi => text_ai::show(ui, rc, &mut pages.text_ai, cx, draft),
        SettingsPage::Privacy => privacy::show(ui, rc, cx, draft),
        SettingsPage::History => history::show(ui, rc, &mut pages.history, cx, &mut draft.history),
        SettingsPage::Pins => pins::show(ui, rc, &mut draft.pins),
        SettingsPage::Advanced => advanced::show(ui, rc, &mut pages.advanced, cx, draft),
    }
}

/// Every page, but only the rows that match.
fn search_results(ui: &mut Ui, rc: &mut Rows, pages: &mut Pages, cx: &mut ViewCx, draft: &mut Settings, query: &str) {
    ui.label(text::headline(tf("settings.search_results", &[("query", query)])));
    ui.add_space(space::XL);
    for page in SettingsPage::ALL {
        rc.prefix = page.title();
        show_page(ui, rc, page, pages, cx, draft);
    }
    if rc.rendered() == 0 {
        ui.add_space(space::XXL);
        empty_state(ui, icon::MAGNIFYING_GLASS, &t("settings.search_empty.title"), &t("settings.search_empty.body"));
    }
}

/// Names of the top-level sections in which `a` and `b` differ.
fn changed_sections(a: &Settings, b: &Settings) -> Vec<&'static str> {
    let mut out = Vec::new();
    macro_rules! diff {
        ($($field:ident),* $(,)?) => { $(if a.$field != b.$field { out.push(stringify!($field)); })* };
    }
    diff!(
        general,
        capture,
        editor,
        hotkeys,
        pipelines,
        default_pipeline,
        destinations,
        default_destination,
        links,
        recording,
        history,
        privacy,
        ocr,
        translate,
        ai,
        pins,
        beautify_presets
    );
    out
}

/// Copy the named sections from `from` into `to`, leaving everything else (the remembered capture region,
/// edits made by other windows) as the app has it.
fn copy_sections(names: &[&str], from: &Settings, to: &mut Settings) {
    macro_rules! copy {
        ($($field:ident),* $(,)?) => { for name in names { $(if *name == stringify!($field) { to.$field = from.$field.clone(); })* } };
    }
    copy!(
        general,
        capture,
        editor,
        hotkeys,
        pipelines,
        default_pipeline,
        destinations,
        default_destination,
        links,
        recording,
        history,
        privacy,
        ocr,
        translate,
        ai,
        pins,
        beautify_presets
    );
    // `last_region` is written by the overlay, not by this window.
    if names.contains(&"capture") {
        to.capture.last_region = from.capture.last_region;
    }
}

impl View for SettingsView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        self.adopt(cx.settings);
        if !self.probed {
            self.probe(cx);
        }
        if std::mem::take(&mut self.pages.text_ai.reprobe) {
            self.env.probe = None;
            self.probe(cx);
        }
        if let Some(edit) = self.search_edit
            && ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::F))
        {
            ctx.memory_mut(|m| m.request_focus(edit));
        }
        self.keyboard_back(ctx);

        let title = t("settings.title");
        Chrome::new(&title).show(cx, ctx, |ui, cx| {
            let p = palette(ui.ctx());
            let sidebar = Frame::new().fill(p.surface).inner_margin(Margin::same(12));
            egui::Panel::left("settings-nav").exact_size(200.0).resizable(false).frame(sidebar).show(ui, |ui| self.nav(ui));
            egui::CentralPanel::default().frame(Frame::new().fill(p.bg)).show(ui, |ui| self.content(ui, cx));
        });

        self.commit(cx, ctx);
        self.typing = ctx.text_edit_focused();
    }

    fn closed(&mut self, cx: &mut ViewCx) {
        self.flush(cx);
        if self.pages.hotkeys.recording() {
            cx.push(Command::PauseHotkeys(false));
        }
    }

    fn on_msg(&mut self, cx: &mut ViewCx, msg: Box<dyn Any + Send>) {
        let msg = match msg.downcast::<SettingsPage>() {
            Ok(page) => {
                self.go(*page);
                return;
            }
            Err(msg) => msg,
        };
        let Ok(msg) = msg.downcast::<Msg>() else { return };
        let msg = *msg;
        let Some(msg) = self.pages.destinations.on_msg(cx, &mut self.draft, msg) else { return };
        let Some(msg) = self.pages.text_ai.on_msg(msg) else { return };
        let Some(msg) = self.pages.history.on_msg(cx, msg) else { return };
        let Some(msg) = self.pages.advanced.on_msg(cx, &mut self.draft, msg) else { return };
        match msg {
            Msg::Probed(probe) => {
                self.env.probe = Some(probe);
            }
            Msg::Picked { target, path } => self.apply_pick(target, path),
            _ => {}
        }
    }
}
