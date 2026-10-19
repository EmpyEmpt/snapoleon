//! Monitor enumeration with DPI, refresh rate, friendly name and HDR state.

use std::{collections::HashMap, ffi::c_void, mem};

use snap_core::{Error, Rect, Result};
use windows::{
    Win32::{
        Devices::Display::{
            DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
            DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
            DISPLAYCONFIG_SDR_WHITE_LEVEL, DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, DisplayConfigGetDeviceInfo,
            GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
        },
        Foundation::{LPARAM, RECT},
        Graphics::Gdi::{
            DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
            MONITORINFOEXW,
        },
        UI::{
            HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
            WindowsAndMessaging::{
                GetSystemMetrics, MONITORINFOF_PRIMARY, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
            },
        },
    },
    core::{BOOL, PCWSTR},
};

use super::{hdr, rect_of, wide_to_string};
use crate::MonitorInfo;

/// What DisplayConfig knows about the display behind a GDI device name (`\.\DISPLAY1`).
struct DisplayExtra {
    friendly: String,
    sdr_white_nits: f32,
}

pub fn id_of(h: HMONITOR) -> u32 {
    h.0 as usize as u32
}

pub fn hmonitor_of(id: u32) -> HMONITOR {
    HMONITOR(id as i32 as isize as *mut c_void)
}

/// Bounding box of all monitors.
pub fn virtual_bounds() -> Rect {
    let (x, y, w, h) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    Rect::new(x, y, w.max(0) as u32, h.max(0) as u32)
}

unsafe extern "system" fn collect(h: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    let out = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
    out.push(h);
    true.into()
}

pub fn list() -> Result<Vec<MonitorInfo>> {
    let mut handles: Vec<HMONITOR> = Vec::new();
    let ok = unsafe { EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut handles as *mut _ as isize)) };
    if !ok.as_bool() || handles.is_empty() {
        return Err(Error::NotFound("no monitors".into()));
    }
    let extras = display_config();
    let hdr_monitors = hdr::hdr_monitors();

    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = mem::size_of::<MONITORINFOEXW>() as u32;
        if !unsafe { GetMonitorInfoW(h, &mut mi as *mut _ as *mut MONITORINFO) }.as_bool() {
            continue;
        }
        let device = wide_to_string(&mi.szDevice);
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        let _ = unsafe { GetDpiForMonitor(h, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };

        let mut mode = DEVMODEW { dmSize: mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
        let refresh_hz = if unsafe { EnumDisplaySettingsW(PCWSTR(mi.szDevice.as_ptr()), ENUM_CURRENT_SETTINGS, &mut mode) }.as_bool() {
            mode.dmDisplayFrequency as f32
        } else {
            0.0
        };

        out.push(MonitorInfo {
            index: 0,
            id: id_of(h),
            name: extras.get(&device).map(|e| e.friendly.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| device.clone()),
            rect: rect_of(mi.monitorInfo.rcMonitor),
            work_area: rect_of(mi.monitorInfo.rcWork),
            scale_factor: dpi_x as f32 / 96.0,
            is_primary: mi.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
            hdr: hdr_monitors.contains(&(h.0 as isize)),
            refresh_hz,
        });
    }
    out.sort_by_key(|m| (!m.is_primary, m.rect.x, m.rect.y));
    for (i, m) in out.iter_mut().enumerate() {
        m.index = i;
    }
    Ok(out)
}

/// SDR white level of the monitor in nits (Windows "SDR content brightness"; 80 nits when unknown).
pub fn sdr_white_nits(h: HMONITOR) -> f32 {
    let mut mi = MONITORINFOEXW::default();
    mi.monitorInfo.cbSize = mem::size_of::<MONITORINFOEXW>() as u32;
    if !unsafe { GetMonitorInfoW(h, &mut mi as *mut _ as *mut MONITORINFO) }.as_bool() {
        return 80.0;
    }
    display_config().remove(&wide_to_string(&mi.szDevice)).map_or(80.0, |e| e.sdr_white_nits)
}

fn header(
    kind: windows::Win32::Devices::Display::DISPLAYCONFIG_DEVICE_INFO_TYPE,
    size: usize,
    p: &DISPLAYCONFIG_PATH_INFO,
    target: bool,
) -> DISPLAYCONFIG_DEVICE_INFO_HEADER {
    let (adapter_id, id) = if target { (p.targetInfo.adapterId, p.targetInfo.id) } else { (p.sourceInfo.adapterId, p.sourceInfo.id) };
    DISPLAYCONFIG_DEVICE_INFO_HEADER { r#type: kind, size: size as u32, adapterId: adapter_id, id }
}

fn display_config() -> HashMap<String, DisplayExtra> {
    let mut out = HashMap::new();
    let (mut n_paths, mut n_modes) = (0u32, 0u32);
    if unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut n_paths, &mut n_modes) }.0 != 0 {
        return out;
    }
    let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); n_paths as usize];
    let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); n_modes as usize];
    if unsafe { QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut n_paths, paths.as_mut_ptr(), &mut n_modes, modes.as_mut_ptr(), None) }.0 != 0
    {
        return out;
    }
    for p in &paths[..n_paths as usize] {
        let mut src = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
        src.header = header(DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, mem::size_of_val(&src), p, false);
        if unsafe { DisplayConfigGetDeviceInfo(&mut src.header) } != 0 {
            continue;
        }
        let mut tgt = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
        tgt.header = header(DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, mem::size_of_val(&tgt), p, true);
        let friendly = if unsafe { DisplayConfigGetDeviceInfo(&mut tgt.header) } == 0 {
            wide_to_string(&tgt.monitorFriendlyDeviceName)
        } else {
            String::new()
        };

        let mut white = DISPLAYCONFIG_SDR_WHITE_LEVEL::default();
        white.header = header(DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL, mem::size_of_val(&white), p, true);
        let sdr_white_nits = if unsafe { DisplayConfigGetDeviceInfo(&mut white.header) } == 0 && white.SDRWhiteLevel > 0 {
            white.SDRWhiteLevel as f32 / 1000.0 * 80.0
        } else {
            80.0
        };
        out.insert(wide_to_string(&src.viewGdiDeviceName), DisplayExtra { friendly, sdr_white_nits });
    }
    out
}
