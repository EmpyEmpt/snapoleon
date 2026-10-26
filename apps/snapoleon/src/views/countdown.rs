//! The count-down tile: a small ring with the seconds left, shown while the app waits (the delay of a delayed
//! capture, the 3-2-1 before a recording). It is click-through, hidden from captures and does not take the
//! keyboard focus, so whatever the user prepared on screen (an open menu) stays as it is. A delayed capture is cancelled
//! with Escape (grabbed globally while the tile is up, see `App::sync_escape`) or by pressing its hotkey again.

use std::{
    f32::consts::TAU,
    time::{Duration, Instant},
};

use egui::{Align2, Context, FontFamily, FontId, Frame, Order, Pos2, Shape, Stroke, StrokeKind};
use snap_core::{Point, t};
use snap_shell::WindowSpec;
use snap_ui::{palette, text};

use super::{OpenCx, View, ViewCx, app_icon};
use crate::commands::Command;

pub const TAG: &str = "countdown";

/// Logical side of the tile.
const TILE_SIZE: f32 = 136.0;
const RING_RADIUS: f32 = 48.0;
const RING_WIDTH: f32 = 4.0;
/// Segments of the progress arc; plenty for a 48 px circle.
const ARC_STEPS: usize = 72;

/// The tile's window, centred on `center` (physical pixels) on a monitor with this scale factor.
pub fn tile_spec(title: String, tag: &str, center: Point, scale: f32) -> WindowSpec {
    let side = (TILE_SIZE * scale).round() as i32;
    WindowSpec {
        title,
        size: Some((TILE_SIZE, TILE_SIZE)),
        position: Some(Point::new(center.x - side / 2, center.y - side / 2)),
        decorations: false,
        resizable: false,
        rounded: true,
        always_on_top: true,
        tool_window: true,
        activate: false,
        exclude_from_capture: true,
        icon: Some(app_icon()),
        tag: tag.into(),
        ..WindowSpec::default()
    }
}

/// Draw the tile: `number` seconds left, `into_second` how far (0..1) the current second has run.
pub fn paint(ctx: &Context, number: u32, into_second: f32) {
    let p = palette(ctx);
    ctx.root_ui(|ui| {
        egui::CentralPanel::default().frame(Frame::new().fill(p.elevated)).show(ui, |ui| {
            let center = ui.max_rect().center();
            let painter = ui.painter();
            painter.circle_stroke(center, RING_RADIUS, Stroke::new(RING_WIDTH, p.border));
            // The ring drains over each second, clockwise from the top.
            let left = 1.0 - into_second;
            let arc: Vec<Pos2> = (0..=ARC_STEPS)
                .map(|i| {
                    let angle = -TAU / 4.0 + TAU * left * i as f32 / ARC_STEPS as f32;
                    center + RING_RADIUS * egui::vec2(angle.cos(), angle.sin())
                })
                .collect();
            painter.add(Shape::line(arc, Stroke::new(RING_WIDTH, p.danger)));
            // The digit lands slightly large and settles within the first quarter second.
            let pop = 1.0 + 0.16 * (1.0 - (into_second * 4.0).min(1.0)).powi(2);
            painter.text(center, egui::Align2::CENTER_CENTER, number.to_string(), FontId::new(52.0 * pop, FontFamily::Monospace), p.text);
        });
    });
    let outline = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("countdown-outline")));
    outline.rect_stroke(ctx.content_rect(), 0.0, Stroke::new(1.0, p.border), StrokeKind::Inside);
}

/// Count `secs` seconds down on the monitor under the cursor, then run `then`. While a count-down is already running
/// this cancels it instead: pressing the hotkey again is a way out.
pub fn open(cx: &mut OpenCx, secs: u32, then: Command) {
    if !cx.shell.windows_with_tag(TAG).is_empty() {
        return cancel(cx);
    }
    let cursor = snap_capture::cursor_position();
    let monitors = cx.shell.monitors();
    let (screen, scale) = monitors.iter().copied().find(|(m, _)| m.contains(cursor)).or(monitors.first().copied()).unwrap_or_default();
    let center = Point::new(screen.x + screen.w as i32 / 2, screen.y + screen.h as i32 / 2);
    let spec = tile_spec(t("app.job.capturing").into_owned(), TAG, center, scale);
    let win = cx.open(spec, DelayView { began: Instant::now(), secs, then: Some(then) });
    cx.shell.set_passthrough(win, true);
}

/// Drop a running count-down and what it would have run.
pub fn cancel(cx: &mut OpenCx) {
    for win in cx.shell.windows_with_tag(TAG) {
        cx.shell.close(win);
    }
}

struct DelayView {
    began: Instant,
    secs: u32,
    then: Option<Command>,
}

impl View for DelayView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let elapsed = self.began.elapsed().as_secs_f32();
        if elapsed >= self.secs as f32 {
            // The window goes first, so it cannot end up in what the command captures.
            cx.close();
            if let Some(then) = self.then.take() {
                cx.push(then);
            }
            return;
        }
        paint(ctx, (self.secs as f32 - elapsed).ceil() as u32, elapsed.fract());
        // In the band under the ring.
        let p = palette(ctx);
        let tile = ctx.content_rect();
        let caption = ctx.layer_painter(egui::LayerId::new(Order::Foreground, egui::Id::unique("countdown-caption")));
        let font = text::caption_style().resolve(&ctx.global_style());
        caption.text(
            Pos2::new(tile.center().x, tile.bottom() - 9.0),
            Align2::CENTER_CENTER,
            t("app.countdown_hint.cancel"),
            font,
            p.text_dim,
        );
        ctx.request_repaint_after(Duration::from_millis(16));
    }
}
