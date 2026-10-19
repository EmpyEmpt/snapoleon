//! Tooltips with an optional hotkey chip.

use egui::Response;

use super::chips::kbd;

/// Attaches a tooltip: `text`, followed by a `kbd` chip for `accel` when given.
pub fn tooltip(resp: Response, text: &str, accel: Option<&str>) -> Response {
    if text.is_empty() && accel.is_none() {
        return resp;
    }
    resp.on_hover_ui(|ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if !text.is_empty() {
                ui.label(text);
            }
            if let Some(accel) = accel {
                kbd(ui, accel);
            }
        });
    })
}
