//! Stub for the combine view: a standard window that names itself, so the app runs end to end.
//! Keep `TAG` and the `open` signature: the rest of the app already calls them.

use egui::Context;
use snap_core::t;
use snap_ui::{icon, widgets::empty_state};

use super::{Chrome, OpenCx, View, ViewCx, window_spec};
use snap_core::RgbaImage;

pub const TAG: &str = "combine";

pub fn open(cx: &mut OpenCx, _images: Vec<RgbaImage>) {
    cx.open(window_spec(t("app.stub.combine.title"), TAG, (960.0, 680.0), (640.0, 460.0)), CombineView);
}

struct CombineView;

impl View for CombineView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let title = t("app.stub.combine.title");
        Chrome::new(&title).show(cx, ctx, |ui, _cx| {
            ui.add_space(((ui.available_height() - 150.0) / 2.0).max(0.0));
            empty_state(ui, icon::SQUARES_FOUR, &title, &t("app.stub.body"));
        });
    }
}
