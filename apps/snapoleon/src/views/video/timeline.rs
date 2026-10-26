//! The scrubber: ruler, thumbnail strip, trim handles and playhead.

use egui::{
    Align2, Color32, CornerRadius, CursorIcon, FontId, Id, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, TextureHandle, Ui, Vec2,
    WidgetInfo, epaint::RectShape, pos2,
};
use snap_core::t;
use snap_ui::{
    palette, radius, text,
    widgets::util::{self, hover_anim},
};

use super::timecode;

pub const STRIP_HEIGHT: f32 = 56.0;
const RULER_HEIGHT: f32 = 20.0;
/// How far the handles stand out above and below the strip.
const HANDLE_OVERHANG: f32 = 4.0;
const HANDLE_WIDTH: f32 = 10.0;
const HANDLE_HIT_WIDTH: f32 = 18.0;
/// Total height of the widget.
pub const HEIGHT: f32 = RULER_HEIGHT + HANDLE_OVERHANG * 2.0 + STRIP_HEIGHT;
/// Trim handles cannot be dragged closer than this.
const MIN_RANGE_MS: u64 = 100;
/// Distance between ruler labels, at least, in logical pixels.
const MIN_LABEL_GAP: f32 = 76.0;
const RULER_STEPS_MS: [u64; 14] = [100, 200, 500, 1000, 2000, 5000, 10_000, 15_000, 30_000, 60_000, 120_000, 300_000, 600_000, 1_800_000];

pub struct Timeline<'a> {
    pub duration_ms: u64,
    pub playhead_ms: u64,
    /// `(in, out)` in milliseconds.
    pub trim: (u64, u64),
    pub thumbs: &'a [Option<TextureHandle>],
    /// Length of one frame, for snapping what the user drags to.
    pub frame_ms: f64,
}

/// What the user did this frame; the caller applies it.
#[derive(Default)]
pub struct Interaction {
    pub seek: Option<u64>,
    pub trim_in: Option<u64>,
    pub trim_out: Option<u64>,
}

/// Number of thumbnails that fill a strip `width` points wide at 16:9.
pub fn thumb_count(width: f32) -> usize {
    ((width / (STRIP_HEIGHT * 16.0 / 9.0)).ceil() as usize).clamp(3, 32)
}

impl Timeline<'_> {
    pub fn show(&self, ui: &mut Ui) -> Interaction {
        let p = palette(ui.ctx());
        let (outer, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), HEIGHT), Sense::hover());
        let strip = Rect::from_min_size(
            pos2(outer.left() + HANDLE_WIDTH / 2.0, outer.top() + RULER_HEIGHT + HANDLE_OVERHANG),
            Vec2::new(outer.width() - HANDLE_WIDTH, STRIP_HEIGHT),
        );
        let duration = self.duration_ms.max(1) as f32;
        let x_of = |ms: u64| strip.left() + strip.width() * (ms as f32 / duration);
        let ms_at = |x: f32| (((x - strip.left()) / strip.width()).clamp(0.0, 1.0) * duration) as u64;
        let snap = |ms: u64| ((ms as f64 / self.frame_ms).round() * self.frame_ms) as u64;
        let (in_x, out_x) = (x_of(self.trim.0), x_of(self.trim.1));

        let id = Id::unique("video-timeline");
        let mut out = Interaction::default();

        // Scrubbing first, so the handles (added later) sit on top of it.
        let scrub = ui.interact(
            Rect::from_x_y_ranges(strip.x_range(), outer.top()..=strip.bottom() + HANDLE_OVERHANG),
            id.with("scrub"),
            Sense::click_and_drag(),
        );
        if (scrub.dragged() || scrub.clicked() || scrub.drag_started())
            && let Some(pos) = scrub.interact_pointer_pos()
        {
            out.seek = Some(snap(ms_at(pos.x)).min(self.duration_ms));
        }
        let handle_rect =
            |x: f32| Rect::from_center_size(pos2(x, strip.center().y), Vec2::new(HANDLE_HIT_WIDTH, STRIP_HEIGHT + HANDLE_OVERHANG * 2.0));
        let in_resp = ui.interact(handle_rect(in_x), id.with("in"), Sense::drag());
        let out_resp = ui.interact(handle_rect(out_x), id.with("out"), Sense::drag());
        let secs = |ms: u64| ms as f64 / 1000.0;
        scrub.widget_info(|| WidgetInfo::slider(true, secs(self.playhead_ms), t("video.a11y.playhead")));
        in_resp.widget_info(|| WidgetInfo::slider(true, secs(self.trim.0), t("video.in")));
        out_resp.widget_info(|| WidgetInfo::slider(true, secs(self.trim.1), t("video.out")));
        for resp in [&in_resp, &out_resp] {
            if resp.hovered() || resp.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
        }
        if in_resp.dragged()
            && let Some(pos) = in_resp.interact_pointer_pos()
        {
            let ms = snap(ms_at(pos.x)).min(self.trim.1.saturating_sub(MIN_RANGE_MS));
            out.trim_in = Some(ms);
            out.seek = Some(ms);
        }
        if out_resp.dragged()
            && let Some(pos) = out_resp.interact_pointer_pos()
        {
            let ms = snap(ms_at(pos.x)).max(self.trim.0 + MIN_RANGE_MS).min(self.duration_ms);
            out.trim_out = Some(ms);
            out.seek = Some(ms);
        }

        if !ui.is_rect_visible(outer) {
            return out;
        }
        self.paint_ruler(ui, outer, strip, &x_of);
        let painter = ui.painter();
        let round = radius::CARD - 2.0;
        painter.rect_filled(strip, round, p.surface_2);
        self.paint_thumbs(ui, strip, round);
        // Dim what the trim leaves out.
        let dim = p.bg.gamma_multiply(0.66);
        let left = Rect::from_min_max(strip.min, pos2(in_x, strip.bottom()));
        let right = Rect::from_min_max(pos2(out_x, strip.top()), strip.max);
        painter.rect_filled(left, CornerRadius { nw: round as u8, sw: round as u8, ne: 0, se: 0 }, dim);
        painter.rect_filled(right, CornerRadius { ne: round as u8, se: round as u8, nw: 0, sw: 0 }, dim);
        painter.rect_stroke(strip, round, Stroke::new(1.0, p.border), StrokeKind::Inside);
        // The kept range is framed in accent between the handles.
        let (top_bar, bottom_bar) = (
            Rect::from_x_y_ranges(in_x..=out_x, strip.top() - 2.0..=strip.top()),
            Rect::from_x_y_ranges(in_x..=out_x, strip.bottom()..=strip.bottom() + 2.0),
        );
        painter.rect_filled(top_bar, 0.0, p.accent);
        painter.rect_filled(bottom_bar, 0.0, p.accent);

        for (x, resp, side) in [(in_x, &in_resp, Side::In), (out_x, &out_resp, Side::Out)] {
            self.paint_handle(ui, strip, x, resp, side);
        }
        self.paint_playhead(ui, outer, strip, x_of(self.playhead_ms.min(self.duration_ms)));
        out
    }

    fn paint_ruler(&self, ui: &Ui, outer: Rect, strip: Rect, x_of: &impl Fn(u64) -> f32) {
        let p = palette(ui.ctx());
        let step = RULER_STEPS_MS
            .iter()
            .copied()
            .find(|s| strip.width() * (*s as f32 / self.duration_ms.max(1) as f32) >= MIN_LABEL_GAP)
            .unwrap_or(RULER_STEPS_MS[RULER_STEPS_MS.len() - 1]);
        let font: FontId = text::mono().resolve(ui.style());
        let font = FontId::new(10.0, font.family);
        let mut at = 0;
        while at <= self.duration_ms {
            let x = x_of(at);
            // Keep the last label inside the widget.
            if x + 24.0 < outer.right() {
                ui.painter().text(pos2(x + 3.0, outer.top() + 1.0), Align2::LEFT_TOP, ruler_label(at, step), font.clone(), p.text_faint);
            }
            ui.painter().vline(x.floor() + 0.5, outer.top() + 12.0..=outer.top() + RULER_HEIGHT, Stroke::new(1.0, p.border));
            at += step;
        }
    }

    fn paint_thumbs(&self, ui: &Ui, strip: Rect, round: f32) {
        let n = self.thumbs.len();
        if n == 0 {
            return;
        }
        let slot = strip.width() / n as f32;
        for (i, thumb) in self.thumbs.iter().enumerate() {
            let Some(texture) = thumb else { continue };
            let rect = Rect::from_min_size(pos2(strip.left() + slot * i as f32, strip.top()), Vec2::new(slot + 0.5, strip.height()));
            // Fill the slot, trimming the picture's sides when it is wider than the slot.
            let aspect = texture.aspect_ratio().max(0.01);
            let visible = (rect.aspect_ratio() / aspect).min(1.0);
            let uv = Rect::from_min_max(pos2((1.0 - visible) / 2.0, 0.0), pos2((1.0 + visible) / 2.0, 1.0));
            let corners = CornerRadius {
                nw: if i == 0 { round as u8 } else { 0 },
                sw: if i == 0 { round as u8 } else { 0 },
                ne: if i == n - 1 { round as u8 } else { 0 },
                se: if i == n - 1 { round as u8 } else { 0 },
            };
            ui.painter().add(Shape::Rect(RectShape::filled(rect, corners, Color32::WHITE).with_texture(texture.id(), uv)));
        }
    }

    fn paint_handle(&self, ui: &Ui, strip: Rect, x: f32, resp: &egui::Response, side: Side) {
        let p = palette(ui.ctx());
        let hover = hover_anim(ui, resp);
        let rect = Rect::from_center_size(pos2(x, strip.center().y), Vec2::new(HANDLE_WIDTH, STRIP_HEIGHT + HANDLE_OVERHANG * 2.0));
        let fill = util::lerp_color(p.accent, p.on_accent, 0.12 * hover);
        ui.painter().add(p.shadow_small().as_shape(rect, 4.0));
        ui.painter().rect_filled(rect, 4.0, fill);
        for dx in [-1.5, 1.5] {
            ui.painter().vline(x + dx, rect.center().y - 6.0..=rect.center().y + 6.0, Stroke::new(1.0, p.on_accent.gamma_multiply(0.75)));
        }
        if resp.dragged() || resp.hovered() {
            let ms = match side {
                Side::In => self.trim.0,
                Side::Out => self.trim.1,
            };
            bubble(ui, pos2(x, rect.top() - 4.0), &timecode(ms));
        }
    }

    fn paint_playhead(&self, ui: &Ui, outer: Rect, strip: Rect, x: f32) {
        let p = palette(ui.ctx());
        let top = outer.top() + RULER_HEIGHT - 4.0;
        let line = Stroke::new(2.0, p.text);
        ui.painter()
            .add(p.shadow_small().as_shape(Rect::from_min_max(pos2(x - 1.0, top), pos2(x + 1.0, strip.bottom() + HANDLE_OVERHANG)), 1.0));
        ui.painter().vline(x, top..=strip.bottom() + HANDLE_OVERHANG, line);
        let head = Rect::from_center_size(pos2(x, top + 3.0), Vec2::new(12.0, 8.0));
        ui.painter().add(Shape::convex_polygon(
            vec![head.left_top(), head.right_top(), pos2(x, head.bottom() + 3.0)],
            p.text,
            Stroke::NONE,
        ));
    }
}

#[derive(Clone, Copy)]
enum Side {
    In,
    Out,
}

/// A small timecode label floating above a handle.
fn bubble(ui: &Ui, bottom_center: Pos2, label: &str) {
    let p = palette(ui.ctx());
    let galley = util::layout(ui, label, &text::mono(), p.text);
    let size = galley.size() + Vec2::new(16.0, 8.0);
    let rect = Rect::from_center_size(pos2(bottom_center.x, bottom_center.y - size.y / 2.0), size);
    let rect = rect.translate(Vec2::new(
        (ui.clip_rect().left() + 4.0 - rect.left()).max(0.0) - (rect.right() - ui.clip_rect().right() + 4.0).max(0.0),
        0.0,
    ));
    ui.painter().add(p.shadow_small().as_shape(rect, 8.0));
    ui.painter().rect_filled(rect, 8.0, p.elevated);
    ui.painter().rect_stroke(rect, 8.0, Stroke::new(1.0, p.border), StrokeKind::Inside);
    util::paint_centered(ui, rect.center(), galley, p.text);
}

/// Ruler labels: whole seconds as `m:ss`, finer steps with tenths.
fn ruler_label(ms: u64, step: u64) -> String {
    let (minutes, seconds, tenths) = (ms / 60_000, ms / 1000 % 60, ms / 100 % 10);
    if step < 1000 { format!("{minutes}:{seconds:02}.{tenths}") } else { format!("{minutes}:{seconds:02}") }
}
