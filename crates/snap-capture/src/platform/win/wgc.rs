//! Windows.Graphics.Capture: occluded windows and HDR (FP16 scRGB) monitors.

use std::{
    slice, thread,
    time::{Duration, Instant},
};

use parking_lot::Mutex;
use snap_core::{Error, Rect, Result, RgbaImage, settings::HdrMode};
use windows::{
    Graphics::{
        Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem},
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
    },
    Win32::{
        Foundation::{HMODULE, HWND, RECT},
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_HARDWARE,
            Direct3D11::{
                D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION,
                D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread,
                ID3D11Texture2D,
            },
            Dxgi::IDXGIDevice,
            Gdi::{HMONITOR, MONITOR_DEFAULTTONEAREST, MonitorFromWindow},
        },
        System::WinRT::{
            Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess},
            Graphics::Capture::IGraphicsCaptureItemInterop,
        },
        UI::WindowsAndMessaging::GetWindowRect,
    },
    core::{Interface, factory},
};

use super::{com, hdr, monitors, os_err, rect_of};
use crate::{
    MonitorInfo,
    platform::{Grab, WindowGrab},
};

const FRAME_TIMEOUT: Duration = Duration::from_millis(2000);

#[derive(Clone)]
struct D3d {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt: IDirect3DDevice,
}

// SAFETY: D3D11 devices are free-threaded and the immediate context is switched to multithread-protected
// mode on creation; the WinRT device wrapper is agile.
unsafe impl Send for D3d {}

static D3D: Mutex<Option<D3d>> = Mutex::new(None);

fn d3d() -> Result<D3d> {
    let mut cached = D3D.lock();
    if let Some(d) = cached.as_ref() {
        return Ok(d.clone());
    }
    let (mut device, mut context) = (None, None);
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
    }
    .map_err(|e| os_err("D3D11CreateDevice", e))?;
    let (device, context) = (device.expect("device returned on success"), context.expect("context returned on success"));
    if let Ok(mt) = context.cast::<ID3D11Multithread>() {
        let _ = unsafe { mt.SetMultithreadProtected(true) };
    }
    let dxgi: IDXGIDevice = device.cast().map_err(|e| os_err("IDXGIDevice", e))?;
    let winrt: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
        .and_then(|d| d.cast())
        .map_err(|e| os_err("CreateDirect3D11DeviceFromDXGIDevice", e))?;
    let d = D3d { device, context, winrt };
    *cached = Some(d.clone());
    Ok(d)
}

/// Forget the cached device (e.g. after a GPU reset) so the next capture builds a fresh one.
fn reset_device() {
    *D3D.lock() = None;
}

/// Pixels copied out of a capture frame, tightly packed.
struct RawFrame {
    width: u32,
    height: u32,
    fp16: bool,
    data: Vec<u8>,
}

/// Capture one frame of `item`, copying only `crop` (frame-relative; whole content when `None`).
fn grab_frame(item: &GraphicsCaptureItem, fp16: bool, crop: Option<Rect>) -> Result<RawFrame> {
    com::ensure();
    let d = d3d()?;
    let size = item.Size().map_err(|e| os_err("GraphicsCaptureItem.Size", e))?;
    let format = if fp16 { DirectXPixelFormat::R16G16B16A16Float } else { DirectXPixelFormat::B8G8R8A8UIntNormalized };
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&d.winrt, format, 2, size).map_err(|e| os_err("frame pool", e))?;
    let session = pool.CreateCaptureSession(item).map_err(|e| os_err("capture session", e))?;
    // Both are best effort: older Windows lacks the border switch, and we draw the cursor ourselves.
    let _ = session.SetIsCursorCaptureEnabled(false);
    let _ = session.SetIsBorderRequired(false);
    session.StartCapture().map_err(|e| os_err("StartCapture", e))?;

    let result = (|| {
        let deadline = Instant::now() + FRAME_TIMEOUT;
        let frame = loop {
            match pool.TryGetNextFrame() {
                Ok(f) => break f,
                Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(2)),
                Err(_) => return Err(Error::Other(anyhow::anyhow!("no frame arrived from Windows.Graphics.Capture"))),
            }
        };
        let content = frame.ContentSize().map_err(|e| os_err("ContentSize", e))?;
        let surface = frame.Surface().map_err(|e| os_err("frame surface", e))?;
        let access: IDirect3DDxgiInterfaceAccess = surface.cast().map_err(|e| os_err("surface access", e))?;
        let texture: ID3D11Texture2D = unsafe { access.GetInterface() }.map_err(|e| os_err("surface texture", e))?;
        let area = Rect::new(0, 0, content.Width.max(0) as u32, content.Height.max(0) as u32);
        let crop = crop.and_then(|c| c.intersect(area)).unwrap_or(area);
        read_texture(&d, &texture, crop, fp16)
    })();

    let _ = session.Close();
    let _ = pool.Close();
    if result.is_err() {
        reset_device();
    }
    result
}

fn read_texture(d: &D3d, texture: &ID3D11Texture2D, crop: Rect, fp16: bool) -> Result<RawFrame> {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { texture.GetDesc(&mut desc) };
    desc.Usage = D3D11_USAGE_STAGING;
    desc.BindFlags = 0;
    desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
    desc.MiscFlags = 0;
    let mut staging = None;
    unsafe { d.device.CreateTexture2D(&desc, None, Some(&mut staging)) }.map_err(|e| os_err("staging texture", e))?;
    let staging = staging.expect("texture returned on success");
    unsafe { d.context.CopyResource(&staging, texture) };

    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe { d.context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }.map_err(|e| os_err("Map", e))?;
    let bpp = if fp16 { 8 } else { 4 };
    let (row_bytes, pitch) = (crop.w as usize * bpp, mapped.RowPitch as usize);
    let mut data = vec![0u8; row_bytes * crop.h as usize];
    for (row, dst) in data.chunks_exact_mut(row_bytes).enumerate() {
        let offset = (crop.y as usize + row) * pitch + crop.x as usize * bpp;
        let src = unsafe { slice::from_raw_parts((mapped.pData as *const u8).add(offset), row_bytes) };
        dst.copy_from_slice(src);
    }
    unsafe { d.context.Unmap(&staging, 0) };
    Ok(RawFrame { width: crop.w, height: crop.h, fp16, data })
}

/// Straight-alpha RGBA from a captured frame. `opaque` forces alpha to 255 (monitor frames).
fn to_image(raw: RawFrame, sdr_white_nits: f32, opaque: bool) -> RgbaImage {
    if raw.fp16 {
        return hdr::tone_map(&raw.data, raw.width, raw.height, sdr_white_nits, opaque);
    }
    let mut data = raw.data;
    for px in data.as_chunks_mut::<4>().0 {
        let (b, g, r, a) = (px[0], px[1], px[2], px[3]);
        let a = if opaque { 255 } else { a };
        // WGC delivers premultiplied alpha.
        let un = |c: u8| if a == 0 || a == 255 { c } else { ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8 };
        px.copy_from_slice(&[un(r), un(g), un(b), a]);
    }
    RgbaImage::from_raw(raw.width, raw.height, data).expect("buffer size matches dimensions")
}

/// HDR monitor snapshot of `region` (virtual-desktop pixels inside `m.rect`), tone-mapped to SDR.
pub fn capture_monitor(m: &MonitorInfo, region: Rect) -> Result<Grab> {
    com::ensure();
    let hmon = monitors::hmonitor_of(m.id);
    let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(|e| os_err("capture interop", e))?;
    let item: GraphicsCaptureItem = unsafe { interop.CreateForMonitor(hmon) }.map_err(|e| os_err("CreateForMonitor", e))?;
    let raw = grab_frame(&item, true, Some(region.relative_to(m.rect.origin())))?;
    Ok(Grab { image: to_image(raw, monitors::sdr_white_nits(hmon), true), tone_mapped: true })
}

fn window_rect(hwnd: HWND) -> Option<Rect> {
    let mut r = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut r) }.ok().map(|_| rect_of(r))
}

/// Capture `hwnd` (also when covered by other windows) and crop to `visible`, its DWM frame.
pub fn capture_window(hwnd: HWND, visible: Rect, o: &crate::CaptureOptions) -> Result<WindowGrab> {
    com::ensure();
    let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(|e| os_err("capture interop", e))?;
    let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(hwnd) }.map_err(|e| os_err("CreateForWindow", e))?;
    let size = item.Size().map_err(|e| os_err("GraphicsCaptureItem.Size", e))?;
    let item_size = Rect::new(0, 0, size.Width.max(0) as u32, size.Height.max(0) as u32);

    // The frame covers either the visible frame or the full window rect (with invisible borders).
    let outer = window_rect(hwnd).unwrap_or(visible);
    let origin = if item_size.size() == visible.size() || item_size.size() != outer.size() { visible.origin() } else { outer.origin() };
    let crop = visible.relative_to(origin);

    let hmon: HMONITOR = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let fp16 = o.hdr == HdrMode::Auto && hdr::hdr_monitors().contains(&(hmon.0 as isize));
    let raw = grab_frame(&item, fp16, Some(crop))?;
    let screen_rect = Rect::new(origin.x + crop.x, origin.y + crop.y, raw.width, raw.height);
    Ok(WindowGrab { image: to_image(raw, monitors::sdr_white_nits(hmon), false), screen_rect, tone_mapped: fp16 })
}
