//! Stub for the about view: a standard window that names itself, so the app runs end to end.
//! Keep `TAG` and the `open` signature: the rest of the app already calls them.

use egui::Context;
use snap_core::t;
use snap_ui::{icon, widgets::empty_state};

use super::{Chrome, OpenCx, View, ViewCx, window_spec};

pub const TAG: &str = "about";

pub fn open(cx: &mut OpenCx) {
    if cx.focus_tagged(TAG) {
        return;
    }
    cx.open(window_spec(t("app.stub.about.title"), TAG, (420.0, 460.0), (360.0, 400.0)), AboutView);
}

struct AboutView;

impl View for AboutView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let title = t("app.stub.about.title");
        Chrome::new(&title).no_maximize().show(cx, ctx, |ui, _cx| {
            ui.add_space(((ui.available_height() - 150.0) / 2.0).max(0.0));
            empty_state(ui, icon::INFO, &title, &t("app.stub.body"));
        });
    }
}
