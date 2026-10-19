//! Linux extras next to the xcap backend: pointer, input injection through XTest. Accessibility
//! elements (AT-SPI) are not implemented.

use std::{thread, time::Duration};

use snap_core::{Error, Point, Rect, Result, RgbaImage};
use xcb::{Connection, x, xtest};

use crate::{ElementInfo, WindowInfo};

const MOTION_NOTIFY: u8 = 6;
const BUTTON_PRESS: u8 = 4;
const BUTTON_RELEASE: u8 = 5;
const WHEEL_UP: u8 = 4;
const WHEEL_DOWN: u8 = 5;

fn connect() -> Result<(Connection, x::Window)> {
    let (conn, index) = Connection::connect(None).map_err(|e| Error::Other(anyhow::anyhow!("X11 connection: {e}")))?;
    let root = conn.get_setup().roots().nth(index as usize).map(|s| s.root()).ok_or_else(|| Error::NotFound("X11 screen".into()))?;
    Ok((conn, root))
}

/// Pointer position in root-window pixels. `None` without an X server (pure Wayland).
pub fn pointer() -> Option<(f64, f64)> {
    let (conn, root) = connect().ok()?;
    let reply = conn.wait_for_reply(conn.send_request(&x::QueryPointer { window: root })).ok()?;
    Some((reply.root_x() as f64, reply.root_y() as f64))
}

pub fn exe_path(pid: u32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok().map(|p| p.to_string_lossy().into_owned())
}

/// X11 needs no grant; Wayland asks through the desktop portal at capture time.
pub fn check_capture_access() -> Result<()> {
    Ok(())
}

pub fn trim_window_frame(img: RgbaImage, _expected: Rect) -> RgbaImage {
    img
}

pub fn elements_at(_p: Point) -> Result<Vec<ElementInfo>> {
    Err(Error::unsupported("accessibility elements on Linux (AT-SPI) are not implemented"))
}

pub fn browser_url(_w: &WindowInfo) -> Option<String> {
    None
}

/// Linux desktops have no common icon layer to hide, so no guard is ever created.
pub enum IconsGuard {}

pub fn hide_desktop_icons() -> Option<IconsGuard> {
    None
}

/// Wheel scrolling through the XTest extension (X11 and XWayland only).
pub fn scroll_at(p: Point, notches: i32) -> Result<()> {
    if notches == 0 {
        return Ok(());
    }
    let (conn, root) = connect().map_err(|_| Error::unsupported("scrolling needs an X11 display"))?;
    let fake = |kind: u8, detail: u8, x: i16, y: i16| {
        conn.send_and_check_request(&xtest::FakeInput { r#type: kind, detail, time: 0, root, root_x: x, root_y: y, deviceid: 0 })
            .map_err(|e| Error::Other(anyhow::anyhow!("XTest: {e}")))
    };
    fake(MOTION_NOTIFY, 0, p.x as i16, p.y as i16)?;
    thread::sleep(Duration::from_millis(10));
    let button = if notches > 0 { WHEEL_DOWN } else { WHEEL_UP };
    for _ in 0..notches.unsigned_abs() {
        fake(BUTTON_PRESS, button, 0, 0)?;
        fake(BUTTON_RELEASE, button, 0, 0)?;
    }
    conn.flush().map_err(|e| Error::Other(anyhow::anyhow!("X11 flush: {e}")))
}
