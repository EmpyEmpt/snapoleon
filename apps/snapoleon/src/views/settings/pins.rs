//! Pins: how pinned screenshots look.

use egui::Ui;
use snap_core::settings::PinSettings;
use snap_ui::widgets;

use super::kit::{Rows, color_well, s};

pub fn show(ui: &mut Ui, rc: &mut Rows, p: &mut PinSettings) {
    rc.section(ui, &s("pins.section.look"), |ui, rc| {
        rc.row(ui, "pins.opacity", |ui| {
            let mut percent = p.opacity * 100.0;
            if ui.add(widgets::Slider::new(&mut percent, 10.0..=100.0).step(5.0).suffix(" %").width(160.0)).changed() {
                p.opacity = percent / 100.0;
            }
        });
        rc.toggle(ui, "pins.shadow", &mut p.shadow);
        rc.row(ui, "pins.border_color", |ui| color_well(ui, &mut p.border_color, [61, 90, 254, 255], true));
    });
}
