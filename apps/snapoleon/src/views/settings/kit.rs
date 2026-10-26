//! Building blocks shared by the settings pages: the searchable row helper, localised option tables,
//! path and secret fields, inline confirmation, and the messages worker threads send back.

use std::{hash::Hash, path::PathBuf};

use egui::{
    Align, Color32, Layout, Rect, Response, Sense, Stroke, StrokeKind, Ui, Widget,
    epaint::text::{LayoutJob, TextWrapping},
};
use snap_core::{
    policy::Policy,
    settings::{CustomUploader, OcrSettings},
    t,
};
use snap_ocr::EngineStatus;
use snap_record::EncoderInfo;
use snap_ui::{
    Palette, Tone, icon, radius, space, text,
    widgets::{self, Button, ButtonKind, TextField, badge, divider, dropdown, setting_row, util},
};

use crate::views::ViewCx;

/// What the settings window learned off the UI thread or once at start.
pub struct Env {
    pub policy: Policy,
    pub probe: Option<Probe>,
    pub portable: bool,
}

/// Hardware and software facts that need a slow probe (device enumeration, spawning `ffmpeg` or `tesseract`).
#[derive(Debug, Clone)]
pub struct Probe {
    pub mics: Vec<String>,
    pub encoders: Vec<EncoderInfo>,
    pub ocr: Vec<EngineStatus>,
    pub ocr_native_langs: Vec<String>,
    pub ocr_tesseract_langs: Vec<String>,
    pub ffmpeg: Option<PathBuf>,
    pub history_count: Option<u64>,
}

impl Probe {
    /// Blocking; call from a worker thread.
    pub fn gather(ocr: &OcrSettings, history_count: Option<u64>) -> Probe {
        Probe {
            mics: snap_record::mic_devices(),
            encoders: snap_record::encoders(),
            ocr: snap_ocr::engines_with(ocr),
            ocr_native_langs: snap_ocr::native_languages(),
            ocr_tesseract_langs: snap_ocr::tesseract_languages(ocr),
            ffmpeg: snap_record::ffmpeg_path(),
            history_count,
        }
    }
}

/// A path a native dialog fills in.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    SaveDir,
    FfmpegPath,
    TesseractPath,
    StepDir { pipeline: String, step: usize },
    LocalDir { dest: String },
    BackgroundImage { preset: usize },
}

impl Pick {
    pub fn is_folder(&self) -> bool {
        matches!(self, Pick::SaveDir | Pick::StepDir { .. } | Pick::LocalDir { .. })
    }
}

/// Everything a worker thread started by the settings window can report.
pub enum Msg {
    Picked {
        target: Pick,
        path: Option<PathBuf>,
    },
    Probed(Probe),
    /// The destination test finished.
    Tested {
        id: String,
        result: Result<(), String>,
    },
    /// OAuth sign-in finished; the tokens are already in the secret store.
    SignedIn {
        id: String,
        result: Result<(), String>,
    },
    AiTested(Result<String, String>),
    /// A `.sxcu` file was chosen and parsed.
    SxcuImported(Result<(String, CustomUploader), String>),
    SettingsExported(Result<PathBuf, String>),
    SxcuExported(Result<PathBuf, String>),
    /// A settings file was chosen and parsed.
    SettingsImported(Result<Box<snap_core::Settings>, String>),
    HistoryCleared(Result<usize, String>),
}

/// Label and description of a row, from `settings.<key>.label` / `.desc`.
pub fn texts(key: &str) -> (String, String) {
    let base = format!("settings.{key}");
    let label = t(&format!("{base}.label")).into_owned();
    let desc_key = format!("{base}.desc");
    let desc = t(&desc_key).into_owned();
    (label, if desc == desc_key { String::new() } else { desc })
}

/// `settings.<key>` as a plain string.
pub fn s(key: &str) -> String {
    t(&format!("settings.{key}")).into_owned()
}

/// Renders rows, or in search mode only the rows that match. A section whose rows all miss is hidden:
/// `section` first runs its body in collect mode, where `row` only counts matches.
pub struct Rows<'a> {
    terms: Vec<String>,
    collect: bool,
    hits: usize,
    rendered: usize,
    /// Page name shown before a section title while searching.
    pub prefix: String,
    pub env: &'a Env,
}

impl<'a> Rows<'a> {
    pub fn new(query: &str, env: &'a Env) -> Self {
        let terms = query.split_whitespace().map(str::to_lowercase).collect();
        Self { terms, collect: false, hits: 0, rendered: 0, prefix: String::new(), env }
    }

    pub fn searching(&self) -> bool {
        !self.terms.is_empty()
    }

    fn matches(&self, label: &str, desc: &str) -> bool {
        if self.terms.is_empty() {
            return true;
        }
        let hay = format!("{label} {desc}").to_lowercase();
        self.terms.iter().all(|term| hay.contains(term.as_str()))
    }

    /// A titled card of rows.
    pub fn section(&mut self, ui: &mut Ui, title: &str, mut body: impl FnMut(&mut Ui, &mut Rows)) {
        if self.searching() {
            self.collect = true;
            self.hits = 0;
            body(ui, self);
            self.collect = false;
            if self.hits == 0 {
                return;
            }
        }
        self.rendered += 1;
        if self.searching() && !self.prefix.is_empty() {
            let title = format!("{} › {title}", self.prefix);
            widgets::section(ui, &title, |ui| body(ui, self));
        } else {
            widgets::section(ui, title, |ui| body(ui, self));
        }
    }

    /// How many sections have been drawn so far.
    pub fn rendered(&self) -> usize {
        self.rendered
    }

    /// One setting: label and description on the left, `control` on the right (added right to left).
    pub fn row(&mut self, ui: &mut Ui, key: &str, control: impl FnOnce(&mut Ui)) {
        self.row_locked(ui, false, key, control);
    }

    /// Like [`row`](Self::row); a locked row is disabled and carries the "managed" badge.
    pub fn row_locked(&mut self, ui: &mut Ui, locked: bool, key: &str, control: impl FnOnce(&mut Ui)) {
        let (label, desc) = texts(key);
        self.row_raw(ui, locked, &label, &desc, control);
    }

    /// A row with literal texts, for labels that are data (actions, pipelines, destinations).
    pub fn row_raw(&mut self, ui: &mut Ui, locked: bool, label: &str, desc: &str, control: impl FnOnce(&mut Ui)) {
        if !self.matches(label, desc) {
            return;
        }
        if self.collect {
            self.hits += 1;
            return;
        }
        setting_row(ui, label, desc, |ui| {
            if locked {
                ui.add_enabled_ui(false, control);
                managed_badge(ui);
            } else {
                control(ui);
            }
        });
    }

    /// A setting whose control needs the full width: label and description on top, `content` under them.
    pub fn stacked(&mut self, ui: &mut Ui, locked: bool, key: &str, content: impl FnOnce(&mut Ui)) {
        let (label, desc) = texts(key);
        if !self.matches(&label, &desc) {
            return;
        }
        if self.collect {
            self.hits += 1;
            return;
        }
        let first = ui.min_rect().height() < 1.0;
        if !first {
            divider(ui);
        }
        ui.add_space(space::MD);
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            if locked {
                ui.horizontal(|ui| {
                    ui.label(label.as_str());
                    managed_badge(ui);
                });
            } else {
                ui.label(label.as_str());
            }
            if !desc.is_empty() {
                ui.label(text::caption(desc));
            }
        });
        ui.add_space(space::SM);
        ui.add_enabled_ui(!locked, content);
        ui.add_space(space::MD);
        ui.add_space(-ui.spacing().item_spacing.y);
    }

    /// Free-form content inside a section (a table, a status line), shown unless searching.
    pub fn content(&mut self, ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
        if self.searching() || self.collect {
            return;
        }
        let first = ui.min_rect().height() < 1.0;
        if !first {
            divider(ui);
        }
        ui.add_space(space::MD);
        add(ui);
        ui.add_space(space::MD);
        ui.add_space(-ui.spacing().item_spacing.y);
    }
}

impl Rows<'_> {
    pub fn toggle(&mut self, ui: &mut Ui, key: &str, value: &mut bool) {
        self.toggle_locked(ui, false, key, value);
    }

    pub fn toggle_locked(&mut self, ui: &mut Ui, locked: bool, key: &str, value: &mut bool) {
        self.row_locked(ui, locked, key, |ui| {
            widgets::toggle(ui, value);
        });
    }

    pub fn choice<T: PartialEq + Clone>(&mut self, ui: &mut Ui, key: &str, value: &mut T, options: &[(T, String)]) {
        self.choice_locked(ui, false, key, value, options);
    }

    pub fn choice_locked<T: PartialEq + Clone>(&mut self, ui: &mut Ui, locked: bool, key: &str, value: &mut T, options: &[(T, String)]) {
        self.row_locked(ui, locked, key, |ui| {
            // Short lists share one width so the controls in a card line up.
            ui.spacing_mut().combo_width = CHOICE_WIDTH;
            dd(ui, key, value, options);
        });
    }

    pub fn segments<T: PartialEq + Clone>(&mut self, ui: &mut Ui, key: &str, value: &mut T, options: &[(T, String)]) {
        self.row(ui, key, |ui| {
            let refs: Vec<(T, &str)> = options.iter().map(|(v, l)| (v.clone(), l.as_str())).collect();
            widgets::segmented(ui, value, &refs);
        });
    }

    pub fn number<N: egui::emath::Numeric>(
        &mut self,
        ui: &mut Ui,
        key: &str,
        value: &mut N,
        range: std::ops::RangeInclusive<N>,
        suffix: &str,
    ) {
        self.row(ui, key, |ui| {
            widgets::number_field(ui, value, range, suffix);
        });
    }

    pub fn text(&mut self, ui: &mut Ui, key: &str, value: &mut String, hint: &str, mono: bool, width: f32) {
        self.row(ui, key, |ui| {
            if mono {
                field_mono(ui, value, hint, width);
            } else {
                field(ui, value, hint, width);
            }
        });
    }

    /// A 0 to 100 slider over a `u8` setting.
    pub fn percent(&mut self, ui: &mut Ui, key: &str, value: &mut u8, range: std::ops::RangeInclusive<u8>) {
        self.row(ui, key, |ui| {
            let mut v = f32::from(*value);
            let (lo, hi) = (f32::from(*range.start()), f32::from(*range.end()));
            if ui.add(widgets::Slider::new(&mut v, lo..=hi).step(1.0).width(160.0)).changed() {
                *value = v.round() as u8;
            }
        });
    }
}

/// The width short dropdowns in setting rows share.
const CHOICE_WIDTH: f32 = 180.0;

/// `want`, or less when the row has less room: a field never pushes past the card edge or over its label.
fn fit_width(ui: &Ui, want: f32) -> f32 {
    want.min(ui.available_width() - space::SM).max(80.0)
}

/// "Managed by your organization" pill.
pub fn managed_badge(ui: &mut Ui) {
    let resp = badge(ui, &format!("{} {}", icon::LOCK_SIMPLE, s("managed")), Tone::Gold);
    resp.on_hover_text(s("managed_hint"));
}

/// A page title with a one-line description.
pub fn page_header(ui: &mut Ui, title: &str, subtitle: &str) {
    ui.label(text::headline(title));
    if !subtitle.is_empty() {
        ui.label(text::dim(subtitle));
    }
    ui.add_space(space::XL);
}

/// A banner explaining that a whole page is locked by policy.
pub fn managed_banner(ui: &mut Ui, message: &str) {
    widgets::card_frame(ui, 12).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(text::dim(icon::LOCK_SIMPLE.to_string()));
            ui.label(text::dim(message));
        });
    });
    ui.add_space(space::LG);
}

/// Localised option table: `(value, t(key))`.
pub fn opts<T: Clone>(pairs: &[(T, &str)]) -> Vec<(T, String)> {
    pairs.iter().map(|(v, key)| (v.clone(), t(key).into_owned())).collect()
}

/// [`dropdown`] over an owned option table.
pub fn dd<T: PartialEq + Clone>(ui: &mut Ui, id: impl Hash + std::fmt::Debug, value: &mut T, options: &[(T, String)]) -> Response {
    let refs: Vec<(T, &str)> = options.iter().map(|(v, l)| (v.clone(), l.as_str())).collect();
    dropdown(ui, id, value, &refs)
}

/// A dropdown over an optional value: `None` is shown as `none_label`.
pub fn dd_opt<T: PartialEq + Clone>(
    ui: &mut Ui,
    id: impl Hash + std::fmt::Debug,
    value: &mut Option<T>,
    none_label: &str,
    options: &[(T, String)],
) {
    let mut all: Vec<(Option<T>, String)> = vec![(None, none_label.to_owned())];
    all.extend(options.iter().map(|(v, l)| (Some(v.clone()), l.clone())));
    dd(ui, id, value, &all);
}

/// Single-line text input of a given width.
pub fn field(ui: &mut Ui, value: &mut String, hint: &str, width: f32) {
    let width = fit_width(ui, width);
    ui.add(TextField::new(value).hint(hint).width(width));
}

/// [`field`] in the mono face, for URLs, paths and tokens.
pub fn field_mono(ui: &mut Ui, value: &mut String, hint: &str, width: f32) {
    let width = fit_width(ui, width);
    ui.add(TextField::new(value).hint(hint).mono(true).width(width));
}

/// A secret input. Whatever is typed goes into `draft` and is written to the keyring by the caller on
/// `lost_focus`; `stored` says whether a secret already exists. Returns true when the field was just committed.
pub fn secret_field(ui: &mut Ui, draft: &mut String, stored: bool, width: f32) -> bool {
    let hint = if stored { s("secret.stored") } else { s("secret.empty") };
    let width = fit_width(ui, width);
    let resp = ui.add(TextField::new(draft).hint(&hint).password(true).mono(true).width(width));
    resp.lost_focus() && !draft.is_empty()
}

/// One secret kept in the secret store under `reference`, edited through a password field.
/// Nothing typed here ever reaches the settings file: the value goes to the store when the field loses focus.
#[derive(Default)]
pub struct SecretSlot {
    input: String,
    /// Whether the store holds a non-empty value. Looked up lazily, once.
    stored: Option<bool>,
    /// A second store key every write is mirrored to.
    pub mirror: Option<String>,
}

impl SecretSlot {
    /// Forget what was looked up (the reference changed).
    pub fn reset(&mut self) {
        self.input.clear();
        self.stored = None;
        self.mirror = None;
    }

    /// Whether the store holds a value for `reference`.
    pub fn is_stored(&mut self, cx: &ViewCx, reference: &Option<String>) -> bool {
        *self.stored.get_or_insert_with(|| {
            reference.as_deref().and_then(|name| cx.engine.secrets().get(name)).is_some_and(|value| !value.is_empty())
        })
    }

    pub fn set_stored(&mut self, stored: bool) {
        self.stored = Some(stored);
    }

    /// Delete the stored value.
    pub fn remove(&mut self, cx: &mut ViewCx, reference: &Option<String>) {
        if let Some(name) = reference.as_deref() {
            let result = cx.engine.secrets().delete(name);
            report(cx, result);
        }
        self.stored = Some(false);
    }

    /// A secret row. `reference` is created as `default_ref` on the first write.
    pub fn row(&mut self, ui: &mut Ui, rc: &mut Rows, key: &str, cx: &mut ViewCx, reference: &mut Option<String>, default_ref: &str) {
        let stored = self.is_stored(cx, reference);
        rc.row(ui, key, |ui| {
            if stored && widgets::ghost_button(ui, &s("secret.remove")).clicked() {
                self.remove(cx, reference);
            }
            if secret_field(ui, &mut self.input, stored, 260.0) {
                let name = reference.get_or_insert_with(|| default_ref.to_owned()).clone();
                let value = std::mem::take(&mut self.input);
                let secrets = cx.engine.secrets();
                let mut result = secrets.set(&name, &value);
                if let (Ok(()), Some(other)) = (&result, &self.mirror) {
                    result = secrets.set(other, &value);
                }
                self.stored = Some(result.is_ok());
                report(cx, result);
            }
        });
    }
}

/// Show a failed secret-store call as a toast.
pub fn report(cx: &mut ViewCx, result: snap_core::Result<()>) {
    if let Err(e) = result {
        cx.toast_error(&e);
    }
}

/// Text field plus a Browse button that opens a native dialog on a worker thread.
/// The result arrives as [`Msg::Picked`]. Added right to left, so call it inside a row control.
pub fn path_field(ui: &mut Ui, cx: &mut ViewCx, value: &mut String, hint: &str, target: Pick, width: f32) {
    let tip = if target.is_folder() { s("choose_folder") } else { s("choose_file") };
    if widgets::icon_button(ui, if target.is_folder() { icon::FOLDER_OPEN } else { icon::FILE }, &tip, None).clicked() {
        browse(cx, value, target);
    }
    let width = fit_width(ui, width - 36.0);
    ui.add(TextField::new(value).hint(hint).mono(true).width(width));
}

/// [`path_field`] for left-to-right layouts such as a form row: the field first, then the button.
pub fn path_field_ltr(ui: &mut Ui, cx: &mut ViewCx, value: &mut String, hint: &str, target: Pick, width: f32) {
    // Leave room for the button that follows.
    let width = (width - 36.0).min(ui.available_width() - 36.0 - space::SM).max(80.0);
    ui.add(TextField::new(value).hint(hint).mono(true).width(width));
    let tip = if target.is_folder() { s("choose_folder") } else { s("choose_file") };
    if widgets::icon_button(ui, if target.is_folder() { icon::FOLDER_OPEN } else { icon::FILE }, &tip, None).clicked() {
        browse(cx, value, target);
    }
}

/// Open the native chooser starting at `current`.
pub fn browse(cx: &mut ViewCx, current: &str, target: Pick) {
    let start = PathBuf::from(current);
    cx.spawn(move || {
        let mut dialog = rfd::FileDialog::new();
        let dir = if start.is_dir() { Some(start.clone()) } else { start.parent().filter(|p| p.is_dir()).map(PathBuf::from) };
        if let Some(dir) = dir {
            dialog = dialog.set_directory(dir);
        }
        let path = if target.is_folder() { dialog.pick_folder() } else { dialog.pick_file() };
        Msg::Picked { target, path }
    });
}

/// A destructive button with inline confirmation: the first click arms it, the second confirms.
/// Returns true on confirmation. Added right to left, so inside a row control it reads `[cancel] [confirm]`.
pub fn confirm_button(ui: &mut Ui, id: impl Hash + std::fmt::Debug, label: &str, confirm_label: &str) -> bool {
    let key = ui.make_persistent_id(id);
    let armed = ui.data(|d| d.get_temp::<bool>(key)).unwrap_or(false);
    if !armed {
        if ui.add(Button::new(label).kind(ButtonKind::Secondary)).clicked() {
            ui.data_mut(|d| d.insert_temp(key, true));
        }
        return false;
    }
    let mut confirmed = false;
    if ui.add(Button::new(confirm_label).kind(ButtonKind::Danger)).clicked() {
        confirmed = true;
        ui.data_mut(|d| d.insert_temp(key, false));
    }
    if widgets::ghost_button(ui, &t("common.cancel")).clicked() {
        ui.data_mut(|d| d.insert_temp(key, false));
    }
    confirmed
}

/// Wrapping row of items (chips, checkboxes).
pub fn wrap(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(space::SM, space::SM);
        ui.with_layout(Layout::left_to_right(Align::Center).with_main_wrap(true), add);
    });
}

/// A small dimmed explanation under a control.
pub fn note(ui: &mut Ui, message: &str) {
    ui.label(text::caption(message));
}

/// Lay `add` out left to right in a control of known `width`, inside a row whose controls are added right to left.
pub fn ltr(ui: &mut Ui, width: f32, add: impl FnOnce(&mut Ui)) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, util::control_height(ui)), Sense::hover());
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(Layout::left_to_right(Align::Center)));
    add(&mut child);
}

/// A labelled field inside a card: a 150 px label column, then the control, left to right.
/// In a narrow card the label column shrinks to leave a wide field its room.
pub fn form_row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    let p = Palette::of(ui);
    ui.horizontal(|ui| {
        let avail = ui.available_width();
        let width = if avail < 150.0 + space::SM + 320.0 { (avail * 0.3).clamp(96.0, 150.0) } else { 150.0 };
        let galley = util::layout_wrapped(ui, label, &text::body_style(), p.text_dim, width);
        let height = galley.size().y.max(util::control_height(ui));
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), Sense::hover());
        let pos = egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0);
        ui.painter().galley(util::snap(ui, pos), galley, p.text_dim);
        add(ui);
    });
}

/// A clickable list entry: icon tile, title, subtitle, `trailing` content (badges) and a chevron.
pub fn list_row(ui: &mut Ui, glyph: &str, title: &str, subtitle: &str, trailing: impl FnOnce(&mut Ui)) -> Response {
    let p = Palette::of(ui);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 56.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), title));
    if ui.is_rect_visible(rect) {
        let hover = util::hover_anim(ui, &resp);
        let painter = ui.painter();
        painter.rect_filled(rect, radius::CARD, util::lerp_color(p.surface, p.surface_2, hover));
        painter.rect_stroke(rect, radius::CARD, Stroke::new(1.0, util::lerp_color(p.border, p.border_strong(), hover)), StrokeKind::Inside);
        let tile = Rect::from_center_size(egui::pos2(rect.left() + 16.0 + 16.0, rect.center().y), egui::vec2(32.0, 32.0));
        painter.rect_filled(tile, 8.0, p.accent_soft);
        util::paint_icon(ui, tile.center(), glyph, 18.0, p.accent);
        util::paint_icon(
            ui,
            egui::pos2(rect.right() - 20.0, rect.center().y),
            icon::CARET_RIGHT,
            14.0,
            util::lerp_color(p.text_faint, p.text_dim, hover),
        );
        // The trailing widgets (a badge) go first so the text knows how much room is left for it.
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(rect.min, egui::pos2(rect.right() - 40.0, rect.max.y)))
                .layout(Layout::right_to_left(Align::Center)),
        );
        trailing(&mut child);
        let used = child.min_rect();
        let text_right = if used.is_positive() { used.left() - space::SM } else { rect.right() - 44.0 };
        let x = rect.left() + 64.0;
        let room = (text_right - x).max(40.0);
        let font = |style: &egui::TextStyle| util::font_id(ui, style);
        let elide = |text: &str, style: &egui::TextStyle, color: Color32| {
            let mut job = LayoutJob::simple_singleline(text.to_owned(), font(style), color);
            job.wrap = TextWrapping { max_width: room, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
            ui.painter().layout_job(job)
        };
        let title_g = elide(title, &text::body_strong_style(), p.text);
        let sub_g = elide(subtitle, &text::caption_style(), p.text_dim);
        let block = title_g.size().y + if subtitle.is_empty() { 0.0 } else { sub_g.size().y + 2.0 };
        let y = rect.center().y - block / 2.0;
        let title_h = title_g.size().y;
        ui.painter().galley(util::snap(ui, egui::pos2(x, y)), title_g, p.text);
        if !subtitle.is_empty() {
            ui.painter().galley(util::snap(ui, egui::pos2(x, y + title_h + 2.0)), sub_g, p.text_dim);
        }
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// "‹ Back" button plus a title, the head of a drill-in view.
pub fn back_header(ui: &mut Ui, back_label: &str) -> bool {
    let mut back = false;
    ui.horizontal(|ui| {
        back = widgets::Button::new(back_label).kind(ButtonKind::Ghost).icon(icon::ARROW_LEFT).ui(ui).clicked();
    });
    ui.add_space(space::SM);
    back
}

/// A machine id from a display name: lowercase ASCII words joined by dashes, made unique with a numeric suffix.
pub fn unique_slug(name: &str, fallback: &str, taken: impl Fn(&str) -> bool) -> String {
    let slug: String = name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let mut base = slug.split('-').filter(|part| !part.is_empty()).collect::<Vec<_>>().join("-");
    if base.is_empty() {
        base = fallback.to_owned();
    }
    let mut id = base.clone();
    let mut n = 2;
    while taken(&id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

/// A single color well editing a `#RRGGBB[AA]` setting. An unparsable value is shown as `fallback`.
pub fn color_well(ui: &mut Ui, hex: &mut String, fallback: [u8; 4], alpha: bool) {
    let mut color = snap_core::settings::parse_hex_color(hex).unwrap_or(fallback);
    if widgets::color_well(ui, &mut color, alpha).changed() {
        *hex = snap_core::settings::to_hex_color(color);
    }
}
