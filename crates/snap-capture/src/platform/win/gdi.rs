//! Fast screen copies with GDI BitBlt into a DIB section.

use std::{ffi::c_void, mem, ptr, slice};

use snap_core::{Error, Rect, Result, RgbaImage};

use super::os_err;
use windows::Win32::{
    Foundation::HWND,
    Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
        DeleteObject, GdiFlush, GetDC, HBITMAP, HDC, HGDIOBJ, ReleaseDC, SRCCOPY, SelectObject,
    },
};

struct ScreenDc(HDC);
impl Drop for ScreenDc {
    fn drop(&mut self) {
        unsafe { ReleaseDC(Some(HWND::default()), self.0) };
    }
}

struct MemDc(HDC);
impl Drop for MemDc {
    fn drop(&mut self) {
        let _ = unsafe { DeleteDC(self.0) };
    }
}

struct Bitmap(HBITMAP);
impl Drop for Bitmap {
    fn drop(&mut self) {
        let _ = unsafe { DeleteObject(HGDIOBJ(self.0.0)) };
    }
}

/// Copy `region` (virtual-desktop pixels) of the composed desktop. Alpha is forced opaque.
pub fn grab(region: Rect) -> Result<RgbaImage> {
    let (w, h) = (region.w as i32, region.h as i32);
    if w <= 0 || h <= 0 {
        return Err(Error::invalid("empty capture region"));
    }
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

    let screen = ScreenDc(unsafe { GetDC(None) });
    if screen.0.is_invalid() {
        return Err(os_err("GetDC", "no screen device context"));
    }
    let mem_dc = MemDc(unsafe { CreateCompatibleDC(Some(screen.0)) });
    let mut bits: *mut c_void = ptr::null_mut();
    let bitmap = Bitmap(
        unsafe { CreateDIBSection(Some(screen.0), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) }.map_err(|e| os_err("CreateDIBSection", e))?,
    );
    let old = unsafe { SelectObject(mem_dc.0, HGDIOBJ(bitmap.0.0)) };
    let blt = unsafe { BitBlt(mem_dc.0, 0, 0, w, h, Some(screen.0), region.x, region.y, SRCCOPY | CAPTUREBLT) };
    unsafe {
        let _ = GdiFlush();
        SelectObject(mem_dc.0, old);
    }
    blt.map_err(|e| os_err("BitBlt", e))?;

    let len = w as usize * h as usize * 4;
    let src = unsafe { slice::from_raw_parts(bits as *const u8, len) };
    let mut out = vec![0u8; len];
    bgra_to_rgba_opaque(src, &mut out);
    Ok(RgbaImage::from_raw(region.w, region.h, out).expect("buffer size matches dimensions"))
}

fn bgra_to_rgba_opaque(src: &[u8], dst: &mut [u8]) {
    fn convert(src: &[u8], dst: &mut [u8]) {
        for (s, d) in src.as_chunks::<4>().0.iter().zip(dst.as_chunks_mut::<4>().0) {
            *d = [s[2], s[1], s[0], 255];
        }
    }
    // ~8 M pixels (4K) take noticeably long on one core; split across cores.
    if src.len() < 4 << 20 {
        return convert(src, dst);
    }
    let workers = rayon::current_num_threads().min(8);
    let chunk = (src.len() / 4).div_ceil(workers) * 4;
    rayon::scope(|s| {
        for (s_chunk, d_chunk) in src.chunks(chunk).zip(dst.chunks_mut(chunk)) {
            s.spawn(move |_| convert(s_chunk, d_chunk));
        }
    });
}
