//! Screen recording: the HUD around the recorded area and the session that drives `snap_record`.
//!
//! A session is several windows that all live and die together: four thin frame windows marking the
//! area (see [`frames`]), a countdown tile before the start (see [`countdown`]) and the draggable control
//! pill (see [`hud`]), which is also the view that owns the [`Recorder`]. Every HUD window is hidden from
//! the recording, both through the window flag and through `RecordOptions::exclude_windows`.

use std::{
    any::Any,
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};

use egui::Context;
use snap_core::{CaptureContext, Error, Point, Rect, Result, settings::VideoFormat, t};
use snap_engine::{Notice, NoticeKind, RecordOptions, RecordTarget};
use snap_platform::permissions::{self, Perm};
use snap_record::{Recorder, Recording};
use snap_shell::{WindowKey, WindowSpec};

use super::{OpenCx, View, ViewCx, app_icon, settings::SettingsPage};
use crate::{commands::Command, notices::error_text};

mod countdown;
mod frames;
mod hud;

pub const TAG: &str = "recorder";
const FRAME_TAG: &str = "recorder-frame";
const COUNTDOWN_TAG: &str = "recorder-countdown";

/// Logical size of the control pill and of the error card it turns into.
const PILL_SIZE: (f32, f32) = (400.0, 48.0);
const CARD_SIZE: (f32, f32) = (420.0, 152.0);
const TILE_SIZE: f32 = 136.0;
/// Frame thickness in logical pixels.
const FRAME_THICKNESS: f32 = 2.0;
/// Gap between the recorded area and the pill, in logical pixels.
const PILL_GAP: f32 = 14.0;
/// How long the "discard?" question stays before it withdraws itself.
const DISCARD_ASK: Duration = Duration::from_secs(4);

pub struct RecorderInput {
    pub target: RecordTarget,
    pub format: VideoFormat,
}

/// Messages the app sends to the recorder window.
pub enum RecorderMsg {
    /// Finish and keep the recording (`Command::StopRecording`, the stop hotkey).
    Stop,
}

/// Starts a recording session. Ignored (with a notice) while one is already running.
pub fn open(cx: &mut OpenCx, input: RecorderInput) {
    if !cx.shell.windows_with_tag(TAG).is_empty() {
        cx.notify(Notice { kind: NoticeKind::Warning, title: t("app.recording.already").into_owned(), ..Notice::default() });
        return;
    }
    let area = target_rect(&input.target);
    let monitors = cx.shell.monitors();
    let anchor = area.map_or(Point::default(), |r| Point::new(r.x + r.w as i32 / 2, r.y + r.h as i32 / 2));
    let (screen, scale) = monitors.iter().copied().find(|(m, _)| m.contains(anchor)).or(monitors.first().copied()).unwrap_or_default();
    let options = cx.engine.record_options(input.target.clone(), input.format);
    let countdown = cx.settings.recording.countdown_secs;
    let stop_hotkey =
        cx.settings.hotkeys.iter().find(|b| b.enabled && b.action == snap_core::hotkey::Action::StopRecording).map(|b| b.accel.clone());

    let shared = Rc::new(Shared::default());
    let spec = WindowSpec {
        title: t("recorder.title").into_owned(),
        size: Some(PILL_SIZE),
        position: Some(pill_position(area, screen, scale)),
        decorations: false,
        resizable: false,
        rounded: true,
        always_on_top: true,
        tool_window: true,
        exclude_from_capture: true,
        icon: Some(app_icon()),
        tag: TAG.into(),
        ..WindowSpec::default()
    };
    let view = RecorderView {
        options,
        shared: shared.clone(),
        area,
        phase: Phase::Countdown { began: Instant::now(), secs: countdown },
        recorder: None,
        stop_hotkey,
        level: 0.0,
        discard_asked: None,
        failure: None,
        stop_when_started: false,
    };
    cx.open(spec, view);

    // The pill is the session's first window; the rest hang off it.
    if let Some(area) = area {
        for rect in frames::edges(area, scale, matches!(input.target, RecordTarget::Monitor(_))) {
            let spec = WindowSpec {
                title: t("recorder.title").into_owned(),
                exclude_from_capture: true,
                icon: Some(app_icon()),
                tag: FRAME_TAG.into(),
                ..WindowSpec::cover(rect)
            };
            let win = cx.open(spec, frames::FrameView { shared: shared.clone() });
            cx.shell.set_passthrough(win, true);
            shared.frames.borrow_mut().push(win);
        }
    }
    if countdown > 0 {
        let size = (TILE_SIZE * scale).round() as i32;
        let center = area.map_or(Point::new(screen.x + screen.w as i32 / 2, screen.y + screen.h as i32 / 2), |r| {
            Point::new(r.x + r.w as i32 / 2, r.y + r.h as i32 / 2)
        });
        let spec = WindowSpec {
            title: t("recorder.title").into_owned(),
            size: Some((TILE_SIZE, TILE_SIZE)),
            position: Some(Point::new(center.x - size / 2, center.y - size / 2)),
            decorations: false,
            resizable: false,
            rounded: true,
            always_on_top: true,
            tool_window: true,
            exclude_from_capture: true,
            icon: Some(app_icon()),
            tag: COUNTDOWN_TAG.into(),
            ..WindowSpec::default()
        };
        let win = cx.open(spec, countdown::TileView { shared: shared.clone() });
        cx.shell.set_passthrough(win, true);
        shared.tile.set(Some(win));
    }
}

/// The physical rectangle that will be recorded, when it can be found.
fn target_rect(target: &RecordTarget) -> Option<Rect> {
    match target {
        RecordTarget::Region(rect) => Some(*rect),
        RecordTarget::Monitor(index) => snap_capture::monitors().ok()?.into_iter().find(|m| m.index == *index).map(|m| m.rect),
        RecordTarget::Window(id) => snap_capture::windows().ok()?.into_iter().find(|w| w.id == *id).map(|w| w.rect),
    }
}

/// Below the recorded area, centred; above it when the monitor has no room below; inside its lower edge
/// when it fills the monitor.
fn pill_position(area: Option<Rect>, screen: Rect, scale: f32) -> Point {
    let (w, h) = ((PILL_SIZE.0 * scale).round() as i32, (PILL_SIZE.1 * scale).round() as i32);
    let gap = (PILL_GAP * scale).round() as i32;
    let Some(area) = area else {
        return Point::new(screen.x + (screen.w as i32 - w) / 2, screen.bottom() - h - 4 * gap);
    };
    let x = area.x + (area.w as i32 - w) / 2;
    let y = if area.bottom() + gap + h <= screen.bottom() - gap {
        area.bottom() + gap
    } else if area.y - gap - h >= screen.y + gap {
        area.y - gap - h
    } else {
        area.bottom() - h - 2 * gap
    };
    Point::new(x.clamp(screen.x + gap, (screen.right() - w - gap).max(screen.x + gap)), y.clamp(screen.y, screen.bottom() - h))
}

/// State the session's windows share (the pill writes, the frames and the tile read).
#[derive(Default)]
struct Shared {
    paused: Cell<bool>,
    /// While counting down: the number on display and how far into its second it is.
    count: Cell<Option<(u32, f32)>>,
    frames: RefCell<Vec<WindowKey>>,
    tile: Cell<Option<WindowKey>>,
}

enum Phase {
    Countdown {
        began: Instant,
        secs: u32,
    },
    /// `snap_record::start` is running on a worker.
    Starting,
    Recording,
    /// Stopping and finalising the file, or throwing it away.
    Saving {
        discarding: bool,
    },
    Failed,
}

/// Something that went wrong, with the way out when there is one.
struct Failure {
    message: String,
    fix: Option<Fix>,
}

#[derive(Clone, Copy)]
enum Fix {
    /// The OS permission to ask for.
    Permission(Perm),
    SettingsPage,
}

/// Results of work done off the UI thread.
enum Msg {
    Started(Result<Recorder>),
    Finished(Result<Finished>),
    Discarded,
}

struct Finished {
    recording: Recording,
    /// The file is on the clipboard (animated formats).
    copied: bool,
}

struct RecorderView {
    options: RecordOptions,
    shared: Rc<Shared>,
    /// The recorded area, for the history context.
    area: Option<Rect>,
    phase: Phase,
    recorder: Option<Recorder>,
    stop_hotkey: Option<String>,
    /// Smoothed audio level, 0..1.
    level: f32,
    discard_asked: Option<Instant>,
    failure: Option<Failure>,
    /// Stop arrived while the recorder was still starting.
    stop_when_started: bool,
}

impl RecorderView {
    fn close_session(&self, cx: &mut ViewCx) {
        for win in self.shared.frames.borrow_mut().drain(..) {
            cx.shell.close(win);
        }
        if let Some(tile) = self.shared.tile.take() {
            cx.shell.close(tile);
        }
    }

    /// The countdown is over (or there was none): ask the OS for what is needed and start on a worker.
    fn begin(&mut self, cx: &mut ViewCx) {
        if let Some(tile) = self.shared.tile.take() {
            cx.shell.close(tile);
        }
        self.shared.count.set(None);
        self.phase = Phase::Starting;
        if let Some(failure) = preflight(&self.options) {
            return self.fail(cx, failure);
        }
        let mut options = self.options.clone();
        options.exclude_windows =
            std::iter::once(cx.win).chain(self.shared.frames.borrow().iter().copied()).filter_map(|win| cx.shell.native_id(win)).collect();
        cx.spawn(move || Msg::Started(snap_record::start(options)));
    }

    fn fail(&mut self, cx: &mut ViewCx, failure: Failure) {
        tracing::warn!("recording failed: {}", failure.message);
        self.close_session(cx);
        self.phase = Phase::Failed;
        self.failure = Some(failure);
        let scale = cx.shell.scale_factor(cx.win).unwrap_or(1.0);
        if let Some(current) = cx.shell.outer_rect(cx.win) {
            let (w, h) = ((CARD_SIZE.0 * scale).round() as u32, (CARD_SIZE.1 * scale).round() as u32);
            // Grow around the pill's centre so the card stays where the user is looking.
            let x = current.x + (current.w as i32 - w as i32) / 2;
            cx.shell.set_outer_rect(cx.win, Rect::new(x, current.y + (current.h as i32 - h as i32) / 2, w, h));
        }
    }

    fn stop(&mut self, cx: &mut ViewCx) {
        match self.phase {
            Phase::Countdown { .. } | Phase::Failed => cx.close(),
            Phase::Starting => self.stop_when_started = true,
            Phase::Recording => {
                let Some(recorder) = self.recorder.take() else { return };
                self.phase = Phase::Saving { discarding: false };
                self.close_session(cx);
                let (engine, probe) = (cx.engine.clone(), self.area.map(|r| Point::new(r.x + r.w as i32 / 2, r.y + r.h as i32 / 2)));
                cx.spawn(move || Msg::Finished(finish(&engine, recorder, probe)));
            }
            Phase::Saving { .. } => {}
        }
    }

    fn discard(&mut self, cx: &mut ViewCx) {
        let Some(recorder) = self.recorder.take() else { return };
        self.phase = Phase::Saving { discarding: true };
        self.close_session(cx);
        cx.spawn(move || {
            recorder.cancel();
            Msg::Discarded
        });
    }

    fn toggle_pause(&mut self, cx: &mut ViewCx) {
        let Some(recorder) = &self.recorder else { return };
        if recorder.is_paused() {
            recorder.resume();
        } else {
            recorder.pause();
        }
        self.shared.paused.set(recorder.is_paused());
        for &frame in self.shared.frames.borrow().iter() {
            cx.shell.repaint(frame);
        }
    }

    /// Per-frame bookkeeping: the countdown, the audio level, a failure inside the recorder.
    fn tick(&mut self, cx: &mut ViewCx, ctx: &Context) {
        if let Phase::Countdown { began, secs } = self.phase {
            let elapsed = began.elapsed().as_secs_f32();
            if elapsed >= secs as f32 {
                self.begin(cx);
            } else {
                let remaining = (secs as f32 - elapsed).ceil() as u32;
                self.shared.count.set(Some((remaining, elapsed.fract())));
                if let Some(tile) = self.shared.tile.get() {
                    cx.shell.repaint(tile);
                }
                ctx.request_repaint_after(Duration::from_millis(16));
            }
        }
        if let Some(recorder) = &self.recorder {
            let raw = recorder.audio_level().clamp(0.0, 1.0).powf(0.6);
            self.level = raw.max(self.level * 0.86);
            let (failure, paused) = (recorder.error(), recorder.is_paused());
            ctx.request_repaint_after(Duration::from_millis(if paused { 250 } else { 50 }));
            if let Some(message) = failure {
                // `stop` collects the same error from the recorder and reports it.
                tracing::warn!("recorder reported: {message}");
                self.stop(cx);
            }
        }
        if self.discard_asked.is_some_and(|at| at.elapsed() >= DISCARD_ASK) {
            self.discard_asked = None;
        }
    }
}

impl View for RecorderView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        self.tick(cx, ctx);
        self.draw(cx, ctx);
    }

    fn close_requested(&mut self, cx: &mut ViewCx) -> bool {
        // Closing the pill (Alt+F4) keeps the recording rather than losing it.
        match self.phase {
            Phase::Recording => {
                self.stop(cx);
                false
            }
            Phase::Saving { .. } => false,
            _ => true,
        }
    }

    fn closed(&mut self, cx: &mut ViewCx) {
        self.close_session(cx);
        cx.push(Command::RecordingState { active: false });
    }

    fn on_msg(&mut self, cx: &mut ViewCx, msg: Box<dyn Any + Send>) {
        let msg = match msg.downcast::<RecorderMsg>() {
            Ok(_) => return self.stop(cx),
            Err(msg) => msg,
        };
        let Ok(msg) = msg.downcast::<Msg>() else { return };
        match *msg {
            Msg::Started(Ok(recorder)) => {
                self.recorder = Some(recorder);
                self.phase = Phase::Recording;
                cx.push(Command::RecordingState { active: true });
                if std::mem::take(&mut self.stop_when_started) {
                    self.stop(cx);
                }
            }
            Msg::Started(Err(e)) => {
                let failure = failure_of(&e);
                self.fail(cx, failure);
            }
            Msg::Finished(Ok(done)) => self.finished(cx, done),
            Msg::Finished(Err(e)) => {
                let failure = failure_of(&e);
                self.fail(cx, failure);
            }
            Msg::Discarded => cx.close(),
        }
    }
}

impl RecorderView {
    fn finished(&mut self, cx: &mut ViewCx, done: Finished) {
        let Finished { recording, copied } = done;
        match recording.format {
            VideoFormat::Mp4 => cx.push(Command::OpenVideoEditor { path: recording.path }),
            VideoFormat::Gif | VideoFormat::Webp => {
                let title = t(if copied { "recorder.done.copied" } else { "recorder.done.saved" }).into_owned();
                let body = recording.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                cx.notify(Notice { kind: NoticeKind::Success, title, body, file: Some(recording.path), ..Notice::default() });
            }
        }
        cx.close();
    }
}

/// Stop the recorder, record the file in history and, for animated formats, put it on the clipboard.
fn finish(engine: &snap_engine::Engine, recorder: Recorder, probe: Option<Point>) -> Result<Finished> {
    let recording = recorder.stop()?;
    let context = probe.map_or_else(CaptureContext::default, |p| engine.context_at(p));
    if let Err(e) = engine.add_recording(&recording, context) {
        tracing::warn!("recording not added to history: {e}");
    }
    let copied = recording.format != VideoFormat::Mp4
        && snap_platform::clipboard::set_files(std::slice::from_ref(&recording.path)).map_err(|e| tracing::warn!("clipboard: {e}")).is_ok();
    Ok(Finished { recording, copied })
}

/// What has to be granted before recording can start (macOS; everywhere else this is always satisfied).
fn preflight(options: &RecordOptions) -> Option<Failure> {
    let wanted = [
        (Perm::ScreenRecording, true, "record.error.screen_permission"),
        (Perm::Microphone, options.microphone, "record.error.microphone_permission"),
    ];
    wanted.into_iter().find(|(perm, needed, _)| *needed && !permissions::granted(*perm)).map(|(perm, _, message)| {
        // The first request shows the system prompt; after a denial only the settings page helps.
        permissions::request(perm);
        Failure { message: t(message).into_owned(), fix: Some(Fix::Permission(perm)) }
    })
}

/// Pair an error with the action that fixes it, when we know one.
fn failure_of(e: &Error) -> Failure {
    let message = error_text(e);
    let is = |key: &str| message == t(key);
    let fix = if matches!(e, Error::Permission(_)) {
        let perm = if is("record.error.microphone_permission") {
            Perm::Microphone
        } else if is("record.error.input_permission") {
            Perm::InputMonitoring
        } else {
            Perm::ScreenRecording
        };
        Some(Fix::Permission(perm))
    } else if ["ffmpeg_missing", "no_h264", "no_native_encoder", "wayland_unsupported", "no_decoder"]
        .iter()
        .any(|k| is(&format!("record.error.{k}")))
    {
        Some(Fix::SettingsPage)
    } else {
        None
    };
    Failure { message, fix }
}

/// Opens the place where the failure can be fixed.
fn apply_fix(cx: &mut ViewCx, fix: Fix) {
    match fix {
        Fix::Permission(perm) => permissions::open_settings(perm),
        Fix::SettingsPage => cx.push(Command::OpenSettings { page: Some(SettingsPage::Recording) }),
    }
}
