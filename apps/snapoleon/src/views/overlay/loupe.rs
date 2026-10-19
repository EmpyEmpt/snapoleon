//! The loupe: a magnified, pixel-gridded circle next to the pointer with a colour readout under it,
//! plus the colour formats and the short history of picked colours the picker mode shows.

use std::{
    f32::consts::TAU,
    sync::atomic::{AtomicU8, Ordering},
};

use egui::{
    Color32, ColorImage, Context, FontId, LayerId, Mesh, Order, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, TextStyle, TextureHandle,
    TextureOptions, epaint::Vertex, pos2, text::LayoutJob, vec2,
};
use parking_lot::Mutex;
use snap_capture::Frozen;
use snap_core::Point;
use snap_image::color;
use snap_ui::palette;

use super::{
    paint::{self, PillText},
    space::{Space, pixel_at},
};
use crate::views::colors::ColorSample;

/// Diameter of the loupe circle in points.
const DIAMETER_PT: f32 = 120.0;
/// Gap between the pointer and the loupe in points.
const GAP_PT: f32 = 20.0;
/// How many picked colours the picker remembers.
const KEEP_PICKS: usize = 8;

/// A way to write a colour down. The picker copies the one the user last chose.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorFormat {
    Hex,
    Rgb,
    Hsl,
    Oklch,
}

static LAST_FORMAT: AtomicU8 = AtomicU8::new(0);
static PICKS: Mutex<Vec<ColorSample>> = Mutex::new(Vec::new());

impl ColorFormat {
    pub const ALL: [ColorFormat; 4] = [ColorFormat::Hex, ColorFormat::Rgb, ColorFormat::Hsl, ColorFormat::Oklch];

    /// The format used last, kept for the life of the process.
    pub fn last() -> Self {
        Self::ALL[(LAST_FORMAT.load(Ordering::Relaxed) as usize).min(Self::ALL.len() - 1)]
    }

    pub fn next(self) -> Self {
        let next = (Self::ALL.iter().position(|f| *f == self).unwrap_or(0) + 1) % Self::ALL.len();
        LAST_FORMAT.store(next as u8, Ordering::Relaxed);
        Self::ALL[next]
    }

    pub fn label(self) -> &'static str {
        match self {
            ColorFormat::Hex => "HEX",
            ColorFormat::Rgb => "RGB",
            ColorFormat::Hsl => "HSL",
            ColorFormat::Oklch => "OKLCH",
        }
    }

    pub fn write(self, c: [u8; 4]) -> String {
        match self {
            ColorFormat::Hex => color::hex(c),
            ColorFormat::Rgb => color::rgb(c),
            ColorFormat::Hsl => color::hsl(c),
            ColorFormat::Oklch => color::oklch(c),
        }
    }
}

/// The latest picked colours, newest first.
pub fn picks() -> Vec<ColorSample> {
    PICKS.lock().clone()
}

pub fn remember(sample: ColorSample) {
    let mut picks = PICKS.lock();
    picks.retain(|p| p.rgba != sample.rgba);
    picks.insert(0, sample);
    picks.truncate(KEEP_PICKS);
}

pub fn color32(c: [u8; 4]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// What the loupe shows and where it may go: the magnification, the readout under it, and what to keep clear of.
pub struct Readout {
    /// Source pixels are magnified this many times.
    pub zoom: u32,
    /// A floating bar the loupe should stay clear of.
    pub avoid: Option<Rect>,
    pub sample: [u8; 4],
    pub at: Point,
    /// Picker mode: the format the value is written in; the readout also lists the other formats.
    pub picker: Option<ColorFormat>,
    /// A short confirmation ("Copied #3D5AFE") shown under the readout.
    pub flash: Option<String>,
}

/// One loupe per cover window. Owns the small texture that holds the magnified pixels.
#[derive(Default)]
pub struct Loupe {
    tex: Option<TextureHandle>,
}

impl Loupe {
    /// Paint the loupe for the pixel under `cursor` (virtual-desktop pixels) in a window described by `space`.
    pub fn paint(&mut self, ctx: &Context, space: Space, cursor: Pos2, frozen: &Frozen, readout: &Readout) {
        let p = palette(ctx);
        let painter = ctx.layer_painter(LayerId::new(Order::Tooltip, egui::Id::unique("snap-loupe")));
        let ppp = space.ppp;
        let win = ctx.content_rect();

        // Whole physical pixels per source pixel, and an odd number of source pixels across, so the pointer
        // pixel sits exactly in the middle cell and every cell edge lands on a device pixel.
        let cell_px = (readout.zoom.clamp(2, 24) as f32 * ppp).round().max(2.0);
        let cells = ((DIAMETER_PT * ppp / cell_px / 2.0).round() as usize * 2 + 1).max(5);
        let diameter = cells as f32 * cell_px / ppp;
        let cell = cell_px / ppp;
        let radius = diameter / 2.0;

        let center_px = pixel_at(cursor);
        let half = (cells / 2) as i32;
        let image = ColorImage::new(
            [cells, cells],
            (0..cells * cells)
                .map(|i| {
                    let at = Point::new(center_px.x + (i % cells) as i32 - half, center_px.y + (i / cells) as i32 - half);
                    frozen.pixel(at).map_or(Color32::TRANSPARENT, color32)
                })
                .collect(),
        );
        let tex = match &mut self.tex {
            Some(tex) => {
                tex.set(image, TextureOptions::NEAREST);
                tex
            }
            None => self.tex.insert(ctx.load_texture("snap-loupe", image, TextureOptions::NEAREST)),
        };

        let (card, lines) = readout_card(&painter, ctx, readout);
        let pointer = space.local(cursor);
        let (total_w, total_h) = (diameter.max(card.x), diameter + 6.0 + card.y);
        // Beside the pointer, on the side that fits the window and keeps clear of any floating bar.
        let candidates = [
            pos2(pointer.x + GAP_PT, pointer.y + GAP_PT),
            pos2(pointer.x - GAP_PT - total_w, pointer.y + GAP_PT),
            pos2(pointer.x + GAP_PT, pointer.y - GAP_PT - total_h),
            pos2(pointer.x - GAP_PT - total_w, pointer.y - GAP_PT - total_h),
        ];
        let inside = |min: &Pos2| win.shrink(4.0).contains_rect(Rect::from_min_size(*min, vec2(total_w, total_h)));
        let clear = |min: &Pos2| readout.avoid.is_none_or(|bar| !bar.intersects(Rect::from_min_size(*min, vec2(total_w, total_h))));
        let mut min = candidates
            .iter()
            .find(|m| inside(m) && clear(m))
            .or_else(|| candidates.iter().find(|m| inside(m)))
            .copied()
            .unwrap_or(candidates[0]);
        min = pos2(
            min.x.clamp(win.left() + 4.0, (win.right() - total_w - 4.0).max(win.left() + 4.0)),
            min.y.clamp(win.top() + 4.0, (win.bottom() - total_h - 4.0).max(win.top() + 4.0)),
        );
        // Whole device pixels, so the magnified cells are not resampled.
        min = pos2((min.x * ppp).round() / ppp, (min.y * ppp).round() / ppp);
        let square = Rect::from_min_size(min, vec2(diameter, diameter));
        let center = square.center();

        painter.add(p.shadow().as_shape(square, radius as u8));
        painter.circle_filled(center, radius, p.bg);
        let mut mesh = Mesh::with_texture(tex.id());
        const SEGMENTS: usize = 96;
        mesh.vertices.push(Vertex { pos: center, uv: pos2(0.5, 0.5), color: Color32::WHITE });
        for i in 0..=SEGMENTS {
            let a = TAU * i as f32 / SEGMENTS as f32;
            let offset = vec2(a.cos(), a.sin()) * radius;
            mesh.vertices.push(Vertex { pos: center + offset, uv: pos2(0.5, 0.5) + offset / diameter, color: Color32::WHITE });
        }
        for i in 0..SEGMENTS as u32 {
            mesh.add_triangle(0, 1 + i, 2 + i);
        }
        painter.add(Shape::mesh(mesh));

        // Pixel grid, clipped to the circle by chord length.
        let grid = Stroke::new(space.px(), Color32::from_black_alpha(if p.dark { 70 } else { 46 }));
        for i in 1..cells {
            let offset = i as f32 * cell - radius;
            let chord = (radius * radius - offset * offset).max(0.0).sqrt();
            painter.vline(square.left() + i as f32 * cell, (center.y - chord)..=(center.y + chord), grid);
            painter.hline((center.x - chord)..=(center.x + chord), square.top() + i as f32 * cell, grid);
        }
        // The pointer pixel: a black frame inside a white one reads on any colour.
        let mid = Rect::from_min_size(square.min + vec2(half as f32 * cell, half as f32 * cell), vec2(cell, cell));
        painter.rect_stroke(mid, 0.0, Stroke::new(space.px(), Color32::WHITE), StrokeKind::Outside);
        painter.rect_stroke(mid, 0.0, Stroke::new(space.px(), Color32::BLACK), StrokeKind::Inside);
        painter.circle_stroke(center, radius, Stroke::new(2.0, Color32::from_white_alpha(235)));
        painter.circle_stroke(center, radius + 1.5, Stroke::new(1.0, Color32::from_black_alpha(110)));

        let card_min = pos2(
            (center.x - card.x / 2.0).clamp(win.left() + 4.0, (win.right() - card.x - 4.0).max(win.left() + 4.0)),
            square.bottom() + 6.0,
        );
        paint_readout(&painter, ctx, card_min, card, lines, readout);
    }
}

type Lines = Vec<std::sync::Arc<egui::Galley>>;

/// Lay out the readout text and return the card size with it.
fn readout_card(painter: &Painter, ctx: &Context, readout: &Readout) -> (egui::Vec2, Lines) {
    let p = palette(ctx);
    let style = ctx.global_style();
    let mono = TextStyle::Monospace.resolve(&style);
    let small = FontId::monospace(11.0);
    let hex = ColorFormat::Hex.write(readout.sample);
    let mut lines = Vec::new();

    let mut line = |segments: &[(&str, FontId, Color32)]| {
        let mut job = LayoutJob::default();
        for (text, font, color) in segments {
            job.append(text, 0.0, egui::TextFormat::simple(font.clone(), *color));
        }
        lines.push(painter.layout_job(job));
    };
    match readout.picker {
        Some(format) => {
            line(&[(&format.write(readout.sample), mono.clone(), p.text)]);
            let mut tabs = Vec::new();
            for f in ColorFormat::ALL {
                tabs.push((f.label(), small.clone(), if f == format { p.accent } else { p.text_faint }));
                tabs.push(("  ", small.clone(), p.text_faint));
            }
            tabs.pop();
            line(&tabs);
        }
        None => {
            line(&[(&hex, mono.clone(), p.text), ("  ", mono.clone(), p.text), (&color::rgb(readout.sample), mono.clone(), p.text_dim)])
        }
    }
    line(&[(&format!("{}, {}", readout.at.x, readout.at.y), small.clone(), p.text_dim)]);

    let text_w = lines.iter().map(|g| g.size().x).fold(0.0, f32::max);
    let text_h: f32 = lines.iter().map(|g| g.size().y).sum::<f32>() + 2.0 * (lines.len() as f32 - 1.0);
    (vec2(text_w + SWATCH_W + 24.0, text_h + 14.0), lines)
}

/// Room for the colour dot left of the text.
const SWATCH_W: f32 = 18.0;

fn paint_readout(painter: &Painter, ctx: &Context, min: Pos2, size: egui::Vec2, lines: Lines, readout: &Readout) {
    let p = palette(ctx);
    let card = Rect::from_min_size(min, size);
    painter.add(p.shadow().as_shape(card, 10u8));
    painter.rect_filled(card, 10.0, p.elevated);
    painter.rect_stroke(card, 10.0, Stroke::new(1.0, p.border), StrokeKind::Inside);

    let x = card.left() + 12.0 + SWATCH_W;
    let mut y = card.top() + 7.0;
    for (i, galley) in lines.into_iter().enumerate() {
        if i == 0 {
            let dot = pos2(card.left() + 12.0 + 5.0, y + galley.size().y / 2.0);
            painter.circle_filled(dot, 5.5, color32(readout.sample));
            painter.circle_stroke(dot, 5.5, Stroke::new(1.0, p.border_strong()));
        }
        let h = galley.size().y;
        painter.galley(pos2(x, y), galley, p.text);
        y += h + 2.0;
    }
    if let Some(flash) = &readout.flash {
        paint::pill(painter, ctx, &[(flash, PillText::Mono, p.success)], |_| pos2(card.left(), card.bottom() + 6.0));
    }
}
