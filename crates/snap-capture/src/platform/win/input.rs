//! Synthetic input.

use std::{mem, thread, time::Duration};

use snap_core::{Error, Point, Result, t};
use windows::Win32::UI::{
    Input::KeyboardAndMouse::{INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput},
    WindowsAndMessaging::SetCursorPos,
};

use super::os_err;

const WHEEL_DELTA: i32 = 120;

/// Move the cursor to `p` and turn the wheel `notches` times (positive = scroll down).
pub fn scroll_at(p: Point, notches: i32) -> Result<()> {
    if notches == 0 {
        return Ok(());
    }
    unsafe { SetCursorPos(p.x, p.y) }.map_err(|e| os_err("SetCursorPos", e))?;
    // Let the target window see the hover before the wheel arrives.
    thread::sleep(Duration::from_millis(10));

    let delta = -notches.signum() * WHEEL_DELTA;
    let wheel = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: MOUSEINPUT { mouseData: delta as u32, dwFlags: MOUSEEVENTF_WHEEL, ..Default::default() } },
    };
    let inputs = vec![wheel; notches.unsigned_abs() as usize];
    let sent = unsafe { SendInput(&inputs, mem::size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        return Err(Error::permission(t("capture.error.input_blocked")));
    }
    Ok(())
}
