//! Stub for the selection overlay: one cover window per monitor over a frozen snapshot. The stub
//! shows the snapshot dimmed and captures the whole monitor on click; Esc cancels.
//! Keep `TAG`, `OverlayMode`, `OverlayRequest`, `OverlayResult` and the `open` signature: the app
//! already calls them, and `crate::actions::overlay_commands` turns an `OverlayResult` into commands.

use std::{cell::RefCell, rc::Rc};

use egui::{Align2, Color32, ColorImage, Context, FontId, Key, Sense, TextureHandle, TextureOptions};
use snap_capture::{CaptureOptions, Frozen};
use snap_core::{Capture, CaptureKind, Rect, settings::VideoFormat, t};
use snap_engine::{Notice, NoticeKind};
use snap_shell::{WindowKey, WindowSpec};
use snap_ui::palette;

use super::{
    OpenCx, View, ViewCx,
    colors::{ColorSample, Measurement},
};
use crate::actions;

pub const TAG: &str = "overlay";

/// What the overlay is for. The mode decides the default toolbar and what a finished selection means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayMode {
    /// Drag a region; the instant toolbar offers the actions.
    Region,
    /// Pick a window (hover highlights, click selects).
    Window,
    /// Pick a UI element (scroll wheel walks up to parents).
    Element,
    /// Select, then copy the recognised text.
    Ocr,
    /// Select, then decode QR codes and barcodes.
    Codes,
    /// Select, then recognise and translate the text.
    Translate,
    /// Select, then pin the pixels to the screen.
    Pin,
    /// Select, then start recording that region in this format.
    Record(VideoFormat),
    /// Select, then auto-scroll and stitch.
    Scroll,
    /// Loupe and click to pick a color.
    ColorPicker,
    /// Measure distances and gaps.
    Measure,
}

#[derive(Debug, Clone)]
pub struct OverlayRequest {
    pub mode: OverlayMode,
    /// Pipeline for the finished capture; `None` = the mode's default.
    pub pipeline: Option<String>,
}

/// A button of the instant toolbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayAction {
    Copy,
    Save,
    Edit,
    Pin,
    Upload,
    /// Recognise text.
    Text,
    /// Decode QR codes and barcodes.
    Qr,
    /// Auto-redact, then edit.
    Redact,
    Record,
    Scroll,
}

/// How an overlay session ended. Hand it to `crate::actions::overlay_commands` and push the commands.
pub enum OverlayResult {
    /// Selection confirmed without a toolbar button (Enter, double click, or any non-Region mode).
    Selected(Capture),
    /// A toolbar button was pressed on this selection.
    Action(OverlayAction, Capture),
    Color(ColorSample),
    Measured(Measurement),
    Cancelled,
}

struct Session {
    frozen: Frozen,
    request: OverlayRequest,
    windows: Vec<WindowKey>,
    done: bool,
}

type Shared = Rc<RefCell<Session>>;

/// Freezes the screen and opens a cover window per monitor. Ignored while an overlay is already open.
pub fn open(cx: &mut OpenCx, request: OverlayRequest) {
    if !cx.shell.windows_with_tag(TAG).is_empty() {
        return;
    }
    let frozen = match snap_capture::freeze(&CaptureOptions::from(&cx.settings.capture)) {
        Ok(frozen) => frozen,
        Err(e) => {
            cx.notify(Notice {
                kind: NoticeKind::Error,
                title: t("app.error.capture").into_owned(),
                body: crate::notices::error_text(&e),
                ..Notice::default()
            });
            return;
        }
    };
    let rects: Vec<Rect> = frozen.monitors.iter().map(|(m, _)| m.rect).collect();
    let cursor = frozen.cursor;
    let session: Shared = Rc::new(RefCell::new(Session { frozen, request, windows: Vec::new(), done: false }));
    let mut focus = None;
    for (monitor, rect) in rects.into_iter().enumerate() {
        let spec = WindowSpec { title: t("app.stub.overlay.title").into_owned(), tag: TAG.into(), ..WindowSpec::cover(rect) };
        let key = cx.open(spec, OverlayView { session: session.clone(), monitor, texture: None });
        session.borrow_mut().windows.push(key);
        if rect.contains(cursor) {
            focus = Some(key);
        }
    }
    // Key presses (Esc) go to the window with focus: the one under the cursor.
    if let Some(key) = focus.or_else(|| session.borrow().windows.first().copied()) {
        cx.shell.focus(key);
    }
}

struct OverlayView {
    session: Shared,
    monitor: usize,
    texture: Option<TextureHandle>,
}

impl OverlayView {
    fn finish(&self, cx: &mut ViewCx, result: impl FnOnce(&Frozen, usize) -> OverlayResult) {
        let mut session = self.session.borrow_mut();
        if std::mem::replace(&mut session.done, true) {
            return;
        }
        let result = result(&session.frozen, self.monitor);
        cx.cmds.extend(actions::overlay_commands(result, &session.request, cx.settings));
        for &win in &session.windows {
            cx.shell.close(win);
        }
    }
}

impl View for OverlayView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let monitor = self.monitor;
        let texture = self.texture.get_or_insert_with(|| {
            let session = self.session.borrow();
            let image = &session.frozen.monitors[monitor].1;
            let size = [image.width() as usize, image.height() as usize];
            ctx.load_texture("overlay-snapshot", ColorImage::from_rgba_unmultiplied(size, image.as_raw()), TextureOptions::LINEAR)
        });
        let mut clicked = false;
        ctx.root_ui(|ui| {
            egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::BLACK)).show(ui, |ui| {
                let rect = ui.max_rect();
                let (_, response) = ui.allocate_exact_size(rect.size(), Sense::click());
                clicked = response.clicked();
                let p = palette(ui.ctx());
                let painter = ui.painter();
                painter.image(texture.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                painter.rect_filled(rect, 0.0, p.scrim);
                painter.text(rect.center(), Align2::CENTER_CENTER, t("app.stub.overlay.title"), FontId::proportional(28.0), p.text);
                painter.text(
                    rect.center() + egui::vec2(0.0, 30.0),
                    Align2::CENTER_CENTER,
                    t("app.stub.overlay.hint"),
                    FontId::proportional(14.0),
                    p.text_dim,
                );
            });
        });
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.finish(cx, |_, _| OverlayResult::Cancelled);
        } else if clicked || ctx.input(|i| i.key_pressed(Key::Enter)) {
            self.finish(cx, |frozen, monitor| {
                let rect = frozen.monitors[monitor].0.rect;
                OverlayResult::Selected(frozen.to_capture(rect, CaptureKind::Monitor))
            });
        }
    }

    fn closed(&mut self, cx: &mut ViewCx) {
        // Closed by the OS (Alt+F4): take the sibling windows down with it.
        self.finish(cx, |_, _| OverlayResult::Cancelled);
    }
}
