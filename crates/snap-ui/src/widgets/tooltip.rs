//! Tooltips with an optional hotkey chip.

use egui::{Response, Role, WidgetInfo};

use super::chips::kbd;

/// Attaches a tooltip: `text`, followed by a `kbd` chip for `accel` when given.
///
/// The text also names `resp` for assistive technology when it has no name of its own: an icon-only clickable is
/// then a button called what its tooltip says, and a control with a name gets the text as its description.
pub fn tooltip(resp: Response, text: &str, accel: Option<&str>) -> Response {
    if text.is_empty() && accel.is_none() {
        return resp;
    }
    if !text.is_empty() {
        describe(&resp, text);
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

fn describe(resp: &Response, text: &str) {
    let clickable = resp.sense.senses_click();
    if !resp.ctx.has_accesskit_node(resp.id) {
        // A hand-drawn clickable without widget info: this makes it a button, with bounds and a click action.
        if clickable {
            resp.widget_info(|| WidgetInfo::labeled(Role::Button, resp.enabled(), text));
        }
        return;
    }
    resp.ctx.accesskit_node_builder(resp.id, |node| {
        if clickable && node.role() == Role::Unknown {
            node.set_role(Role::Button);
        }
        match node.label() {
            None | Some("") if node.labelled_by().is_empty() => node.set_label(text),
            Some(label) if label != text && node.description().is_none() => node.set_description(text),
            _ => {}
        }
    });
}
