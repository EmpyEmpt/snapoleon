//! Procedurally drawn window frames: macOS window, browser chrome, phone bezel.
//!
//! Everything is drawn with tiny-skia at a unit `u` derived from the screenshot width, so a
//! 1200 px capture and a 2x HiDPI 2400 px capture get proportionally identical chrome.

use image::RgbaImage;
use snap_core::settings::FrameStyle;
use snap_core::{CaptureContext, t};
use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LineJoin, LinearGradient, Mask, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Point,
    SpreadMode, Stroke, Transform,
};

use crate::effects::round_corners;
use crate::text;
use crate::util::{from_pixmap, new_pixmap, to_pixmap};

type Rgba = [u8; 4];

/// Wraps `img` in the given frame. `corner_radius` rounds the outer shape of window frames
/// (the phone uses its own); `cx` supplies the title / address text where known.
pub fn draw_frame(img: &RgbaImage, style: FrameStyle, corner_radius: u32, cx: &CaptureContext) -> RgbaImage {
    match style {
        FrameStyle::None => {
            let mut out = img.clone();
            round_corners(&mut out, corner_radius);
            out
        }
        FrameStyle::MacWindow => mac_window(img, false, corner_radius, cx),
        FrameStyle::MacWindowDark => mac_window(img, true, corner_radius, cx),
        FrameStyle::Browser => browser(img, false, corner_radius, cx),
        FrameStyle::BrowserDark => browser(img, true, corner_radius, cx),
        FrameStyle::Phone => phone(img),
    }
}

fn unit(width: u32) -> f32 {
    (width as f32 / 1200.0).clamp(0.85, 2.4)
}

// ---------------------------------------------------------------- macOS window

fn mac_window(img: &RgbaImage, dark: bool, radius: u32, cx: &CaptureContext) -> RgbaImage {
    let u = unit(img.width());
    let bar = (30.0 * u).round();
    let (w, h) = (img.width(), img.height() + bar as u32);
    let mut pm = new_pixmap(w, h);

    let (top, bottom, hairline, title_color) = if dark {
        ([62, 62, 65, 255], [50, 50, 53, 255], [0, 0, 0, 120], [255, 255, 255, 165])
    } else {
        ([238, 238, 240, 255], [226, 226, 228, 255], [0, 0, 0, 38], [0, 0, 0, 150])
    };
    fill_vgradient(&mut pm, &rect_path(0.0, 0.0, w as f32, bar), 0.0, bar, top, bottom);
    fill(&mut pm, &rect_path(0.0, bar - 1.0, w as f32, 1.0), hairline);

    let lights: [(Rgba, Rgba); 3] =
        [([255, 95, 87, 255], [224, 68, 62, 255]), ([254, 188, 46, 255], [222, 161, 35, 255]), ([40, 200, 64, 255], [26, 171, 41, 255])];
    for (i, (face, ring)) in lights.iter().enumerate() {
        let c = (20.0 + 20.0 * i as f32) * u;
        let path = circle_path(c, bar / 2.0, 6.0 * u);
        fill(&mut pm, &path, *face);
        stroke(&mut pm, &circle_path(c, bar / 2.0, 5.75 * u), *ring, (0.6 * u).max(0.75), LineCap::Butt);
    }

    let title = cx.window_title.as_deref().or(cx.app_name.as_deref()).map(str::trim).filter(|s| !s.is_empty());
    if let Some(title) = title {
        let size = 13.0 * u;
        let room = w as f32 - 2.0 * 84.0 * u;
        if room > 40.0 * u {
            let shown = text::ellipsize(title, size, room);
            let tw = text::width(&shown, size);
            text::draw(&mut pm, &shown, (w as f32 - tw) / 2.0, bar / 2.0 + size * 0.36, size, title_color);
        }
    }

    blit_pixmap(&mut pm, img, 0, bar as i32);
    finish_window(pm, radius, dark)
}

// -------------------------------------------------------------------- browser

struct BrowserColors {
    strip: Rgba,
    toolbar: Rgba,
    pill: Rgba,
    icon: Rgba,
    text: Rgba,
    dim: Rgba,
    separator: Rgba,
}

fn browser(img: &RgbaImage, dark: bool, radius: u32, cx: &CaptureContext) -> RgbaImage {
    let u = unit(img.width());
    let c = if dark {
        BrowserColors {
            strip: [32, 33, 36, 255],
            toolbar: [53, 54, 58, 255],
            pill: [32, 33, 36, 255],
            icon: [189, 193, 198, 255],
            text: [232, 234, 237, 255],
            dim: [154, 160, 166, 255],
            separator: [0, 0, 0, 90],
        }
    } else {
        BrowserColors {
            strip: [222, 225, 230, 255],
            toolbar: [255, 255, 255, 255],
            pill: [241, 243, 244, 255],
            icon: [95, 99, 104, 255],
            text: [32, 33, 36, 255],
            dim: [95, 99, 104, 255],
            separator: [0, 0, 0, 30],
        }
    };
    let (strip_h, tool_h) = ((36.0 * u).round(), (40.0 * u).round());
    let top = strip_h + tool_h;
    let wf = img.width() as f32;
    let mut pm = new_pixmap(img.width(), img.height() + top as u32);

    fill(&mut pm, &rect_path(0.0, 0.0, wf, strip_h), c.strip);
    fill(&mut pm, &rect_path(0.0, strip_h, wf, tool_h), c.toolbar);

    for (i, face) in [[255, 95, 87, 255], [254, 188, 46, 255], [40, 200, 64, 255]].into_iter().enumerate() {
        fill(&mut pm, &circle_path((17.0 + 17.0 * i as f32) * u, strip_h / 2.0, 5.5 * u), face);
    }

    // Active tab, joined to the toolbar by concave feet.
    let tab_x0 = 80.0 * u;
    let tab_w = (230.0 * u).min((wf - tab_x0 - 56.0 * u).max(120.0 * u));
    let tab_x1 = tab_x0 + tab_w;
    let (tab_top, foot, round) = (5.0 * u, 8.0 * u, 9.0 * u);
    let base = strip_h + 1.0;
    let mut pb = PathBuilder::new();
    pb.move_to(tab_x0 - foot, base);
    pb.quad_to(tab_x0, base, tab_x0, base - foot);
    pb.line_to(tab_x0, tab_top + round);
    pb.quad_to(tab_x0, tab_top, tab_x0 + round, tab_top);
    pb.line_to(tab_x1 - round, tab_top);
    pb.quad_to(tab_x1, tab_top, tab_x1, tab_top + round);
    pb.line_to(tab_x1, base - foot);
    pb.quad_to(tab_x1, base, tab_x1 + foot, base);
    pb.close();
    if let Some(tab) = pb.finish() {
        fill(&mut pm, &tab, c.toolbar);
    }

    let ty = (tab_top + strip_h) / 2.0;
    globe(&mut pm, tab_x0 + 18.0 * u, ty, 6.0 * u, u, c.icon);
    let tab_title = cx
        .window_title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| cx.url.as_deref().map(|u| split_url(u).0.to_string()).filter(|s| !s.is_empty()))
        .unwrap_or_else(|| t("image.frame.new_tab").into_owned());
    let title_size = 12.0 * u;
    let title = text::ellipsize(&tab_title, title_size, tab_w - 62.0 * u);
    text::draw(&mut pm, &title, tab_x0 + 32.0 * u, ty + title_size * 0.36, title_size, c.text);
    cross(&mut pm, tab_x1 - 17.0 * u, ty, 3.6 * u, 1.3 * u, c.icon);
    plus(&mut pm, tab_x1 + 26.0 * u, ty + 0.5 * u, 5.0 * u, 1.4 * u, c.icon);

    // Toolbar: back, forward, reload, address pill, menu.
    let cy = strip_h + tool_h / 2.0;
    let sw = 1.7 * u;
    arrow(&mut pm, 26.0 * u, cy, -1.0, u, sw, c.icon);
    arrow(&mut pm, 58.0 * u, cy, 1.0, u, sw, c.icon);
    reload(&mut pm, 90.0 * u, cy, u, sw, c.icon);

    let pill_x0 = 118.0 * u;
    let pill_x1 = wf - 54.0 * u;
    let pill_h = 28.0 * u;
    if pill_x1 - pill_x0 > 80.0 * u {
        let pill = rrect_path(pill_x0, cy - pill_h / 2.0, pill_x1 - pill_x0, pill_h, pill_h / 2.0);
        fill(&mut pm, &pill, c.pill);
        lock(&mut pm, pill_x0 + 17.0 * u, cy, u, c.dim);
        let size = 13.0 * u;
        let tx = pill_x0 + 34.0 * u;
        let room = pill_x1 - tx - 16.0 * u;
        let baseline = cy + size * 0.36;
        match cx.url.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(url) => {
                let (host, rest) = split_url(url);
                let shown_host = text::ellipsize(host, size, room);
                let used = text::draw(&mut pm, &shown_host, tx, baseline, size, c.text);
                if !rest.is_empty() && shown_host == host {
                    let shown_rest = text::ellipsize(rest, size, room - used);
                    text::draw(&mut pm, &shown_rest, tx + used, baseline, size, c.dim);
                }
            }
            None => {
                let hint = text::ellipsize(&t("image.frame.search_hint"), size, room);
                text::draw(&mut pm, &hint, tx, baseline, size, c.dim);
            }
        }
        for i in -1..=1 {
            fill(&mut pm, &circle_path(wf - 24.0 * u, cy + i as f32 * 5.0 * u, 1.4 * u), c.icon);
        }
    }
    fill(&mut pm, &rect_path(0.0, top - 1.0, wf, 1.0), c.separator);

    blit_pixmap(&mut pm, img, 0, top as i32);
    finish_window(pm, radius, dark)
}

/// `("host", "/path?query")` with the scheme and `www.` stripped for display.
fn split_url(url: &str) -> (&str, &str) {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    match rest.find(['/', '?', '#']) {
        Some(i) => (&rest[..i], rest[i..].trim_end_matches('/')),
        None => (rest, ""),
    }
}

// ----------------------------------------------------------------------- phone

fn phone(img: &RgbaImage) -> RgbaImage {
    // Landscape shots are framed as a rotated portrait phone so the island lands on the left.
    if img.width() as f32 > img.height() as f32 * 1.1 {
        let upright = image::imageops::rotate90(img);
        return image::imageops::rotate270(&phone_portrait(&upright));
    }
    phone_portrait(img)
}

fn phone_portrait(img: &RgbaImage) -> RgbaImage {
    let u = (img.width() as f32 / 390.0).clamp(0.6, 4.0);
    let (band, bezel, side) = (3.0 * u, 9.0 * u, 3.5 * u);
    let edge = band + bezel;
    let (sw, sh) = (img.width() as f32, img.height() as f32);
    let (bw, bh) = (sw + 2.0 * edge, sh + 2.0 * edge);
    let mut pm = new_pixmap((bw + 2.0 * side).round() as u32, bh.round() as u32);
    let ox = side;

    let screen_r = (0.13 * sw).min(sw.min(sh) / 2.0);
    let body_r = screen_r + edge;

    // Side buttons sit behind the body so only the protruding part shows.
    let button = |pm: &mut Pixmap, x: f32, y: f32, h: f32| {
        let (x0, w) = (x, side + 1.5 * u);
        let p = rrect_path(x0, y, w, h, 1.2 * u);
        fill_hgradient(pm, &p, x0, x0 + w, [88, 88, 92, 255], [52, 52, 56, 255]);
    };
    button(&mut pm, 0.0, bh * 0.135, bh * 0.032);
    button(&mut pm, 0.0, bh * 0.205, bh * 0.065);
    button(&mut pm, 0.0, bh * 0.295, bh * 0.065);
    let mut right = |y: f32, h: f32| {
        let w = side + 1.5 * u;
        let x0 = ox + bw - 1.5 * u;
        let p = rrect_path(x0, y, w, h, 1.2 * u);
        fill_hgradient(&mut pm, &p, x0, x0 + w, [52, 52, 56, 255], [88, 88, 92, 255]);
    };
    right(bh * 0.24, bh * 0.105);

    // Titanium frame, black bezel, then the screen.
    let outer = rrect_path(ox, 0.0, bw, bh, body_r);
    fill_vgradient(&mut pm, &outer, 0.0, bh, [112, 112, 118, 255], [66, 66, 71, 255]);
    stroke(&mut pm, &rrect_path(ox + 0.5, 0.5, bw - 1.0, bh - 1.0, body_r - 0.5), [255, 255, 255, 70], (0.9 * u).max(1.0), LineCap::Butt);
    fill(&mut pm, &rrect_path(ox + band, band, bw - 2.0 * band, bh - 2.0 * band, body_r - band), [10, 10, 12, 255]);

    let (screen_x, screen_y) = (ox + edge, edge);
    let screen = rrect_path(screen_x, screen_y, sw, sh, screen_r);
    if let Some(mut mask) = Mask::new(pm.width(), pm.height()) {
        mask.fill_path(&screen, FillRule::Winding, true, Transform::identity());
        let shot = to_pixmap(img);
        pm.draw_pixmap(
            screen_x.round() as i32,
            screen_y.round() as i32,
            shot.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            Some(&mask),
        );
    }

    // Dynamic island with its camera lens.
    let (iw, ih) = (0.29 * sw, 0.088 * sw);
    let ix = screen_x + (sw - iw) / 2.0;
    let iy = screen_y + 0.028 * sw;
    fill(&mut pm, &rrect_path(ix, iy, iw, ih, ih / 2.0), [0, 0, 0, 255]);
    let lens = (ix + iw - ih * 0.5, iy + ih / 2.0);
    fill(&mut pm, &circle_path(lens.0, lens.1, ih * 0.27), [18, 20, 34, 255]);
    fill(&mut pm, &circle_path(lens.0, lens.1, ih * 0.13), [36, 44, 84, 255]);

    from_pixmap(pm)
}

// -------------------------------------------------------------- shared helpers

/// Clips the finished window to its rounded outline and draws a hairline edge.
fn finish_window(content: Pixmap, radius: u32, dark: bool) -> RgbaImage {
    let (w, h) = (content.width() as f32, content.height() as f32);
    let r = (radius as f32).min(w.min(h) / 2.0);
    if r < 0.5 {
        let mut pm = content;
        stroke(&mut pm, &rect_path(0.5, 0.5, w - 1.0, h - 1.0), edge_color(dark), 1.0, LineCap::Butt);
        return from_pixmap(pm);
    }
    let shape = rrect_path(0.0, 0.0, w, h, r);
    let mut out = new_pixmap(content.width(), content.height());
    if let Some(mut mask) = Mask::new(out.width(), out.height()) {
        mask.fill_path(&shape, FillRule::Winding, true, Transform::identity());
        out.draw_pixmap(0, 0, content.as_ref(), &PixmapPaint::default(), Transform::identity(), Some(&mask));
    }
    stroke(&mut out, &rrect_path(0.5, 0.5, w - 1.0, h - 1.0, r - 0.5), edge_color(dark), 1.0, LineCap::Butt);
    from_pixmap(out)
}

fn edge_color(dark: bool) -> Rgba {
    if dark { [255, 255, 255, 34] } else { [0, 0, 0, 46] }
}

fn blit_pixmap(pm: &mut Pixmap, img: &RgbaImage, x: i32, y: i32) {
    let shot = to_pixmap(img);
    pm.draw_pixmap(x, y, shot.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
}

fn color(c: Rgba) -> Color {
    Color::from_rgba8(c[0], c[1], c[2], c[3])
}

fn paint(c: Rgba) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(color(c));
    p.anti_alias = true;
    p
}

fn fill(pm: &mut Pixmap, path: &Path, c: Rgba) {
    pm.fill_path(path, &paint(c), FillRule::Winding, Transform::identity(), None);
}

fn fill_vgradient(pm: &mut Pixmap, path: &Path, y0: f32, y1: f32, top: Rgba, bottom: Rgba) {
    gradient(pm, path, Point::from_xy(0.0, y0), Point::from_xy(0.0, y1), top, bottom);
}

fn fill_hgradient(pm: &mut Pixmap, path: &Path, x0: f32, x1: f32, left: Rgba, right: Rgba) {
    gradient(pm, path, Point::from_xy(x0, 0.0), Point::from_xy(x1, 0.0), left, right);
}

fn gradient(pm: &mut Pixmap, path: &Path, a: Point, b: Point, from: Rgba, to: Rgba) {
    let stops = vec![GradientStop::new(0.0, color(from)), GradientStop::new(1.0, color(to))];
    let mut p = Paint { anti_alias: true, ..Paint::default() };
    match LinearGradient::new(a, b, stops, SpreadMode::Pad, Transform::identity()) {
        Some(shader) => p.shader = shader,
        None => p.set_color(color(from)),
    }
    pm.fill_path(path, &p, FillRule::Winding, Transform::identity(), None);
}

fn stroke(pm: &mut Pixmap, path: &Path, c: Rgba, width: f32, cap: LineCap) {
    let s = Stroke { width, line_cap: cap, line_join: LineJoin::Round, ..Stroke::default() };
    pm.stroke_path(path, &paint(c), &s, Transform::identity(), None);
}

fn rect_path(x: f32, y: f32, w: f32, h: f32) -> Path {
    PathBuilder::from_rect(tiny_skia::Rect::from_xywh(x, y, w.max(0.01), h.max(0.01)).expect("finite rect"))
}

fn circle_path(cx: f32, cy: f32, r: f32) -> Path {
    PathBuilder::from_circle(cx, cy, r.max(0.01)).expect("finite circle")
}

fn rrect_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Path {
    const K: f32 = 0.552_285;
    let r = r.clamp(0.0, w.min(h) / 2.0);
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
    pb.finish().expect("rounded rect is a valid path")
}

/// Appends a circular arc (angles in radians, y down) as cubic segments of at most 90 degrees.
fn arc(pb: &mut PathBuilder, cx: f32, cy: f32, r: f32, a0: f32, a1: f32, move_first: bool) {
    let segments = ((a1 - a0).abs() / std::f32::consts::FRAC_PI_2).ceil().max(1.0) as usize;
    let step = (a1 - a0) / segments as f32;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let at = |a: f32| (cx + r * a.cos(), cy + r * a.sin());
    let (sx, sy) = at(a0);
    if move_first {
        pb.move_to(sx, sy);
    }
    for i in 0..segments {
        let (s, e) = (a0 + step * i as f32, a0 + step * (i + 1) as f32);
        let (p0, p3) = (at(s), at(e));
        let c1 = (p0.0 - k * r * s.sin(), p0.1 + k * r * s.cos());
        let c2 = (p3.0 + k * r * e.sin(), p3.1 - k * r * e.cos());
        pb.cubic_to(c1.0, c1.1, c2.0, c2.1, p3.0, p3.1);
    }
}

fn line_path(points: &[(f32, f32)]) -> Option<Path> {
    let mut pb = PathBuilder::new();
    pb.move_to(points[0].0, points[0].1);
    for p in &points[1..] {
        pb.line_to(p.0, p.1);
    }
    pb.finish()
}

fn cross(pm: &mut Pixmap, cx: f32, cy: f32, half: f32, width: f32, c: Rgba) {
    for pts in [[(cx - half, cy - half), (cx + half, cy + half)], [(cx - half, cy + half), (cx + half, cy - half)]] {
        if let Some(p) = line_path(&pts) {
            stroke(pm, &p, c, width, LineCap::Round);
        }
    }
}

fn plus(pm: &mut Pixmap, cx: f32, cy: f32, half: f32, width: f32, c: Rgba) {
    for pts in [[(cx - half, cy), (cx + half, cy)], [(cx, cy - half), (cx, cy + half)]] {
        if let Some(p) = line_path(&pts) {
            stroke(pm, &p, c, width, LineCap::Round);
        }
    }
}

/// Left (`dir = -1`) or right (`dir = 1`) arrow centered at `cx`.
fn arrow(pm: &mut Pixmap, cx: f32, cy: f32, dir: f32, u: f32, width: f32, c: Rgba) {
    let (tail, head, wing) = (-6.5 * u * dir, 6.5 * u * dir, 5.0 * u);
    if let Some(p) = line_path(&[(cx + tail, cy), (cx + head, cy)]) {
        stroke(pm, &p, c, width, LineCap::Round);
    }
    if let Some(p) = line_path(&[(cx + head - 5.0 * u * dir, cy - wing), (cx + head, cy), (cx + head - 5.0 * u * dir, cy + wing)]) {
        stroke(pm, &p, c, width, LineCap::Round);
    }
}

fn reload(pm: &mut Pixmap, cx: f32, cy: f32, u: f32, width: f32, c: Rgba) {
    let r = 5.5 * u;
    let (a0, a1) = (-20f32.to_radians(), 290f32.to_radians());
    let mut pb = PathBuilder::new();
    arc(&mut pb, cx, cy, r, a0, a1, true);
    if let Some(p) = pb.finish() {
        stroke(pm, &p, c, width, LineCap::Round);
    }
    // Arrow head at the end of the sweep, pointing along its tangent.
    let end = (cx + r * a1.cos(), cy + r * a1.sin());
    let tangent = (-a1.sin(), a1.cos());
    let normal = (tangent.1, -tangent.0);
    let (tip, back, half) = (3.2 * u, 1.6 * u, 3.4 * u);
    let mut pb = PathBuilder::new();
    pb.move_to(end.0 + tangent.0 * tip, end.1 + tangent.1 * tip);
    pb.line_to(end.0 - tangent.0 * back + normal.0 * half, end.1 - tangent.1 * back + normal.1 * half);
    pb.line_to(end.0 - tangent.0 * back - normal.0 * half, end.1 - tangent.1 * back - normal.1 * half);
    pb.close();
    if let Some(p) = pb.finish() {
        fill(pm, &p, c);
    }
}

fn globe(pm: &mut Pixmap, cx: f32, cy: f32, r: f32, u: f32, c: Rgba) {
    let w = 1.2 * u;
    stroke(pm, &circle_path(cx, cy, r), c, w, LineCap::Butt);
    if let Some(oval) = tiny_skia::Rect::from_xywh(cx - r * 0.45, cy - r, r * 0.9, r * 2.0).and_then(PathBuilder::from_oval) {
        stroke(pm, &oval, c, w, LineCap::Butt);
    }
    if let Some(p) = line_path(&[(cx - r, cy), (cx + r, cy)]) {
        stroke(pm, &p, c, w, LineCap::Butt);
    }
}

fn lock(pm: &mut Pixmap, cx: f32, cy: f32, u: f32, c: Rgba) {
    fill(pm, &rrect_path(cx - 4.0 * u, cy - 0.5 * u, 8.0 * u, 6.0 * u, 1.3 * u), c);
    let mut pb = PathBuilder::new();
    pb.move_to(cx - 2.4 * u, cy - 0.5 * u);
    pb.line_to(cx - 2.4 * u, cy - 2.0 * u);
    arc(&mut pb, cx, cy - 2.0 * u, 2.4 * u, std::f32::consts::PI, 2.0 * std::f32::consts::PI, false);
    pb.line_to(cx + 2.4 * u, cy - 0.5 * u);
    if let Some(p) = pb.finish() {
        stroke(pm, &p, c, 1.3 * u, LineCap::Butt);
    }
}
