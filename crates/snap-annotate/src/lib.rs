//! Non-destructive annotation documents for screenshots.
//!
//! A [`Document`] is an untouched base image plus a list of vector [`Shape`]s. [`render`] rasterizes
//! it for export (destructive redaction, crop, readable downscaling, beautify) and [`render_view`]
//! draws it at view resolution for the live editor. [`hit_test`], [`handles`] and [`drag`]
//! implement selection and editing, [`Undo`] keeps history, and [`save_project`] /
//! [`load_project`] read and write `.snap` files.

mod cache;
mod effects;
mod font;
mod geometry;
mod interact;
mod math;
mod model;
mod paint;
mod project;
mod render;
mod resample;
mod shapes;
mod stamps;

pub use interact::{Handle, bounds, drag, handles, hit_test};
pub use model::{ArrowHead, DocMeta, Document, Shape, ShapeKind, Stamp, Style, TextBg, Undo};
pub use project::{load_project, save_project};
pub use render::{RenderOptions, View, render, render_view};
pub use tiny_skia;

/// Measured size of wrapped text at em size `size`, for caret and box placement in the editor.
/// `max_w` wraps lines; the returned width is that of the widest line.
pub fn text_layout(text: &str, size: f32, bold: bool, max_w: Option<f32>) -> (f32, f32) {
    let l = font::layout(font::Face::pick(bold), text, size, max_w);
    (l.width, l.height())
}
