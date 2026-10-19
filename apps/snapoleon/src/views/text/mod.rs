//! The text result window: recognised or translated text in an editable area, with the actions you want next
//! (copy, copy as a table, translate, search the web, open as a file) and any QR or barcode results.

use std::{any::Any, path::PathBuf};

use egui::{Context, Frame, Id, Key, Margin, Modifiers, Rect, RichText, ScrollArea, Sense, Stroke, TextEdit, Ui, UiBuilder, Vec2};
use snap_core::{Error, settings::TranslateProvider, t, tf};
use snap_ocr::codes::Code;
use snap_ui::{
    Palette, ToastKind, icon,
    text::{self, body_style, caption_style},
    widgets::{Button, ButtonKind, badge, dropdown, empty_state, icon_button, spinner, tooltip, util},
};

use super::{
    Chrome, OpenCx, View, ViewCx,
    library::{menu, paint},
    window_spec,
};
use crate::{commands::Command, views::settings::SettingsPage};

mod table;

use table::Table;

pub const TAG: &str = "text";

/// Languages offered for translation, as the ISO codes the translation services take.
const LANGUAGES: [&str; 12] = ["en", "ru", "de", "fr", "es", "it", "pt", "pl", "uk", "tr", "zh", "ja"];
const MARGIN: f32 = 16.0;
/// Narrower than this the source and the translation stack instead of sitting side by side.
const SIDE_BY_SIDE: f32 = 700.0;

pub fn open(cx: &mut OpenCx, title: String, text: String) {
    open_with_codes(cx, title, text, Vec::new());
}

/// Whether translating can work at all: a service is chosen, and when that service is the AI endpoint, AI is set up.
pub fn translation_ready(settings: &snap_core::Settings) -> bool {
    match settings.translate.provider {
        TranslateProvider::None => false,
        TranslateProvider::Ai => settings.ai.enabled && !settings.ai.endpoint.trim().is_empty() && !settings.ai.model.trim().is_empty(),
        TranslateProvider::LibreTranslate { .. } | TranslateProvider::DeepL { .. } => true,
    }
}

/// Like [`open`], with decoded QR codes and barcodes listed above the text.
pub fn open_with_codes(cx: &mut OpenCx, title: String, text: String, codes: Vec<Code>) {
    let title = if title.trim().is_empty() { t("text.title").into_owned() } else { title };
    let target = cx.settings.translate.target_lang.clone();
    let spec = window_spec(title.clone(), TAG, (700.0, 620.0), (460.0, 380.0));
    cx.open(spec, TextView::new(title, text, codes, target));
}

enum Msg {
    Translated { target: String, result: Result<String, Error> },
    Saved(Result<PathBuf, Error>),
}

struct Translation {
    text: String,
    target: String,
}

#[derive(Clone, Copy)]
enum TableCopy {
    Spreadsheet,
    Markdown,
}

struct TextView {
    title: String,
    text: String,
    mono: bool,
    codes: Vec<Code>,
    table: Option<Table>,
    /// The text `table` was detected in, so detection only reruns after an edit.
    table_source: String,
    links: Vec<String>,
    target: String,
    translating: bool,
    translation: Option<Translation>,
    /// Anchor of the open "copy as table" menu.
    table_menu: Option<Rect>,
    edit_id: Id,
}

impl TextView {
    fn new(title: String, text: String, codes: Vec<Code>, target: String) -> Self {
        let target = if LANGUAGES.contains(&target.as_str()) { target } else { LANGUAGES[0].to_owned() };
        Self {
            title,
            text,
            mono: false,
            codes,
            table: None,
            table_source: String::from("\0"),
            links: Vec::new(),
            target,
            translating: false,
            translation: None,
            table_menu: None,
            edit_id: Id::unique(("text-view-edit", snap_core::now_ms())),
        }
    }

    /// Re-detect the table and links when the text changed.
    fn analyze(&mut self) {
        if self.table_source != self.text {
            self.table = Table::detect(&self.text);
            self.links = find_links(&self.text);
            self.table_source.clone_from(&self.text);
        }
    }

    fn copy(&self, cx: &mut ViewCx, text: String) {
        cx.push(Command::CopyText(text));
        cx.toast(ToastKind::Success, t("common.copied"));
    }

    fn translate(&mut self, cx: &mut ViewCx) {
        if self.translating || self.text.trim().is_empty() {
            return;
        }
        if !translation_ready(cx.settings) {
            cx.toast_action(
                ToastKind::Info,
                t("text.translate_off"),
                t("text.translate_setup"),
                Command::OpenSettings { page: Some(SettingsPage::TextAi) },
            );
            return;
        }
        self.translating = true;
        let (engine, text, target) = (cx.engine.clone(), self.text.clone(), self.target.clone());
        cx.spawn(move || Msg::Translated { result: engine.translate(&text, Some(&target)), target });
    }

    /// The text, or the selected part of it, to hand to a search engine.
    fn search_text(&self, ctx: &Context) -> String {
        let selected = TextEdit::load_state(ctx, self.edit_id)
            .and_then(|state| state.cursor.char_range())
            .filter(|range| range.primary != range.secondary)
            .map(|range| {
                let (a, b) = (range.primary.index.0, range.secondary.index.0);
                self.text.chars().skip(a.min(b)).take(a.abs_diff(b)).collect::<String>()
            });
        let text = selected.unwrap_or_else(|| self.text.clone());
        text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(300).collect()
    }

    fn open_as_file(&self, cx: &ViewCx) {
        let text = self.text.clone();
        cx.spawn(move || {
            let dir = snap_core::paths::cache_dir().join("text");
            let path = dir.join(format!("text-{}.txt", snap_core::now_ms()));
            Msg::Saved(std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, text)).map(|()| path).map_err(Error::from))
        });
    }

    // --- layout ------------------------------------------------------------------------------------------------

    fn body(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        let area = ui.available_rect_before_wrap().shrink2(Vec2::new(MARGIN, 12.0));
        let mut inner = ui.new_child(UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(egui::Align::Min)));
        inner.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
        let ui = &mut inner;
        ui.set_clip_rect(ui.clip_rect().intersect(area.expand(MARGIN)));

        self.analyze();
        self.toolbar(ui, cx);
        self.translate_bar(ui, cx);
        ui.add_space(4.0);

        if self.text.trim().is_empty() && self.codes.is_empty() && self.translation.is_none() {
            ui.add_space(24.0);
            empty_state(ui, icon::TEXT_AA, &t("text.empty.title"), &t("text.empty.body"));
            return;
        }

        // Fixed-height blocks first; the editor takes what is left.
        // The stats line and the gap before it, then each block as laid out: header, gap, rows with their gaps.
        let mut reserved = 20.0 + 8.0 + 8.0;
        if !self.codes.is_empty() {
            reserved += self.codes.len().min(3) as f32 * (44.0 + 8.0) + 28.0 + 8.0;
        }
        if let Some(table) = &self.table {
            reserved += (table.rows.len() as f32 * 28.0).min(160.0) + 2.0 + 28.0 + 8.0 + 8.0;
        }
        if !self.links.is_empty() {
            reserved += self.links.len().min(3) as f32 * (36.0 + 8.0) + 28.0 + 8.0;
        }
        let editors_h = (ui.available_height() - reserved).max(140.0);

        if !self.codes.is_empty() {
            self.codes_block(ui, cx);
        }
        self.editors(ui, cx, editors_h);
        if self.table.is_some() {
            self.table_block(ui, cx);
        }
        if !self.links.is_empty() {
            self.links_block(ui, cx);
        }
        self.stats(ui);
    }

    fn toolbar(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        let has_text = !self.text.trim().is_empty();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.add_enabled_ui(has_text, |ui| {
                let copy = ui.add(Button::new(&t("text.copy")).kind(ButtonKind::Primary).icon(icon::COPY));
                if tooltip(copy, &t("text.copy_tip"), Some("CmdOrCtrl+Shift+C")).clicked() {
                    self.copy(cx, self.text.clone());
                }
                if self.table.is_some() {
                    let button = ui.add(Button::new(&t("text.copy_table")).icon(icon::TABLE));
                    if button.clicked() {
                        self.table_menu = if self.table_menu.is_some() { None } else { Some(button.rect) };
                    }
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.add_enabled_ui(has_text, |ui| {
                    if icon_button(ui, icon::FILE_TEXT, &t("text.open_file"), None).clicked() {
                        self.open_as_file(cx);
                    }
                    if icon_button(ui, icon::GLOBE, &t("text.search_web"), Some("CmdOrCtrl+E")).clicked() {
                        self.search(cx, ui.ctx());
                    }
                });
                let (glyph, tip) = if self.mono { (icon::TEXT_AA, t("text.font.sans")) } else { (icon::CODE, t("text.font.mono")) };
                if icon_button(ui, glyph, &tip, None).clicked() {
                    self.mono = !self.mono;
                }
            });
        });
    }

    fn search(&self, cx: &mut ViewCx, ctx: &Context) {
        let query = self.search_text(ctx);
        if !query.is_empty() {
            cx.push(Command::OpenUrl(format!("https://duckduckgo.com/?q={}", percent_encode(&query))));
        }
    }

    fn translate_bar(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        let has_text = !self.text.trim().is_empty();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.label(text::dim(t("text.translate_to")));
            let names: Vec<String> = LANGUAGES.iter().map(|code| t(&format!("text.lang.{code}")).into_owned()).collect();
            let options: Vec<(String, &str)> =
                LANGUAGES.iter().zip(&names).map(|(code, name)| ((*code).to_owned(), name.as_str())).collect();
            dropdown(ui, "text-target", &mut self.target, &options);
            if self.translating {
                spinner(ui);
                ui.label(text::dim(t("text.translating")));
            } else {
                let button = ui.add_enabled(has_text, Button::new(&t("text.translate")).icon(icon::TRANSLATE));
                if tooltip(button, "", Some("CmdOrCtrl+Enter")).clicked() {
                    self.translate(cx);
                }
            }
        });
    }

    /// The source text, and the translation next to it or under it.
    fn editors(&mut self, ui: &mut Ui, cx: &mut ViewCx, height: f32) {
        let Some(translation) = self.translation.as_mut() else {
            let size = Vec2::new(ui.available_width(), height);
            edit_area(ui, self.edit_id, &mut self.text, self.mono, size);
            return;
        };
        let side_by_side = ui.available_width() >= SIDE_BY_SIDE;
        let (width, gap) = (ui.available_width(), 12.0);
        let (pane_w, pane_h) = if side_by_side { ((width - gap) / 2.0, height) } else { (width, (height - gap) / 2.0) };
        let mut action = None;
        let mut pane = |ui: &mut Ui, header: String, content: &mut String, id: Id, copy: bool, mono: bool| {
            ui.vertical(|ui| {
                ui.set_width(pane_w);
                ui.spacing_mut().item_spacing.y = 6.0;
                ui.horizontal(|ui| {
                    ui.label(RichText::new(header).text_style(snap_ui::text::body_strong_style()));
                    if copy {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            if icon_button(ui, icon::X, &t("text.translation.dismiss"), None).clicked() {
                                action = Some(PaneAction::Dismiss);
                            }
                            if icon_button(ui, icon::ARROW_U_UP_LEFT, &t("text.translation.use"), None).clicked() {
                                action = Some(PaneAction::Use);
                            }
                            if icon_button(ui, icon::COPY, &t("text.translation.copy"), None).clicked() {
                                action = Some(PaneAction::Copy);
                            }
                        });
                    }
                });
                edit_area(ui, id, content, mono, Vec2::new(pane_w, pane_h - 28.0 - 6.0));
            });
        };
        let source_header = t("text.source").into_owned();
        let target_name = t(&format!("text.lang.{}", translation.target)).into_owned();
        let header = tf("text.translation_title", &[("lang", &target_name)]);
        let (edit_id, mono) = (self.edit_id, self.mono);
        let (text, translated) = (&mut self.text, &mut translation.text);
        if side_by_side {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                pane(ui, source_header, text, edit_id, false, mono);
                pane(ui, header, translated, edit_id.with("translation"), true, mono);
            });
        } else {
            pane(ui, source_header, text, edit_id, false, mono);
            ui.add_space(gap - 8.0);
            pane(ui, header, translated, edit_id.with("translation"), true, mono);
        }
        match action {
            Some(PaneAction::Copy) => self.copy(cx, self.translation.as_ref().map(|t| t.text.clone()).unwrap_or_default()),
            Some(PaneAction::Use) => {
                if let Some(done) = self.translation.take() {
                    self.text = done.text;
                }
            }
            Some(PaneAction::Dismiss) => self.translation = None,
            None => {}
        }
    }

    fn section_header(ui: &mut Ui, title: &str, trailing: impl FnOnce(&mut Ui)) {
        ui.horizontal(|ui| {
            ui.set_height(28.0);
            ui.label(RichText::new(title).text_style(snap_ui::text::body_strong_style()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                trailing(ui);
            });
        });
    }

    fn table_block(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        let Some(table) = &self.table else { return };
        let size = format!("{} × {}", table.rows.len(), table.cols);
        Self::section_header(ui, &t("text.table.title"), |ui| {
            ui.label(text::mono_text(size).color(Palette::of(ui).text_dim));
        });
        table::show(ui, table, 160.0);
        let _ = cx;
    }

    fn codes_block(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        let p = Palette::of(ui);
        Self::section_header(ui, &t("text.codes.title"), |ui| {
            badge(ui, &self.codes.len().to_string(), snap_ui::Tone::Neutral);
        });
        for code in self.codes.clone().iter().take(3) {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 44.0), Sense::hover());
            ui.painter().rect_filled(rect, 10.0, p.surface_2);
            ui.painter().rect_stroke(rect, 10.0, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
            let glyph = if code.format.to_ascii_lowercase().contains("qr") { icon::QR_CODE } else { icon::BARCODE };
            util::paint_icon(ui, egui::pos2(rect.left() + 22.0, rect.center().y), glyph, 20.0, p.text_dim);
            let buttons = if is_link(&code.text) { 2.0 } else { 1.0 };
            let text_w = rect.width() - 44.0 - buttons * 32.0 - 12.0;
            let value = paint::elided(ui, &code.text, egui::FontId::monospace(12.0), p.text, text_w);
            let kind = paint::elided(ui, &code_kind(&code.format), caption_style().resolve(ui.style()), p.text_dim, text_w);
            let block = value.size().y + 2.0 + kind.size().y;
            let mut y = rect.center().y - block / 2.0;
            ui.painter().galley(util::snap(ui, egui::pos2(rect.left() + 44.0, y)), value.clone(), p.text);
            y += value.size().y + 2.0;
            ui.painter().galley(util::snap(ui, egui::pos2(rect.left() + 44.0, y)), kind, p.text_dim);
            let mut x = rect.right() - 8.0;
            let actions = [
                Some((icon::COPY, t("text.copy_value"), false)),
                is_link(&code.text).then(|| (icon::ARROW_SQUARE_OUT, t("text.open_link"), true)),
            ];
            for (glyph, tip, open) in actions.into_iter().flatten().collect::<Vec<_>>().into_iter().rev() {
                let r = Rect::from_min_size(egui::pos2(x - 28.0, rect.center().y - 14.0), Vec2::splat(28.0));
                x -= 32.0;
                let resp = ui.interact(r, Id::unique(("code-action", &code.text, open)), Sense::click());
                let hover = util::hover_anim(ui, &resp);
                ui.painter().rect_filled(r, 6.0, p.hover().gamma_multiply(hover));
                util::paint_icon(ui, r.center(), glyph, 15.0, util::lerp_color(p.text_dim, p.text, hover));
                if tooltip(resp, &tip, None).clicked() {
                    if open {
                        cx.push(Command::OpenUrl(normalize_link(&code.text)));
                    } else {
                        self.copy(cx, code.text.clone());
                    }
                }
            }
        }
    }

    fn links_block(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        let p = Palette::of(ui);
        Self::section_header(ui, &t("text.links.title"), |_| ());
        for link in self.links.clone().iter().take(3) {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::hover());
            ui.painter().rect_filled(rect, 10.0, p.surface_2);
            ui.painter().rect_stroke(rect, 10.0, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
            util::paint_icon(ui, egui::pos2(rect.left() + 20.0, rect.center().y), icon::LINK, 15.0, p.text_dim);
            let galley = paint::elided(
                ui,
                link.strip_prefix("mailto:").unwrap_or(link),
                egui::FontId::monospace(12.0),
                p.accent,
                rect.width() - 40.0 - 2.0 * 32.0 - 12.0,
            );
            ui.painter().galley(util::snap(ui, egui::pos2(rect.left() + 38.0, rect.center().y - galley.size().y / 2.0)), galley, p.accent);
            let actions = [(icon::ARROW_SQUARE_OUT, t("text.open_link"), true), (icon::COPY, t("text.copy_value"), false)];
            for (n, (glyph, tip, open)) in actions.into_iter().enumerate() {
                let r =
                    Rect::from_min_size(egui::pos2(rect.right() - 8.0 - 28.0 - n as f32 * 32.0, rect.center().y - 14.0), Vec2::splat(28.0));
                let resp = ui.interact(r, Id::unique(("link-action", link, open)), Sense::click());
                let hover = util::hover_anim(ui, &resp);
                ui.painter().rect_filled(r, 6.0, p.hover().gamma_multiply(hover));
                util::paint_icon(ui, r.center(), glyph, 15.0, util::lerp_color(p.text_dim, p.text, hover));
                if tooltip(resp, &tip, None).clicked() {
                    if open {
                        cx.push(Command::OpenUrl(normalize_link(link)));
                    } else {
                        self.copy(cx, link.clone());
                    }
                }
            }
        }
    }

    fn stats(&self, ui: &mut Ui) {
        let p = Palette::of(ui);
        let chars = self.text.chars().count();
        let words = self.text.split_whitespace().count();
        let lines = if self.text.is_empty() { 0 } else { self.text.lines().count() };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (n, (label, value)) in
                [("text.stats.chars", chars), ("text.stats.words", words), ("text.stats.lines", lines)].into_iter().enumerate()
            {
                if n > 0 {
                    ui.add_space(12.0);
                }
                ui.label(text::caption(t(label)));
                ui.label(text::mono_text(value.to_string()).size(11.0).color(p.text_dim));
            }
        });
    }

    fn menu_layer(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let Some(anchor) = self.table_menu else { return };
        let Some(table) = &self.table else {
            self.table_menu = None;
            return;
        };
        let items = [
            menu::Item::new(icon::TABLE, t("text.table.spreadsheet"), TableCopy::Spreadsheet),
            menu::Item::new(icon::HASH, t("text.table.markdown"), TableCopy::Markdown),
        ];
        match menu::show(ctx, Id::unique("text-table-menu"), menu::Anchor::Start(anchor), &items) {
            menu::Outcome::Open => {}
            menu::Outcome::Closed => self.table_menu = None,
            menu::Outcome::Chosen(choice) => {
                self.table_menu = None;
                let result = match choice {
                    TableCopy::Spreadsheet => snap_platform::clipboard::set_html(&table.html(), &table.tsv()),
                    TableCopy::Markdown => snap_platform::clipboard::set_text(&table.markdown()),
                };
                match result {
                    Ok(()) => cx.toast(ToastKind::Success, t("common.copied")),
                    Err(e) => cx.toast_error(&e),
                }
            }
        }
    }

    fn keyboard(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let command_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        if ctx.input_mut(|i| i.consume_key(command_shift, Key::C)) && !self.text.is_empty() {
            self.copy(cx, self.text.clone());
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter)) {
            self.translate(cx);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::E)) {
            self.search(cx, ctx);
        }
        if self.table_menu.is_none() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            // Esc first leaves the editor, so a stray press never closes a window you are typing in.
            if ctx.memory(|m| m.has_focus(self.edit_id) || m.has_focus(self.edit_id.with("translation"))) {
                ctx.memory_mut(|m| m.stop_text_input());
            } else {
                cx.close();
            }
        }
    }
}

enum PaneAction {
    Copy,
    Use,
    Dismiss,
}

impl View for TextView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let title = self.title.clone();
        Chrome::new(&title).show(cx, ctx, |ui, cx| self.body(ui, cx));
        self.menu_layer(cx, ctx);
        self.keyboard(cx, ctx);
    }

    fn on_msg(&mut self, cx: &mut ViewCx, msg: Box<dyn Any + Send>) {
        let Ok(msg) = msg.downcast::<Msg>() else { return };
        match *msg {
            Msg::Translated { target, result } => {
                self.translating = false;
                match result {
                    Ok(text) => self.translation = Some(Translation { text, target }),
                    Err(e) => cx.toast_error(&e),
                }
            }
            Msg::Saved(Ok(path)) => cx.push(Command::OpenPath(path)),
            Msg::Saved(Err(e)) => cx.toast_error(&e),
        }
    }
}

/// A bordered multi-line editor that fills `size`, in the proportional or the monospace font.
fn edit_area(ui: &mut Ui, id: Id, content: &mut String, mono: bool, size: Vec2) {
    let p = Palette::of(ui);
    let focused = ui.memory(|m| m.has_focus(id));
    let focus = util::anim(ui.ctx(), id.with("focus"), focused, util::Motion::HOVER);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 10.0, p.surface_2);
    let border = util::lerp_color(p.border, p.accent, focus);
    painter.rect_stroke(rect, 10.0, Stroke::new(1.0, border), egui::StrokeKind::Inside);
    if focus > 0.0 {
        painter.rect_stroke(rect, 10.0, Stroke::new(2.0, p.accent.gamma_multiply(0.3 * focus)), egui::StrokeKind::Outside);
    }
    let inner = rect.shrink2(Vec2::new(12.0, 10.0));
    let mut child = ui.new_child(UiBuilder::new().max_rect(inner));
    let style = if mono { text::mono() } else { body_style() };
    ScrollArea::vertical().id_salt(id.with("scroll")).auto_shrink([false, false]).show(&mut child, |ui| {
        let edit = TextEdit::multiline(content)
            .id(id)
            .frame(Frame::NONE)
            .margin(Margin::ZERO)
            .font(util::font_id(ui, &style))
            .text_color(p.text)
            .desired_width(f32::INFINITY)
            .desired_rows(4)
            .lock_focus(true);
        ui.add_sized(Vec2::new(ui.available_width(), ui.available_height().max(40.0)), edit);
    });
    // The whole area is the click target, not just the lines of text.
    let click = ui.interact(rect, id.with("area"), Sense::click());
    if click.clicked() {
        ui.memory_mut(|m| m.request_focus(id));
    }
}

/// The symbology of a decoded code as a label: "QR code", "EAN 13".
fn code_kind(format: &str) -> String {
    if format.eq_ignore_ascii_case("qrcode") { t("text.codes.qr").into_owned() } else { format.to_uppercase() }
}

fn is_link(text: &str) -> bool {
    let text = text.trim();
    text.starts_with("http://") || text.starts_with("https://") || text.starts_with("www.")
}

/// `www.` links get a scheme; everything else is passed through.
fn normalize_link(link: &str) -> String {
    if link.starts_with("www.") { format!("https://{link}") } else { link.to_owned() }
}

/// URLs and e-mail addresses in `text`, in order, without duplicates.
fn find_links(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let word = word.trim_matches(|c: char| matches!(c, '(' | ')' | '[' | ']' | '<' | '>' | ',' | ';' | '"' | '\'' | '.' | '!' | '?'));
        let email = word
            .split_once('@')
            .is_some_and(|(user, host)| !user.is_empty() && host.contains('.') && !host.ends_with('.') && !word.contains("://"));
        let link = is_link(word).then(|| word.to_owned()).or_else(|| email.then(|| format!("mailto:{word}")));
        if let Some(link) = link.filter(|l| l.len() > 8 || l.starts_with("mailto:"))
            && !found.contains(&link)
        {
            found.push(link);
        }
    }
    found
}

fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
