//! Input overlays composited onto recorded frames: cursor halo, click ripples
//! and keystroke chips, drawn with tiny-skia and ab_glyph.

use std::time::{Duration, Instant};

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use crossbeam_channel::Receiver;
use rdev::{Button, Key};
use snap_core::{Point, Result};
use tiny_skia::{Color, FillRule, Paint, Path, PathBuilder, Pixmap, Stroke, Transform};

use crate::{
    frame::Frame,
    input::{self, InputEvent, Mods},
};

static FONT_BYTES: &[u8] = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");

const RIPPLE_LIFE: Duration = Duration::from_millis(650);
const CHIP_LIFE: Duration = Duration::from_millis(1700);
const CHIP_FADE: Duration = Duration::from_millis(400);
const TYPING_JOIN: Duration = Duration::from_millis(1400);
const MAX_CHIPS: usize = 3;
const MAX_TYPING_CHARS: usize = 28;

pub(crate) struct OverlayConfig {
    pub highlight: bool,
    pub keystrokes: bool,
    /// Multiplier from input-event coordinates to frame pixels (1 except on scaled macOS displays).
    pub input_scale: f32,
}

struct Ripple {
    x: f32,
    y: f32,
    born: Instant,
    color: [u8; 3],
}

struct Chip {
    text: String,
    typing: bool,
    last: Instant,
}

pub(crate) struct Overlay {
    rx: Receiver<InputEvent>,
    cfg: OverlayConfig,
    font: FontRef<'static>,
    cursor: Option<(f32, f32)>,
    down: Option<Button>,
    ripples: Vec<Ripple>,
    chips: Vec<Chip>,
}

impl Overlay {
    pub fn new(cfg: OverlayConfig, initial_cursor: Option<Point>) -> Result<Self> {
        let rx = input::subscribe()?;
        let font = FontRef::try_from_slice(FONT_BYTES).map_err(|e| snap_core::Error::Other(anyhow::anyhow!("overlay font: {e}")))?;
        Ok(Self {
            rx,
            cfg,
            font,
            cursor: initial_cursor.map(|p| (p.x as f32, p.y as f32)),
            down: None,
            ripples: Vec::new(),
            chips: Vec::new(),
        })
    }

    /// Consume pending input and draw the overlays onto `frame`. `origin` is the
    /// virtual-desktop position of the frame's top-left pixel; `cursor` is the live pointer
    /// position when the OS can report it (pointer events alone miss programmatic moves).
    pub fn draw(&mut self, frame: &mut Frame, origin: Point, cursor: Option<Point>, now: Instant) {
        self.pump(now);
        if let Some(p) = cursor {
            self.cursor = Some((p.x as f32, p.y as f32));
        }
        self.ripples.retain(|r| now.duration_since(r.born) < RIPPLE_LIFE);
        self.chips.retain(|c| now.duration_since(c.last) < CHIP_LIFE);

        let ui = (frame.h as f32 / 1080.0).clamp(0.75, 2.5);
        if self.cfg.highlight {
            self.draw_pointer(frame, origin, now, ui);
        }
        if self.cfg.keystrokes {
            self.draw_chips(frame, now, ui);
        }
    }

    fn pump(&mut self, now: Instant) {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                InputEvent::Move { x, y } => self.cursor = Some((x as f32, y as f32)),
                InputEvent::ButtonDown { button, x, y } => {
                    self.cursor = Some((x as f32, y as f32));
                    self.down = Some(button);
                    let color = match button {
                        Button::Right => [249, 115, 22],
                        Button::Middle => [148, 163, 184],
                        _ => [59, 130, 246],
                    };
                    self.ripples.push(Ripple { x: x as f32, y: y as f32, born: now, color });
                }
                InputEvent::ButtonUp => self.down = None,
                InputEvent::KeyDown { key, text, mods } => self.on_key(key, text, mods, now),
            }
        }
    }

    fn on_key(&mut self, key: Key, text: Option<String>, mods: Mods, now: Instant) {
        if !self.cfg.keystrokes {
            return;
        }
        if mods.any_command() {
            self.chips.push(Chip { text: input::format_shortcut(mods, key), typing: false, last: now });
        } else if key == Key::Backspace {
            match self.chips.last_mut() {
                Some(c) if c.typing && now.duration_since(c.last) < TYPING_JOIN => {
                    c.text.pop();
                    c.last = now;
                    if c.text.is_empty() {
                        self.chips.pop();
                    }
                }
                _ => self.chips.push(Chip { text: "Backspace".into(), typing: false, last: now }),
            }
        } else if let Some(t) = text {
            match self.chips.last_mut() {
                Some(c) if c.typing && now.duration_since(c.last) < TYPING_JOIN => {
                    c.text.push_str(&t);
                    let excess = c.text.chars().count().saturating_sub(MAX_TYPING_CHARS);
                    if excess > 0 {
                        c.text = c.text.chars().skip(excess).collect();
                    }
                    c.last = now;
                }
                _ => self.chips.push(Chip { text: t, typing: true, last: now }),
            }
        } else {
            self.chips.push(Chip { text: self.label(key), typing: false, last: now });
        }
        let excess = self.chips.len().saturating_sub(MAX_CHIPS);
        self.chips.drain(..excess);
    }

    /// Key label with a word fallback for glyphs the font lacks (arrows).
    fn label(&self, key: Key) -> String {
        let label = input::key_label(key);
        if label.chars().all(|c| self.font.glyph_id(c).0 != 0) {
            return label;
        }
        match key {
            Key::UpArrow => "Up",
            Key::DownArrow => "Down",
            Key::LeftArrow => "Left",
            Key::RightArrow => "Right",
            _ => "?",
        }
        .to_string()
    }

    fn draw_pointer(&self, frame: &mut Frame, origin: Point, now: Instant, ui: f32) {
        let to_frame = |x: f32, y: f32| ((x * self.cfg.input_scale) - origin.x as f32, (y * self.cfg.input_scale) - origin.y as f32);

        for r in &self.ripples {
            let t = now.duration_since(r.born).as_secs_f32() / RIPPLE_LIFE.as_secs_f32();
            let eased = 1.0 - (1.0 - t).powi(3);
            let radius = (12.0 + 44.0 * eased) * ui;
            let alpha = (1.0 - t).clamp(0.0, 1.0);
            let (fx, fy) = to_frame(r.x, r.y);
            let size = (radius * 2.0 + 8.0).ceil() as u32;
            let Some(mut pm) = Pixmap::new(size, size) else { continue };
            let c = size as f32 / 2.0;
            if let Some(path) = PathBuilder::from_circle(c, c, radius) {
                fill(&mut pm, &path, [r.color[0], r.color[1], r.color[2], (alpha * 70.0) as u8]);
                stroke(&mut pm, &path, [r.color[0], r.color[1], r.color[2], (alpha * 230.0) as u8], 3.0 * ui);
            }
            frame.blend_premul_rgba(pm.data(), size, size, (fx - c).round() as i32, (fy - c).round() as i32, 1.0);
        }

        let Some((cx, cy)) = self.cursor else { return };
        let (fx, fy) = to_frame(cx, cy);
        if fx < -80.0 || fy < -80.0 || fx > frame.w as f32 + 80.0 || fy > frame.h as f32 + 80.0 {
            return;
        }
        let radius = if self.down.is_some() { 20.0 } else { 28.0 } * ui;
        let size = (radius * 2.0 + 8.0).ceil() as u32;
        if let Some(mut pm) = Pixmap::new(size, size) {
            let c = size as f32 / 2.0;
            if let Some(path) = PathBuilder::from_circle(c, c, radius) {
                let (fill_a, ring_a) = if self.down.is_some() { (110, 235) } else { (62, 190) };
                fill(&mut pm, &path, [255, 214, 10, fill_a]);
                stroke(&mut pm, &path, [255, 190, 0, ring_a], 2.0 * ui);
            }
            frame.blend_premul_rgba(pm.data(), size, size, (fx - c).round() as i32, (fy - c).round() as i32, 1.0);
        }
    }

    fn draw_chips(&self, frame: &mut Frame, now: Instant, ui: f32) {
        if self.chips.is_empty() {
            return;
        }
        let font_px = (22.0 * ui).round();
        let pad_x = (16.0 * ui).round();
        let pad_y = (9.0 * ui).round();
        let gap = (10.0 * ui).round();
        let scale = PxScale::from(font_px);
        let scaled = self.font.as_scaled(scale);
        let height = (scaled.ascent() - scaled.descent()).ceil() + pad_y * 2.0;

        let widths: Vec<f32> = self.chips.iter().map(|c| text_width(&scaled, &c.text) + pad_x * 2.0).collect();
        let total: f32 = widths.iter().sum::<f32>() + gap * (widths.len() as f32 - 1.0);
        let mut x = ((frame.w as f32 - total) / 2.0).max(4.0);
        let y = frame.h as f32 - height - 56.0 * ui;

        for (chip, w) in self.chips.iter().zip(widths) {
            let age = now.duration_since(chip.last);
            let opacity = if age > CHIP_LIFE - CHIP_FADE { 1.0 - (age - (CHIP_LIFE - CHIP_FADE)).as_secs_f32() / CHIP_FADE.as_secs_f32() } else { 1.0 };
            let (pw, ph) = (w.ceil() as u32, height.ceil() as u32);
            if let Some(mut pm) = Pixmap::new(pw, ph)
                && let Some(bg) = rounded_rect(0.0, 0.0, pw as f32, ph as f32, height / 2.0)
            {
                fill(&mut pm, &bg, [24, 24, 28, 214]);
                draw_text(&mut pm, &scaled, &chip.text, pad_x, pad_y + scaled.ascent());
                frame.blend_premul_rgba(pm.data(), pw, ph, x.round() as i32, y.round() as i32, opacity.clamp(0.0, 1.0));
            }
            x += w + gap;
        }
    }
}

fn text_width<F: Font>(font: &ab_glyph::PxScaleFont<&F>, text: &str) -> f32 {
    let mut w = 0.0;
    let mut prev = None;
    for c in text.chars() {
        let id = font.glyph_id(c);
        if let Some(p) = prev {
            w += font.kern(p, id);
        }
        w += font.h_advance(id);
        prev = Some(id);
    }
    w
}

fn draw_text<F: Font>(pm: &mut Pixmap, font: &ab_glyph::PxScaleFont<&F>, text: &str, x: f32, baseline: f32) {
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let mut pen = x;
    let mut prev = None;
    for c in text.chars() {
        let id = font.glyph_id(c);
        if let Some(p) = prev {
            pen += font.kern(p, id);
        }
        let glyph = id.with_scale_and_position(font.scale(), ab_glyph::point(pen, baseline));
        if let Some(outlined) = font.font().outline_glyph(glyph) {
            let b = outlined.px_bounds();
            let data = pm.data_mut();
            outlined.draw(|gx, gy, cov| {
                let px = b.min.x as i32 + gx as i32;
                let py = b.min.y as i32 + gy as i32;
                if px < 0 || py < 0 || px >= pw || py >= ph {
                    return;
                }
                let o = (py as usize * pw as usize + px as usize) * 4;
                let a = (cov * 255.0) as u32;
                // White text over the existing (premultiplied) chip background.
                let inv = 255 - a;
                for ch in &mut data[o..o + 3] {
                    *ch = (255 * a / 255 + *ch as u32 * inv / 255).min(255) as u8;
                }
                data[o + 3] = (a + data[o + 3] as u32 * inv / 255).min(255) as u8;
            });
        }
        pen += font.h_advance(id);
        prev = Some(id);
    }
}

fn paint(rgba: [u8; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]));
    p.anti_alias = true;
    p
}

fn fill(pm: &mut Pixmap, path: &Path, rgba: [u8; 4]) {
    pm.fill_path(path, &paint(rgba), FillRule::Winding, Transform::identity(), None);
}

fn stroke(pm: &mut Pixmap, path: &Path, rgba: [u8; 4], width: f32) {
    let s = Stroke { width, ..Stroke::default() };
    pm.stroke_path(path, &paint(rgba), &s, Transform::identity(), None);
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let r = r.min(w / 2.0).min(h / 2.0);
    let k = 0.552_284_8 * r;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}
