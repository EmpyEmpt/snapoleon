//! The four thin windows that outline the recorded area: a solid danger frame while recording, a dashed
//! one while paused. They are click-through and hidden from the recording.

use std::rc::Rc;

use egui::{Color32, Context, Frame, Pos2, Rect as UiRect};
use snap_core::Rect;
use snap_ui::{palette, widgets::util::lerp_color};

use super::{FRAME_THICKNESS, Shared, View, ViewCx};

/// Dash and gap lengths of the paused frame, in physical pixels.
const DASH: f32 = 10.0;
const GAP: f32 = 6.0;

/// The four edges as physical rects: just outside the area, so the frame never ends up in the picture;
/// inside its border when the area is a whole monitor and there is no outside.
pub fn edges(area: Rect, scale: f32, inside: bool) -> [Rect; 4] {
    let t = (FRAME_THICKNESS * scale).round().max(1.0) as i32;
    let (x, y, w, h) = (area.x, area.y, area.w as i32, area.h as i32);
    let rect = |x: i32, y: i32, w: i32, h: i32| Rect::new(x, y, w.max(1) as u32, h.max(1) as u32);
    if inside {
        [rect(x, y, w, t), rect(x, y + h - t, w, t), rect(x, y + t, t, h - 2 * t), rect(x + w - t, y + t, t, h - 2 * t)]
    } else {
        [rect(x - t, y - t, w + 2 * t, t), rect(x - t, y + h, w + 2 * t, t), rect(x - t, y, t, h), rect(x + w, y, t, h)]
    }
}

pub struct FrameView {
    pub shared: Rc<Shared>,
}

impl View for FrameView {
    fn ui(&mut self, _cx: &mut ViewCx, ctx: &Context) {
        let p = palette(ctx);
        let paused = self.shared.paused.get();
        ctx.root_ui(|ui| {
            // Dashes sit on a dim gap colour; solid is simply the fill.
            let fill = if paused { lerp_color(p.danger, Color32::BLACK, 0.78) } else { p.danger };
            egui::CentralPanel::default().frame(Frame::new().fill(fill)).show(ui, |ui| {
                if !paused {
                    return;
                }
                let rect = ui.max_rect();
                let ppp = ui.pixels_per_point();
                let horizontal = rect.width() >= rect.height();
                let (dash, gap) = (DASH / ppp, GAP / ppp);
                let (start, end) = if horizontal { (rect.left(), rect.right()) } else { (rect.top(), rect.bottom()) };
                let mut at = start;
                while at < end {
                    let to = (at + dash).min(end);
                    let segment = if horizontal {
                        UiRect::from_min_max(Pos2::new(at, rect.top()), Pos2::new(to, rect.bottom()))
                    } else {
                        UiRect::from_min_max(Pos2::new(rect.left(), at), Pos2::new(rect.right(), to))
                    };
                    ui.painter().rect_filled(segment, 0.0, p.danger);
                    at = to + gap;
                }
            });
        });
    }
}
