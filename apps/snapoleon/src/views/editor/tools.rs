//! The editor's tools: what they are called, how they look in the rail and which style they start with.

use std::collections::HashMap;

use egui::Key;
use snap_annotate::{ArrowHead, ShapeKind, Stamp, Style, TextBg};
use snap_core::{Settings, settings::RedactStyle, t};
use snap_ui::{icon, widgets::parse_color};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    Select,
    Crop,
    Arrow,
    Line,
    Rect,
    Ellipse,
    Pen,
    Highlighter,
    Text,
    Callout,
    Step,
    Redact,
    Spotlight,
    Magnifier,
    Stamp,
    Measure,
}

impl Tool {
    /// Rail order. The inner slices are separated by a hairline.
    pub const GROUPS: [&'static [Tool]; 5] = [
        &[Tool::Select, Tool::Crop],
        &[Tool::Arrow, Tool::Line, Tool::Rect, Tool::Ellipse, Tool::Pen, Tool::Highlighter],
        &[Tool::Text, Tool::Callout, Tool::Step, Tool::Stamp],
        &[Tool::Redact, Tool::Spotlight, Tool::Magnifier],
        &[Tool::Measure],
    ];

    pub fn icon(self) -> &'static str {
        match self {
            Tool::Select => icon::CURSOR,
            Tool::Crop => icon::CROP,
            Tool::Arrow => icon::ARROW_UP_RIGHT,
            Tool::Line => icon::LINE_SEGMENT,
            Tool::Rect => icon::SQUARE,
            Tool::Ellipse => icon::CIRCLE,
            Tool::Pen => icon::PEN_NIB,
            Tool::Highlighter => icon::HIGHLIGHTER,
            Tool::Text => icon::TEXT_T,
            Tool::Callout => icon::CHAT_TEARDROP_TEXT,
            Tool::Step => icon::NUMBER_CIRCLE_ONE,
            Tool::Redact => icon::EYE_SLASH,
            Tool::Spotlight => icon::FLASHLIGHT,
            Tool::Magnifier => icon::MAGNIFYING_GLASS_PLUS,
            Tool::Stamp => icon::STAMP,
            Tool::Measure => icon::RULER,
        }
    }

    pub fn key(self) -> Key {
        match self {
            Tool::Select => Key::V,
            Tool::Crop => Key::C,
            Tool::Arrow => Key::A,
            Tool::Line => Key::L,
            Tool::Rect => Key::R,
            Tool::Ellipse => Key::O,
            Tool::Pen => Key::P,
            Tool::Highlighter => Key::H,
            Tool::Text => Key::T,
            Tool::Callout => Key::B,
            Tool::Step => Key::N,
            Tool::Redact => Key::X,
            Tool::Spotlight => Key::S,
            Tool::Magnifier => Key::M,
            Tool::Stamp => Key::K,
            Tool::Measure => Key::U,
        }
    }

    /// The hotkey as shown in tooltips.
    pub fn hotkey(self) -> &'static str {
        self.key().name()
    }

    /// Stable name, stored in `settings.editor.last_tool`.
    pub fn id(self) -> &'static str {
        match self {
            Tool::Select => "select",
            Tool::Crop => "crop",
            Tool::Arrow => "arrow",
            Tool::Line => "line",
            Tool::Rect => "rect",
            Tool::Ellipse => "ellipse",
            Tool::Pen => "pen",
            Tool::Highlighter => "highlighter",
            Tool::Text => "text",
            Tool::Callout => "callout",
            Tool::Step => "step",
            Tool::Redact => "redact",
            Tool::Spotlight => "spotlight",
            Tool::Magnifier => "magnifier",
            Tool::Stamp => "stamp",
            Tool::Measure => "measure",
        }
    }

    pub fn from_id(id: &str) -> Option<Tool> {
        Tool::GROUPS.iter().flat_map(|g| g.iter()).copied().find(|t| t.id() == id)
    }

    pub fn label(self) -> String {
        t(&format!("editor.tool.{}", self.id())).into_owned()
    }

    /// The tool whose options edit a shape of this kind. Pasted images have none.
    pub fn of_kind(kind: &ShapeKind) -> Option<Tool> {
        Some(match kind {
            ShapeKind::Arrow { .. } => Tool::Arrow,
            ShapeKind::Line { .. } => Tool::Line,
            ShapeKind::Rect { .. } => Tool::Rect,
            ShapeKind::Ellipse { .. } => Tool::Ellipse,
            ShapeKind::Pen { .. } => Tool::Pen,
            ShapeKind::Highlighter { .. } => Tool::Highlighter,
            ShapeKind::Text { .. } => Tool::Text,
            ShapeKind::Callout { .. } => Tool::Callout,
            ShapeKind::Step { .. } => Tool::Step,
            ShapeKind::Redact { .. } => Tool::Redact,
            ShapeKind::Spotlight { .. } => Tool::Spotlight,
            ShapeKind::Magnifier { .. } => Tool::Magnifier,
            ShapeKind::Stamp { .. } => Tool::Stamp,
            ShapeKind::Measure { .. } => Tool::Measure,
            ShapeKind::Image { .. } => return None,
        })
    }
}

/// Options that are not part of a shape's [`Style`]; shared by the tools that use them.
#[derive(Clone, Debug, PartialEq)]
pub struct Extras {
    pub font_size: f32,
    pub bold: bool,
    pub text_bg: TextBg,
    pub arrow_head: ArrowHead,
    pub curved: bool,
    pub double: bool,
    pub rounded: bool,
    pub redact: RedactStyle,
    pub spot_ellipse: bool,
    pub stamp: Stamp,
}

/// The style every tool starts with, scaled to the capture's density.
pub struct ToolSet {
    pub styles: HashMap<Tool, Style>,
    pub extras: Extras,
    pub palette: Vec<[u8; 4]>,
}

impl ToolSet {
    pub fn new(settings: &Settings, ui_scale: f32) -> Self {
        let editor = &settings.editor;
        let palette: Vec<[u8; 4]> = editor.palette.iter().filter_map(|c| parse_color(c)).collect();
        let palette = if palette.is_empty() { snap_ui::widgets::DEFAULT_PALETTE.to_vec() } else { palette };
        let width = editor.stroke_width.clamp(1.0, 40.0) * ui_scale;
        let base = Style { stroke: palette[0], width, ..Style::default() };
        let mut styles = HashMap::new();
        for tool in Tool::GROUPS.iter().flat_map(|g| g.iter()) {
            styles.insert(*tool, base);
        }
        let yellow = [255, 204, 0, 255];
        styles.insert(Tool::Highlighter, Style { stroke: yellow, width: (width * 1.6).round(), ..base });
        styles.insert(Tool::Step, Style { fill: Some([255, 255, 255, 255]), ..base });
        styles.insert(Tool::Text, Style { fill: None, shadow: false, ..base });
        styles.insert(Tool::Redact, Style { stroke: [0, 0, 0, 255], ..base });
        styles.insert(Tool::Measure, Style { width: (width * 0.5).max(2.0), ..base });
        let extras = Extras {
            font_size: (editor.font_size.clamp(8.0, 200.0) * ui_scale).round(),
            bold: false,
            text_bg: TextBg::None,
            arrow_head: ArrowHead::Triangle,
            curved: false,
            double: false,
            rounded: false,
            redact: settings.privacy.redact_style,
            spot_ellipse: false,
            stamp: Stamp::Check,
        };
        Self { styles, extras, palette }
    }
}
