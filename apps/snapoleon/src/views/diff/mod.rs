//! Stub for the diff view: a standard window that names itself, so the app runs end to end.
//! Keep `TAG` and the `open` signature: the rest of the app already calls them.

use egui::Context;
use snap_core::t;
use snap_ui::{icon, widgets::empty_state};

use super::{Chrome, OpenCx, View, ViewCx, window_spec};
use snap_core::RgbaImage;

pub const TAG: &str = "diff";

pub fn open(cx: &mut OpenCx, _a: Option<RgbaImage>, _b: Option<RgbaImage>) {
    cx.open(window_spec(t("app.stub.diff.title"), TAG, (1000.0, 700.0), (680.0, 460.0)), DiffView);
}

struct DiffView;

impl View for DiffView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let title = t("app.stub.diff.title");
        Chrome::new(&title).show(cx, ctx, |ui, _cx| {
            ui.add_space(((ui.available_height() - 150.0) / 2.0).max(0.0));
            empty_state(ui, icon::GIT_DIFF, &title, &t("app.stub.body"));
        });
    }
}
