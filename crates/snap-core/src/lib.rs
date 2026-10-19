//! Snapoleon core: the contracts every other crate speaks.
//!
//! Nothing in here touches the OS beyond resolving directories. Keep it that
//! way: this crate must stay cheap to compile because everything depends on it.

pub mod capture;
pub mod error;
pub mod fonts;
pub mod geom;
pub mod hotkey;
pub mod i18n;
pub mod ipc;
pub mod naming;
pub mod paths;
pub mod pipeline;
pub mod policy;
pub mod secrets;
pub mod settings;

pub use capture::{Capture, CaptureContext, CaptureId, CaptureKind};
pub use error::{Error, Result};
pub use geom::{Point, PointF, Rect, RectF, Size};
pub use i18n::{t, tf};
pub use image::RgbaImage;
pub use settings::Settings;

pub const APP_NAME: &str = "Snapoleon";
pub const APP_ID: &str = "app.snapoleon";
pub const URL_SCHEME: &str = "snapoleon";
pub const PROJECT_EXT: &str = "snap";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
