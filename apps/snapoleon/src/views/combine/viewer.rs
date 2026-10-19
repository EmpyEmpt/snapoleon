//! Zoom and pan over one image: wheel zooms around the cursor, drag pans, double click toggles fit
//! and 100%. Shared by the combine and diff views.

use egui::{Color32, Id, Painter, Pos2, Rect, Response, Sense, Stroke, StrokeKind, TextureHandle, Ui, Vec2};
use snap_ui::{
    Palette, icon,
    widgets::{floating_toolbar, icon_button, toolbar_separator},
};

use snap_core::t;

const MIN_ZOOM: f32 = 0.02;
const MAX_ZOOM: f32 = 32.0;
const STEP: f32 = 1.25;

#[derive(Default)]
pub struct Viewer {
    /// Image pixels per screen pixel; `None` follows the window (fit).
    zoom: Option<f32>,
    /// Offset of the image center from the center of the canvas, in points.
    pan: Vec2,
}

/// Where the image is this frame.
pub struct Placed {
    pub image_rect: Rect,
    pub canvas: Rect,
    pub response: Response,
}

impl Placed {
    /// Draw `tex` over the whole image rect, clipped to the canvas.
    pub fn paint(&self, ui: &Ui, tex: &TextureHandle) {
        self.paint_clipped(ui, tex, self.image_rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)));
    }

    /// Draw part of `tex`: `uv` is the part of the texture shown in `dest`.
    pub fn paint_clipped(&self, ui: &Ui, tex: &TextureHandle, dest: Rect, uv: Rect) {
        self.painter(ui).image(tex.id(), dest, uv, Color32::WHITE);
    }

    pub fn painter(&self, ui: &Ui) -> Painter {
        ui.painter_at(self.canvas)
    }

    /// The 1 px outline that separates the image from the canvas.
    pub fn paint_outline(&self, ui: &Ui) {
        let p = Palette::of(ui);
        self.painter(ui).rect_stroke(self.image_rect.expand(0.5), 0.0, Stroke::new(1.0, p.border), StrokeKind::Outside);
    }
}

impl Viewer {
    /// Back to fit-to-window.
    pub fn fit(&mut self) {
        self.zoom = None;
        self.pan = Vec2::ZERO;
    }

    fn fit_zoom(canvas: Rect, px: Vec2, ppp: f32) -> f32 {
        let avail = (canvas.size() - Vec2::splat(48.0)).max(Vec2::splat(32.0)) * ppp;
        (avail.x / px.x).min(avail.y / px.y).min(1.0)
    }

    pub fn zoom_percent(&self, canvas: Rect, px: Vec2, ppp: f32) -> u32 {
        (self.zoom.unwrap_or_else(|| Self::fit_zoom(canvas, px, ppp)) * 100.0).round() as u32
    }

    /// Handle input for this frame and return where the `px`-sized image sits inside `canvas`.
    pub fn place(&mut self, ui: &mut Ui, canvas: Rect, px: Vec2) -> Placed {
        let ppp = ui.pixels_per_point();
        let response = ui.interact(canvas, ui.scope_id().with("viewer"), Sense::click_and_drag());
        let fit = Self::fit_zoom(canvas, px, ppp);
        let zoom_now = self.zoom.unwrap_or(fit);
        let center = canvas.center() + self.pan;

        if let Some(pointer) = response.hover_pos() {
            let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = (scroll * 0.0016).exp() * pinch;
            if (factor - 1.0).abs() > f32::EPSILON {
                let next = (zoom_now * factor).clamp(MIN_ZOOM, MAX_ZOOM);
                // Keep the image point under the cursor where it is.
                let k = next / zoom_now;
                self.pan = (pointer - canvas.center()) - (pointer - center) * k;
                self.zoom = Some(next);
            }
        }
        if response.dragged() {
            self.pan += response.drag_delta();
        }
        if response.double_clicked() {
            if self.zoom.is_some() {
                self.fit();
            } else {
                self.zoom = Some(1.0);
                self.pan = Vec2::ZERO;
            }
        }

        let zoom = self.zoom.unwrap_or(fit);
        let size = px * zoom / ppp;
        // Small images stay centered; large ones can be dragged until an edge reaches the canvas edge.
        let limit = ((size - canvas.size()) / 2.0).max(Vec2::ZERO) + Vec2::splat(64.0).min(canvas.size() / 4.0);
        self.pan = Vec2::new(self.pan.x.clamp(-limit.x, limit.x), self.pan.y.clamp(-limit.y, limit.y));
        if size.x <= canvas.width() && size.y <= canvas.height() {
            self.pan = Vec2::ZERO;
        }
        let image_rect = Rect::from_center_size((canvas.center() + self.pan).round(), size);
        Placed { image_rect, canvas, response }
    }

    /// The zoom pill in the corner of the canvas.
    pub fn controls(&mut self, ui: &mut Ui, canvas: Rect, px: Vec2) {
        let ppp = ui.pixels_per_point();
        let current = self.zoom.unwrap_or_else(|| Self::fit_zoom(canvas, px, ppp));
        let percent = self.zoom_percent(canvas, px, ppp);
        let pos = canvas.left_bottom() + Vec2::new(12.0, -12.0 - 36.0);
        let mut next = None;
        floating_toolbar(ui.ctx(), Id::unique("viewer-controls"), pos, |ui| {
            if icon_button(ui, icon::MINUS, &t("combine.viewer.zoom_out"), None).clicked() {
                next = Some((current / STEP).max(MIN_ZOOM));
            }
            ui.add_sized(Vec2::new(44.0, 28.0), egui::Label::new(snap_ui::text::mono_text(format!("{percent}%"))));
            if icon_button(ui, icon::PLUS, &t("combine.viewer.zoom_in"), None).clicked() {
                next = Some((current * STEP).min(MAX_ZOOM));
            }
            toolbar_separator(ui);
            if icon_button(ui, icon::CORNERS_IN, &t("combine.viewer.fit"), None).clicked() {
                self.fit();
            }
        });
        if let Some(zoom) = next {
            self.zoom = Some(zoom);
        }
    }
}
