//! The capture overlay: one borderless cover window per monitor over a frozen snapshot of the screen,
//! all driven by one shared session so a selection can span monitors.
//!
//! - `session`: the shared state; `flow`: what input does to it; `input`: one frame of input.
//! - `selection`: the rectangle and its drag/resize/move logic; `targets`: windows and UI elements to snap to.
//! - `paint`, `loupe`, `bars`, `measure`: drawing, the magnifier, the floating bars, the ruler.
//!
//! Everything is expressed in physical pixels of the virtual desktop; `space` converts to and from each
//! window's points, so a mixed-DPI setup needs no special cases. Keep `TAG`, `OverlayMode`, `OverlayRequest`,
//! `OverlayResult` and the `open` signature: the app calls them, and `crate::actions::overlay_commands` turns
//! an `OverlayResult` into commands.

mod bars;
mod flow;
mod input;
mod loupe;
mod measure;
mod paint;
mod selection;
mod session;
mod space;
mod targets;

use std::{any::Any, cell::RefCell, rc::Rc, time::Instant};

use egui::{Color32, ColorImage, Context, CursorIcon, LayerId, Rect, TextureHandle, TextureId, TextureOptions, pos2};
use snap_capture::CaptureOptions;
use snap_core::{Capture, Rect as PxRect, settings::VideoFormat, t};
use snap_engine::{Notice, NoticeKind};
use snap_shell::WindowSpec;
use snap_ui::palette;

use self::{
    input::Input,
    loupe::{Loupe, Readout},
    selection::{Edges, Hit},
    session::{BarKind, Finish, Session, Shared},
    space::{Space, pixel_at, rect_f},
    targets::ElementsReply,
};
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

/// Freezes the screen and opens a cover window per monitor. Ignored while an overlay is already open.
pub fn open(cx: &mut OpenCx, request: OverlayRequest) {
    if !cx.shell.windows_with_tag(TAG).is_empty() {
        return;
    }
    let started = Instant::now();
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
    tracing::info!("froze the screen in {} ms", started.elapsed().as_millis());
    let rects: Vec<snap_core::Rect> = frozen.monitors.iter().map(|(m, _)| m.rect).collect();
    let cursor = frozen.cursor;
    let mode = request.mode;
    let session: Shared = Rc::new(RefCell::new(Session::new(frozen, request, cx.settings, started)));
    let mut focus = None;
    for (monitor, rect) in rects.into_iter().enumerate() {
        // Excluded from captures so a recording or scrolling capture started from here never sees the overlay.
        let spec =
            WindowSpec { title: t("overlay.title").into_owned(), tag: TAG.into(), exclude_from_capture: true, ..WindowSpec::cover(rect) };
        let key =
            cx.open(spec, OverlayView { session: session.clone(), monitor, texture: None, loupe: Loupe::default(), had_pointer: false });
        session.borrow_mut().windows.push(key);
        if rect.contains(cursor) {
            focus = Some(key);
        }
    }
    // Key presses go to the window with focus: the one under the cursor.
    if let Some(key) = focus.or_else(|| session.borrow().windows.first().copied()) {
        cx.shell.focus(key);
    }
    if mode != OverlayMode::ColorPicker {
        // Starts the accessibility worker now so the first Ctrl-hover or element pick is not slow.
        snap_capture::warm_up();
    }
}

struct OverlayView {
    session: Shared,
    monitor: usize,
    texture: Option<TextureHandle>,
    loupe: Loupe,
    /// The pointer was in this window last frame (to hand over keyboard focus when it arrives).
    had_pointer: bool,
}

/// The monitor's snapshot as a texture. Alpha is forced opaque: a screen has none. The conversion of a few
/// million pixels is split over threads so it stays well under a frame.
fn upload(ctx: &Context, image: &snap_core::RgbaImage) -> TextureHandle {
    let size = [image.width() as usize, image.height() as usize];
    let mut pixels = vec![Color32::BLACK; size[0] * size[1]];
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get().min(8));
    let chunk = pixels.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for (n, out) in pixels.chunks_mut(chunk).enumerate() {
            let source = &image.as_raw()[n * chunk * 4..];
            scope.spawn(move || {
                for (px, rgba) in out.iter_mut().zip(source.as_chunks::<4>().0) {
                    *px = Color32::from_rgb(rgba[0], rgba[1], rgba[2]);
                }
            });
        }
    });
    ctx.load_texture("overlay-snapshot", ColorImage::new(size, pixels), TextureOptions::NEAREST)
}

impl OverlayView {
    fn finish(&self, cx: &mut ViewCx, finish: Finish) {
        let mut session = self.session.borrow_mut();
        if std::mem::replace(&mut session.done, true) {
            return;
        }
        let frozen = &session.frozen;
        let (result, region) = match finish {
            Finish::Cancel => (OverlayResult::Cancelled, None),
            Finish::Selected(rect, kind) => (OverlayResult::Selected(frozen.to_capture(rect, kind)), Some(rect)),
            Finish::Action(action, rect, kind) => (OverlayResult::Action(action, frozen.to_capture(rect, kind)), Some(rect)),
            Finish::Color(sample) => (OverlayResult::Color(sample), None),
            Finish::Measured(measurement) => (OverlayResult::Measured(measurement), None),
        };
        let mut request = session.request.clone();
        if let OverlayMode::Record(_) = request.mode {
            // The record bar may have switched the format.
            request.mode = OverlayMode::Record(session.record.format);
        }
        cx.cmds.extend(actions::overlay_commands(result, &request, cx.settings));
        if let Some(rect) = region {
            cx.update_settings(|s| s.capture.last_region = Some(rect));
        }
        for &win in &session.windows {
            cx.shell.close(win);
        }
    }

    /// Everything visible in one window for this frame. Returns how the session ended, when a bar did it.
    fn paint(&mut self, cx: &mut ViewCx, ctx: &Context, s: &mut Session, space: Space, input: &Input, tex: TextureId) -> Option<Finish> {
        let p = palette(ctx);
        let win = ctx.content_rect();
        let painter = ctx.layer_painter(LayerId::background());
        painter.image(tex, win, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);

        let mode = s.mode();
        let cursor = s.cursor.pos();
        let monitor_rect = s.frozen.monitors[self.monitor].0.rect;
        let here = cursor.is_some_and(|c| monitor_rect.contains(pixel_at(c)));
        let idle = s.sel.rect.is_none() && !s.sel.is_dragging();
        let hover = if idle && (s.selects_region() || s.picks_target()) { s.hover_target() } else { None };
        let local = |r: PxRect| space.local_rect(rect_f(r));

        if s.opts.dim && !matches!(mode, OverlayMode::ColorPicker | OverlayMode::Measure) {
            let hole = s.sel.rect.or(hover.as_ref().map(|t| t.rect));
            let color = if hole.is_some() { p.scrim } else { p.scrim.gamma_multiply(0.6) };
            paint::scrim(&painter, color, win, hole.map(local));
        }

        if let Some(r) = s.sel.rect {
            paint::selection(&painter, &p, space, local(r), true);
            if s.dominant_monitor(r) == self.monitor {
                paint::size_label(&painter, ctx, win, local(r), (r.w, r.h), None);
            }
        } else if let Some(t) = &hover {
            paint::highlight(&painter, &p, local(t.rect));
            if here {
                paint::size_label(&painter, ctx, win, local(t.rect), (t.rect.w, t.rect.h), Some(&t.label));
            }
        }

        if mode == OverlayMode::Measure {
            let target = s.hover_target();
            let rays = (s.ctrl && here).then(|| measure::rays(&s.frozen, s.point().unwrap_or_default(), target.as_ref().map(|t| t.rect)));
            let scene = measure::Scene { measure: &s.measure, hover: target.as_ref(), cursor: cursor.filter(|_| here), rays };
            measure::paint(&painter, ctx, space, win, &scene);
        }

        let guides = match mode {
            OverlayMode::ColorPicker => false,
            OverlayMode::Window | OverlayMode::Element => hover.is_none(),
            OverlayMode::Measure => s.measure.line.is_none(),
            _ => true,
        };
        if let (true, true, true, Some(c)) = (s.opts.crosshair && guides, here && !input.over_ui, idle, cursor) {
            paint::crosshair(&painter, space, win, c);
        }

        let hint_here = s.point().and_then(|p| s.monitor_at(p)).unwrap_or(0) == self.monitor;
        if !s.engaged && idle && hint_here {
            paint::hint(ctx, win, &hint_text(mode), paint::hint_icon(mode), input.pos);
        }

        bars::begin_frame(ctx);
        let mut finish = None;
        if let (Some(r), Some(kind), false) = (s.sel.rect, s.bar(), s.sel.is_dragging())
            && s.dominant_monitor(r) == self.monitor
        {
            let anchor = local(r).intersect(win);
            if anchor.width() > 0.0 && anchor.height() > 0.0 {
                let event = match kind {
                    BarKind::Instant => bars::instant(ctx, anchor),
                    BarKind::Record => bars::record(ctx, anchor, &mut s.record),
                    BarKind::Scroll => bars::scroll(ctx, anchor),
                };
                if let Some(event) = event {
                    finish = s.bar_event(cx, event);
                }
            }
        }
        if mode == OverlayMode::ColorPicker && hint_here {
            let picks = loupe::picks();
            if !picks.is_empty()
                && let Some(sample) = bars::recent_colors(ctx, win, &picks, s.format)
            {
                s.copy_recent(sample);
                finish = Some(s.pick_recent(sample));
            }
        }

        let magnifier = s.opts.magnifier || matches!(mode, OverlayMode::ColorPicker | OverlayMode::Measure);
        if let (true, Some(c)) = (here && !input.over_ui && magnifier && s.sel.dragging_edges() != Some(Edges::ALL), cursor)
            && let Some(sample) = s.frozen.pixel(pixel_at(c))
        {
            let readout = Readout {
                zoom: s.opts.zoom,
                avoid: bars::shown_rect(ctx),
                sample,
                at: pixel_at(c),
                picker: (mode == OverlayMode::ColorPicker).then_some(s.format),
                flash: s.flash.as_ref().map(|(text, _)| text.clone()),
            };
            self.loupe.paint(ctx, space, c, &s.frozen, &readout);
        }

        if !input.over_ui && input.pos.is_some() {
            ctx.set_cursor_icon(pointer_icon(s, space, input, cursor, hover.is_some()));
        }
        finish
    }
}

fn pointer_icon(s: &Session, space: Space, input: &Input, cursor: Option<egui::Pos2>, hovering_target: bool) -> CursorIcon {
    if s.sel.is_creating() {
        return CursorIcon::Crosshair;
    }
    if let Some(edges) = s.sel.dragging_edges() {
        return if edges == Edges::ALL || input.space { CursorIcon::Grabbing } else { edges.cursor() };
    }
    if s.picks_target() && hovering_target {
        return CursorIcon::PointingHand;
    }
    if s.selects_region()
        && let Some(hit) = cursor.and_then(|c| s.sel.hit(c, space.ppp))
    {
        return match hit {
            Hit::Handle(edges) => edges.cursor(),
            Hit::Inside => CursorIcon::Grab,
        };
    }
    CursorIcon::Crosshair
}

fn hint_text(mode: OverlayMode) -> String {
    t(match mode {
        OverlayMode::Region => "overlay.hint.region",
        OverlayMode::Window => "overlay.hint.window",
        OverlayMode::Element => "overlay.hint.element",
        OverlayMode::Ocr => "overlay.hint.ocr",
        OverlayMode::Codes => "overlay.hint.codes",
        OverlayMode::Translate => "overlay.hint.translate",
        OverlayMode::Pin => "overlay.hint.pin",
        OverlayMode::Record(_) => "overlay.hint.record",
        OverlayMode::Scroll => "overlay.hint.scroll",
        OverlayMode::ColorPicker => "overlay.hint.color",
        OverlayMode::Measure => "overlay.hint.measure",
    })
    .into_owned()
}

impl View for OverlayView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let session = self.session.clone();
        let mut s = session.borrow_mut();
        if s.done {
            return;
        }
        let tex = self.texture.get_or_insert_with(|| {
            ctx.all_styles_mut(|style| style.interaction.tooltip_delay = 0.25);
            let began = Instant::now();
            let texture = upload(ctx, &s.frozen.monitors[self.monitor].1);
            tracing::info!("snapshot texture built in {} ms", began.elapsed().as_millis());
            texture
        });
        let tex = tex.id();
        // Buttons grab keyboard focus when clicked; Enter and Space must keep meaning "accept" and "move".
        ctx.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });

        let space = Space::new(s.frozen.monitors[self.monitor].0.rect, ctx.pixels_per_point());
        let input = Input::read(ctx);
        let hovered = input.pos.is_some();
        if hovered && !self.had_pointer && !input.focused {
            cx.shell.focus(cx.win);
        }
        self.had_pointer = hovered;

        let win = cx.win;
        let mut finish = s.update(cx, ctx, &input, space, win);
        let from_bar = self.paint(cx, ctx, &mut s, space, &input, tex);
        finish = finish.or(from_bar);

        if input.active {
            for &sibling in s.windows.iter().filter(|&&w| w != win) {
                cx.shell.repaint(sibling);
            }
        }
        if !s.ready_logged
            && let Some(shown) = cx.shell.first_frame_time(win)
        {
            s.ready_logged = true;
            tracing::info!("overlay ready {} ms after the hotkey ({} ms to render)", s.opened.elapsed().as_millis(), shown.as_millis());
        }
        drop(s);
        if let Some(finish) = finish {
            self.finish(cx, finish);
        }
    }

    fn on_msg(&mut self, cx: &mut ViewCx, msg: Box<dyn Any + Send>) {
        if let Ok(reply) = msg.downcast::<ElementsReply>() {
            let mut s = self.session.borrow_mut();
            s.targets.apply(*reply);
            for &win in &s.windows {
                cx.shell.repaint(win);
            }
        }
    }

    fn closed(&mut self, cx: &mut ViewCx) {
        // Closed by the OS (Alt+F4): take the sibling windows down with it.
        self.finish(cx, Finish::Cancel);
    }
}
