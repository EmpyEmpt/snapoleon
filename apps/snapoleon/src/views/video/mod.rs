//! The video editor: a player with a scrubber and trim handles on the left, the export panel on the right.
//!
//! Pictures come from `snap_record::edit::frame_at` on worker threads (see [`frames`]); playback shows
//! those frames as fast as they can be decoded, without sound. Exporting runs `snap_record::edit::export`
//! on a worker and offers the usual follow-ups on the result (see [`export`]).

use std::{
    any::Any,
    path::PathBuf,
    sync::{Arc, atomic::AtomicU64},
    time::{Duration, Instant},
};

use crossbeam_channel::Sender;
use egui::{
    Context, Frame, Key, Margin, Modifiers, Rect as UiRect, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Ui, Vec2, pos2,
};
use snap_core::{Result, RgbaImage, settings::VideoFormat, t, tf};
use snap_record::edit::{self, VideoInfo};
use snap_ui::{
    icon, palette, radius, text,
    widgets::{Button, ButtonKind, empty_state, icon_button, spinner, util},
};

use self::{
    export::{ExportDone, ExportForm, ExportState, UploadState},
    frames::FrameCache,
    timeline::Timeline,
};
use super::{Chrome, OpenCx, View, ViewCx, ViewHandle, window_spec};

mod export;
mod frames;
mod timeline;

pub const TAG: &str = "video";

/// Height of everything under the player: transport row, scrubber and the range readout, with gaps.
const BELOW_PLAYER: f32 = 36.0 + 12.0 + timeline::HEIGHT + 8.0 + 22.0 + 12.0;
const EXPORT_PANEL_WIDTH: f32 = 300.0;
/// Thumbnails are not re-laid out more often than this while the window is resized.
const THUMB_RELAYOUT: Duration = Duration::from_millis(400);

pub fn open(cx: &mut OpenCx, path: PathBuf) {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let title = tf("video.title_file", &[("name", &name)]);
    cx.open(
        window_spec(title.clone(), TAG, (1120.0, 720.0), (840.0, 580.0)),
        VideoView { path, title, load: Load::Probing, started: false, focused: true },
    );
}

/// Results of work done off the UI thread.
enum Msg {
    Probed(Result<VideoInfo>),
    Frame { ms: u64, image: Result<RgbaImage> },
    Thumb { generation: u64, index: usize, image: RgbaImage },
    Exported(Result<ExportDone>),
    UploadProgress(f32),
    Uploaded(Result<String>),
}

struct VideoView {
    path: PathBuf,
    title: String,
    load: Load,
    started: bool,
    /// Whether the window had focus on its last frame; finished exports only notify when it did not.
    focused: bool,
}

enum Load {
    Probing,
    Failed(String),
    Ready(Box<Editor>),
}

impl View for VideoView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        if !std::mem::replace(&mut self.started, true) {
            let path = self.path.clone();
            cx.spawn(move || Msg::Probed(edit::probe(&path)));
        }
        self.focused = ctx.input(|i| i.viewport().focused).unwrap_or(false);
        let title = self.title.clone();
        let load = &mut self.load;
        Chrome::new(&title).show(cx, ctx, |ui, cx| match load {
            Load::Probing => centered(ui, |ui| {
                spinner(ui);
            }),
            Load::Failed(message) => {
                ui.add_space(((ui.available_height() - 190.0) / 2.0).max(0.0));
                empty_state(ui, icon::FILM_STRIP, &t("video.error.title"), &t("video.error.body"));
                // What the decoder said, for the curious.
                ui.vertical_centered(|ui| {
                    ui.set_max_width(520.0);
                    ui.add(egui::Label::new(text::caption(message.clone())).wrap());
                });
            }
            Load::Ready(editor) => editor.ui(ui, cx, ctx),
        });
    }

    fn on_msg(&mut self, cx: &mut ViewCx, msg: Box<dyn Any + Send>) {
        let Ok(msg) = msg.downcast::<Msg>() else { return };
        match (*msg, &mut self.load) {
            (Msg::Probed(Ok(info)), Load::Probing) => {
                self.load = Load::Ready(Box::new(Editor::new(self.path.clone(), info, cx.handle())));
            }
            (Msg::Probed(Err(e)), Load::Probing) => self.load = Load::Failed(crate::notices::error_text(&e)),
            (Msg::Exported(result), Load::Ready(editor)) => editor.exported(cx, result, self.focused),
            (Msg::Uploaded(result), Load::Ready(editor)) => editor.uploaded(cx, result),
            (msg, Load::Ready(editor)) => editor.on_msg(msg),
            _ => {}
        }
    }
}

fn centered(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.add_space((ui.available_height() / 2.0 - 16.0).max(0.0));
    ui.vertical_centered(add);
}

struct Playback {
    started: Instant,
    from_ms: u64,
}

struct Editor {
    path: PathBuf,
    info: VideoInfo,
    playhead_ms: u64,
    /// Kept range, `(in, out)`.
    trim: (u64, u64),
    playing: Option<Playback>,

    preview: Sender<u64>,
    /// Frame (by [`Editor::key`]) the worker is decoding, and the one the playhead is on.
    in_flight: Option<u64>,
    wanted: u64,
    incoming: Vec<(u64, RgbaImage)>,
    frame_error: Option<String>,
    cache: FrameCache,
    shown: Option<TextureHandle>,

    handle: ViewHandle,
    thumbs: Vec<Option<TextureHandle>>,
    thumb_incoming: Vec<(usize, RgbaImage)>,
    thumb_generation: u64,
    thumb_current: Arc<AtomicU64>,
    thumb_laid_out: Option<Instant>,

    form: ExportForm,
    export: ExportState,
    upload: UploadState,
}

impl Editor {
    fn new(path: PathBuf, mut info: VideoInfo, handle: ViewHandle) -> Self {
        // Some containers report no usable frame rate; 30 keeps the frame maths sane.
        if !(1.0..=240.0).contains(&info.fps) {
            info.fps = 30.0;
        }
        let source = match path.extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref() {
            Some("gif") => VideoFormat::Gif,
            Some("webp") => VideoFormat::Webp,
            _ => VideoFormat::Mp4,
        };
        let preview = frames::spawn_preview(path.clone(), handle.clone());
        let mut editor = Self {
            trim: (0, info.duration_ms),
            path,
            info,
            playhead_ms: 0,
            playing: None,
            preview,
            in_flight: None,
            wanted: 0,
            incoming: Vec::new(),
            frame_error: None,
            cache: FrameCache::default(),
            shown: None,
            handle,
            thumbs: Vec::new(),
            thumb_incoming: Vec::new(),
            thumb_generation: 0,
            thumb_current: Arc::new(AtomicU64::new(0)),
            thumb_laid_out: None,
            form: ExportForm::new(source),
            export: ExportState::Idle,
            upload: UploadState::Idle,
        };
        editor.request_frame();
        editor
    }

    fn frame_ms(&self) -> f64 {
        1000.0 / f64::from(self.info.fps)
    }

    fn key(&self, ms: u64) -> u64 {
        (ms as f64 / self.frame_ms()).round() as u64
    }

    fn ms_of(&self, key: u64) -> u64 {
        ((key as f64 * self.frame_ms()).round() as u64).min(self.info.duration_ms.saturating_sub(1))
    }

    // ----- frames -----

    /// Make the picture under the playhead the one on screen, asking the worker when it is not cached.
    fn request_frame(&mut self) {
        self.wanted = self.key(self.playhead_ms);
        if let Some(texture) = self.cache.get(self.wanted) {
            self.shown = Some(texture);
            return;
        }
        self.send_request(self.wanted);
    }

    fn send_request(&mut self, key: u64) {
        if self.in_flight.is_none() && self.preview.send(self.ms_of(key)).is_ok() {
            self.in_flight = Some(key);
        }
    }

    fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Frame { ms, image } => {
                self.in_flight = None;
                let key = self.key(ms);
                match image {
                    Ok(image) => {
                        self.frame_error = None;
                        self.incoming.push((key, image));
                    }
                    Err(e) => {
                        tracing::warn!("frame at {ms} ms: {e}");
                        self.frame_error = Some(crate::notices::error_text(&e));
                        return;
                    }
                }
                // The playhead moved on while this one was decoding: go and get where it is now.
                if key != self.wanted {
                    self.send_request(self.wanted);
                }
            }
            Msg::Thumb { generation, index, image } => {
                if generation == self.thumb_generation {
                    self.thumb_incoming.push((index, image));
                }
            }
            Msg::UploadProgress(fraction) => {
                if let UploadState::Running(shown) = &mut self.upload {
                    *shown = fraction;
                }
            }
            Msg::Probed(_) | Msg::Exported(_) | Msg::Uploaded(_) => {}
        }
    }

    fn drain_incoming(&mut self, ctx: &Context) {
        for (key, image) in self.incoming.drain(..) {
            let texture = ctx.load_texture(format!("video-frame-{key}"), frames::color_image(&image), TextureOptions::LINEAR);
            if key == self.wanted || self.shown.is_none() {
                self.shown = Some(texture.clone());
            }
            self.cache.put(key, texture);
        }
        for (index, image) in self.thumb_incoming.drain(..) {
            if let Some(slot) = self.thumbs.get_mut(index) {
                *slot = Some(ctx.load_texture(format!("video-thumb-{index}"), frames::color_image(&image), TextureOptions::LINEAR));
            }
        }
    }

    /// Lay the thumbnail strip out for `width`; decoding restarts only when the count changes.
    fn layout_thumbs(&mut self, ctx: &Context, width: f32) {
        let count = timeline::thumb_count(width);
        if count == self.thumbs.len() {
            return;
        }
        if self.thumb_laid_out.is_some_and(|at| at.elapsed() < THUMB_RELAYOUT) {
            ctx.request_repaint_after(THUMB_RELAYOUT);
            return;
        }
        self.thumb_generation += 1;
        self.thumb_current.store(self.thumb_generation, std::sync::atomic::Ordering::Relaxed);
        self.thumb_laid_out = Some(Instant::now());
        self.thumbs = vec![None; count];
        frames::spawn_thumbnails(
            &self.path,
            self.handle.clone(),
            self.info.duration_ms,
            count,
            self.thumb_generation,
            self.thumb_current.clone(),
        );
    }

    // ----- transport -----

    fn seek(&mut self, ms: u64) {
        self.playhead_ms = ms.min(self.info.duration_ms);
        if self.playing.is_some() {
            self.playing = Some(Playback { started: Instant::now(), from_ms: self.playhead_ms });
        }
        self.request_frame();
    }

    fn toggle_play(&mut self) {
        if self.playing.take().is_some() {
            return;
        }
        let near_end = self.playhead_ms + self.frame_ms() as u64 >= self.trim.1;
        if near_end || self.playhead_ms < self.trim.0 {
            self.playhead_ms = self.trim.0;
        }
        self.playing = Some(Playback { started: Instant::now(), from_ms: self.playhead_ms });
        self.request_frame();
    }

    fn advance(&mut self, ctx: &Context) {
        let Some(playback) = &self.playing else { return };
        let at = playback.from_ms + playback.started.elapsed().as_millis() as u64;
        if at >= self.trim.1 {
            self.playhead_ms = self.trim.1;
            self.playing = None;
        } else {
            self.playhead_ms = at;
            ctx.request_repaint();
        }
        self.request_frame();
    }

    fn set_in(&mut self, ms: u64) {
        let max = self.trim.1.saturating_sub(100);
        self.trim.0 = ms.min(max);
    }

    fn set_out(&mut self, ms: u64) {
        self.trim.1 = ms.max(self.trim.0 + 100).min(self.info.duration_ms);
    }

    /// Shortcuts. They are taken out of the input before the widgets see it, so a button that still has
    /// keyboard focus from a click does not also fire on Space.
    fn keyboard(&mut self, ctx: &Context) {
        // Typing in a field (the custom size) must not trigger shortcuts.
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let pressed = |modifiers: Modifiers, key: Key| ctx.input_mut(|i| i.consume_key(modifiers, key));
        let frame = self.frame_ms().round() as u64;
        if pressed(Modifiers::NONE, Key::Space) {
            self.toggle_play();
        }
        for (key, forward) in [(Key::ArrowLeft, false), (Key::ArrowRight, true)] {
            let step = if pressed(Modifiers::SHIFT, key) {
                1000
            } else if pressed(Modifiers::NONE, key) {
                frame
            } else {
                continue;
            };
            self.seek(if forward { self.playhead_ms + step } else { self.playhead_ms.saturating_sub(step) });
        }
        if pressed(Modifiers::NONE, Key::Home) {
            self.seek(self.trim.0);
        }
        if pressed(Modifiers::NONE, Key::End) {
            self.seek(self.trim.1);
        }
        if pressed(Modifiers::NONE, Key::I) {
            self.set_in(self.playhead_ms);
        }
        if pressed(Modifiers::NONE, Key::O) {
            self.set_out(self.playhead_ms);
        }
    }

    // ----- layout -----

    fn ui(&mut self, ui: &mut Ui, cx: &mut ViewCx, ctx: &Context) {
        self.drain_incoming(ctx);
        self.advance(ctx);
        self.keyboard(ctx);
        let p = palette(ctx);
        let panel = egui::Panel::right("video-export")
            .exact_size(EXPORT_PANEL_WIDTH)
            .resizable(false)
            .frame(Frame::new().fill(p.surface).inner_margin(Margin::same(16)))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.set_width(EXPORT_PANEL_WIDTH - 32.0);
                    self.export_panel(ui, cx)
                });
            });
        ui.painter().vline(panel.response.rect.left() + 0.5, panel.response.rect.y_range(), Stroke::new(1.0, p.border));
        egui::CentralPanel::default()
            .frame(Frame::new().fill(p.bg).inner_margin(Margin::same(16)))
            .show(ui, |ui| self.player_column(ui, ctx));
    }

    fn player_column(&mut self, ui: &mut Ui, ctx: &Context) {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let player_height = (ui.available_height() - BELOW_PLAYER).max(140.0);
        let (rect, surface) = ui.allocate_exact_size(Vec2::new(ui.available_width(), player_height), Sense::click());
        let playing = self.playing.is_some();
        surface.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), t(if playing { "video.pause" } else { "video.play" }))
        });
        self.paint_player(ui, rect);
        if surface.clicked() {
            self.toggle_play();
        }
        ui.add_space(12.0);
        self.transport(ui);
        ui.add_space(12.0);

        self.layout_thumbs(ctx, ui.available_width());
        let events = Timeline {
            duration_ms: self.info.duration_ms,
            playhead_ms: self.playhead_ms,
            trim: self.trim,
            thumbs: &self.thumbs,
            frame_ms: self.frame_ms(),
        }
        .show(ui);
        if let Some(ms) = events.trim_in {
            self.set_in(ms);
        }
        if let Some(ms) = events.trim_out {
            self.set_out(ms);
        }
        if let Some(ms) = events.seek {
            self.seek(ms);
        }
        ui.add_space(8.0);
        self.range_readout(ui);
    }

    fn paint_player(&self, ui: &mut Ui, rect: UiRect) {
        let p = palette(ui.ctx());
        let painter = ui.painter();
        painter.rect_filled(rect, radius::CARD, p.surface_2);
        painter.rect_stroke(rect, radius::CARD, Stroke::new(1.0, p.border), StrokeKind::Inside);
        let (width, height) = (self.info.width.max(1) as f32, self.info.height.max(1) as f32);
        let inner = rect.shrink(12.0);
        let scale = (inner.width() / width).min(inner.height() / height);
        let picture = UiRect::from_center_size(rect.center(), Vec2::new(width * scale, height * scale));
        match (&self.shown, &self.frame_error) {
            (Some(texture), _) => {
                painter.image(texture.id(), picture, UiRect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), egui::Color32::WHITE);
                painter.rect_stroke(picture, 0.0, Stroke::new(1.0, p.border), StrokeKind::Outside);
            }
            (None, Some(message)) => {
                util::paint_centered(ui, rect.center(), util::layout(ui, message, &text::body_style(), p.text_dim), p.text_dim);
            }
            (None, None) => {
                let mut spinner_ui =
                    ui.new_child(egui::UiBuilder::new().max_rect(UiRect::from_center_size(rect.center(), Vec2::splat(24.0))));
                spinner(&mut spinner_ui);
            }
        }
    }

    fn transport(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.x = 8.0;
        let p = palette(ui.ctx());
        ui.horizontal(|ui| {
            let playing = self.playing.is_some();
            let (glyph, label) = if playing { (icon::PAUSE, t("video.pause")) } else { (icon::PLAY, t("video.play")) };
            let play = ui.add(Button::new(&label).kind(ButtonKind::Primary).icon(glyph).min_width(96.0));
            if snap_ui::widgets::tooltip(play, &label, Some("Space")).clicked() {
                self.toggle_play();
            }
            if icon_button(ui, icon::SKIP_BACK, &t("video.to_start"), Some("Home")).clicked() {
                self.seek(self.trim.0);
            }
            ui.label(text::mono_text(timecode(self.playhead_ms)).color(p.text));
            ui.label(text::mono_text(format!("/ {}", timecode(self.info.duration_ms))).color(p.text_faint));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.trim != (0, self.info.duration_ms)
                    && icon_button(ui, icon::ARROW_COUNTER_CLOCKWISE, &t("video.trim_reset"), None).clicked()
                {
                    self.trim = (0, self.info.duration_ms);
                }
                if icon_button(ui, icon::ARROW_LINE_LEFT, &t("video.set_out"), Some("O")).clicked() {
                    self.set_out(self.playhead_ms);
                }
                if icon_button(ui, icon::ARROW_LINE_RIGHT, &t("video.set_in"), Some("I")).clicked() {
                    self.set_in(self.playhead_ms);
                }
            });
        });
    }

    /// `In 0:01.200` left, the kept length in the middle, `Out 0:04.600` right.
    fn range_readout(&self, ui: &mut Ui) {
        let p = palette(ui.ctx());
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover());
        let mono = text::mono();
        let side = |label: String, ms: u64| format!("{label}  {}", timecode(ms));
        let left = util::layout(ui, &side(t("video.in").into_owned(), self.trim.0), &mono, p.text_dim);
        let right = util::layout(ui, &side(t("video.out").into_owned(), self.trim.1), &mono, p.text_dim);
        let middle = util::layout(ui, &format!("{}  {}", t("video.length"), timecode(self.trim.1 - self.trim.0)), &mono, p.text);
        let y = rect.center().y;
        ui.painter().galley(util::snap(ui, pos2(rect.left(), y - left.size().y / 2.0)), left, p.text_dim);
        ui.painter().galley(util::snap(ui, pos2(rect.right() - right.size().x, y - right.size().y / 2.0)), right, p.text_dim);
        ui.painter().galley(util::snap(ui, pos2(rect.center().x - middle.size().x / 2.0, y - middle.size().y / 2.0)), middle, p.text);
    }
}

/// `m:ss.mmm`, or `h:mm:ss.mmm` from the first hour on.
fn timecode(ms: u64) -> String {
    let (hours, minutes, seconds, millis) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
    if hours > 0 { format!("{hours}:{minutes:02}:{seconds:02}.{millis:03}") } else { format!("{minutes}:{seconds:02}.{millis:03}") }
}
