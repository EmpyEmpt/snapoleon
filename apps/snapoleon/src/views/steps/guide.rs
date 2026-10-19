//! The guide editor that opens when a step capture session finishes: an editable title and intro,
//! a reorderable list of step cards and the export / AI panel.

use std::{any::Any, time::Duration};

use egui::{Align, Context, Frame, Layout, Margin, ScrollArea, TextureHandle, Ui, Vec2};
use image::RgbaImage;
use snap_core::{Capture, CaptureContext, CaptureKind, Error, t, tf};
use snap_engine::{
    CaptureRequest,
    steps::{Guide, StepShot},
};
use snap_image::resize::thumbnail;
use snap_ui::{
    Palette, ToastKind, space, text,
    widgets::{Button, ButtonKind, danger_button, empty_state_with, secondary_button},
};

use super::{
    GUIDE_TAG,
    export::{self, Kind, Rewrite, Written},
    session::{Shot, THUMB_SIDE},
};
use crate::{
    commands::Command,
    events::AppEvent,
    views::{Chrome, OpenCx, View, ViewCx, editor::EditorInput, window_spec},
};

pub fn open(cx: &mut OpenCx, shots: Vec<Shot>) {
    let title = tf("steps.default_title", &[("date", &chrono::Local::now().format("%Y-%m-%d").to_string())]);
    open_with(cx, title, String::new(), shots);
}

/// The dev harness's finished demo guide.
pub fn open_demo(cx: &mut OpenCx, guide: Guide) {
    let shots = guide.steps.into_iter().map(Shot::new).collect();
    open_with(cx, guide.title, guide.intro, shots);
}

fn open_with(cx: &mut OpenCx, title: String, intro: String, shots: Vec<Shot>) {
    let mut view = GuideView::new(title, intro);
    for shot in shots {
        view.push(shot.shot, shot.thumb);
    }
    cx.open(window_spec(t("steps.guide.title"), GUIDE_TAG, (1120.0, 800.0), (840.0, 560.0)), view);
}

/// One step of the guide.
pub(super) struct Step {
    pub id: u64,
    pub shot: StepShot,
    /// Small copy for the card, made off the UI thread.
    pub thumb: RgbaImage,
    pub tex: Option<TextureHandle>,
}

/// What a worker thread can tell the editor.
enum Msg {
    /// A new picture for step `target`, or a new step at the end when `None`.
    Image {
        target: Option<u64>,
        image: RgbaImage,
        thumb: RgbaImage,
        context: Option<CaptureContext>,
    },
    /// The editor was closed without finishing; nothing changes.
    Unchanged,
    Failed(Error),
}

/// A long operation the panel is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Busy {
    Export(Kind),
    Ai,
    /// The window is hidden while the screen is captured.
    Capture,
}

pub(super) struct GuideView {
    pub title: String,
    pub intro: String,
    pub steps: Vec<Step>,
    next_id: u64,
    pub secs_per_step: f32,
    pub busy: Option<Busy>,
    /// The step card being dragged by its handle.
    pub dragging: Option<u64>,
    /// Edited or never exported: closing asks first.
    dirty: bool,
    confirm_close: bool,
}

impl GuideView {
    fn new(title: String, intro: String) -> Self {
        Self {
            title,
            intro,
            steps: Vec::new(),
            next_id: 1,
            secs_per_step: 3.0,
            busy: None,
            dragging: None,
            dirty: true,
            confirm_close: false,
        }
    }

    fn push(&mut self, shot: StepShot, thumb: RgbaImage) {
        self.steps.push(Step { id: self.next_id, shot, thumb, tex: None });
        self.next_id += 1;
    }

    pub(super) fn touch(&mut self) {
        self.dirty = true;
    }

    pub(super) fn remove(&mut self, id: u64) {
        self.steps.retain(|s| s.id != id);
        self.touch();
    }

    pub(super) fn move_step(&mut self, id: u64, offset: isize) {
        let Some(from) = self.steps.iter().position(|s| s.id == id) else { return };
        let to = from.saturating_add_signed(offset).min(self.steps.len() - 1);
        if to != from {
            let step = self.steps.remove(from);
            self.steps.insert(to, step);
            self.touch();
        }
    }

    /// Move `id` in front of the step that is at `before` now (`len` = the end).
    pub(super) fn move_before(&mut self, id: u64, before: usize) {
        let Some(from) = self.steps.iter().position(|s| s.id == id) else { return };
        let to = if before > from { before - 1 } else { before };
        if to != from {
            let step = self.steps.remove(from);
            self.steps.insert(to, step);
            self.touch();
        }
    }

    /// The guide as the engine exports it. Copies the images.
    pub(super) fn guide(&self) -> Guide {
        Guide { title: self.title.clone(), intro: self.intro.clone(), steps: self.steps.iter().map(|s| s.shot.clone()).collect() }
    }

    pub(super) fn start_export(&mut self, cx: &ViewCx, kind: Kind) {
        self.busy = Some(Busy::Export(kind));
        export::export(cx, kind, self.guide(), self.secs_per_step.round() as u32);
    }

    pub(super) fn start_ai(&mut self, cx: &ViewCx) {
        self.busy = Some(Busy::Ai);
        let ids = self.steps.iter().map(|s| s.id).collect();
        export::rewrite(cx, cx.engine.clone(), self.guide(), ids);
    }

    pub(super) fn copy_markdown(&self, cx: &mut ViewCx) {
        let captions: Vec<&str> = self.steps.iter().map(|s| s.shot.caption.as_str()).collect();
        cx.push(Command::CopyText(export::markdown_text(&self.title, &self.intro, &captions)));
        cx.toast(ToastKind::Success, t("steps.copied_markdown"));
    }

    /// Open the image editor on a step; the result replaces the step's picture.
    pub(super) fn annotate(&mut self, cx: &mut ViewCx, id: u64) {
        let Some(step) = self.steps.iter().find(|s| s.id == id) else { return };
        let mut capture = Capture::from_image(CaptureKind::Region, step.shot.image.clone());
        capture.context = step.shot.context.clone();
        let (reply, answer) = crossbeam_channel::bounded(1);
        cx.push(Command::OpenEditor(EditorInput { reply: Some(reply), ..EditorInput::from_capture(capture) }));
        cx.spawn(move || match answer.recv() {
            Ok(Some(done)) => Msg::Image { target: Some(id), thumb: thumbnail(&done.image, THUMB_SIDE), image: done.image, context: None },
            _ => Msg::Unchanged,
        });
    }

    /// Hide this window, count down, grab the screen and bring the window back with it. Replaces
    /// the picture of `target`, or adds a step when `None`.
    pub(super) fn capture_screen(&mut self, cx: &mut ViewCx, target: Option<u64>) {
        self.busy = Some(Busy::Capture);
        cx.shell.set_visible(cx.win, false);
        let (engine, proxy) = (cx.engine.clone(), cx.proxy.clone());
        let secs = cx.settings.capture.delay_secs.clamp(1, 10);
        cx.spawn(move || {
            for left in (1..=secs).rev() {
                let label = tf("app.countdown", &[("n", &left.to_string())]);
                proxy.send(AppEvent::JobProgress { label, frac: Some(left as f32 / secs as f32) });
                std::thread::sleep(Duration::from_secs(1));
            }
            proxy.send(AppEvent::JobProgress { label: String::new(), frac: None });
            match engine.capture(&CaptureRequest::ActiveMonitor) {
                Ok(capture) => Msg::Image {
                    target,
                    thumb: thumbnail(&capture.image, THUMB_SIDE),
                    image: capture.image,
                    context: Some(capture.context),
                },
                Err(e) => Msg::Failed(e),
            }
        });
    }

    fn apply_image(&mut self, target: Option<u64>, image: RgbaImage, thumb: RgbaImage, context: Option<CaptureContext>) {
        match target.and_then(|id| self.steps.iter_mut().find(|s| s.id == id)) {
            Some(step) => {
                // A click point only means something on the picture it was taken on.
                if step.shot.image.dimensions() != image.dimensions() || context.is_some() {
                    step.shot.click = None;
                }
                step.shot.image = image;
                step.thumb = thumb;
                step.tex = None;
                if let Some(context) = context {
                    step.shot.context = context;
                }
            }
            None => {
                let shot = StepShot {
                    image,
                    caption: String::new(),
                    click: None,
                    context: context.unwrap_or_default(),
                    taken_at_ms: snap_core::now_ms(),
                };
                self.push(shot, thumb);
            }
        }
        self.touch();
    }

    fn apply_rewrite(&mut self, rewrite: Rewrite) {
        self.title = rewrite.title;
        self.intro = rewrite.intro;
        for (id, caption) in rewrite.captions {
            if let Some(step) = self.steps.iter_mut().find(|s| s.id == id) {
                step.shot.caption = caption;
            }
        }
        self.touch();
    }

    fn body(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        let p = Palette::of(ui);
        egui::Panel::right("steps-side")
            .exact_size(312.0)
            .resizable(false)
            .frame(Frame::new().fill(p.surface).inner_margin(Margin::same(space::LG as i8)))
            .show(ui, |ui| self.side(ui, cx));
        egui::CentralPanel::default().frame(Frame::new().fill(p.bg)).show(ui, |ui| {
            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                let side = ((ui.available_width() - 780.0) / 2.0).max(space::XL);
                Frame::new()
                    .inner_margin(Margin { left: side as i8, right: side as i8, top: space::XL as i8, bottom: space::XL as i8 })
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        self.document(ui, cx);
                    });
            });
        });
    }

    fn document(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        ui.spacing_mut().item_spacing = Vec2::splat(space::SM);
        let mut edited = false;
        let faint = Palette::of(ui).text_faint;
        edited |= ui.add(doc_field(&mut self.title, &t("steps.guide.title_hint"), text::headline_style(), 1, faint)).changed();
        edited |= ui.add(doc_field(&mut self.intro, &t("steps.guide.intro_hint"), egui::TextStyle::Body, 2, faint)).changed();
        if edited {
            self.touch();
        }
        ui.add_space(space::MD);
        if self.steps.is_empty() {
            let mut add = false;
            empty_state_with(ui, snap_ui::icon::LIST_NUMBERS, &t("steps.guide.empty_title"), &t("steps.guide.empty_body"), |ui| {
                add = ui.add(Button::new(&t("steps.guide.add_step")).icon(snap_ui::icon::CAMERA).kind(ButtonKind::Primary)).clicked();
            });
            if add {
                self.capture_screen(cx, None);
            }
            return;
        }
        self.cards(ui, cx);
        ui.add_space(space::SM);
        ui.horizontal(|ui| {
            ui.add_enabled_ui(self.busy.is_none(), |ui| {
                if ui.add(Button::new(&t("steps.guide.add_step")).icon(snap_ui::icon::CAMERA)).clicked() {
                    self.capture_screen(cx, None);
                }
            });
        });
    }

    /// Ask before throwing away a guide that was never exported.
    fn confirm_close_dialog(&mut self, ctx: &Context, cx: &mut ViewCx) {
        if !self.confirm_close {
            return;
        }
        let p = snap_ui::palette(ctx);
        let frame = Frame::new()
            .fill(p.elevated)
            .stroke(egui::Stroke::new(1.0, p.border))
            .corner_radius(14.0)
            .inner_margin(Margin::same(space::XL as i8))
            .shadow(p.shadow());
        let modal = egui::Modal::new(egui::Id::unique("steps-confirm-close")).backdrop_color(p.scrim).frame(frame).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.spacing_mut().item_spacing = Vec2::splat(space::SM);
            ui.label(text::title(t("steps.close.title")));
            ui.label(text::dim(t("steps.close.body")));
            ui.add_space(space::SM);
            let mut discard = false;
            let mut keep = false;
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                discard = danger_button(ui, &t("steps.close.discard")).clicked();
                keep = secondary_button(ui, &t("steps.close.keep")).clicked();
            });
            (discard, keep)
        });
        let (discard, keep) = modal.inner;
        if discard {
            self.dirty = false;
            cx.close();
        } else if keep || modal.should_close() {
            self.confirm_close = false;
        }
    }
}

/// A borderless text field that reads like part of the document.
fn doc_field<'a>(value: &'a mut String, hint: &str, style: egui::TextStyle, rows: usize, hint_color: egui::Color32) -> egui::TextEdit<'a> {
    let edit = if rows > 1 { egui::TextEdit::multiline(value).desired_rows(rows) } else { egui::TextEdit::singleline(value) };
    let hint = egui::RichText::new(hint).text_style(style.clone()).color(hint_color);
    edit.font(style).hint_text(hint).frame(Frame::NONE).desired_width(f32::INFINITY)
}

impl View for GuideView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let title = t("steps.guide.title");
        Chrome::new(&title).show(cx, ctx, |ui, cx| self.body(ui, cx));
        self.confirm_close_dialog(ctx, cx);
    }

    fn close_requested(&mut self, _cx: &mut ViewCx) -> bool {
        if self.dirty && !self.steps.is_empty() && self.busy.is_none() {
            self.confirm_close = true;
            return false;
        }
        true
    }

    fn on_msg(&mut self, cx: &mut ViewCx, msg: Box<dyn Any + Send>) {
        let msg = match msg.downcast::<Msg>() {
            Ok(msg) => return self.on_worker(cx, *msg),
            Err(msg) => msg,
        };
        let msg = match msg.downcast::<snap_core::Result<Option<Written>>>() {
            Ok(done) => {
                self.busy = None;
                match *done {
                    Ok(Some(written)) => {
                        self.dirty = false;
                        let toast = tf("steps.saved", &[("name", &export::display_name(&written.path))]);
                        cx.toast_action(ToastKind::Success, toast, t("app.notice.show_in_folder"), Command::Reveal(written.path));
                    }
                    Ok(None) => {}
                    Err(e) => cx.toast_error(&e),
                }
                return;
            }
            Err(msg) => msg,
        };
        if let Ok(done) = msg.downcast::<snap_core::Result<Rewrite>>() {
            self.busy = None;
            match *done {
                Ok(rewrite) => self.apply_rewrite(rewrite),
                Err(e) => cx.toast_error(&e),
            }
        }
    }
}

impl GuideView {
    fn on_worker(&mut self, cx: &mut ViewCx, msg: Msg) {
        if self.busy == Some(Busy::Capture) && !matches!(msg, Msg::Unchanged) {
            self.busy = None;
            cx.shell.set_visible(cx.win, true);
            cx.shell.focus(cx.win);
        }
        match msg {
            Msg::Image { target, image, thumb, context } => self.apply_image(target, image, thumb, context),
            Msg::Unchanged => {}
            Msg::Failed(e) => cx.toast_error(&e),
        }
    }
}
