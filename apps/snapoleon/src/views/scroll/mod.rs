//! Stub for the scrolling capture HUD: a small always-on-top window next to the capture area.
//! Keep `TAG`, `ScrollInput` and the `open` signature: the app already calls them.

use egui::Context;
use snap_core::{Point, Rect, t};
use snap_shell::WindowSpec;
use snap_ui::{
    icon,
    widgets::{empty_state, secondary_button},
};

use super::{OpenCx, View, ViewCx, app_icon};

pub const TAG: &str = "scroll";

pub struct ScrollInput {
    /// The area to scroll, in virtual-desktop physical pixels.
    pub rect: Rect,
    /// Pipeline for the stitched capture; `None` = the default pipeline.
    pub pipeline: Option<String>,
}

/// Starts a scrolling capture. Ignored while one is already running.
pub fn open(cx: &mut OpenCx, input: ScrollInput) {
    if cx.focus_tagged(TAG) {
        return;
    }
    let spec = WindowSpec {
        title: t("app.stub.scroll.title").into_owned(),
        size: Some((320.0, 200.0)),
        position: Some(Point::new(input.rect.right() + 16, input.rect.y)),
        decorations: false,
        always_on_top: true,
        tool_window: true,
        exclude_from_capture: true,
        icon: Some(app_icon()),
        tag: TAG.into(),
        ..WindowSpec::default()
    };
    cx.open(spec, ScrollView);
}

struct ScrollView;

impl View for ScrollView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let mut cancel = false;
        ctx.root_ui(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                empty_state(ui, icon::ARROWS_VERTICAL, &t("app.stub.scroll.title"), &t("app.stub.body"));
                ui.vertical_centered(|ui| cancel = secondary_button(ui, &t("common.cancel")).clicked());
            });
        });
        if cancel {
            cx.close();
        }
    }
}
