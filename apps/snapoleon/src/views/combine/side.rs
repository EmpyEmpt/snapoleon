//! The right panel of the combine view: layout options and the export buttons.

use egui::{Align, Layout, Ui, Vec2};
use snap_core::t;
use snap_image::combine::Align as CellAlign;
use snap_ui::{
    Palette, icon, space, text,
    widgets::{Button, ButtonKind, Slider, color_swatches, divider, dropdown, number_field, segmented, tooltip},
};

use super::{CombineView, Mode, ViewCx};

/// Swatches offered for the background: clear, paper, white, two darks and the accent.
const BACKGROUNDS: [[u8; 4]; 6] =
    [[0, 0, 0, 0], [247, 247, 245, 255], [255, 255, 255, 255], [29, 32, 40, 255], [14, 15, 19, 255], [61, 90, 254, 255]];

fn title(ui: &mut Ui, key: &str) {
    ui.label(text::body_strong(t(key)));
}

/// A label on the left and its control on the right, 32 px tall.
fn row(ui: &mut Ui, label: &str, control: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.set_min_height(32.0);
        ui.label(label);
        ui.with_layout(Layout::right_to_left(Align::Center), control);
    });
}

impl CombineView {
    pub(super) fn side(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        let before = self.params;
        ui.spacing_mut().item_spacing = Vec2::splat(space::SM);
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            self.export(ui, cx);
            ui.add_space(space::MD);
            divider(ui);
            ui.add_space(space::MD);
            ui.with_layout(Layout::top_down(Align::Min), |ui| self.options(ui));
        });
        if self.params != before {
            self.dirty = true;
        }
    }

    fn options(&mut self, ui: &mut Ui) {
        title(ui, "combine.layout.title");
        let modes =
            [(Mode::Row, t("combine.layout.row")), (Mode::Column, t("combine.layout.column")), (Mode::Grid, t("combine.layout.grid"))];
        let options: Vec<(Mode, &str)> = modes.iter().map(|(m, l)| (*m, l.as_ref())).collect();
        segmented(ui, &mut self.params.mode, &options);
        ui.add_space(space::XS);

        if self.params.mode == Mode::Grid {
            row(ui, &t("combine.layout.columns"), |ui| {
                number_field(ui, &mut self.params.cols, 1..=8, "");
            });
        }
        ui.horizontal(|ui| {
            ui.set_min_height(24.0);
            ui.label(t("combine.layout.gap"));
        });
        let mut gap = self.params.gap as f32;
        let rail = ui.available_width() - 56.0;
        if ui.add(Slider::new(&mut gap, 0.0..=96.0).suffix(" px").width(rail)).changed() {
            self.params.gap = gap.round() as u32;
        }
        row(ui, &t("combine.layout.align"), |ui| {
            let (start, end) = match self.params.mode {
                Mode::Row => ("combine.align.top", "combine.align.bottom"),
                Mode::Column => ("combine.align.left", "combine.align.right"),
                Mode::Grid => ("combine.align.start", "combine.align.end"),
            };
            let labels = [t(start), t("combine.align.center"), t(end)];
            let options =
                [(CellAlign::Start, labels[0].as_ref()), (CellAlign::Center, labels[1].as_ref()), (CellAlign::End, labels[2].as_ref())];
            dropdown(ui, "combine-align", &mut self.params.align, &options);
        });

        ui.add_space(space::MD);
        title(ui, "combine.background.title");
        color_swatches(ui, &mut self.params.background, &BACKGROUNDS);
    }

    /// Output size and the Copy / Save / Edit buttons, anchored to the bottom of the panel.
    fn export(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        let enabled = self.can_export();
        let width = ui.available_width();
        let half = (width - space::SM) / 2.0;
        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal(|ui| {
                let save = ui.add(Button::new(&t("common.save")).icon(icon::FLOPPY_DISK).min_width(half));
                if tooltip(save, "", Some("CmdOrCtrl+S")).clicked() {
                    self.save(cx);
                }
                let edit = ui.add(Button::new(&t("combine.edit")).icon(icon::PENCIL_SIMPLE).min_width(half));
                if edit.clicked() {
                    self.edit(cx);
                }
            });
            let copy = ui.add(Button::new(&t("common.copy")).icon(icon::COPY).kind(ButtonKind::Primary).min_width(width));
            if tooltip(copy, "", Some("CmdOrCtrl+C")).clicked() {
                self.copy(cx);
            }
        });
        let p = Palette::of(ui);
        ui.add_space(space::XS);
        match (&self.preview, self.tiles.len()) {
            (_, n) if n < 2 => {
                ui.label(text::caption(t("combine.need_two")));
            }
            (Some(preview), _) => {
                ui.horizontal(|ui| {
                    ui.label(text::caption(t("combine.output")));
                    ui.label(text::mono_text(format!("{} × {}", preview.image.width(), preview.image.height())).color(p.text_dim));
                });
            }
            (None, _) => {}
        }
    }
}
