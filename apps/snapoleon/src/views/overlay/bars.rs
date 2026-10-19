//! The floating bars: the instant toolbar under a finished region, the record bar (format and audio
//! before recording starts), the scroll bar, and the recent-colors strip of the picker.

use egui::{Align2, Area, Context, Id, Order, Rect, Sense, Stroke, Ui, Vec2, pos2};
use snap_core::{settings::VideoFormat, t};
use snap_ui::{
    Palette, icon,
    widgets::{
        Button, ButtonKind, chip_with_icon, floating_toolbar_anchored, icon_button, segmented, toolbar_pill, toolbar_separator, tooltip,
    },
};

use super::{
    OverlayAction,
    loupe::{ColorFormat, color32},
};
use crate::views::colors::ColorSample;

/// What a bar reports.
pub enum BarEvent {
    Action(OverlayAction),
    /// The bar's main button: start recording / start scrolling.
    Confirm,
    Close,
    /// An audio toggle of the record bar changed.
    Audio,
}

/// One instant-toolbar button.
pub struct ActionSpec {
    pub action: OverlayAction,
    icon: &'static str,
    label: &'static str,
    pub hotkey: &'static str,
}

const fn spec(action: OverlayAction, icon: &'static str, label: &'static str, hotkey: &'static str) -> ActionSpec {
    ActionSpec { action, icon, label, hotkey }
}

/// `Copy · Save · Edit · Pin · Upload | Text · QR · Redact | Record · Scroll`, as DESIGN.md lays it out.
pub const GROUPS: [&[ActionSpec]; 3] = [
    &[
        spec(OverlayAction::Copy, icon::COPY, "overlay.action.copy", "CmdOrCtrl+C"),
        spec(OverlayAction::Save, icon::FLOPPY_DISK, "overlay.action.save", "CmdOrCtrl+S"),
        spec(OverlayAction::Edit, icon::PENCIL_SIMPLE, "overlay.action.edit", "E"),
        spec(OverlayAction::Pin, icon::PUSH_PIN, "overlay.action.pin", "P"),
        spec(OverlayAction::Upload, icon::CLOUD_ARROW_UP, "overlay.action.upload", "U"),
    ],
    &[
        spec(OverlayAction::Text, icon::TEXT_T, "overlay.action.text", "T"),
        spec(OverlayAction::Qr, icon::QR_CODE, "overlay.action.qr", "Q"),
        spec(OverlayAction::Redact, icon::EYE_SLASH, "overlay.action.redact", "R"),
    ],
    &[
        spec(OverlayAction::Record, icon::RECORD, "overlay.action.record", "Shift+R"),
        spec(OverlayAction::Scroll, icon::SCROLL, "overlay.action.scroll", "S"),
    ],
];

/// Format and audio choices of the record bar.
#[derive(Clone, Copy)]
pub struct RecordOptions {
    pub format: VideoFormat,
    pub microphone: bool,
    pub system_audio: bool,
}

fn rect_id() -> Id {
    Id::unique("snap-bar-rect")
}

/// Forget last frame's bar position; call at the start of each frame before showing bars.
pub fn begin_frame(ctx: &Context) {
    ctx.data_mut(|d| d.remove::<Rect>(rect_id()));
}

/// Where a bar was shown this frame, so the loupe can stay clear of it.
pub fn shown_rect(ctx: &Context) -> Option<Rect> {
    ctx.data(|d| d.get_temp(rect_id()))
}

/// A bar under (or over) `anchor`, remembering where it ended up.
fn bar(ctx: &Context, name: &str, anchor: Rect, add: impl FnOnce(&mut Ui) -> Option<BarEvent>) -> Option<BarEvent> {
    let out = floating_toolbar_anchored(ctx, Id::unique(name), anchor, ctx.content_rect(), add);
    ctx.data_mut(|d| d.insert_temp(rect_id(), out.response.rect));
    out.inner
}

pub fn instant(ctx: &Context, anchor: Rect) -> Option<BarEvent> {
    bar(ctx, "snap-bar-instant", anchor, |ui| {
        let mut event = None;
        for group in GROUPS {
            for item in group.iter() {
                if icon_button(ui, item.icon, &t(item.label), Some(item.hotkey)).clicked() {
                    event = Some(BarEvent::Action(item.action));
                }
            }
            toolbar_separator(ui);
        }
        if icon_button(ui, icon::X, &t("overlay.action.close"), Some("Esc")).clicked() {
            event = Some(BarEvent::Close);
        }
        event
    })
}

pub fn record(ctx: &Context, anchor: Rect, options: &mut RecordOptions) -> Option<BarEvent> {
    bar(ctx, "snap-bar-record", anchor, |ui| {
        let mut event = None;
        segmented(ui, &mut options.format, &[(VideoFormat::Mp4, "MP4"), (VideoFormat::Gif, "GIF"), (VideoFormat::Webp, "WebP")]);
        toolbar_separator(ui);
        // Only MP4 carries sound.
        ui.add_enabled_ui(options.format == VideoFormat::Mp4, |ui| {
            let mic = chip_with_icon(
                ui,
                icon::MICROPHONE,
                &t("overlay.record.microphone"),
                options.microphone && options.format == VideoFormat::Mp4,
            );
            if mic.clicked() {
                options.microphone = !options.microphone;
                event = Some(BarEvent::Audio);
            }
            let system = chip_with_icon(
                ui,
                icon::SPEAKER_HIGH,
                &t("overlay.record.system_audio"),
                options.system_audio && options.format == VideoFormat::Mp4,
            );
            if system.clicked() {
                options.system_audio = !options.system_audio;
                event = Some(BarEvent::Audio);
            }
        });
        toolbar_separator(ui);
        if ui.add(Button::new(&t("overlay.record.start")).kind(ButtonKind::Danger).icon(icon::RECORD)).clicked() {
            event = Some(BarEvent::Confirm);
        }
        if icon_button(ui, icon::X, &t("overlay.action.close"), Some("Esc")).clicked() {
            event = Some(BarEvent::Close);
        }
        event
    })
}

pub fn scroll(ctx: &Context, anchor: Rect) -> Option<BarEvent> {
    bar(ctx, "snap-bar-scroll", anchor, |ui| {
        let mut event = None;
        if ui.add(Button::new(&t("overlay.scroll.start")).kind(ButtonKind::Primary).icon(icon::SCROLL)).clicked() {
            event = Some(BarEvent::Confirm);
        }
        if icon_button(ui, icon::X, &t("overlay.action.close"), Some("Esc")).clicked() {
            event = Some(BarEvent::Close);
        }
        event
    })
}

/// The colours picked recently, as a strip at the bottom of the window. Returns the clicked one.
pub fn recent_colors(ctx: &Context, win: Rect, picks: &[ColorSample], format: ColorFormat) -> Option<ColorSample> {
    let mut clicked = None;
    Area::new(Id::unique("snap-bar-colors"))
        .order(Order::Foreground)
        .pivot(Align2::CENTER_BOTTOM)
        .fixed_pos(pos2(win.center().x, win.bottom() - 32.0))
        .constrain(true)
        .show(ctx, |ui| {
            toolbar_pill(ui, |ui| {
                let p = Palette::of(ui);
                ui.add_space(8.0);
                ui.label(snap_ui::text::caption(t("overlay.picker.recent")));
                ui.add_space(6.0);
                for sample in picks {
                    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::click());
                    let hover = resp.hovered();
                    ui.painter().circle_filled(rect.center(), 9.0, color32(sample.rgba));
                    ui.painter().circle_stroke(rect.center(), 9.0, Stroke::new(1.0, p.border_strong()));
                    if hover {
                        ui.painter().circle_stroke(rect.center(), 12.0, Stroke::new(2.0, p.accent));
                    }
                    if tooltip(resp, &format.write(sample.rgba), None).clicked() {
                        clicked = Some(*sample);
                    }
                }
                ui.add_space(4.0);
            });
        });
    clicked
}
