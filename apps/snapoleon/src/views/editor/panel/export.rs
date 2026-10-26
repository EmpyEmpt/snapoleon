//! The Export tab: format, quality and size of the finished image, and AI alt text for it.

use egui::{Stroke, Ui};
use snap_core::{CaptureContext, Result, settings::ImageFormat, t};
use snap_engine::Engine;
use snap_ui::{
    ToastKind, icon, palette, radius, space, text,
    widgets::{Button, ButtonKind, Slider, dropdown, segmented, spinner},
};

use super::{PanelCx, field, heading, switch_row};
use crate::{
    commands::Command,
    views::{
        editor::output::{ExportParams, Msg},
        file_size,
        settings::SettingsPage,
    },
};

/// Maximum widths offered next to "original size".
const WIDTHS: [u32; 7] = [3840, 2560, 1920, 1600, 1280, 1024, 800];

#[derive(Default)]
enum Alt {
    #[default]
    Idle,
    Running,
    Done(String),
}

#[derive(Default)]
pub struct State {
    alt: Alt,
}

impl State {
    pub fn on_alt_text(&mut self, result: Result<String>, cx: &mut crate::views::ViewCx) {
        self.alt = match result {
            Ok(text) => Alt::Done(text),
            Err(e) => {
                cx.toast_error(&e);
                Alt::Idle
            }
        };
    }
}

pub fn ui(ui: &mut Ui, st: &mut State, cx: &mut PanelCx) {
    let settings = cx.vcx.settings.clone();
    let params = ExportParams::from_settings(&settings);

    heading(ui, &t("editor.export.format"));
    let mut format = settings.capture.format;
    let formats = [(ImageFormat::Png, "PNG"), (ImageFormat::Jpeg, "JPG"), (ImageFormat::Webp, "WebP")];
    segmented(ui, &mut format, &formats);
    if format != settings.capture.format {
        cx.vcx.update_settings(move |s| s.capture.format = format);
    }

    match format {
        ImageFormat::Png => {
            let mut optimize = settings.capture.optimize_png;
            if switch_row(ui, &t("editor.export.optimize"), None, &mut optimize) {
                cx.vcx.update_settings(move |s| s.capture.optimize_png = optimize);
            }
        }
        ImageFormat::Jpeg | ImageFormat::Webp => {
            let mut quality = params.quality as f32;
            field(ui, &t("editor.export.quality"), |ui| {
                let rail = (ui.available_width() - 60.0).max(80.0);
                if ui.add(Slider::new(&mut quality, 30.0..=100.0).step(1.0).suffix("%").width(rail)).changed() {
                    let q = quality as u8;
                    cx.vcx.update_settings(move |s| match format {
                        ImageFormat::Jpeg => s.capture.jpeg_quality = q,
                        _ => s.capture.webp_quality = q,
                    });
                }
            });
        }
    }

    heading(ui, &t("editor.export.size"));
    let mut max = settings.editor.max_export_width;
    let mut options: Vec<(u32, String)> = vec![(0, t("editor.export.original").into_owned())];
    options.extend(WIDTHS.iter().map(|w| (*w, format!("{w} px"))));
    if !options.iter().any(|(w, _)| *w == max) {
        options.push((max, format!("{max} px")));
        options.sort_by_key(|(w, _)| if *w == 0 { u32::MAX } else { *w });
        options.rotate_right(1);
    }
    let option_refs: Vec<(u32, &str)> = options.iter().map(|(w, l)| (*w, l.as_str())).collect();
    field(ui, &t("editor.export.max_width"), |ui| {
        if dropdown(ui, "editor-max-width", &mut max, &option_refs).changed() {
            cx.vcx.update_settings(move |s| s.editor.max_export_width = max);
        }
    });
    let mut readable = settings.editor.readable_downscale;
    if switch_row(ui, &t("editor.export.readable"), Some(&t("editor.export.readable_hint")), &mut readable) {
        cx.vcx.update_settings(move |s| s.editor.readable_downscale = readable);
    }

    result_card(ui, cx, params);

    heading(ui, &t("editor.export.alt_title"));
    alt_text(ui, st, cx);
}

/// What the image will be: pixel size and weight.
fn result_card(ui: &mut Ui, cx: &mut PanelCx, params: ExportParams) {
    let p = palette(ui.ctx());
    let estimate = cx.previews.latest().map(|e| (e.size, e.bytes));
    let current = cx.previews.is_current(cx.s.rev, params);
    egui::Frame::new()
        .fill(p.surface_2)
        .corner_radius(radius::CARD)
        .stroke(Stroke::new(1.0, p.border))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0;
            let dim = |ui: &mut Ui, label: String, value: String| {
                ui.horizontal(|ui| {
                    ui.label(text::dim(label));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(text::mono_text(value));
                    });
                });
            };
            let waiting = "…".to_string();
            let size = estimate.map_or(waiting.clone(), |((w, h), _)| format!("{w} × {h} px"));
            dim(ui, t("editor.export.result_size").into_owned(), size);
            let weight = match (estimate, params.format) {
                (Some((_, Some(bytes))), _) => format!("~{}", file_size(bytes)),
                _ => waiting,
            };
            dim(ui, t("editor.export.file_size").into_owned(), weight);
            if !current {
                ui.horizontal(|ui| {
                    spinner(ui);
                    ui.label(text::caption(t("editor.export.updating")));
                });
            }
        });
}

fn alt_text(ui: &mut Ui, st: &mut State, cx: &mut PanelCx) {
    let available = cx.vcx.engine.ai().available();
    match &st.alt {
        Alt::Running => {
            ui.horizontal(|ui| {
                spinner(ui);
                ui.label(text::dim(t("editor.export.alt_running")));
            });
        }
        _ => {
            ui.label(text::caption(if available { t("editor.export.alt_hint") } else { t("editor.export.alt_off") }));
            if available {
                if ui.add(Button::new(&t("editor.export.alt_make")).icon(icon::SPARKLE).kind(ButtonKind::Secondary)).clicked() {
                    st.alt = Alt::Running;
                    let (engine, doc) = (cx.vcx.engine.clone(), cx.s.doc.clone());
                    cx.vcx.spawn(move || Msg::AltText(make_alt_text(&engine, &doc)));
                }
            } else if ui.add(Button::new(&t("editor.export.alt_settings")).kind(ButtonKind::Secondary)).clicked() {
                cx.vcx.push(Command::OpenSettings { page: Some(SettingsPage::TextAi) });
            }
        }
    }
    if let Alt::Done(alt) = &st.alt {
        let p = palette(ui.ctx());
        egui::Frame::new()
            .fill(p.surface_2)
            .corner_radius(radius::CARD)
            .stroke(Stroke::new(1.0, p.border))
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(egui::Label::new(alt.as_str()).selectable(true));
            });
        if ui.add(Button::new(&t("common.copy")).icon(icon::COPY)).clicked() {
            cx.vcx.push(Command::CopyText(alt.clone()));
            cx.vcx.toast(ToastKind::Success, t("common.copied"));
        }
    }
    ui.add_space(space::SM);
}

/// Alt text for the annotated picture (without its background), from the AI endpoint.
fn make_alt_text(engine: &Engine, doc: &snap_annotate::Document) -> Result<String> {
    let image = snap_annotate::render(doc, &snap_annotate::RenderOptions { scale: 1.0, readable: false, beautify: false });
    let context: CaptureContext = doc.meta.context.clone();
    engine.alt_text(&snap_image::resize::fit(&image, Some(1600), Some(1600), true), &context)
}
