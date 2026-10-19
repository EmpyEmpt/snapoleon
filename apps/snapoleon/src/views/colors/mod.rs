//! The colors and measure tool: a history of picked colors with every format, a WCAG contrast
//! checker and the recent measurements. Picks live in the process (see `store`), so the window
//! can be closed and reopened without losing them.

mod demo;
mod panels;
mod store;

use std::{any::Any, time::Instant};

use egui::{Context, Margin, ScrollArea};
use snap_core::{Point, Rect, hotkey::Action, t};
use snap_ui::{
    icon, space,
    widgets::{Button, ButtonKind, divider, empty_state, tooltip},
};

use self::panels::{BLACK, Out, Shown, WHITE};
use super::{Chrome, OpenCx, View, ViewCx, window_spec};
use crate::commands::Command;

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
    demo::seed(&mut store::lock());
    let win = match cx.shell.windows_with_tag(TAG).first() {
        Some(&win) if cx.focus_tagged(TAG) => win,
        _ => cx.open(window_spec(t("colors.title"), TAG, (480.0, 680.0), (420.0, 460.0)), ColorsView::new()),
    };
    match input {
        ColorToolInput::Open => {}
        ColorToolInput::Picked(sample) => cx.post(win, sample),
        ColorToolInput::Measured(measurement) => cx.post(win, measurement),
    }
}

struct ColorsView {
    /// Index into the history of the color shown in the top card.
    selected: usize,
    /// The contrast checker's two colors.
    text: [u8; 4],
    background: [u8; 4],
    /// The format that was just copied, for the check mark.
    copied: Option<(String, Instant)>,
}

impl ColorsView {
    fn new() -> Self {
        let history = store::lock();
        let text = history.colors.first().map_or(BLACK, |c| c.rgba);
        let background = history.colors.get(1).map_or(WHITE, |c| c.rgba);
        Self { selected: 0, text, background, copied: None }
    }

    /// The hotkey bound to `action`, for tooltips.
    fn hotkey(cx: &ViewCx, action: Action) -> Option<String> {
        cx.settings.hotkeys.iter().find(|b| b.action == action && b.enabled).map(|b| b.accel.clone())
    }

    fn toolbar(&self, ui: &mut egui::Ui, cx: &mut ViewCx) {
        egui::Frame::new().inner_margin(Margin::symmetric(space::LG as i8, space::MD as i8)).show(ui, |ui| {
            ui.horizontal(|ui| {
                let tools = [
                    (Action::ColorPicker, icon::EYEDROPPER, "colors.pick", ButtonKind::Primary),
                    (Action::Measure, icon::RULER, "colors.measure", ButtonKind::Secondary),
                ];
                for (action, glyph, label, kind) in tools {
                    let label = t(label);
                    let resp = ui.add(Button::new(&label).icon(glyph).kind(kind));
                    let hotkey = Self::hotkey(cx, action);
                    if tooltip(resp, "", hotkey.as_deref()).clicked() {
                        cx.push(Command::StartCapture { action, pipeline: None });
                    }
                }
            });
        });
        divider(ui);
    }
}

impl View for ColorsView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let title = t("colors.title");
        let (colors, measurements) = {
            let history = store::lock();
            (history.colors.clone(), history.measurements.clone())
        };
        self.selected = self.selected.min(colors.len().saturating_sub(1));
        let mut out = Out::default();
        Chrome::new(&title).no_maximize().show(cx, ctx, |ui, cx| {
            self.toolbar(ui, cx);
            if colors.is_empty() && measurements.is_empty() {
                ui.add_space(((ui.available_height() - 140.0) / 2.0).max(0.0));
                empty_state(ui, icon::EYEDROPPER, &t("colors.empty.title"), &t("colors.empty.body"));
                return;
            }
            let mut shown = Shown {
                colors: &colors,
                measurements: &measurements,
                selected: self.selected,
                text: &mut self.text,
                background: &mut self.background,
                copied: self.copied.as_ref(),
            };
            ui.spacing_mut().item_spacing.y = 0.0;
            ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                egui::Frame::new().inner_margin(Margin::same(space::LG as i8)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = space::SM;
                    if !colors.is_empty() {
                        panels::selected(ui, &shown, &mut out);
                        panels::recent(ui, &shown, &mut out);
                        panels::contrast(ui, &mut shown, &mut out);
                    }
                    if !measurements.is_empty() {
                        panels::measurements(ui, &shown, &mut out);
                    }
                });
            });
        });
        self.apply(cx, ctx, out);
    }

    fn on_msg(&mut self, _cx: &mut ViewCx, msg: Box<dyn Any + Send>) {
        let msg = match msg.downcast::<ColorSample>() {
            Ok(sample) => {
                store::lock().add_color(*sample);
                self.selected = 0;
                if sample.rgba != self.text {
                    self.background = self.text;
                    self.text = sample.rgba;
                }
                return;
            }
            Err(msg) => msg,
        };
        if let Ok(measurement) = msg.downcast::<Measurement>() {
            store::lock().add_measurement(*measurement);
        }
    }
}

impl ColorsView {
    /// Turn what the user did in this frame into state changes and commands.
    fn apply(&mut self, cx: &mut ViewCx, ctx: &Context, out: Out) {
        if let Some(i) = out.select {
            self.selected = i;
        }
        if out.swap {
            std::mem::swap(&mut self.text, &mut self.background);
        }
        if out.clear_colors {
            store::lock().colors.clear();
            self.selected = 0;
        }
        if out.clear_measurements {
            store::lock().measurements.clear();
        }
        if let Some((key, value)) = out.copy {
            cx.push(Command::CopyText(value));
            self.copied = Some((key, Instant::now()));
            ctx.request_repaint_after(std::time::Duration::from_millis(1500));
        }
    }
}
