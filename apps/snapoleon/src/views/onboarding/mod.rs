//! First run: three cards (the hotkey, where files go, permissions), brand mark, progress dots and Skip.

use std::{
    any::Any,
    path::PathBuf,
    time::{Duration, Instant},
};

use egui::{Align, Context, Layout, Panel, Pos2, Rect, ScrollArea, Sense, Stroke, StrokeKind, Ui, UiBuilder, Widget, vec2};
use snap_core::{Settings, paths, pipeline::Pipeline, t};
use snap_platform::permissions::{self, Perm};
use snap_ui::{
    Palette, Tone, icon, logo, palette, radius, space, text,
    widgets::{self, Button, ButtonKind, RecorderState, badge, card_frame, hotkey_recorder, toolbar_pill},
};

use super::{
    Chrome, OpenCx, View, ViewCx,
    settings::{pipeline_label, step_label},
    window_spec,
};
use crate::commands::Command;

pub const TAG: &str = "onboarding";

/// How long a card takes to fade in.
const FADE: f32 = 0.18;
/// The mock-up toolbar on the hotkey card: its glyphs, their button size and the pill's margin plus border.
const MOCK_GLYPHS: [&str; 5] = [icon::COPY, icon::FLOPPY_DISK, icon::PENCIL_SIMPLE, icon::PUSH_PIN, icon::CLOUD_ARROW_UP];
const MOCK_BUTTON: f32 = 32.0;
const MOCK_PILL_CHROME: f32 = 14.0;
/// How often the permission status is read again while the last card is open.
const PERMISSION_POLL: Duration = Duration::from_millis(900);

pub fn open(cx: &mut OpenCx) {
    if cx.focus_tagged(TAG) {
        return;
    }
    let view = Onboarding::new(cx.settings);
    cx.open(window_spec(t("onboarding.title"), TAG, (720.0, 740.0), (640.0, 560.0)), view);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Card {
    Hotkey,
    Files,
    Permissions,
}

impl Card {
    const ALL: [Card; 3] = [Card::Hotkey, Card::Files, Card::Permissions];

    fn index(self) -> usize {
        Card::ALL.iter().position(|c| *c == self).unwrap_or(0)
    }

    fn key(self) -> &'static str {
        match self {
            Card::Hotkey => "hotkey",
            Card::Files => "files",
            Card::Permissions => "permissions",
        }
    }
}

/// The folder chosen in the native dialog.
struct Picked(Option<PathBuf>);

struct Onboarding {
    card: Card,
    shown_since: Instant,
    /// The capture hotkey as edited; synced from the settings while the recorder is idle.
    accel: String,
    recorder: RecorderState,
    recorder_focus: bool,
    granted: [bool; 3],
    polled: Instant,
    done: bool,
}

/// The permissions asked for on macOS, in order; the first is required.
const PERMISSIONS: [Perm; 3] = [Perm::ScreenRecording, Perm::Accessibility, Perm::Microphone];

impl Onboarding {
    fn new(settings: &Settings) -> Self {
        Self {
            card: Card::Hotkey,
            shown_since: Instant::now(),
            accel: capture_accel(settings),
            recorder: RecorderState::default(),
            recorder_focus: false,
            granted: PERMISSIONS.map(permissions::granted),
            polled: Instant::now(),
            done: false,
        }
    }

    fn go(&mut self, card: Card) {
        if self.card != card {
            self.card = card;
            self.shown_since = Instant::now();
        }
    }

    fn step(&mut self, delta: isize) {
        let next = (self.card.index() as isize + delta).clamp(0, Card::ALL.len() as isize - 1);
        self.go(Card::ALL[next as usize]);
    }

    /// Mark the first run as done, once, and optionally close the window.
    fn finish(&mut self, cx: &mut ViewCx, close: bool) {
        if !self.done {
            self.done = true;
            cx.update_settings(|s| s.general.first_run_done = true);
        }
        if close {
            cx.close();
        }
    }

    /// Fade-in factor of the card that was just opened (1 once settled, always 1 with reduced motion).
    fn fade(&self, ctx: &Context) -> f32 {
        if ctx.global_style().animation_time <= 0.0 {
            return 1.0;
        }
        let t = (self.shown_since.elapsed().as_secs_f32() / FADE).clamp(0.0, 1.0);
        if t < 1.0 {
            ctx.request_repaint();
        }
        1.0 - (1.0 - t).powi(3)
    }
}

/// The accelerator of the region-capture hotkey, or the first bound one.
fn capture_accel(settings: &Settings) -> String {
    settings.hotkeys.iter().find(|b| b.action == snap_core::hotkey::Action::CaptureRegion).map(|b| b.accel.clone()).unwrap_or_default()
}

/// Only macOS gates screen capture, input and the microphone behind per-app permissions.
fn wants_permissions() -> bool {
    cfg!(target_os = "macos")
}

impl View for Onboarding {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        if self.polled.elapsed() >= PERMISSION_POLL {
            self.polled = Instant::now();
            self.granted = PERMISSIONS.map(permissions::granted);
        }
        if self.card == Card::Permissions && wants_permissions() {
            ctx.request_repaint_after(PERMISSION_POLL);
        }
        if !self.recorder_focus {
            self.accel = capture_accel(cx.settings);
        }
        self.keyboard(cx, ctx);

        let title = t("onboarding.title");
        Chrome::new(&title).no_maximize().show(cx, ctx, |ui, cx| {
            let p = palette(ui.ctx());
            let footer =
                egui::Frame::new().fill(p.surface).stroke(Stroke::new(1.0, p.border)).inner_margin(egui::Margin::symmetric(24, 12));
            Panel::bottom("onboarding-footer").exact_size(64.0).resizable(false).frame(footer).show(ui, |ui| self.footer(ui, cx));
            egui::CentralPanel::default().frame(egui::Frame::new().fill(p.bg)).show(ui, |ui| {
                ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.body(ui, cx));
            });
        });
    }

    fn closed(&mut self, cx: &mut ViewCx) {
        self.finish(cx, false);
        if self.recorder_focus {
            cx.push(Command::PauseHotkeys(false));
        }
    }

    fn on_msg(&mut self, cx: &mut ViewCx, msg: Box<dyn Any + Send>) {
        if let Ok(Picked(Some(dir))) = msg.downcast::<Picked>().map(|b| *b) {
            let dir = dir.display().to_string();
            cx.update_settings(move |s| s.capture.save_dir = dir);
        }
    }
}

impl Onboarding {
    /// Enter and the arrow keys move between cards, unless the hotkey recorder owns the keyboard.
    fn keyboard(&mut self, cx: &mut ViewCx, ctx: &Context) {
        if self.recorder_focus {
            return;
        }
        let (next, back) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) || i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft),
            )
        });
        if back {
            self.step(-1);
        } else if next {
            if self.card == Card::Permissions {
                self.finish(cx, true);
            } else {
                self.step(1);
            }
        }
    }

    fn body(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        let fade = self.fade(ui.ctx());
        let width = (ui.available_width() - 2.0 * space::XL).clamp(300.0, 520.0);
        ui.vertical_centered(|ui| {
            ui.add_space(space::XL);
            ui.set_opacity(fade);
            ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::top_down(Align::Center), |ui| {
                ui.spacing_mut().item_spacing = vec2(space::SM, space::SM);
                self.hero(ui);
                ui.add_space(space::LG);
                ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::top_down(Align::Min), |ui| {
                    card_frame(ui, 24).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        match self.card {
                            Card::Hotkey => self.hotkey_card(ui, cx),
                            Card::Files => files_card(ui, cx),
                            Card::Permissions => self.permissions_card(ui, cx),
                        }
                    });
                });
            });
            ui.add_space(space::XL);
        });
    }

    fn hero(&self, ui: &mut Ui) {
        let (rect, _) = ui.allocate_exact_size(vec2(72.0, 72.0), Sense::hover());
        logo(&ui.painter_at(rect), rect);
        ui.add_space(space::XS);
        let key = self.card.key();
        let title = if self.card == Card::Permissions && !wants_permissions() { "ready" } else { key };
        ui.label(text::headline(t(&format!("onboarding.{title}.title"))));
        ui.add(egui::Label::new(text::dim(t(&format!("onboarding.{title}.body")))).halign(Align::Center));
    }

    // -----------------------------------------------------------------------------------------------------------------
    // Card 1: the hotkey

    fn hotkey_card(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        ui.label(text::body_strong(t("onboarding.hotkey.label")));
        ui.horizontal(|ui| {
            let before = self.accel.clone();
            let resp = hotkey_recorder(ui, &mut self.accel, &mut self.recorder);
            if resp.has_focus() != self.recorder_focus {
                self.recorder_focus = resp.has_focus();
                cx.push(Command::PauseHotkeys(self.recorder_focus));
            }
            if resp.changed() && self.accel != before {
                if self.accel.trim().is_empty() {
                    self.accel = before;
                } else {
                    let accel = self.accel.clone();
                    cx.update_settings(move |s| set_capture_accel(s, &accel));
                }
            }
        });
        ui.add(egui::Label::new(text::caption(t("onboarding.hotkey.change"))).wrap());
        crate::views::settings::hotkeys::system_warning(ui, cx, &self.accel);
        ui.add_space(space::MD);
        ui.separator();
        ui.add_space(space::MD);
        ui.label(text::body_strong(t("onboarding.hotkey.then")));
        ui.add(egui::Label::new(text::dim(t("onboarding.hotkey.explain"))).wrap());
        ui.add_space(space::SM);
        ui.vertical_centered(|ui| {
            // A centred layout cannot centre content of unknown width, so the row gets the pill's exact width.
            let width = MOCK_GLYPHS.len() as f32 * MOCK_BUTTON + (MOCK_GLYPHS.len() - 1) as f32 * 2.0 + MOCK_PILL_CHROME;
            ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::left_to_right(Align::Center), |ui| {
                toolbar_pill(ui, |ui| {
                    let p = Palette::of(ui);
                    for glyph in MOCK_GLYPHS {
                        let (rect, _) = ui.allocate_exact_size(vec2(MOCK_BUTTON, MOCK_BUTTON), Sense::hover());
                        widgets::util::paint_icon(ui, rect.center(), glyph, 16.0, p.text_dim);
                    }
                });
            });
        });
    }

    // -----------------------------------------------------------------------------------------------------------------
    // Card 3: permissions

    fn permissions_card(&self, ui: &mut Ui, cx: &mut ViewCx) {
        if !wants_permissions() {
            let portable = paths::is_portable();
            let mut launch = cx.settings.general.launch_at_login && !portable;
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(text::body_strong(t("onboarding.ready.launch")));
                    let hint = if portable { "onboarding.ready.portable" } else { "onboarding.ready.launch_hint" };
                    ui.label(text::caption(t(hint)));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_enabled_ui(!portable, |ui| {
                        if widgets::toggle(ui, &mut launch).changed() {
                            cx.update_settings(move |s| s.general.launch_at_login = launch);
                        }
                    });
                });
            });
            return;
        }
        for (i, perm) in PERMISSIONS.into_iter().enumerate() {
            if i > 0 {
                ui.add_space(space::SM);
            }
            permission_row(ui, perm, self.granted[i]);
        }
        ui.add_space(space::SM);
        ui.label(text::caption(t("onboarding.permissions.restart")));
    }

    // -----------------------------------------------------------------------------------------------------------------

    fn footer(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        let rect = ui.max_rect();
        let side = |left: bool| {
            let w = 220.0;
            if left {
                Rect::from_min_size(rect.min, vec2(w, rect.height()))
            } else {
                Rect::from_min_size(Pos2::new(rect.max.x - w, rect.min.y), vec2(w, rect.height()))
            }
        };

        // Progress dots.
        let p = palette(ui.ctx());
        let gap = 8.0;
        let widths = Card::ALL.map(|c| if c == self.card { 18.0 } else { 6.0 });
        let total = widths.iter().sum::<f32>() + gap * (widths.len() as f32 - 1.0);
        let mut x = rect.center().x - total / 2.0;
        for (card, width) in Card::ALL.into_iter().zip(widths) {
            let active = card == self.card;
            let dot = Rect::from_min_size(Pos2::new(x, rect.center().y - 3.0), vec2(width, 6.0));
            let resp = ui.interact(dot.expand(6.0), ui.scope_id().with(("dot", card.index())), Sense::click());
            let color = if active {
                p.accent
            } else if resp.hovered() {
                p.text_faint
            } else {
                p.border
            };
            ui.painter().rect_filled(dot, 3.0, color);
            if resp.clicked() {
                self.go(card);
            }
            x += width + gap;
        }

        let last = self.card == Card::Permissions;
        let mut left = ui.new_child(UiBuilder::new().max_rect(side(true)).layout(Layout::left_to_right(Align::Center)));
        if !last && Button::new(&t("onboarding.skip")).kind(ButtonKind::Ghost).ui(&mut left).clicked() {
            self.finish(cx, true);
        }
        let mut right = ui.new_child(UiBuilder::new().max_rect(side(false)).layout(Layout::right_to_left(Align::Center)));
        if last {
            if Button::new(&t("onboarding.finish")).kind(ButtonKind::Primary).icon(icon::ROCKET_LAUNCH).ui(&mut right).clicked() {
                self.finish(cx, true);
            }
        } else if Button::new(&t("onboarding.next")).kind(ButtonKind::Primary).min_width(88.0).ui(&mut right).clicked() {
            self.step(1);
        }
        if self.card != Card::Hotkey && Button::new(&t("onboarding.back")).kind(ButtonKind::Ghost).ui(&mut right).clicked() {
            self.step(-1);
        }
    }
}

/// Point the region-capture binding at `accel`, adding one when the action has none.
fn set_capture_accel(s: &mut Settings, accel: &str) {
    use snap_core::hotkey::{Action, HotkeyBinding};
    match s.hotkeys.iter_mut().find(|b| b.action == Action::CaptureRegion) {
        Some(binding) => {
            binding.accel = accel.to_owned();
            binding.enabled = true;
        }
        None => s.hotkeys.push(HotkeyBinding { accel: accel.to_owned(), action: Action::CaptureRegion, pipeline: None, enabled: true }),
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Card 2: where files go

fn files_card(ui: &mut Ui, cx: &mut ViewCx) {
    let p = Palette::of(ui);
    ui.label(text::body_strong(t("onboarding.files.folder")));
    let folder = paths::save_dir(&cx.settings.capture.save_dir);
    ui.horizontal(|ui| {
        egui::Frame::new()
            .fill(p.surface_2)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(radius::CARD)
            .inner_margin(egui::Margin::symmetric(12, 6))
            .show(ui, |ui| {
                ui.set_width((ui.available_width() - 110.0).max(120.0));
                ui.add(egui::Label::new(text::mono_text(folder.display().to_string())).truncate());
            });
        if widgets::secondary_button(ui, &t("onboarding.files.choose")).clicked() {
            let start = folder.clone();
            cx.spawn(move || Picked(rfd::FileDialog::new().set_directory(start).pick_folder()));
        }
    });
    ui.add_space(space::MD);
    ui.separator();
    ui.add_space(space::MD);
    ui.label(text::body_strong(t("onboarding.files.after")));
    ui.add(egui::Label::new(text::dim(t("onboarding.files.after_hint"))).wrap());
    ui.add_space(space::XS);
    let current = cx.settings.default_pipeline.clone();
    let mut chosen = None;
    for pipeline in &cx.settings.pipelines {
        if choice_row(ui, pipeline.id == current, &pipeline_label(pipeline), &summary(pipeline)).clicked() {
            chosen = Some(pipeline.id.clone());
        }
    }
    if let Some(id) = chosen {
        cx.update_settings(move |s| s.default_pipeline = id);
    }
}

/// The steps of a pipeline as one line, e.g. "Copy image, Save to disk, Notify".
fn summary(pipeline: &Pipeline) -> String {
    if pipeline.steps.is_empty() {
        return t("onboarding.files.no_steps").into_owned();
    }
    pipeline.steps.iter().map(step_label).collect::<Vec<_>>().join(", ")
}

/// A selectable row with a radio mark, a title and one dim line.
fn choice_row(ui: &mut Ui, selected: bool, title: &str, subtitle: &str) -> egui::Response {
    let p = Palette::of(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, ui.is_enabled(), selected, title));
    if ui.is_rect_visible(rect) {
        let hover = widgets::util::hover_anim(ui, &resp);
        let sel = widgets::util::anim(ui.ctx(), resp.id.with("sel"), selected, widgets::util::Motion::HOVER);
        let painter = ui.painter();
        painter.rect_filled(rect, radius::CARD, widgets::util::lerp_color(p.surface, p.surface_2, hover));
        painter.rect_filled(rect, radius::CARD, p.accent_soft.gamma_multiply(sel));
        let border = widgets::util::lerp_color(widgets::util::lerp_color(p.border, p.border_strong(), hover), p.accent, sel);
        painter.rect_stroke(rect, radius::CARD, Stroke::new(1.0, border), StrokeKind::Inside);
        let center = Pos2::new(rect.left() + 20.0, rect.center().y);
        painter.circle_stroke(center, 8.0, Stroke::new(1.5, widgets::util::lerp_color(p.text_faint, p.accent, sel)));
        painter.circle_filled(center, 4.0 * sel, p.accent);
        let title_g = widgets::util::layout(ui, title, &text::body_strong_style(), p.text);
        let sub_g = widgets::util::layout_wrapped(ui, subtitle, &text::caption_style(), p.text_dim, rect.width() - 56.0 - 12.0);
        let block = title_g.size().y + 2.0 + sub_g.size().y.min(14.0);
        let y = rect.center().y - block / 2.0;
        let title_h = title_g.size().y;
        ui.painter().galley(widgets::util::snap(ui, Pos2::new(rect.left() + 40.0, y)), title_g, p.text);
        ui.painter().with_clip_rect(rect.shrink2(vec2(0.0, 2.0))).galley(
            widgets::util::snap(ui, Pos2::new(rect.left() + 40.0, y + title_h + 2.0)),
            sub_g,
            p.text_dim,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ---------------------------------------------------------------------------------------------------------------------
// Card 3 rows

fn permission_row(ui: &mut Ui, perm: Perm, granted: bool) {
    let p = Palette::of(ui);
    let (key, glyph) = match perm {
        Perm::ScreenRecording => ("screen", icon::MONITOR),
        Perm::Accessibility => ("accessibility", icon::CURSOR),
        Perm::InputMonitoring => ("input", icon::KEYBOARD),
        Perm::Microphone => ("microphone", icon::MICROPHONE),
    };
    let required = perm == Perm::ScreenRecording;
    egui::Frame::new().fill(p.surface_2).stroke(Stroke::new(1.0, p.border)).corner_radius(radius::CARD).inner_margin(12).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (tile, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
            ui.painter().rect_filled(tile, 8.0, p.accent_soft);
            widgets::util::paint_icon(ui, tile.center(), glyph, 18.0, p.accent);
            ui.vertical(|ui| {
                ui.set_width((ui.available_width() - 130.0).max(120.0));
                ui.horizontal(|ui| {
                    ui.label(text::body_strong(t(&format!("onboarding.permissions.{key}.title"))));
                    if required {
                        badge(ui, &t("onboarding.permissions.required"), Tone::Gold);
                    }
                });
                ui.add(egui::Label::new(text::caption(t(&format!("onboarding.permissions.{key}.body")))).wrap());
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if granted {
                    badge(ui, &t("onboarding.permissions.granted"), Tone::Success);
                } else {
                    if widgets::icon_button(ui, icon::ARROW_SQUARE_OUT, &t("onboarding.permissions.open_settings"), None).clicked() {
                        permissions::open_settings(perm);
                    }
                    let kind = if required { ButtonKind::Primary } else { ButtonKind::Secondary };
                    if Button::new(&t("onboarding.permissions.allow")).kind(kind).ui(ui).clicked() {
                        permissions::request(perm);
                    }
                }
            });
        });
    });
}
