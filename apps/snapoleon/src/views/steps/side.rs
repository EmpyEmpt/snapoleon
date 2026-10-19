//! The right panel of the guide editor: AI rewrite and the export buttons.

use egui::{Align, Layout, Ui, Vec2};
use snap_core::{t, tf};
use snap_ui::{
    Palette, icon, space, text,
    widgets::{Button, ButtonKind, Slider, divider, ghost_button, spinner},
};

use super::{
    export::Kind,
    guide::{Busy, GuideView},
};
use crate::{
    commands::Command,
    views::{ViewCx, settings::SettingsPage},
};

fn title(ui: &mut Ui, key: &str) {
    ui.label(text::body_strong(t(key)));
}

impl GuideView {
    pub(super) fn side(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        ui.spacing_mut().item_spacing = Vec2::splat(space::SM);
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.label(text::caption(tf("steps.panel.count", &[("n", &self.steps.len().to_string())])));
            ui.add_space(space::SM);
            divider(ui);
            ui.add_space(space::MD);
            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                self.ai(ui, cx);
                ui.add_space(space::LG);
                self.export(ui, cx);
            });
        });
    }

    /// Whether the configured AI endpoint and model are set (the same test as `Ai::available`).
    fn ai_ready(cx: &ViewCx) -> bool {
        let ai = &cx.settings.ai;
        ai.enabled && !ai.endpoint.trim().is_empty() && !ai.model.trim().is_empty()
    }

    fn ai(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        title(ui, "steps.panel.ai");
        if !Self::ai_ready(cx) {
            ui.label(text::caption(t("steps.panel.ai_off")));
            if ghost_button(ui, &t("steps.panel.ai_setup")).clicked() {
                cx.push(Command::OpenSettings { page: Some(SettingsPage::TextAi) });
            }
            return;
        }
        let working = self.busy == Some(Busy::Ai);
        let width = ui.available_width();
        ui.add_enabled_ui(self.busy.is_none() && !self.steps.is_empty(), |ui| {
            let label = t(if working { "steps.panel.ai_working" } else { "steps.panel.ai_write" });
            if ui.add(Button::new(&label).icon(icon::SPARKLE).min_width(width)).clicked() {
                self.start_ai(cx);
            }
        });
        if working {
            ui.horizontal(|ui| {
                spinner(ui);
                ui.label(text::caption(t("steps.panel.ai_wait")));
            });
        } else {
            ui.label(text::caption(t("steps.panel.ai_hint")));
        }
    }

    fn export(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        title(ui, "steps.panel.export");
        let width = ui.available_width();
        let exporting = matches!(self.busy, Some(Busy::Export(_)));
        let ready = self.busy.is_none() && !self.steps.is_empty();
        ui.add_enabled_ui(ready, |ui| {
            let entries = [
                (Kind::Markdown, icon::FILE_MD, "steps.panel.markdown"),
                (Kind::Html, icon::FILE_HTML, "steps.panel.html"),
                (Kind::Video, icon::FILM_STRIP, "steps.panel.video"),
            ];
            for (kind, glyph, label) in entries {
                if ui.add(Button::new(&t(label)).icon(glyph).min_width(width)).clicked() {
                    self.start_export(cx, kind);
                }
            }
            ui.add_space(space::XS);
            ui.label(text::caption(t("steps.panel.secs")));
            let rail = ui.available_width() - 64.0;
            ui.add(Slider::new(&mut self.secs_per_step, 1.0..=10.0).step(1.0).suffix(&t("steps.panel.secs_unit")).width(rail));
            ui.add_space(space::XS);
            if ui.add(Button::new(&t("steps.panel.copy")).icon(icon::COPY).kind(ButtonKind::Ghost).min_width(width)).clicked() {
                self.copy_markdown(cx);
            }
        });
        if exporting {
            ui.horizontal(|ui| {
                spinner(ui);
                ui.label(text::caption(t("steps.panel.exporting")));
            });
        } else if self.steps.is_empty() {
            let p = Palette::of(ui);
            ui.label(text::caption(t("steps.panel.nothing")).color(p.text_dim));
        }
    }
}
