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

/// Read a window by X11 id straight off the server, without needing window-manager (EWMH) support.
/// Returns the pixels and the window's position on the root.
pub fn window_by_id(id: u64) -> Result<(RgbaImage, Rect)> {
    let x_err = |e: xcb::Error| Error::Other(anyhow::anyhow!("X11: {e}"));
    let id = u32::try_from(id).map_err(|_| Error::invalid("window id"))?;
    let window: x::Window = xcb::XidNew::new(id);
    let (conn, root) = connect()?;
    let geometry = conn.wait_for_reply(conn.send_request(&x::GetGeometry { drawable: x::Drawable::Window(window) })).map_err(x_err)?;
    let origin = conn
        .wait_for_reply(conn.send_request(&x::TranslateCoordinates { src_window: window, dst_window: root, src_x: 0, src_y: 0 }))
        .map_err(x_err)?;
    let (w, h) = (geometry.width() as u32, geometry.height() as u32);
    let reply = conn
        .wait_for_reply(conn.send_request(&x::GetImage {
            format: x::ImageFormat::ZPixmap,
            drawable: x::Drawable::Window(window),
            x: 0,
            y: 0,
            width: geometry.width(),
            height: geometry.height(),
            plane_mask: u32::MAX,
        }))
        .map_err(x_err)?;
    let bgrx = reply.data();
    if bgrx.len() < w as usize * h as usize * 4 {
        return Err(Error::unsupported(format!("{}-bit X11 visuals", reply.depth())));
    }
    let mut rgba = Vec::with_capacity(bgrx.len());
    for px in bgrx.as_chunks::<4>().0 {
        rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
    }
    let image = RgbaImage::from_raw(w, h, rgba).ok_or_else(|| Error::invalid("X11 image size"))?;
    Ok((image, Rect::new(origin.dst_x() as i32, origin.dst_y() as i32, w, h)))
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
