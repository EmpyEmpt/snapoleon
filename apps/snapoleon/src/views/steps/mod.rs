//! Step capture: a session HUD that records screenshots (a step per click) and, when the session
//! finishes, the guide editor with captions, reordering, AI rewrite and Markdown / HTML / MP4
//! export. The HUD is the singleton `TAG` window; each finished guide gets its own editor window.

mod cards;
mod demo;
mod export;
mod guide;
mod hud;
mod session;
mod side;

use snap_core::{Point, t};
use snap_shell::WindowSpec;

use super::{OpenCx, app_icon};

pub const TAG: &str = "steps";
/// Tag of the guide editor windows.
const GUIDE_TAG: &str = "steps-guide";

/// Logical size of the HUD.
const HUD_SIZE: (f32, f32) = (440.0, 60.0);
/// Distance of the HUD from the top of the screen, in logical pixels.
const HUD_TOP: f32 = 24.0;

/// Messages the app sends to a running step capture window.
pub enum StepsMsg {
    /// The step-capture hotkey was pressed while a session is running: finish it and open the guide.
    Toggle,
}

/// Starts a session. When one is already running, the window is brought forward instead.
pub fn open(cx: &mut OpenCx) {
    if cx.focus_tagged(TAG) {
        return;
    }
    if demo::mode() == Some(demo::Demo::Guide) {
        guide::open_demo(cx, demo::guide());
        return;
    }
    let (area, scale) = cx.shell.monitors().first().copied().unwrap_or((snap_core::Rect::default(), 1.0));
    let x = area.x + ((area.w as f32 - HUD_SIZE.0 * scale) / 2.0) as i32;
    let spec = WindowSpec {
        title: t("steps.hud.title").into_owned(),
        size: Some(HUD_SIZE),
        position: Some(Point::new(x, area.y + (HUD_TOP * scale) as i32)),
        decorations: false,
        resizable: false,
        rounded: true,
        always_on_top: true,
        tool_window: true,
        // The user is about to work in other apps: keep their focus.
        activate: false,
        exclude_from_capture: true,
        icon: Some(app_icon()),
        tag: TAG.into(),
        ..WindowSpec::default()
    };
    cx.open(spec, hud::HudView::new());
}
