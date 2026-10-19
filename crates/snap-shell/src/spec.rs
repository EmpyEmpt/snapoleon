use snap_core::{Point, Rect, RgbaImage, t};

/// Stable handle of a shell window; never reused within a run.
pub type WindowKey = u64;

/// Everything needed to open a window. Build with struct-update syntax from
/// [`WindowSpec::default`].
#[derive(Debug, Clone)]
pub struct WindowSpec {
    pub title: String,
    /// Initial inner size in logical pixels.
    pub size: Option<(f32, f32)>,
    /// Minimum inner size in logical pixels.
    pub min_size: Option<(f32, f32)>,
    /// Initial outer position in physical, virtual-desktop pixels.
    pub position: Option<Point>,
    /// Exact physical rect for overlay windows: borderless, topmost, no shadow,
    /// no taskbar entry. Takes precedence over `size`, `position` and the
    /// decoration flags, and is re-enforced whenever the OS moves or rescales
    /// the window (mixed-DPI multi-monitor setups).
    pub cover: Option<Rect>,
    pub decorations: bool,
    pub resizable: bool,
    pub always_on_top: bool,
    pub skip_taskbar: bool,
    /// Show the window as soon as its first frame has been rendered. Windows
    /// with `visible: false` still render a first frame while hidden so that
    /// [`Shell::set_visible`](crate::Shell::set_visible) is instant.
    pub visible: bool,
    /// Tool/utility window: no taskbar or Alt-Tab entry (Windows
    /// `WS_EX_TOOLWINDOW`), floating level on all spaces (macOS), X11
    /// `_NET_WM_WINDOW_TYPE_UTILITY`.
    pub tool_window: bool,
    /// Take the keyboard focus when first shown. `false` for windows that must not disturb what the user is
    /// looking at (a menu that closes when its window loses focus), like a countdown before a delayed capture.
    pub activate: bool,
    /// Hide the window from screenshots and screen recordings (Windows and macOS).
    pub exclude_from_capture: bool,
    /// Rounded corners (Windows 11 DWM); no-op elsewhere.
    pub rounded: bool,
    /// Clip the window to a pill (fully round ends). Windows only; elsewhere the window stays a rectangle.
    /// Changeable later with [`Shell::set_pill`](crate::Shell::set_pill).
    pub pill: bool,
    pub icon: Option<RgbaImage>,
    /// Free-form label for [`Shell::windows_with_tag`](crate::Shell::windows_with_tag).
    pub tag: String,
}

impl Default for WindowSpec {
    fn default() -> Self {
        Self {
            title: t("shell.default_title").into_owned(),
            size: None,
            min_size: None,
            position: None,
            cover: None,
            decorations: true,
            resizable: true,
            always_on_top: false,
            skip_taskbar: false,
            visible: true,
            tool_window: false,
            activate: true,
            exclude_from_capture: false,
            rounded: false,
            pill: false,
            icon: None,
            tag: String::new(),
        }
    }
}

impl WindowSpec {
    /// Overlay window pinned to an exact physical rect.
    pub fn cover(rect: Rect) -> Self {
        Self {
            cover: Some(rect),
            decorations: false,
            resizable: false,
            always_on_top: true,
            skip_taskbar: true,
            tool_window: true,
            ..Self::default()
        }
    }

    /// True when the OS must be told this window is a tool window.
    pub(crate) fn is_tool(&self) -> bool {
        self.tool_window || self.skip_taskbar || self.cover.is_some()
    }
}

/// Process-wide shell settings.
pub struct ShellConfig {
    /// Reverse-DNS application id (X11 `WM_CLASS`, Wayland `app_id`).
    pub app_id: String,
    /// macOS: run as an accessory app (no Dock icon, no menu bar).
    pub accessory: bool,
    /// Applied to every new `egui::Context` before its first frame
    /// (fonts, visuals, style).
    pub theme_init: Box<dyn Fn(&egui::Context)>,
}

impl ShellConfig {
    pub fn new(app_id: impl Into<String>) -> Self {
        Self { app_id: app_id.into(), accessory: false, theme_init: Box::new(|_| {}) }
    }
}
