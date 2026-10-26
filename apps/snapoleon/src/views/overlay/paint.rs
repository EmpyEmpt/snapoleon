//! Drawing for the overlay: scrim, selection chrome, snap highlight, crosshair, labels and hint.

use egui::{
    Align2, Area, Color32, Context, FontId, Id, Order, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, TextFormat, TextStyle, Vec2,
    epaint::Shadow, pos2, text::LayoutJob, vec2,
};
use snap_ui::{
    Palette, icon, palette, text,
    widgets::{kbd, toolbar_pill},
};

use super::{selection::GRAB_PT, space::Space};

/// Handle square, in points.
const HANDLE_PT: f32 = 8.0;

#[derive(Clone, Copy)]
pub enum PillText {
    Mono,
    Caption,
}

fn font(ctx: &Context, kind: PillText) -> FontId {
    let style = ctx.global_style();
    match kind {
        PillText::Mono => TextStyle::Monospace.resolve(&style),
        PillText::Caption => text::caption_style().resolve(&style),
    }
}

/// An elevated one-line pill. `place` receives the pill's size and returns its top-left corner, so callers
/// can anchor it above, below or inside something without measuring text themselves. Returns the pill's rect.
pub fn pill(painter: &Painter, ctx: &Context, parts: &[(&str, PillText, Color32)], place: impl FnOnce(Vec2) -> Pos2) -> Rect {
    let p = palette(ctx);
    let mut job = LayoutJob::default();
    for (content, kind, color) in parts {
        job.append(content, 0.0, TextFormat::simple(font(ctx, *kind), *color));
    }
    let galley = painter.layout_job(job);
    let size = vec2(galley.size().x + 20.0, (galley.size().y + 8.0).max(22.0));
    let rect = Rect::from_min_size(place(size), size);
    painter.add(p.shadow_small().as_shape(rect, 11u8));
    painter.rect_filled(rect, 11.0, p.elevated);
    painter.rect_stroke(rect, 11.0, Stroke::new(1.0, p.border), StrokeKind::Inside);
    painter.galley(pos2(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0), galley, p.text);
    rect
}

/// Dim everything except `hole`. Without a hole the whole window is dimmed.
pub fn scrim(painter: &Painter, color: Color32, win: Rect, hole: Option<Rect>) {
    let hole = hole.map(|h| h.intersect(win)).filter(|h| h.width() > 0.0 && h.height() > 0.0);
    let Some(hole) = hole else {
        painter.rect_filled(win, 0.0, color);
        return;
    };
    for band in [
        Rect::from_min_max(win.min, pos2(win.max.x, hole.min.y)),
        Rect::from_min_max(pos2(win.min.x, hole.max.y), win.max),
        Rect::from_min_max(pos2(win.min.x, hole.min.y), pos2(hole.min.x, hole.max.y)),
        Rect::from_min_max(pos2(hole.max.x, hole.min.y), pos2(win.max.x, hole.max.y)),
    ] {
        painter.rect_filled(band, 0.0, color);
    }
}

/// The hovered window or element: accent wash and a 2 px outline.
pub fn highlight(painter: &Painter, p: &Palette, rect: Rect) {
    painter.rect_filled(rect, 0.0, p.accent.gamma_multiply(0.08));
    painter.rect_stroke(rect, 0.0, Stroke::new(2.0, p.accent), StrokeKind::Inside);
}

/// The selection: a 1 px accent border just outside, a white hairline just inside, and optionally the handles.
pub fn selection(painter: &Painter, p: &Palette, space: Space, rect: Rect, handles: bool) {
    let px = space.px();
    painter.rect_stroke(rect, 0.0, Stroke::new(px, p.accent), StrokeKind::Outside);
    painter.rect_stroke(rect, 0.0, Stroke::new(px, Color32::from_white_alpha(215)), StrokeKind::Inside);
    if !handles {
        return;
    }
    let side = (HANDLE_PT * space.ppp).round() / space.ppp;
    let mut spots = vec![rect.left_top(), rect.right_top(), rect.left_bottom(), rect.right_bottom()];
    if rect.width() >= 3.0 * GRAB_PT {
        spots.extend([pos2(rect.center().x, rect.top()), pos2(rect.center().x, rect.bottom())]);
    }
    if rect.height() >= 3.0 * GRAB_PT {
        spots.extend([pos2(rect.left(), rect.center().y), pos2(rect.right(), rect.center().y)]);
    }
    for spot in spots {
        let at = pos2((spot.x * space.ppp).round() / space.ppp, (spot.y * space.ppp).round() / space.ppp);
        let handle = Rect::from_center_size(at, Vec2::splat(side));
        painter.add(Shadow { offset: [0, 1], blur: 3, spread: 0, color: Color32::from_black_alpha(90) }.as_shape(handle, 1u8));
        painter.rect_filled(handle, 1.0, Color32::WHITE);
        painter.rect_stroke(handle, 1.0, Stroke::new(space.px(), p.accent), StrokeKind::Inside);
    }
}

/// Dashed 1 px guide lines across the window through the pointer pixel, at 40% white.
pub fn crosshair(painter: &Painter, space: Space, win: Rect, cursor: Pos2) {
    let c = space.pixel_center(cursor);
    let stroke = Stroke::new(space.px(), Color32::from_white_alpha(102));
    painter.extend(Shape::dashed_line(&[pos2(c.x, win.top()), pos2(c.x, win.bottom())], stroke, 4.0, 4.0));
    painter.extend(Shape::dashed_line(&[pos2(win.left(), c.y), pos2(win.right(), c.y)], stroke, 4.0, 4.0));
}

/// A line that reads on any background: a light casing around the colour.
pub fn cased_line(painter: &Painter, space: Space, a: Pos2, b: Pos2, color: Color32) {
    let px = space.px();
    painter.line_segment([a, b], Stroke::new(3.0 * px, Color32::from_white_alpha(170)));
    painter.line_segment([a, b], Stroke::new(px, color));
}

/// The `w × h` pill above a rectangle's top-left corner, inside it when there is no room above.
pub fn size_label(painter: &Painter, ctx: &Context, win: Rect, rect: Rect, size: (u32, u32), caption: Option<&str>) {
    let p = palette(ctx);
    let dims = format!("{} × {}", size.0, size.1);
    let caption = caption.map(|c| format!("{}  ", truncate(c, 36)));
    let mut parts = Vec::new();
    if let Some(caption) = &caption {
        parts.push((caption.as_str(), PillText::Caption, p.text_dim));
    }
    parts.push((dims.as_str(), PillText::Mono, p.text));
    pill(painter, ctx, &parts, |size| {
        let above = rect.left_top() - vec2(0.0, size.y + 6.0);
        let min = if above.y >= win.top() + 4.0 { above } else { rect.left_top() + vec2(8.0, 8.0) };
        pos2(
            min.x.clamp(win.left() + 4.0, (win.right() - size.x - 4.0).max(win.left() + 4.0)),
            min.y.clamp(win.top() + 4.0, win.bottom() - size.y - 4.0),
        )
    });
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The line of instructions at the top of the screen before the first interaction. It never takes the
/// pointer, and fades while the pointer is near so it cannot be in the way. `whole_screen` adds the F key.
pub fn hint(ctx: &Context, win: Rect, message: &str, glyph: &str, pointer: Option<Pos2>, whole_screen: bool) {
    let id = Id::unique("snap-overlay-hint");
    let rect: Option<Rect> = ctx.data(|d| d.get_temp(id));
    let near = matches!((rect, pointer), (Some(r), Some(p)) if r.expand(48.0).contains(p));
    let opacity = 0.2 + 0.8 * ctx.animate_bool_with_time(id.with("fade"), !near, 0.12);
    let out = Area::new(id)
        .order(Order::Foreground)
        .interactable(false)
        .pivot(Align2::CENTER_TOP)
        .fixed_pos(pos2(win.center().x, win.top() + 24.0))
        .show(ctx, |ui| {
            ui.set_opacity(opacity);
            toolbar_pill(ui, |ui| {
                let p = Palette::of(ui);
                ui.add_space(8.0);
                ui.label(egui::RichText::new(glyph).color(p.accent).size(15.0));
                ui.add_space(6.0);
                ui.label(message);
                if whole_screen {
                    ui.add_space(12.0);
                    kbd(ui, "F");
                    ui.add_space(6.0);
                    ui.label(text::caption(snap_core::t("overlay.monitor.hint")));
                }
                ui.add_space(12.0);
                kbd(ui, "Esc");
                ui.add_space(6.0);
                ui.label(text::caption(snap_core::t("overlay.hint.cancel")));
                ui.add_space(8.0);
            });
        });
    ctx.data_mut(|d| d.insert_temp(id, out.response.rect));
}

/// The glyph that goes with each mode's hint.
pub fn hint_icon(mode: super::OverlayMode) -> &'static str {
    use super::OverlayMode as M;
    match mode {
        M::Region | M::Pin | M::Scroll => icon::SELECTION,
        M::Window => icon::APP_WINDOW,
        M::Element => icon::CURSOR_CLICK,
        M::Ocr | M::Translate => icon::TEXT_T,
        M::Codes => icon::QR_CODE,
        M::Record(_) => icon::RECORD,
        M::ColorPicker => icon::EYEDROPPER,
        M::Measure => icon::RULER,
    }
}
