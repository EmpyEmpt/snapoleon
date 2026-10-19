//! Stub for step capture: the session HUD and, afterwards, the guide editor. Keep `TAG`,
//! `StepsMsg` and the `open` signature: the app already calls them.

use std::any::Any;

use egui::Context;
use snap_core::t;
use snap_ui::{icon, widgets::empty_state};

use super::{Chrome, OpenCx, View, ViewCx, window_spec};

pub const TAG: &str = "steps";

/// Messages the app sends to a running step capture window.
pub enum StepsMsg {
    /// The step-capture hotkey was pressed while a session is running: take a shot, or finish the
    /// session when it has none pending.
    Toggle,
}

/// Starts a session. When one is already running, the window is brought forward instead.
pub fn open(cx: &mut OpenCx) {
    if cx.focus_tagged(TAG) {
        return;
    }
    cx.open(window_spec(t("app.stub.steps.title"), TAG, (720.0, 520.0), (560.0, 420.0)), StepsView);
}

struct StepsView;

impl View for StepsView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let title = t("app.stub.steps.title");
        Chrome::new(&title).show(cx, ctx, |ui, _cx| {
            ui.add_space(((ui.available_height() - 150.0) / 2.0).max(0.0));
            empty_state(ui, icon::LIST_NUMBERS, &title, &t("app.stub.body"));
        });
    }

    fn on_msg(&mut self, cx: &mut ViewCx, msg: Box<dyn Any + Send>) {
        if msg.downcast::<StepsMsg>().is_ok() {
            cx.close();
        }
    }
}
