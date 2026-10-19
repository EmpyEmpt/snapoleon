//! The fonts every crate draws text with, embedded once. They are subsets (see `tools/subset_fonts.py`): Latin,
//! Cyrillic, punctuation, arrows and a few symbols. Anything else falls back to the system or egui's fonts.

pub static INTER_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
pub static INTER_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");
pub static INTER_BOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-Bold.ttf");
pub static JETBRAINS_MONO: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Medium.ttf");
