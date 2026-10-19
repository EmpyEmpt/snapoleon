//! Stub for the library view: a standard window that names itself, so the app runs end to end.
//! Keep `TAG` and the `open` signature: the rest of the app already calls them.

use egui::Context;
use snap_core::t;
use snap_ui::{icon, widgets::empty_state};

use super::{Chrome, OpenCx, View, ViewCx, window_spec};

pub const TAG: &str = "library";

pub fn open(cx: &mut OpenCx) {
    if cx.focus_tagged(TAG) {
        return;
    }
    cx.open(window_spec(t("app.stub.library.title"), TAG, (1100.0, 720.0), (760.0, 520.0)), LibraryView);
}

struct LibraryView;

impl View for LibraryView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let title = t("app.stub.library.title");
        Chrome::new(&title).show(cx, ctx, |ui, _cx| {
            ui.add_space(((ui.available_height() - 150.0) / 2.0).max(0.0));
            empty_state(ui, icon::IMAGES, &title, &t("app.stub.body"));
        });
    }
}
