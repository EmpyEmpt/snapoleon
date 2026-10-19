//! Pure-Rust image operations: readable downscale, redaction, beautify, stitching, encoding.
//!
//! Images are straight-alpha sRGB [`image::RgbaImage`]s; rectangles are `snap_core::Rect` in
//! image pixels. Nothing here touches the OS, so every function behaves identically on all platforms.

pub mod beautify;
pub mod color;
pub mod combine;
pub mod diff;
pub mod effects;
pub mod encode;
pub mod resize;
pub mod stitch;

mod blur;
mod frame;
mod text;
mod util;
