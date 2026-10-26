//! The capture overlay: one borderless cover window per monitor over a frozen snapshot of the screen,
//! all driven by one shared session so a selection can span monitors.
//!
//! - `session`: the shared state; `flow`: what input does to it; `input`: one frame of input.
//! - `selection`: the rectangle and its drag/resize/move logic; `targets`: windows and UI elements to snap to.
//! - `pool`: the cover windows that stay open, hidden, between sessions so the overlay appears at once.
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
mod pool;
mod selection;
mod session;
mod space;
mod targets;

use std::{
    any::Any,
    cell::RefCell,
    rc::{Rc, Weak},
    time::Instant,
};

use egui::{Color32, ColorImage, Context, CursorIcon, LayerId, Rect, TextureHandle, TextureId, TextureOptions, pos2};
use snap_capture::CaptureOptions;
use snap_core::{Capture, Rect as PxRect, Settings, pipeline::Step, settings::VideoFormat, t};
use snap_engine::{Notice, NoticeKind};
use snap_ui::palette;

use self::{
    input::Input,
    loupe::{Loupe, Readout},
    pool::{Seat, Slot},
    selection::{Edges, Hit},
    session::{BarKind, Finish, Session, Shared},
    space::{Space, pixel_at, rect_f},
    targets::ElementsReply,
};
use super::{
    OpenCx, View, ViewCx,
    colors::{ColorSample, Measurement},
};
use crate::{actions, commands::Command};

pub const TAG: &str = "overlay";

/// Whether an overlay session is running (the covers that rest hidden between sessions do not count).
pub fn active() -> bool {
    pool::active()
}

/// End the running session as if Escape was pressed in it. For the ways out that do not go through a cover's own
/// keyboard input: the global Escape and a second press of a capture hotkey.
pub fn cancel(cx: &mut OpenCx) {
    for win in cx.shell.windows_with_tag(TAG) {
        cx.post(win, Cancel);
    }
}

/// Message to a cover: end its session (see [`cancel`]). Resting covers ignore it.
struct Cancel;

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

/// Keep the cover windows open, hidden, between sessions, and open them now. The tray app calls this once at
/// start-up; the first overlay is then as fast as every later one.
pub fn prewarm(cx: &mut OpenCx) {
    pool::enable();
    match snap_capture::monitors() {
        Ok(monitors) => {
            let began = Instant::now();
            let rects: Vec<snap_core::Rect> = monitors.iter().map(|m| m.rect).collect();
            pool::covers(cx, &rects, false);
            tracing::info!("{} overlay covers opened hidden in {} ms", rects.len(), began.elapsed().as_millis());
        }
        Err(e) => tracing::warn!("overlay covers not pre-opened: {e}"),
    }
}

/// Follow `general.instant_overlay` while the app runs: create the pool now, or release it.
pub fn set_instant(cx: &mut OpenCx, on: bool) {
    if on && !pool::enabled() {
        prewarm(cx);
    } else if !on && pool::enabled() {
        pool::disable(cx);
    }
}

/// Freezes the screen and puts a cover window over every monitor. Ignored while an overlay is already open.
pub fn open(cx: &mut OpenCx, request: OverlayRequest) {
    if pool::active() {
        return;
    }
    open_frozen(cx, request);
}

/// Freeze the screen, with the desktop icons out of the way when that is set, and open the covers.
pub fn open_frozen(cx: &mut OpenCx, request: OverlayRequest) {
    if pool::active() {
        return;
    }
    let started = Instant::now();
    let frozen = {
        let _icons = snap_engine::icons_hidden(cx.settings);
        snap_capture::freeze(&CaptureOptions::from(&cx.settings.capture))
    };
    let frozen = match frozen {
        Ok(frozen) => frozen,
        Err(e) => {
            // Without Screen Recording no capture can work. The first time, the system prompt (which also lists the app
            // in System Settings); every time, Settings > Privacy with the status and the way to fix it.
            #[cfg(target_os = "macos")]
            if matches!(e, snap_core::Error::Permission(_)) {
                snap_platform::permissions::request(snap_platform::permissions::Perm::ScreenRecording);
                cx.proxy.send(crate::events::AppEvent::Command(crate::commands::Command::OpenSettings {
                    page: Some(super::settings::SettingsPage::Privacy),
                }));
            }
            cx.notify(Notice {
                kind: NoticeKind::Error,
                title: t("app.error.capture").into_owned(),
                body: crate::notices::error_text(&e),
                permission: crate::notices::missing_permission(&e),
                ..Notice::default()
            });
            return;
        }
    };
    tracing::info!("froze the screen in {} ms", started.elapsed().as_millis());
    // The covers sit above system alerts; one that is up now (the screen-capture prompt this freeze may have raised)
    // could not be answered under them. The snapshot is only wallpaper then anyway.
    #[cfg(target_os = "macos")]
    if snap_capture::system_alert_on_screen() {
        tracing::warn!("a system alert is on screen: the overlay stays down so it can be answered");
        cx.notify(alert_notice());
        return;
    }
    let rects: Vec<snap_core::Rect> = frozen.monitors.iter().map(|(m, _)| m.rect).collect();
    let cursor = frozen.cursor;
    let mode = request.mode;
    let resting = pool::ready_for(&rects);
    let session: Shared = Rc::new(RefCell::new(Session::new(frozen, request, cx.settings, started)));
    let covers = pool::covers(cx, &rects, true);
    if !covers.iter().any(|c| cx.shell.exists(c.key)) {
        // The OS refused every window (the app already said so); there is nothing to select on.
        return;
    }
    pool::set_active(true);
    // Before a cover takes the keyboard: the app to give it back to when the session ends.
    cx.shell.remember_focus();
    let mut focus = None;
    for (monitor, cover) in covers.iter().enumerate() {
        *cover.slot.borrow_mut() = Some(Seat { session: session.clone(), monitor });
        session.borrow_mut().windows.push(cover.key);
        if resting {
            cx.shell.present(cover.key);
        }
        if cover.rect.contains(cursor) {
            focus = Some(cover.key);
        }
    }
    session.borrow_mut().presented = Instant::now();
    // Key presses go to the window with focus: the one under the cursor. It takes the focus when it appears.
    if let Some(key) = focus.or_else(|| session.borrow().windows.first().copied()) {
        cx.shell.focus(key);
    }
    if mode != OverlayMode::ColorPicker {
        // Starts the accessibility worker now so the first Ctrl-hover or element pick is not slow.
        snap_capture::warm_up();
    }
}

#[cfg(target_os = "macos")]
fn alert_notice() -> Notice {
    Notice {
        kind: NoticeKind::Warning,
        title: t("overlay.alert.title").into_owned(),
        body: t("overlay.alert.body").into_owned(),
        ..Notice::default()
    }
}

/// macOS may put its screen-capture prompt up a moment after the freeze, under the covers by then: look for one a few
/// times a second while the session is young.
#[cfg(target_os = "macos")]
fn alert_raised(s: &mut Session, ctx: &Context) -> bool {
    use std::time::Duration;
    const WATCH: Duration = Duration::from_secs(2);
    const EVERY: Duration = Duration::from_millis(250);
    if s.presented.elapsed() >= WATCH {
        return false;
    }
    ctx.request_repaint_after(EVERY);
    if s.alert_checked.elapsed() < EVERY {
        return false;
    }
    s.alert_checked = Instant::now();
    snap_capture::system_alert_on_screen()
}

/// Whether the user goes straight back to the app they were in: nothing that follows the session opens a window of
/// ours that wants the keyboard (pins and the recorder and scroll HUDs do not). The editor, the text and code windows
/// and the color tool keep it here.
fn hands_focus_back(commands: &[Command], settings: &Settings) -> bool {
    let shows_window =
        |step: &Step| matches!(step, Step::Annotate | Step::Ocr { copy: false, .. } | Step::Translate { .. } | Step::DecodeCodes { .. });
    commands.iter().all(|command| match command {
        Command::Shutter | Command::OpenPin { .. } | Command::StartRecording { .. } | Command::StartScrolling { .. } => true,
        Command::RunSteps { steps, .. } => !steps.iter().any(shows_window),
        Command::RunPipeline { pipeline, .. } => settings.pipeline(pipeline).is_some_and(|p| !p.steps.iter().any(shows_window)),
        _ => false,
    })
}

struct OverlayView {
    /// The session this window is part of right now; `None` while the cover rests hidden in the pool.
    seat: Slot,
    /// The session the texture and loupe below belong to.
    bound: Weak<RefCell<Session>>,
    monitor: usize,
    texture: Option<TextureHandle>,
    loupe: Loupe,
    /// The pointer was in this window last frame (to hand over keyboard focus when it arrives).
    had_pointer: bool,
    /// Points at the top of this monitor hidden by the camera housing, once asked for this session.
    safe_top: Option<f32>,
}

impl OverlayView {
    fn new(seat: Slot) -> Self {
        Self { seat, bound: Weak::new(), monitor: 0, texture: None, loupe: Loupe::default(), had_pointer: false, safe_top: None }
    }

    /// A resting cover paints nothing and gives its snapshot texture back.
    fn rest(&mut self, ctx: &Context) {
        self.texture = None;
        self.bound = Weak::new();
        self.loupe = Loupe::default();
        self.had_pointer = false;
        self.safe_top = None;
        ctx.root_ui(|ui| {
            egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::BLACK)).show(ui, |_| {});
        });
    }
}

/// The monitor's snapshot as a texture. Alpha is forced opaque: a screen has none. The conversion of a few
/// million pixels is split over the shared worker pool (not fresh threads: with an OpenGL driver loaded every new
/// thread costs it about 90 KB that is never returned) so it stays well under a frame.
fn upload(ctx: &Context, image: &snap_core::RgbaImage) -> TextureHandle {
    let size = [image.width() as usize, image.height() as usize];
    let mut pixels = vec![Color32::BLACK; size[0] * size[1]];
    let chunk = pixels.len().div_ceil(rayon::current_num_threads().min(8)).max(1);
    rayon::scope(|scope| {
        for (n, out) in pixels.chunks_mut(chunk).enumerate() {
            let source = &image.as_raw()[n * chunk * 4..];
            scope.spawn(move |_| {
                for (px, rgba) in out.iter_mut().zip(source.as_chunks::<4>().0) {
                    *px = Color32::from_rgb(rgba[0], rgba[1], rgba[2]);
                }
            });
        }
    });
    ctx.load_texture("overlay-snapshot", ColorImage::new(size, pixels), TextureOptions::NEAREST)
}

impl OverlayView {
    fn finish(&self, cx: &mut ViewCx, shared: &Shared, finish: Finish) {
        let mut session = shared.borrow_mut();
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
        let commands = actions::overlay_commands(result, &request, cx.settings);
        let focus_back = hands_focus_back(&commands, cx.settings);
        cx.cmds.extend(commands);
        if let Some(rect) = region {
            cx.update_settings(|s| s.capture.last_region = Some(rect));
        }
        let windows = session.windows.clone();
        drop(session);
        pool::release(shared);
        for win in windows {
            if pool::enabled() && pool::holds(win) {
                // Back to resting: hidden, with one more pass to hand the snapshot texture back.
                cx.shell.set_visible(win, false);
                cx.shell.refresh_hidden(win);
            } else {
                cx.shell.close(win);
            }
        }
        if focus_back {
            cx.shell.restore_focus();
        }
    }

    /// Everything visible in one window for this frame. Returns how the session ended, when a bar did it.
    fn paint(&mut self, cx: &mut ViewCx, ctx: &Context, s: &mut Session, space: Space, input: &Input, tex: TextureId) -> Option<Finish> {
        let p = palette(ctx);
        let win = ctx.content_rect();
        // What labels, the hint and the bars may use: the strip under a camera housing (MacBook notch) is left out.
        let safe = Rect::from_min_max(pos2(win.left(), win.top() + self.safe_top.unwrap_or(0.0)), win.max);
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
                paint::size_label(&painter, ctx, safe, local(r), (r.w, r.h), None);
            }
        } else if let Some(t) = &hover {
            paint::highlight(&painter, &p, local(t.rect));
            if here {
                paint::size_label(&painter, ctx, safe, local(t.rect), (t.rect.w, t.rect.h), Some(&t.label));
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
            paint::hint(ctx, safe, &hint_text(mode), paint::hint_icon(mode), input.pos, s.selects_region());
        }

        bars::begin_frame(ctx);
        let mut finish = None;
        if let (Some(r), Some(kind), false) = (s.sel.rect, s.bar(), s.sel.is_dragging())
            && s.dominant_monitor(r) == self.monitor
        {
            let anchor = local(r).intersect(win);
            if anchor.width() > 0.0 && anchor.height() > 0.0 {
                let event = match kind {
                    BarKind::Instant => bars::instant(ctx, anchor, safe),
                    BarKind::Record => bars::record(ctx, anchor, safe, &mut s.record),
                    BarKind::Scroll => bars::scroll(ctx, anchor, safe),
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
        let Some(Seat { session, monitor }) = self.seat.borrow().clone() else { return self.rest(ctx) };
        let mut s = session.borrow_mut();
        if s.done {
            return;
        }
        if !self.bound.ptr_eq(&Rc::downgrade(&session)) {
            // A new session on a cover that was resting: nothing of the last one may show.
            self.bound = Rc::downgrade(&session);
            self.texture = None;
            self.loupe = Loupe::default();
            self.had_pointer = false;
            self.safe_top = None;
        }
        self.monitor = monitor;
        if self.safe_top.is_none() {
            self.safe_top = Some(cx.shell.safe_area_top(cx.win));
        }
        #[cfg(target_os = "macos")]
        if alert_raised(&mut s, ctx) {
            tracing::warn!("a system alert came up over the overlay: the session ends so it can be answered");
            drop(s);
            cx.notify(alert_notice());
            return self.finish(cx, &session, Finish::Cancel);
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
            let ready = s.presented.saturating_duration_since(s.opened) + shown;
            tracing::info!("overlay ready {} ms after the hotkey ({} ms to render)", ready.as_millis(), shown.as_millis());
        }
        drop(s);
        if let Some(finish) = finish {
            self.finish(cx, &session, finish);
        }
    }

    fn on_msg(&mut self, cx: &mut ViewCx, msg: Box<dyn Any + Send>) {
        let Some(seat) = self.seat.borrow().clone() else { return };
        if msg.is::<Cancel>() {
            return self.finish(cx, &seat.session, Finish::Cancel);
        }
        if let Ok(reply) = msg.downcast::<ElementsReply>() {
            let mut s = seat.session.borrow_mut();
            if let Some(e) = s.targets.apply(*reply) {
                // No prompt while the covers are up: they would hide it. The notice's button asks, or leads to
                // System Settings.
                cx.notify(Notice {
                    kind: NoticeKind::Warning,
                    title: t("app.permission.elements").into_owned(),
                    body: crate::notices::error_text(&e),
                    permission: crate::notices::missing_permission(&e),
                    ..Notice::default()
                });
            }
            for &win in &s.windows {
                cx.shell.repaint(win);
            }
        }
    }

    fn close_requested(&mut self, cx: &mut ViewCx) -> bool {
        // Alt+F4 on a cover ends the session; a pooled cover is only hidden, never destroyed.
        let seat = self.seat.borrow().clone();
        if let Some(seat) = seat {
            self.finish(cx, &seat.session, Finish::Cancel);
        }
        !pool::enabled()
    }

    fn closed(&mut self, cx: &mut ViewCx) {
        pool::forget(cx.win);
        // Destroyed by the OS: take the sibling windows down with the session.
        let seat = self.seat.borrow().clone();
        if let Some(seat) = seat {
            self.finish(cx, &seat.session, Finish::Cancel);
        }
    }
}
