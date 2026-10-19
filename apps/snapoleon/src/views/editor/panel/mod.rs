//! The right panel: Beautify, Text (OCR) and Export.

use egui::{Frame, Margin, RichText, Ui, Vec2};
use snap_core::{RectF, t};
use snap_ui::{palette, space, text, widgets::segmented};

use super::{previews::Previews, session::Session};
use crate::views::ViewCx;

mod beautify;
mod export;
mod ocr;

pub const WIDTH: f32 = 280.0;
/// Padding between the panel's edge and its content.
pub const MARGIN: f32 = 16.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Beautify,
    Text,
    Export,
}

impl Tab {
    pub fn from_name(name: &str) -> Option<Tab> {
        match name {
            "beautify" => Some(Tab::Beautify),
            "text" => Some(Tab::Text),
            "export" => Some(Tab::Export),
            _ => None,
        }
    }
}

/// What the panel needs from the editor while it is drawn.
pub struct PanelCx<'a, 'b, 'c> {
    pub s: &'a mut Session,
    pub vcx: &'a mut ViewCx<'b, 'c>,
    pub previews: &'a mut Previews,
}

/// What the panel asks the canvas to do.
#[derive(Default)]
pub struct PanelOut {
    /// Scroll this rectangle (document pixels) into view: the recognised line the user just picked.
    pub reveal: Option<RectF>,
}

pub struct Panel {
    pub open: bool,
    pub tab: Tab,
    beautify: beautify::State,
    pub ocr: ocr::State,
    pub export: export::State,
}

impl Panel {
    pub fn new(open: bool) -> Self {
        Self { open, tab: Tab::Export, beautify: beautify::State::default(), ocr: ocr::State::default(), export: export::State::default() }
    }

    /// The frame the panel draws itself in.
    pub fn frame(ui: &Ui) -> Frame {
        let p = palette(ui.ctx());
        Frame::new().fill(p.surface).inner_margin(Margin::symmetric(MARGIN as i8, 12))
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx) -> PanelOut {
        let mut out = PanelOut::default();
        ui.spacing_mut().item_spacing = Vec2::new(space::SM, space::SM);
        let tabs =
            [(Tab::Beautify, t("editor.panel.beautify")), (Tab::Text, t("editor.panel.text")), (Tab::Export, t("editor.panel.export"))];
        let tabs: Vec<(Tab, &str)> = tabs.iter().map(|(tab, label)| (*tab, label.as_ref())).collect();
        ui.vertical_centered_justified(|ui| {
            segmented(ui, &mut self.tab, &tabs);
        });
        ui.add_space(space::XS);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let width = ui.available_width();
            ui.set_width(width);
            ui.set_max_width(width);
            match self.tab {
                Tab::Beautify => beautify::ui(ui, &mut self.beautify, cx),
                Tab::Text => out.reveal = ocr::ui(ui, &mut self.ocr, cx),
                Tab::Export => export::ui(ui, &mut self.export, cx),
            }
        });
        out
    }

    /// The recognised line to flag on the canvas, while the Text tab is showing.
    pub fn highlight(&self) -> Option<RectF> {
        (self.open && self.tab == Tab::Text).then(|| self.ocr.highlight()).flatten()
    }

    pub fn run_ocr(&mut self, s: &Session, vcx: &ViewCx) {
        self.tab = Tab::Text;
        self.ocr.recognise(s, vcx);
    }

    pub fn on_recognised(&mut self, result: snap_core::Result<snap_engine::OcrResult>, cx: &mut ViewCx) {
        self.ocr.on_recognised(result, cx);
    }

    pub fn on_translated(&mut self, result: snap_core::Result<String>, cx: &mut ViewCx) {
        self.ocr.on_translated(result, cx);
    }

    pub fn on_alt_text(&mut self, result: snap_core::Result<String>, cx: &mut ViewCx) {
        self.export.on_alt_text(result, cx);
    }
}

/// A small caption above a control.
pub fn field<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.label(text::caption(label));
        add(ui)
    })
    .inner
}

/// A centered icon, title and sentence that wrap to the panel's width, with one action under them.
pub fn empty<R>(ui: &mut Ui, glyph: &str, title: &str, body: &str, action: impl FnOnce(&mut Ui) -> R) -> R {
    let p = palette(ui.ctx());
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = space::SM;
        ui.add_space(space::LG);
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(32.0), egui::Sense::hover());
        snap_ui::widgets::util::paint_icon(ui, rect.center(), glyph, 28.0, p.text_faint);
        ui.label(text::title(title));
        ui.scope(|ui| {
            ui.set_max_width(ui.available_width());
            ui.label(text::dim(body));
        });
        ui.add_space(space::XS);
        action(ui)
    })
    .inner
}

/// A switch with a wrapping label and an optional explanation below it. Returns whether it was flipped.
pub fn switch_row(ui: &mut Ui, label: &str, hint: Option<&str>, on: &mut bool) -> bool {
    let mut flipped = false;
    ui.horizontal(|ui| {
        let text_w = (ui.available_width() - 48.0).max(80.0);
        match hint {
            // A one-line label and its toggle share a centre line.
            None => {
                ui.set_min_height(28.0);
                ui.add(egui::Label::new(label).truncate());
            }
            Some(hint) => {
                ui.vertical(|ui| {
                    ui.set_max_width(text_w);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.add(egui::Label::new(label).wrap());
                    ui.add(egui::Label::new(text::caption(hint)).wrap());
                });
            }
        }
        let align = if hint.is_some() { egui::Align::Min } else { egui::Align::Center };
        ui.with_layout(egui::Layout::right_to_left(align), |ui| {
            flipped = snap_ui::widgets::toggle(ui, on).changed();
        });
    });
    flipped
}

/// A section heading.
pub fn heading(ui: &mut Ui, label: &str) {
    ui.add_space(space::XS);
    ui.label(RichText::new(label).text_style(text::body_strong_style()));
}

/// `1.2 MB`, `482 KB`, `812 B`.
pub fn bytes_label(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{:.1} MB", b / (KB * KB))
    }
}
