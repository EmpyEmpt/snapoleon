//! Small painting helpers shared by the grid, the drawer and the quick look.

use std::sync::Arc;

use egui::{
    Color32, CornerRadius, FontId, Galley, Mesh, Painter, Pos2, Rect, Shape, Stroke, Ui, Vec2,
    epaint::text::{LayoutJob, TextWrapping},
    load::SizedTexture,
};
use snap_core::capture::tidy_title;
use snap_history::Entry;
use snap_ui::{Palette, icon};

/// Single-line text that ends in an ellipsis instead of overflowing `max_width`.
pub fn elided(ui: &Ui, text: &str, font: FontId, color: Color32, max_width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = TextWrapping { max_width: max_width.max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    ui.painter().layout_job(job)
}

/// Text wrapped to `max_width`, cut with an ellipsis after `rows` rows.
pub fn clamped(ui: &Ui, text: &str, font: FontId, color: Color32, max_width: f32, rows: usize) -> Arc<Galley> {
    let mut job = LayoutJob::simple(text.to_owned(), font, color, max_width.max(1.0));
    job.wrap = TextWrapping { max_width: max_width.max(1.0), max_rows: rows, break_anywhere: false, overflow_character: Some('…') };
    ui.painter().layout_job(job)
}

/// A five-pointed star; `fill` paints the inside (the favorite state), the outline is always drawn.
pub fn star(painter: &Painter, center: Pos2, radius: f32, fill: Option<Color32>, outline: Color32) {
    let points: Vec<Pos2> = (0..10)
        .map(|i| {
            let angle = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
            let r = if i % 2 == 0 { radius } else { radius * 0.46 };
            center + Vec2::angled(angle) * r
        })
        .collect();
    if let Some(fill) = fill {
        // A star is star-shaped around its center, so a triangle fan is exact (the tessellator would mangle a concave path).
        let mut mesh = Mesh::default();
        mesh.colored_vertex(center, fill);
        for p in &points {
            mesh.colored_vertex(*p, fill);
        }
        for i in 0..10u32 {
            mesh.add_triangle(0, 1 + i, 1 + (i + 1) % 10);
        }
        painter.add(Shape::mesh(mesh));
    }
    painter.add(Shape::closed_line(points, Stroke::new(1.3, outline)));
}

/// A 32 px gradient from `color` (opaque) to transparent along one vertical edge of `rect`, to hint at content that scrolls away.
pub fn edge_fade(painter: &Painter, rect: Rect, color: Color32, right_edge: bool) {
    let clear = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 0);
    let width = 32.0;
    let (x_solid, x_clear) = if right_edge { (rect.right(), rect.right() - width) } else { (rect.left(), rect.left() + width) };
    let mut mesh = Mesh::default();
    mesh.colored_vertex(Pos2::new(x_solid, rect.top()), color);
    mesh.colored_vertex(Pos2::new(x_clear, rect.top()), clear);
    mesh.colored_vertex(Pos2::new(x_clear, rect.bottom()), clear);
    mesh.colored_vertex(Pos2::new(x_solid, rect.bottom()), color);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(Shape::mesh(mesh));
}

/// Which part of a texture a `target`-shaped box shows when the image is cropped to cover it. Tall images keep their top.
pub fn cover_uv(image: Vec2, target: Vec2) -> Rect {
    let (ia, ta) = (image.x / image.y.max(1.0), target.x / target.y.max(1.0));
    if ia > ta {
        let w = ta / ia;
        Rect::from_min_max(Pos2::new((1.0 - w) / 2.0, 0.0), Pos2::new((1.0 + w) / 2.0, 1.0))
    } else {
        let h = ia / ta;
        let top = if ia < 1.0 { 0.0 } else { (1.0 - h) / 2.0 };
        Rect::from_min_max(Pos2::new(0.0, top), Pos2::new(1.0, top + h))
    }
}

/// Fit `image` inside `bounds`, centered, never larger than `max_scale` times its size.
pub fn fit(image: Vec2, bounds: Rect, max_scale: f32) -> Rect {
    let scale = (bounds.width() / image.x.max(1.0)).min(bounds.height() / image.y.max(1.0)).min(max_scale);
    Rect::from_center_size(bounds.center(), image * scale)
}

pub fn paint_texture(ui: &Ui, tex: &egui::TextureHandle, rect: Rect, uv: Rect, round: CornerRadius) {
    egui::Image::from_texture(SizedTexture::from_handle(tex)).uv(uv).corner_radius(round).paint_at(ui, rect);
}

/// The title a capture goes by: its window title, else the app, else what kind of capture it is.
pub fn title_of(entry: &Entry) -> String {
    // Tidied here too: captures taken before titles were tidied at capture time keep their raw title.
    entry
        .context
        .window_title
        .as_deref()
        .and_then(tidy_title)
        .or_else(|| app_of(entry).map(str::to_owned))
        .unwrap_or_else(|| snap_history::kind_label(entry.kind).into_owned())
}

pub fn app_of(entry: &Entry) -> Option<&str> {
    entry.context.app_name.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// The app glyph tile: the app's initial on a soft accent square, or a kind icon when the app is unknown.
pub fn app_tile(ui: &Ui, p: &Palette, rect: Rect, entry: &Entry) {
    let recording = entry.kind == snap_core::CaptureKind::Recording;
    ui.painter().rect_filled(rect, 8.0, p.accent_soft);
    let initial = app_of(entry).and_then(|a| a.chars().find(|c| c.is_alphanumeric())).filter(|_| !recording);
    match initial {
        Some(c) => {
            let font = snap_ui::text::body_strong_style().resolve(ui.style());
            let galley = ui.painter().layout_no_wrap(c.to_uppercase().to_string(), font, p.accent);
            snap_ui::widgets::util::paint_centered(ui, rect.center(), galley, p.accent);
        }
        None => {
            let glyph = if recording { icon::VIDEO_CAMERA } else { icon::IMAGE };
            snap_ui::widgets::util::paint_icon(ui, rect.center(), glyph, 15.0, p.accent);
        }
    }
}
