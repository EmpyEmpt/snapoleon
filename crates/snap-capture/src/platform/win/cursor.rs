//! Cursor position and a hotspot-correct RGBA rendering of the current cursor.

use std::{ffi::c_void, mem, ptr, slice};

use snap_core::{Point, RgbaImage};
use windows::Win32::{
    Foundation::POINT,
    Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject,
        GdiFlush, GetDC, GetObjectW, HGDIOBJ, ReleaseDC, SelectObject,
    },
    UI::WindowsAndMessaging::{
        CURSOR_SHOWING, CURSORINFO, DI_NORMAL, DrawIconEx, GetCursorInfo, GetCursorPos, GetIconInfo, HICON, ICONINFO,
    },
};

use crate::platform::CursorSprite;

pub fn position() -> Point {
    let mut p = POINT::default();
    let _ = unsafe { GetCursorPos(&mut p) };
    Point::new(p.x, p.y)
}

pub fn sprite() -> Option<CursorSprite> {
    let mut ci = CURSORINFO { cbSize: mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
    unsafe { GetCursorInfo(&mut ci) }.ok()?;
    if ci.flags.0 & CURSOR_SHOWING.0 == 0 {
        return None;
    }
    let hicon = HICON(ci.hCursor.0);
    let mut info = ICONINFO::default();
    unsafe { GetIconInfo(hicon, &mut info) }.ok()?;

    // Monochrome cursors stack the AND and XOR masks in one bitmap of double height.
    let color_bitmap = !info.hbmColor.is_invalid();
    let mut bm = BITMAP::default();
    let source = if color_bitmap { info.hbmColor } else { info.hbmMask };
    let got = unsafe { GetObjectW(HGDIOBJ(source.0), mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut c_void)) };
    unsafe {
        if color_bitmap {
            let _ = DeleteObject(HGDIOBJ(info.hbmColor.0));
        }
        let _ = DeleteObject(HGDIOBJ(info.hbmMask.0));
    }
    if got == 0 {
        return None;
    }
    let (w, h) = (bm.bmWidth, if color_bitmap { bm.bmHeight } else { bm.bmHeight / 2 });
    if w <= 0 || h <= 0 {
        return None;
    }

    // Draw the cursor over black and over white; the difference reveals per-pixel alpha for both
    // alpha cursors and classic mask cursors.
    let on_black = render(hicon, w, h, 0x00)?;
    let on_white = render(hicon, w, h, 0xff)?;
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let (black, _) = on_black.as_chunks::<4>();
    let (white, _) = on_white.as_chunks::<4>();
    let (pixels, _) = rgba.as_chunks_mut::<4>();
    for ((b, wh), out) in black.iter().zip(white).zip(pixels) {
        let alpha = (0..3).map(|c| 255 - (wh[c] as i32 - b[c] as i32).clamp(0, 255)).max().unwrap_or(0);
        if alpha == 0 {
            continue;
        }
        // BGRA → RGBA, un-premultiplying the black-background colour.
        for (dst, src) in out[..3].iter_mut().zip([b[2], b[1], b[0]]) {
            *dst = (src as i32 * 255 / alpha).min(255) as u8;
        }
        out[3] = alpha as u8;
    }
    let image = RgbaImage::from_raw(w as u32, h as u32, rgba)?;
    let origin = Point::new(ci.ptScreenPos.x - info.xHotspot as i32, ci.ptScreenPos.y - info.yHotspot as i32);
    Some(CursorSprite { image, origin })
}

/// BGRA pixels of `icon` drawn over a solid grey level `bg`.
fn render(icon: HICON, w: i32, h: i32, bg: u8) -> Option<Vec<u8>> {
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    unsafe {
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen));
        let mut bits: *mut c_void = ptr::null_mut();
        let bitmap = CreateDIBSection(Some(screen), &bmi, DIB_RGB_COLORS, &mut bits, None, 0);
        let result = bitmap.ok().and_then(|bitmap| {
            let old = SelectObject(dc, HGDIOBJ(bitmap.0));
            let len = (w * h * 4) as usize;
            slice::from_raw_parts_mut(bits as *mut u8, len).fill(bg);
            let drawn = DrawIconEx(dc, 0, 0, icon, w, h, 0, None, DI_NORMAL).is_ok();
            let _ = GdiFlush();
            let pixels = drawn.then(|| slice::from_raw_parts(bits as *const u8, len).to_vec());
            SelectObject(dc, old);
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            pixels
        });
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
        result
    }
}
