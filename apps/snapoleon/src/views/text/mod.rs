//! Stub for the text result view: a standard window that names itself, so the app runs end to end.
//! Keep `TAG` and the `open` signature: the rest of the app already calls them.

use egui::Context;
use snap_core::t;
use snap_ui::{icon, widgets::empty_state};

use super::{Chrome, OpenCx, View, ViewCx, window_spec};

pub const TAG: &str = "text";

pub fn open(cx: &mut OpenCx, _title: String, _text: String) {
    cx.open(window_spec(t("app.stub.text.title"), TAG, (560.0, 480.0), (380.0, 280.0)), TextView);
}

struct TextView;

impl View for TextView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let title = t("app.stub.text.title");
        Chrome::new(&title).show(cx, ctx, |ui, _cx| {
            ui.add_space(((ui.available_height() - 150.0) / 2.0).max(0.0));
            empty_state(ui, icon::TEXT_AA, &title, &t("app.stub.body"));
        });
    }
}
