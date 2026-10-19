//! Stub for the video editor view: a standard window that names itself, so the app runs end to end.
//! Keep `TAG` and the `open` signature: the rest of the app already calls them.

use egui::Context;
use snap_core::t;
use snap_ui::{icon, widgets::empty_state};

use super::{Chrome, OpenCx, View, ViewCx, window_spec};
use std::path::PathBuf;

pub const TAG: &str = "video";

pub fn open(cx: &mut OpenCx, _path: PathBuf) {
    cx.open(window_spec(t("app.stub.video.title"), TAG, (960.0, 640.0), (640.0, 420.0)), VideoView);
}

struct VideoView;

impl View for VideoView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let title = t("app.stub.video.title");
        Chrome::new(&title).show(cx, ctx, |ui, _cx| {
            ui.add_space(((ui.available_height() - 150.0) / 2.0).max(0.0));
            empty_state(ui, icon::FILM_STRIP, &title, &t("app.stub.body"));
        });
    }
}
