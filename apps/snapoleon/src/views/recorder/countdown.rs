//! The 3-2-1 tile shown over the recorded area before the recording starts.

use std::{f32::consts::TAU, rc::Rc};

use egui::{Context, FontFamily, FontId, Frame, Pos2, Shape, Stroke, StrokeKind};
use snap_ui::palette;

use super::{Shared, View, ViewCx};

const RING_RADIUS: f32 = 48.0;
const RING_WIDTH: f32 = 4.0;
/// Segments of the progress arc; plenty for a 48 px circle.
const ARC_STEPS: usize = 72;

pub struct TileView {
    pub shared: Rc<Shared>,
}

impl View for TileView {
    fn ui(&mut self, _cx: &mut ViewCx, ctx: &Context) {
        let Some((number, into_second)) = self.shared.count.get() else { return };
        let p = palette(ctx);
        ctx.root_ui(|ui| {
            egui::CentralPanel::default().frame(Frame::new().fill(p.elevated)).show(ui, |ui| {
                let rect = ui.max_rect();
                let center = rect.center();
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
                painter.text(
                    center,
                    egui::Align2::CENTER_CENTER,
                    number.to_string(),
                    FontId::new(52.0 * pop, FontFamily::Monospace),
                    p.text,
                );
            });
        });
        let outline = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("recorder-tile-outline")));
        outline.rect_stroke(ctx.content_rect(), 0.0, Stroke::new(1.0, p.border), StrokeKind::Inside);
    }
}
