//! Snapoleon design system for egui: theme, icons, widgets.
//!
//! Call [`install`] once per `egui::Context`, then build screens from [`widgets`] and the
//! tokens in [`theme`]. See `docs/DESIGN.md` for the language these implement.

mod brand;
mod fonts;
pub mod icon;
mod platform;
pub mod text;
mod theme;
mod toasts;
pub mod widgets;

pub use brand::{LogoColors, app_icon_rgba, app_icon_svg, logo, logo_colored, tray_icon_rgba, tray_icon_rgba_colored};
pub use platform::{reduce_motion, system_dark};
pub use theme::{CONTROL_HEIGHT, CONTROL_HEIGHT_COMPACT, Palette, ThemeSpec, Tone, install, palette, parse_hex, radius, space, style_for};
pub use toasts::{Toast, ToastKind, Toasts};
