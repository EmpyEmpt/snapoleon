//! Inline text editing on the canvas: caret and selection state, key handling and the geometry that
//! maps character positions to document coordinates.
//!
//! The renderer lays text out with a greedy word wrap at fixed line pitch (1.3 em); `visual_lines`
//! mirrors that wrap using `text_layout` as the only measuring tool, so the caret lands where the glyphs are.

use egui::{Event, Key, Modifiers};
use snap_annotate::{Shape, ShapeKind, text_layout};
use snap_core::{PointF, RectF};

/// Line pitch of annotation text in em.
const LINE_PITCH: f32 = 1.3;

/// What the editor needs to know to lay a shape's text out.
#[derive(Clone, Copy)]
pub struct Metrics {
    pub size: f32,
    pub bold: bool,
    pub wrap: Option<f32>,
}

impl Metrics {
    pub fn pitch(&self) -> f32 {
        self.size * LINE_PITCH
    }

    fn width(&self, s: &str) -> f32 {
        text_layout(s, self.size, self.bold, None).0
    }
}

/// The text of a shape with the geometry of its first line.
pub struct Geom<'a> {
    pub text: &'a str,
    pub metrics: Metrics,
    /// Top-left of the first line, in document pixels.
    pub origin: PointF,
}

pub fn geom(shape: &Shape) -> Option<Geom<'_>> {
    match &shape.kind {
        ShapeKind::Text { pos, text, size, bold, max_width, .. } => {
            Some(Geom { text, metrics: Metrics { size: *size, bold: *bold, wrap: *max_width }, origin: *pos })
        }
        ShapeKind::Callout { rect, text, size, .. } => {
            let r = RectF::new(rect.x.min(rect.right()), rect.y.min(rect.bottom()), rect.w.abs(), rect.h.abs());
            let pad = size * 0.6;
            let metrics = Metrics { size: *size, bold: false, wrap: Some((r.w - 2.0 * pad).max(*size)) };
            let height = visual_lines(text, &metrics).len() as f32 * metrics.pitch();
            Some(Geom { text, metrics, origin: PointF::new(r.x + pad, r.y + ((r.h - height) / 2.0).max(size * 0.3)) })
        }
        _ => None,
    }
}

/// The editable text of a shape.
pub fn text_mut(shape: &mut Shape) -> Option<&mut String> {
    match &mut shape.kind {
        ShapeKind::Text { text, .. } | ShapeKind::Callout { text, .. } => Some(text),
        _ => None,
    }
}

/// Character ranges `[start, end)` of every visual line, newline characters excluded.
pub fn visual_lines(text: &str, m: &Metrics) -> Vec<(usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let mut lines = Vec::new();
    let mut start = 0;
    loop {
        let end = chars[start..].iter().position(|c| *c == '\n').map_or(chars.len(), |i| start + i);
        match m.wrap {
            None => lines.push((start, end)),
            Some(width) => wrap_paragraph(&chars, start, end, width.max(m.size), m, &mut lines),
        }
        if end >= chars.len() {
            break;
        }
        start = end + 1;
    }
    lines
}

/// Greedy wrap of `chars[start..end]`, as the renderer does it: words split at single spaces, overlong words by character.
fn wrap_paragraph(chars: &[char], start: usize, end: usize, width: f32, m: &Metrics, out: &mut Vec<(usize, usize)>) {
    let measure = |a: usize, b: usize| m.width(&chars[a..b].iter().collect::<String>());
    let mut cur: Option<(usize, usize)> = None;
    let mut word_start = start;
    loop {
        let word_end = chars[word_start..end].iter().position(|c| *c == ' ').map_or(end, |i| word_start + i);
        let open = cur.filter(|(s, e)| e > s);
        if measure(open.map_or(word_start, |(s, _)| s), word_end) <= width {
            cur = Some((open.map_or(word_start, |(s, _)| s), word_end));
        } else {
            if let Some(line) = open {
                out.push(line);
            }
            let mut piece_start = word_start;
            for i in word_start + 1..word_end {
                if measure(piece_start, i + 1) > width {
                    out.push((piece_start, i));
                    piece_start = i;
                }
            }
            cur = Some((piece_start, word_end));
        }
        if word_end >= end {
            break;
        }
        word_start = word_end + 1;
    }
    out.push(cur.unwrap_or((end, end)));
}

/// The visual line a caret at `idx` is on and its x offset from the line start.
pub fn caret_xy(text: &str, m: &Metrics, lines: &[(usize, usize)], idx: usize) -> (usize, f32) {
    let line = lines
        .iter()
        .enumerate()
        .find(|(i, (_, end))| idx < *end || (idx == *end && lines.get(i + 1).is_none_or(|next| next.0 > idx)))
        .map_or(lines.len().saturating_sub(1), |(i, _)| i);
    let (start, _) = lines[line];
    let prefix: String = text.chars().skip(start).take(idx.saturating_sub(start)).collect();
    (line, m.width(&prefix))
}

/// The character index on `line` whose caret is closest to `x`.
pub fn index_at(text: &str, m: &Metrics, lines: &[(usize, usize)], line: usize, x: f32) -> usize {
    let (start, end) = lines[line.min(lines.len() - 1)];
    let chars: Vec<char> = text.chars().collect();
    let mut best = (start, f32::MAX);
    for idx in start..=end {
        let prefix: String = chars[start..idx].iter().collect();
        let d = (m.width(&prefix) - x).abs();
        if d < best.1 {
            best = (idx, d);
        }
    }
    best.0
}

/// A caret in an editing session.
pub struct TextEdit {
    pub id: u64,
    pub caret: usize,
    /// The other end of the selection; equal to `caret` when nothing is selected.
    pub anchor: usize,
    /// Whether the undo checkpoint for this session has been taken.
    pub checkpointed: bool,
    /// Index in the shape list of a shape created for this session. It is held back from undo until the
    /// first change so that abandoning an empty text leaves no trace.
    pub fresh_at: Option<usize>,
    /// Egui time of the last caret movement, for blinking.
    pub moved_at: f64,
}

/// What a batch of key events did.
#[derive(Default)]
pub struct Edited {
    pub changed: bool,
    pub moved: bool,
    pub finish: bool,
}

impl TextEdit {
    pub fn new(id: u64, caret: usize, fresh_at: Option<usize>) -> Self {
        Self { id, caret, anchor: caret, checkpointed: false, fresh_at, moved_at: 0.0 }
    }

    pub fn selection(&self) -> Option<(usize, usize)> {
        (self.caret != self.anchor).then(|| (self.caret.min(self.anchor), self.caret.max(self.anchor)))
    }

    fn move_to(&mut self, idx: usize, extend: bool) {
        self.caret = idx;
        if !extend {
            self.anchor = idx;
        }
    }

    pub fn selected_text(&self, text: &str) -> Option<String> {
        self.selection().map(|(a, b)| text.chars().skip(a).take(b - a).collect())
    }

    /// Apply `events` to a copy of the shape's text; the caller writes it back (after its undo checkpoint) when `changed`.
    pub fn apply(&mut self, events: &[Event], text: &mut String, m: &Metrics) -> Edited {
        let mut out = Edited::default();
        for event in events {
            match event {
                Event::Text(s) => self.insert(text, s, &mut out),
                Event::Paste(s) => self.insert(text, &s.replace("\r\n", "\n"), &mut out),
                Event::Copy => {
                    if let Some(sel) = self.selected_text(text) {
                        let _ = snap_platform::clipboard::set_text(&sel);
                    }
                }
                Event::Cut => {
                    if let Some(sel) = self.selected_text(text) {
                        let _ = snap_platform::clipboard::set_text(&sel);
                        self.insert(text, "", &mut out);
                    }
                }
                Event::Key { key, pressed: true, modifiers, .. } => self.key(*key, *modifiers, text, m, &mut out),
                _ => {}
            }
        }
        out
    }

    fn insert(&mut self, text: &mut String, s: &str, out: &mut Edited) {
        let s: String = s.chars().filter(|c| !c.is_control() || *c == '\n').collect();
        if s.is_empty() && self.selection().is_none() {
            return;
        }
        if let Some((a, b)) = self.selection() {
            replace_chars(text, a, b, "");
            self.caret = a;
        }
        let at = byte_at(text, self.caret);
        text.insert_str(at, &s);
        self.caret += s.chars().count();
        self.anchor = self.caret;
        out.changed = true;
        out.moved = true;
    }

    fn key(&mut self, key: Key, mods: Modifiers, text: &mut String, m: &Metrics, out: &mut Edited) {
        let len = text.chars().count();
        let extend = mods.shift;
        match key {
            Key::Escape => out.finish = true,
            Key::Enter if mods.command => out.finish = true,
            Key::Enter => self.insert(text, "\n", out),
            Key::Backspace | Key::Delete => {
                let forward = key == Key::Delete;
                if self.selection().is_none() {
                    let target = match (forward, mods.command) {
                        (false, false) => self.caret.saturating_sub(1),
                        (true, false) => (self.caret + 1).min(len),
                        (false, true) => word_boundary(text, self.caret, false),
                        (true, true) => word_boundary(text, self.caret, true),
                    };
                    if target == self.caret {
                        return;
                    }
                    self.anchor = target;
                }
                self.insert(text, "", out);
            }
            Key::ArrowLeft | Key::ArrowRight => {
                let forward = key == Key::ArrowRight;
                let target = match (self.selection(), extend, mods.command) {
                    (Some((a, b)), false, false) => {
                        if forward {
                            b
                        } else {
                            a
                        }
                    }
                    (_, _, true) => word_boundary(text, self.caret, forward),
                    _ if forward => (self.caret + 1).min(len),
                    _ => self.caret.saturating_sub(1),
                };
                self.move_to(target, extend);
                out.moved = true;
            }
            Key::ArrowUp | Key::ArrowDown => {
                let lines = visual_lines(text, m);
                let (line, x) = caret_xy(text, m, &lines, self.caret);
                let target = if key == Key::ArrowUp {
                    if line == 0 { 0 } else { index_at(text, m, &lines, line - 1, x) }
                } else if line + 1 >= lines.len() {
                    len
                } else {
                    index_at(text, m, &lines, line + 1, x)
                };
                self.move_to(target, extend);
                out.moved = true;
            }
            Key::Home | Key::End => {
                let lines = visual_lines(text, m);
                let (line, _) = caret_xy(text, m, &lines, self.caret);
                let target = match (key == Key::Home, mods.command) {
                    (true, true) => 0,
                    (false, true) => len,
                    (true, false) => lines[line].0,
                    (false, false) => lines[line].1,
                };
                self.move_to(target, extend);
                out.moved = true;
            }
            Key::A if mods.command => {
                self.anchor = 0;
                self.caret = len;
                out.moved = true;
            }
            _ => {}
        }
    }
}

fn byte_at(s: &str, char_idx: usize) -> usize {
    s.char_indices().nth(char_idx).map_or(s.len(), |(b, _)| b)
}

fn replace_chars(s: &mut String, from: usize, to: usize, with: &str) {
    let (a, b) = (byte_at(s, from), byte_at(s, to));
    s.replace_range(a..b, with);
}

/// The start of the previous word, or the end of the next one.
fn word_boundary(text: &str, from: usize, forward: bool) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut i = from.min(chars.len());
    if forward {
        while i < chars.len() && !word(chars[i]) {
            i += 1;
        }
        while i < chars.len() && word(chars[i]) {
            i += 1;
        }
    } else {
        while i > 0 && !word(chars[i - 1]) {
            i -= 1;
        }
        while i > 0 && word(chars[i - 1]) {
            i -= 1;
        }
    }
    i
}
