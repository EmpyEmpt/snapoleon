//! Stub for the color picker history and measure results view. Keep `TAG`, the input types and
//! the `open` signature: the overlay and the app already use them.

use std::any::Any;

use egui::Context;
use snap_core::{Point, Rect, t};
use snap_image::color;
use snap_ui::{icon, text, widgets::empty_state};

use super::{Chrome, OpenCx, View, ViewCx, window_spec};

pub const TAG: &str = "colors";

/// A pixel picked in the overlay's color picker mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorSample {
    pub rgba: [u8; 4],
    /// Virtual-desktop physical pixels.
    pub at: Point,
}

/// A measurement taken in the overlay's measure mode, in virtual-desktop physical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measurement {
    /// The dragged area or the first element.
    pub a: Rect,
    /// The second element when the user measured the gap between two elements.
    pub b: Option<Rect>,
}

pub enum ColorToolInput {
    /// Just show the history.
    Open,
    Picked(ColorSample),
    Measured(Measurement),
}

/// Opens the window, or brings it forward. A sample or measurement is delivered to the window as
/// `on_msg(Box<ColorSample>)` / `on_msg(Box<Measurement>)`.
pub fn open(cx: &mut OpenCx, input: ColorToolInput) {
    let win = match cx.shell.windows_with_tag(TAG).first() {
        Some(&win) if cx.focus_tagged(TAG) => win,
        _ => cx.open(window_spec(t("app.stub.colors.title"), TAG, (480.0, 560.0), (380.0, 420.0)), ColorsView { last: None }),
    };
    match input {
        ColorToolInput::Open => {}
        ColorToolInput::Picked(sample) => cx.post(win, sample),
        ColorToolInput::Measured(measurement) => cx.post(win, measurement),
    }
}

struct ColorsView {
    last: Option<ColorSample>,
}

impl View for ColorsView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let title = t("app.stub.colors.title");
        let last = self.last;
        Chrome::new(&title).no_maximize().show(cx, ctx, |ui, _cx| {
            ui.add_space(((ui.available_height() - 170.0) / 2.0).max(0.0));
            empty_state(ui, icon::EYEDROPPER, &title, &t("app.stub.body"));
            if let Some(sample) = last {
                ui.vertical_centered(|ui| ui.label(text::mono_text(color::hex(sample.rgba))));
            }
        });
    }

    fn on_msg(&mut self, _cx: &mut ViewCx, msg: Box<dyn Any + Send>) {
        if let Ok(sample) = msg.downcast::<ColorSample>() {
            self.last = Some(*sample);
        }
    }
}
