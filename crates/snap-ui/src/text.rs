//! Type scale: named egui `TextStyle`s and `RichText` shortcuts.

use std::collections::BTreeMap;

use egui::{FontFamily, FontId, RichText, Style, TextStyle};

use crate::fonts;

/// Inter 11 / regular (dimmed by [`caption`]).
pub fn caption_style() -> TextStyle {
    TextStyle::Name("caption".into())
}

/// Inter 13 / regular.
pub fn body_style() -> TextStyle {
    TextStyle::Body
}

/// Inter 13 / semibold.
pub fn body_strong_style() -> TextStyle {
    TextStyle::Name("body_strong".into())
}

/// Inter 16 / semibold.
pub fn title_style() -> TextStyle {
    TextStyle::Name("title".into())
}

/// Inter 22 / bold.
pub fn headline_style() -> TextStyle {
    TextStyle::Name("headline".into())
}

/// JetBrains Mono 12 / medium.
pub fn mono() -> TextStyle {
    TextStyle::Monospace
}

pub(crate) fn apply_text_styles(style: &mut Style) {
    let inter = |size: f32| FontId::new(size, FontFamily::Proportional);
    let styles: BTreeMap<TextStyle, FontId> = [
        (TextStyle::Small, inter(11.0)),
        (TextStyle::Body, inter(13.0)),
        (TextStyle::Button, FontId::new(13.0, fonts::semibold())),
        (TextStyle::Heading, FontId::new(22.0, fonts::bold())),
        (TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace)),
        (caption_style(), inter(11.0)),
        (body_strong_style(), FontId::new(13.0, fonts::semibold())),
        (title_style(), FontId::new(16.0, fonts::semibold())),
        (headline_style(), FontId::new(22.0, fonts::bold())),
    ]
    .into();
    style.text_styles = styles;
}

/// Small dimmed text for secondary information.
pub fn caption(s: impl Into<String>) -> RichText {
    RichText::new(s).text_style(caption_style()).weak()
}

pub fn body_strong(s: impl Into<String>) -> RichText {
    RichText::new(s).text_style(body_strong_style())
}

pub fn title(s: impl Into<String>) -> RichText {
    RichText::new(s).text_style(title_style())
}

/// 22 px bold with slightly tightened tracking.
pub fn headline(s: impl Into<String>) -> RichText {
    RichText::new(s).text_style(headline_style()).extra_letter_spacing(-0.3)
}

/// Monospace for numbers, sizes, hex values and hotkeys.
pub fn mono_text(s: impl Into<String>) -> RichText {
    RichText::new(s).text_style(mono())
}

/// Dimmed body text.
pub fn dim(s: impl Into<String>) -> RichText {
    RichText::new(s).weak()
}
