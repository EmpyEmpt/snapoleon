//! The Snapoleon mark: a bicorne hat whose brim is a pair of capture brackets.
//!
//! One piece of unit-square geometry feeds three renderers (egui painter, tiny-skia, SVG) so they
//! can never drift apart.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2};
use snap_core::RgbaImage;
use tiny_skia::{
    BlendMode, Color, FillRule, GradientStop, LinearGradient, Paint, Path, PathBuilder, Pixmap,
    Point, SpreadMode, Transform,
};

use crate::theme::Palette;

type Pt = (f32, f32);

/// Hat silhouette as four closed cubic segments in the unit square: a bicorne seen from the front,
/// a thick arch whose two corners point down.
const HAT: [[Pt; 4]; 4] = [
    [(0.06, 0.60), (0.09, 0.31), (0.28, 0.13), (0.50, 0.13)],
    [(0.50, 0.13), (0.72, 0.13), (0.91, 0.31), (0.94, 0.60)],
    [(0.94, 0.60), (0.80, 0.44), (0.66, 0.38), (0.50, 0.38)],
    [(0.50, 0.38), (0.34, 0.38), (0.20, 0.44), (0.06, 0.60)],
];

const COCKADE: (Pt, f32) = ((0.5, 0.255), 0.04);
const BRACKET_TOP: f32 = 0.66;
const BRACKET_BOTTOM: f32 = 0.88;
const BRACKET_OUTER: f32 = 0.10;
const BRACKET_REACH: f32 = 0.36;
/// Vertical center of the mark inside the unit square.
const MARK_CENTER_Y: f32 = (0.13 + BRACKET_BOTTOM) / 2.0;

/// Colors for [`logo_colored`].
#[derive(Debug, Clone, Copy)]
pub struct LogoColors {
    pub hat: Color32,
    pub brackets: Color32,
    pub cockade: Color32,
}

impl LogoColors {
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            hat: p.text,
            brackets: p.accent,
            cockade: p.gold,
        }
    }

    pub fn mono(color: Color32) -> Self {
        Self {
            hat: color,
            brackets: color,
            cockade: color,
        }
    }
}

/// Bracket thickness: heavier at tiny sizes so the strokes stay at least ~1.4 px.
fn bracket_thickness(px: f32) -> f32 {
    (1.5 / px.max(1.0)).clamp(0.085, 0.15)
}

/// The four bracket arms as `[x0, y0, x1, y1]` rectangles that do not overlap.
fn bracket_rects(t: f32) -> [[f32; 4]; 4] {
    let (l, r) = (BRACKET_OUTER, 1.0 - BRACKET_OUTER);
    [
        [l, BRACKET_TOP, l + t, BRACKET_BOTTOM],
        [l + t, BRACKET_BOTTOM - t, BRACKET_REACH, BRACKET_BOTTOM],
        [r - t, BRACKET_TOP, r, BRACKET_BOTTOM],
        [
            1.0 - BRACKET_REACH,
            BRACKET_BOTTOM - t,
            r - t,
            BRACKET_BOTTOM,
        ],
    ]
}

// ---------------------------------------------------------------------------------------------
// egui painter

/// Draws the brand mark in `rect` (fit as a centered square) using the active palette.
pub fn logo(painter: &Painter, rect: Rect) {
    let palette = crate::theme::palette(painter.ctx());
    logo_colored(painter, rect, LogoColors::from_palette(&palette));
}

pub fn logo_colored(painter: &Painter, rect: Rect, colors: LogoColors) {
    let side = rect.width().min(rect.height());
    let origin = rect.center() - Vec2::splat(side / 2.0);
    let map = |(u, v): Pt| Pos2::new(origin.x + u * side, origin.y + v * side);

    let mut outline = Vec::with_capacity(HAT.len() * 12);
    for seg in &HAT {
        let pts = seg.map(map);
        for i in 0..12 {
            outline.push(cubic(pts, i as f32 / 12.0));
        }
    }
    painter.add(Shape::convex_polygon(outline, colors.hat, Stroke::NONE));

    for [x0, y0, x1, y1] in bracket_rects(bracket_thickness(side)) {
        let r = Rect::from_min_max(map((x0, y0)), map((x1, y1)));
        painter.rect_filled(r, side * 0.012, colors.brackets);
    }
    if side >= 40.0 {
        let (c, r) = COCKADE;
        painter.circle_filled(map(c), r * side, colors.cockade);
    }
}

fn cubic(p: [Pos2; 4], t: f32) -> Pos2 {
    let u = 1.0 - t;
    let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
    Pos2::new(
        p[0].x * w[0] + p[1].x * w[1] + p[2].x * w[2] + p[3].x * w[3],
        p[0].y * w[0] + p[1].y * w[1] + p[2].y * w[2] + p[3].y * w[3],
    )
}

// ---------------------------------------------------------------------------------------------
// tiny-skia rasterizer

/// Maps unit coordinates of the mark into a pixmap: uniform scale and offset.
#[derive(Clone, Copy)]
struct Place {
    scale: f32,
    ox: f32,
    oy: f32,
}

impl Place {
    /// Places the mark so its visual center sits at (`cx`, `cy`) with the unit square `side` pixels wide.
    fn centered(cx: f32, cy: f32, side: f32) -> Self {
        Self {
            scale: side,
            ox: cx - side * 0.5,
            oy: cy - side * MARK_CENTER_Y,
        }
    }

    fn map(self, (u, v): Pt) -> Point {
        Point::from_xy(self.ox + u * self.scale, self.oy + v * self.scale)
    }
}

fn hat_path(place: Place) -> Path {
    let mut pb = PathBuilder::new();
    let start = place.map(HAT[0][0]);
    pb.move_to(start.x, start.y);
    for seg in &HAT {
        let (a, b, c) = (place.map(seg[1]), place.map(seg[2]), place.map(seg[3]));
        pb.cubic_to(a.x, a.y, b.x, b.y, c.x, c.y);
    }
    pb.close();
    pb.finish().expect("hat path is non-degenerate")
}

fn rounded_rect_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Path {
    const K: f32 = 0.552_284_8;
    let k = r * K;
    let (x1, y1) = (x + w, y + h);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x1 - r, y);
    pb.cubic_to(x1 - r + k, y, x1, y + r - k, x1, y + r);
    pb.line_to(x1, y1 - r);
    pb.cubic_to(x1, y1 - r + k, x1 - r + k, y1, x1 - r, y1);
    pb.line_to(x + r, y1);
    pb.cubic_to(x + r - k, y1, x, y1 - r + k, x, y1 - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish().expect("rounded rect is non-degenerate")
}

fn paint_color(c: [u8; 4]) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(c[0], c[1], c[2], c[3]);
    paint.anti_alias = true;
    paint
}

/// Draws hat, brackets and (optionally) cockade onto `pm`.
fn draw_mark(
    pm: &mut Pixmap,
    place: Place,
    hat: [u8; 4],
    brackets: [u8; 4],
    cockade: Option<[u8; 4]>,
    thickness: f32,
) {
    pm.fill_path(
        &hat_path(place),
        &paint_color(hat),
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    let bracket_paint = paint_color(brackets);
    let mut pb = PathBuilder::new();
    for [x0, y0, x1, y1] in bracket_rects(thickness) {
        let (a, b) = (place.map((x0, y0)), place.map((x1, y1)));
        if let Some(r) = tiny_skia::Rect::from_ltrb(a.x, a.y, b.x, b.y) {
            pb.push_rect(r);
        }
    }
    if let Some(path) = pb.finish() {
        pm.fill_path(
            &path,
            &bracket_paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    if let (Some(color), (c, r)) = (cockade, COCKADE) {
        let c = place.map(c);
        if let Some(path) = PathBuilder::from_circle(c.x, c.y, r * place.scale) {
            pm.fill_path(
                &path,
                &paint_color(color),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
}

fn into_image(pm: Pixmap) -> RgbaImage {
    let (w, h) = (pm.width(), pm.height());
    RgbaImage::from_raw(w, h, pm.take_demultiplied()).expect("pixmap buffer matches its size")
}

const ICON_TOP: [u8; 3] = [0x7E, 0x91, 0xFF];
const ICON_BOTTOM: [u8; 3] = [0x2A, 0x3E, 0xD6];
const GOLD: [u8; 4] = [0xF2, 0xC6, 0x4F, 0xFF];
const RECORD_RED: [u8; 4] = [0xFF, 0x4D, 0x52, 0xFF];

/// App icon: the mark in white on a rounded square with an Imperial-blue gradient. `size` in pixels.
pub fn app_icon_rgba(size: u32) -> RgbaImage {
    let size = size.max(1);
    let s = size as f32;
    let mut pm = Pixmap::new(size, size).expect("icon size is non-zero");

    let radius = s * ICON_RADIUS;
    let bg = rounded_rect_path(0.0, 0.0, s, s, radius);
    let gradient = LinearGradient::new(
        Point::from_xy(0.0, 0.0),
        Point::from_xy(s, s),
        vec![
            GradientStop::new(
                0.0,
                Color::from_rgba8(ICON_TOP[0], ICON_TOP[1], ICON_TOP[2], 255),
            ),
            GradientStop::new(
                1.0,
                Color::from_rgba8(ICON_BOTTOM[0], ICON_BOTTOM[1], ICON_BOTTOM[2], 255),
            ),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    )
    .expect("two-stop gradient is valid");
    let paint = Paint {
        shader: gradient,
        anti_alias: true,
        ..Paint::default()
    };
    pm.fill_path(&bg, &paint, FillRule::Winding, Transform::identity(), None);

    // Soft sheen over the upper half.
    let sheen = LinearGradient::new(
        Point::from_xy(0.0, 0.0),
        Point::from_xy(0.0, s * 0.62),
        vec![
            GradientStop::new(0.0, Color::from_rgba8(255, 255, 255, 46)),
            GradientStop::new(1.0, Color::from_rgba8(255, 255, 255, 0)),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    )
    .expect("two-stop gradient is valid");
    let paint = Paint {
        shader: sheen,
        anti_alias: true,
        ..Paint::default()
    };
    pm.fill_path(&bg, &paint, FillRule::Winding, Transform::identity(), None);

    let side = s * ICON_MARK;
    let place = Place::centered(s * 0.5, s * 0.5, side);
    let cockade = (size >= 48).then_some(GOLD);
    draw_mark(
        &mut pm,
        place,
        [255, 255, 255, 255],
        [255, 255, 255, 235],
        cockade,
        bracket_thickness(side),
    );
    into_image(pm)
}

/// Fraction of the icon the mark's unit square spans.
const ICON_MARK: f32 = 0.80;
/// Corner radius of the icon background as a fraction of its size.
const ICON_RADIUS: f32 = 0.225;

/// Tray glyph on transparent: white on dark taskbars/menu bars, near-black on light ones.
/// With `recording`, a red dot with a clear halo sits in the bottom-right corner.
pub fn tray_icon_rgba(size: u32, recording: bool) -> RgbaImage {
    let ink = if crate::platform::system_dark().unwrap_or(true) {
        [255, 255, 255, 255]
    } else {
        [24, 24, 28, 255]
    };
    tray_icon_rgba_colored(size, recording, ink)
}

/// [`tray_icon_rgba`] with an explicit glyph color (e.g. black for macOS template images).
pub fn tray_icon_rgba_colored(size: u32, recording: bool, ink: [u8; 4]) -> RgbaImage {
    let size = size.max(1);
    let s = size as f32;
    let mut pm = Pixmap::new(size, size).expect("icon size is non-zero");
    let side = s * 0.98;
    let place = Place::centered(s * 0.5, s * 0.5, side);
    draw_mark(
        &mut pm,
        place,
        ink,
        ink,
        None,
        bracket_thickness(side).max(0.1),
    );
    if recording {
        let (cx, cy) = (s * 0.78, s * 0.78);
        let mut clear = Paint {
            blend_mode: BlendMode::Clear,
            anti_alias: true,
            ..Paint::default()
        };
        clear.set_color_rgba8(0, 0, 0, 255);
        if let Some(path) = PathBuilder::from_circle(cx, cy, s * 0.235) {
            pm.fill_path(
                &path,
                &clear,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        if let Some(path) = PathBuilder::from_circle(cx, cy, s * 0.17) {
            pm.fill_path(
                &path,
                &paint_color(RECORD_RED),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
    into_image(pm)
}

// ---------------------------------------------------------------------------------------------
// SVG

/// The app icon as an SVG document (1024 × 1024 viewBox), geometry identical to [`app_icon_rgba`].
pub fn app_icon_svg() -> String {
    let side = 1024.0 * ICON_MARK;
    let place = Place::centered(512.0, 512.0, side);
    let f = |n: f32| {
        format!("{:.2}", n)
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    };

    let mut hat = String::new();
    let start = place.map(HAT[0][0]);
    hat.push_str(&format!("M{} {}", f(start.x), f(start.y)));
    for seg in &HAT {
        let (a, b, c) = (place.map(seg[1]), place.map(seg[2]), place.map(seg[3]));
        hat.push_str(&format!(
            "C{} {} {} {} {} {}",
            f(a.x),
            f(a.y),
            f(b.x),
            f(b.y),
            f(c.x),
            f(c.y)
        ));
    }
    hat.push('Z');

    let mut brackets = String::new();
    for [x0, y0, x1, y1] in bracket_rects(bracket_thickness(side)) {
        let (a, b) = (place.map((x0, y0)), place.map((x1, y1)));
        brackets.push_str(&format!(
            "M{} {}H{}V{}H{}Z",
            f(a.x),
            f(a.y),
            f(b.x),
            f(b.y),
            f(a.x)
        ));
    }
    let (c, r) = COCKADE;
    let c = place.map(c);
    let hex = |c: [u8; 3]| format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2]);
    let radius = 1024.0 * ICON_RADIUS;

    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" width="1024" height="1024" role="img" aria-label="Snapoleon">
  <title>Snapoleon</title>
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="1024" y2="1024" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="{top}"/>
      <stop offset="1" stop-color="{bottom}"/>
    </linearGradient>
    <linearGradient id="sheen" x1="0" y1="0" x2="0" y2="635" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#fff" stop-opacity="0.18"/>
      <stop offset="1" stop-color="#fff" stop-opacity="0"/>
    </linearGradient>
  </defs>
  <rect width="1024" height="1024" rx="{radius}" fill="url(#bg)"/>
  <rect width="1024" height="1024" rx="{radius}" fill="url(#sheen)"/>
  <path d="{hat}" fill="#fff"/>
  <path d="{brackets}" fill="#fff" fill-opacity="0.92"/>
  <circle cx="{cx}" cy="{cy}" r="{cr}" fill="{gold}"/>
</svg>
"##,
        top = hex(ICON_TOP),
        bottom = hex(ICON_BOTTOM),
        radius = f(radius),
        cx = f(c.x),
        cy = f(c.y),
        cr = f(r * side),
        gold = hex([GOLD[0], GOLD[1], GOLD[2]]),
    )
}
