//! The Text tab: recognise the text in the picture, copy it, translate it, and find a line on the canvas.

use egui::{Align, Layout, Sense, Stroke, StrokeKind, Ui, Vec2};
use snap_annotate::{RenderOptions, render};
use snap_core::t;
use snap_core::{PointF, RectF, Result, tf};
use snap_engine::OcrResult;
use snap_ui::{
    ToastKind, icon, palette, radius, space, text,
    widgets::{Button, ButtonKind, primary_button, secondary_button, spinner, tooltip, util::hover_anim},
};

use super::{PanelCx, empty, field};
use crate::{
    commands::Command,
    views::{editor::output::Msg, settings::SettingsPage},
};

enum Job {
    Idle,
    Running,
    Failed(String),
}

enum Translation {
    Idle,
    Running,
    Done(String),
    Failed(String),
}

struct Recognised {
    lines: Vec<(String, RectF)>,
    text: String,
    table: Option<String>,
}

pub struct State {
    job: Job,
    result: Option<Recognised>,
    /// The document revision the text was read from.
    rev: u64,
    /// Where the recognised image sat in the document (its crop), so that line boxes can be mapped back.
    origin: PointF,
    selected: Option<usize>,
    translation: Translation,
}

impl Default for State {
    fn default() -> Self {
        Self { job: Job::Idle, result: None, rev: 0, origin: PointF::default(), selected: None, translation: Translation::Idle }
    }
}

impl State {
    /// Read the text of the picture on a worker thread.
    pub fn recognise(&mut self, s: &crate::views::editor::session::Session, vcx: &crate::views::ViewCx) {
        self.job = Job::Running;
        self.rev = s.rev;
        let doc = s.doc.clone();
        let crop = doc.crop_rect();
        self.origin = PointF::new(crop.x as f32, crop.y as f32);
        let engine = vcx.engine.clone();
        vcx.spawn(move || {
            // Redactions are part of the render, so text under them is not read.
            let image = render(&doc, &RenderOptions { scale: 1.0, readable: false, beautify: false });
            Msg::Ocr(engine.ocr(&image))
        });
    }

    /// The line the user picked, in document pixels.
    pub fn highlight(&self) -> Option<RectF> {
        let line = self.result.as_ref()?.lines.get(self.selected?)?;
        Some(RectF::new(line.1.x + self.origin.x, line.1.y + self.origin.y, line.1.w, line.1.h))
    }

    pub fn on_recognised(&mut self, result: Result<OcrResult>, cx: &mut crate::views::ViewCx) {
        match result {
            Ok(ocr) => {
                self.result = Some(Recognised {
                    lines: ocr.lines.iter().filter(|l| !l.text.trim().is_empty()).map(|l| (l.text.clone(), l.rect)).collect(),
                    text: ocr.text(),
                    table: ocr.to_markdown_table(),
                });
                self.job = Job::Idle;
                self.selected = None;
                self.translation = Translation::Idle;
            }
            Err(e) => {
                self.job = Job::Failed(crate::notices::error_text(&e));
                cx.toast_error(&e);
            }
        }
    }

    pub fn on_translated(&mut self, result: Result<String>, cx: &mut crate::views::ViewCx) {
        self.translation = match result {
            Ok(text) => Translation::Done(text),
            Err(e) => {
                cx.toast_error(&e);
                Translation::Failed(crate::notices::error_text(&e))
            }
        };
    }
}

/// Draws the tab. Returns a rectangle (document pixels) the canvas should scroll to when a line was just picked.
pub fn ui(ui: &mut Ui, st: &mut State, cx: &mut PanelCx) -> Option<RectF> {
    let mut reveal = None;
    match (&st.job, &st.result) {
        (Job::Running, _) => {
            let warming = snap_ocr::warming_up(&cx.vcx.settings.ocr);
            ui.add_space(space::XL);
            ui.vertical_centered(|ui| {
                spinner(ui);
                ui.label(text::dim(t(if warming { "ocr.warmup.label" } else { "editor.ocr.running" })));
                if warming {
                    ui.label(text::caption(t("ocr.warmup.hint")));
                }
            });
        }
        (_, None) => {
            let body = match &st.job {
                Job::Failed(message) => message.clone(),
                _ => t("editor.ocr.empty_body").into_owned(),
            };
            let run =
                empty(ui, icon::TEXT_AA, &t("editor.ocr.empty_title"), &body, |ui| primary_button(ui, &t("editor.ocr.run")).clicked());
            if run {
                st.recognise(cx.s, cx.vcx);
            }
        }
        (_, Some(_)) => reveal = result_ui(ui, st, cx),
    }
    reveal
}

fn result_ui(ui: &mut Ui, st: &mut State, cx: &mut PanelCx) -> Option<RectF> {
    let mut reveal = None;
    let (full_text, table, line_count) = {
        let r = st.result.as_ref()?;
        (r.text.clone(), r.table.clone(), r.lines.len())
    };

    if line_count == 0 {
        let again = empty(ui, icon::TEXT_AA, &t("editor.ocr.none_title"), &t("editor.ocr.none_body"), |ui| {
            secondary_button(ui, &t("editor.ocr.again")).clicked()
        });
        if again {
            st.recognise(cx.s, cx.vcx);
        }
        return None;
    }

    ui.horizontal(|ui| {
        ui.label(text::caption(tf("editor.ocr.lines", &[("n", &line_count.to_string())])));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if snap_ui::widgets::icon_button(ui, icon::ARROW_CLOCKWISE, &t("editor.ocr.again"), None).clicked() {
                st.recognise(cx.s, cx.vcx);
            }
        });
    });
    if st.rev != cx.s.rev {
        ui.label(text::caption(t("editor.ocr.stale")));
    }

    // The lines, each a button that flags its place on the canvas.
    let p = palette(ui.ctx());
    egui::Frame::new()
        .fill(p.surface_2)
        .corner_radius(radius::CARD)
        .stroke(Stroke::new(1.0, p.border))
        .inner_margin(egui::Margin::same(4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 2.0;
            egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let lines: Vec<String> = st.result.iter().flat_map(|r| r.lines.iter().map(|l| l.0.clone())).collect();
                for (i, line) in lines.iter().enumerate() {
                    let selected = st.selected == Some(i);
                    if line_row(ui, line, selected, &p).clicked() {
                        st.selected = if selected { None } else { Some(i) };
                        reveal = st.highlight();
                    }
                }
            });
        });

    ui.horizontal_wrapped(|ui| {
        if ui.add(Button::new(&t("editor.ocr.copy")).icon(icon::COPY)).clicked() {
            cx.vcx.push(Command::CopyText(full_text.clone()));
            cx.vcx.toast(ToastKind::Success, t("common.copied"));
        }
        if let Some(table) = &table {
            let resp = ui.add(Button::new(&t("editor.ocr.copy_table")).icon(icon::TABLE));
            if resp.clicked() {
                cx.vcx.push(Command::CopyText(table.clone()));
                cx.vcx.toast(ToastKind::Success, t("common.copied"));
            }
            tooltip(resp, &t("editor.ocr.copy_table_tip"), None);
        }
    });

    ui.add_space(space::XS);
    let target = cx.vcx.settings.translate.target_lang.to_uppercase();
    field(ui, &t("editor.ocr.translate"), |ui| match &st.translation {
        Translation::Running => {
            ui.horizontal(|ui| {
                spinner(ui);
                ui.label(text::dim(t("editor.ocr.translating")));
            });
        }
        _ if !crate::views::text::translation_ready(cx.vcx.settings) => {
            ui.add(egui::Label::new(text::dim(t("editor.ocr.translate_off"))).wrap());
            let setup = ui.add(Button::new(&t("editor.ocr.translate_setup")).icon(icon::GEAR).kind(ButtonKind::Secondary));
            if setup.clicked() {
                cx.vcx.push(Command::OpenSettings { page: Some(SettingsPage::TextAi) });
            }
        }
        _ => {
            let label = tf("editor.ocr.translate_to", &[("lang", &target)]);
            if ui.add(Button::new(&label).icon(icon::TRANSLATE).kind(ButtonKind::Secondary)).clicked() {
                st.translation = Translation::Running;
                let engine = cx.vcx.engine.clone();
                let source = full_text.clone();
                cx.vcx.spawn(move || Msg::Translated(engine.translate(&source, None)));
            }
        }
    });
    match &st.translation {
        Translation::Done(translated) => {
            egui::Frame::new()
                .fill(p.surface_2)
                .corner_radius(radius::CARD)
                .stroke(Stroke::new(1.0, p.border))
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(egui::Label::new(translated.as_str()).selectable(true));
                });
            if ui.add(Button::new(&t("editor.ocr.copy_translation")).icon(icon::COPY)).clicked() {
                cx.vcx.push(Command::CopyText(translated.clone()));
                cx.vcx.toast(ToastKind::Success, t("common.copied"));
            }
        }
        Translation::Failed(message) => {
            ui.label(egui::RichText::new(message).color(p.danger).text_style(text::caption_style()));
        }
        _ => {}
    }
    reveal
}

/// One recognised line as a selectable row.
fn line_row(ui: &mut Ui, line: &str, selected: bool, p: &snap_ui::Palette) -> egui::Response {
    let width = ui.available_width();
    let galley = ui.painter().layout(
        line.to_string(),
        text::body_style().resolve(ui.style()),
        if selected { p.accent } else { p.text },
        width - 16.0,
    );
    let size = Vec2::new(width, galley.size().y + 10.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hover = hover_anim(ui, &resp);
    let painter = ui.painter();
    painter.rect_filled(rect, radius::CONTROL, p.hover().gamma_multiply(hover * if selected { 0.0 } else { 1.0 }));
    if selected {
        painter.rect_filled(rect, radius::CONTROL, p.accent_soft);
        painter.rect_stroke(rect, radius::CONTROL, Stroke::new(1.0, p.accent.gamma_multiply(0.4)), StrokeKind::Inside);
    }
    painter.galley(rect.min + Vec2::new(8.0, 5.0), galley, p.text);
    resp
}
