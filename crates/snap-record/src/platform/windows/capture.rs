//! Windows.Graphics.Capture sources (monitor, region, window).

use std::{
    mem::size_of,
    sync::{Arc, OnceLock},
    time::Duration,
};

use anyhow::anyhow;
use parking_lot::Mutex;
use snap_core::{Error, Point, Rect, Result, t, tf};
use windows::Win32::{
    Foundation::{HWND, RECT},
    Graphics::{
        Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute},
        Gdi::{GetMonitorInfoW, HMONITOR, MONITORINFO},
    },
    UI::WindowsAndMessaging::{IsWindow, SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE},
};
use windows_capture::{
    capture::{CaptureControl, Context, GraphicsCaptureApiHandler},
    frame::Frame as WcFrame,
    graphics_capture_api::{GraphicsCaptureApi, InternalCaptureControl},
    monitor::Monitor,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings, GraphicsCaptureItemType,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    },
    window::Window,
};

use super::TimerResolution;
use crate::{
    RecordTarget,
    frame::{Frame, even},
    platform::{FrameSlot, FrameSource, OriginFn, SourceSpec},
};

/// A monitor in virtual-desktop pixels.
struct MonitorGeom {
    monitor: Monitor,
    rect: Rect,
}

fn monitors() -> Result<Vec<MonitorGeom>> {
    let list = Monitor::enumerate().map_err(|e| Error::Other(anyhow!("enumerate monitors: {e}")))?;
    let mut out = Vec::with_capacity(list.len());
    for monitor in list {
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        // SAFETY: `info.cbSize` is set; the handle came from EnumDisplayMonitors.
        if unsafe { GetMonitorInfoW(HMONITOR(monitor.as_raw_hmonitor()), &mut info) }.as_bool() {
            let r = info.rcMonitor;
            out.push(MonitorGeom { monitor, rect: Rect::new(r.left, r.top, (r.right - r.left) as u32, (r.bottom - r.top) as u32) });
        }
    }
    Ok(out)
}

/// Where one captured monitor lands in the output.
struct Piece {
    /// Crop in monitor-local pixels (x0, y0, x1, y1), or `None` for the whole frame.
    crop: Option<(u32, u32, u32, u32)>,
    /// Destination offset in the output canvas.
    dst: (i32, i32),
}

/// Merges the pieces' latest frames into the published output frame.
struct Composer {
    slot: Arc<FrameSlot>,
    /// Output size: preset for regions, taken from the first frame for monitors and windows.
    size: OnceLock<(u32, u32)>,
    /// One piece that always matches the output exactly: publish its frames untouched.
    passthrough: bool,
    latest: Mutex<Vec<Option<Frame>>>,
    dst: Vec<(i32, i32)>,
    first: Mutex<Option<crossbeam_channel::Sender<()>>>,
}

impl Composer {
    fn update(&self, index: usize, w: u32, h: u32, pitch: usize, raw: &[u8]) {
        let mut packed = Vec::with_capacity(w as usize * h as usize * 4);
        for y in 0..h as usize {
            packed.extend_from_slice(&raw[y * pitch..y * pitch + w as usize * 4]);
        }
        let piece = Frame { w, h, data: packed };
        let size = *self.size.get_or_init(|| (even(w), even(h)));
        let out = if self.passthrough && (piece.w, piece.h) == size {
            piece
        } else {
            let mut latest = self.latest.lock();
            latest[index] = Some(piece);
            let mut canvas = Frame::black(size.0, size.1);
            for (p, dst) in latest.iter().zip(&self.dst) {
                if let Some(p) = p {
                    canvas.blit(&p.data, p.stride(), p.w, p.h, dst.0, dst.1);
                }
            }
            canvas
        };
        self.slot.set(out);
        if let Some(tx) = self.first.lock().take() {
            let _ = tx.send(());
        }
    }
}

struct Flags {
    composer: Arc<Composer>,
    index: usize,
    crop: Option<(u32, u32, u32, u32)>,
}

struct Handler {
    flags: Flags,
}

impl GraphicsCaptureApiHandler for Handler {
    type Flags = Flags;
    type Error = anyhow::Error;

    fn new(ctx: Context<Self::Flags>) -> std::result::Result<Self, Self::Error> {
        Ok(Self { flags: ctx.flags })
    }

    fn on_frame_arrived(&mut self, frame: &mut WcFrame, _: InternalCaptureControl) -> std::result::Result<(), Self::Error> {
        let (fw, fh) = (frame.width(), frame.height());
        let mut buffer = match self.flags.crop {
            Some((x0, y0, x1, y1)) => {
                let (x1, y1) = (x1.min(fw), y1.min(fh));
                if x0 >= x1 || y0 >= y1 {
                    return Ok(());
                }
                frame.buffer_crop(x0, y0, x1, y1)?
            }
            None => frame.buffer()?,
        };
        let (w, h, pitch) = (buffer.width(), buffer.height(), buffer.row_pitch() as usize);
        self.flags.composer.update(self.flags.index, w, h, pitch, buffer.as_raw_buffer());
        Ok(())
    }
}

type Control = CaptureControl<Handler, anyhow::Error>;

fn start_capture<T>(item: T, cursor: bool, composer: &Arc<Composer>, index: usize, crop: Option<(u32, u32, u32, u32)>) -> Result<Control>
where
    T: TryInto<GraphicsCaptureItemType> + Clone + Send + 'static,
{
    let settings = |border: DrawBorderSettings| {
        Settings::new(
            item.clone(),
            if cursor { CursorCaptureSettings::WithCursor } else { CursorCaptureSettings::WithoutCursor },
            border,
            SecondaryWindowSettings::Default,
            MinimumUpdateIntervalSettings::Default,
            DirtyRegionSettings::Default,
            ColorFormat::Bgra8,
            Flags { composer: composer.clone(), index, crop },
        )
    };
    // Hide the yellow capture border where the OS allows it (Windows 11 / recent 10), else accept it.
    Handler::start_free_threaded(settings(DrawBorderSettings::WithoutBorder))
        .or_else(|_| Handler::start_free_threaded(settings(DrawBorderSettings::Default)))
        .map_err(|e| Error::Other(anyhow!("start capture: {e}")))
}

struct WgcSource {
    controls: Vec<Control>,
    size: (u32, u32),
    origin: OriginFn,
    excluded: Vec<HWND>,
    _timer: TimerResolution,
}

// SAFETY: HWND values are plain handles; every other field is Send.
unsafe impl Send for WgcSource {}

impl FrameSource for WgcSource {
    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn origin_fn(&self) -> OriginFn {
        self.origin.clone()
    }

    fn stop(self: Box<Self>) {
        let this = *self;
        for c in this.controls {
            if let Err(e) = c.stop() {
                tracing::debug!("capture stop: {e}");
            }
        }
        for hwnd in this.excluded {
            // SAFETY: restoring the affinity set in `open_source`.
            let _ = unsafe { SetWindowDisplayAffinity(hwnd, WDA_NONE) };
        }
    }
}

pub(crate) fn open_source(spec: &SourceSpec, slot: Arc<FrameSlot>) -> Result<Box<dyn FrameSource>> {
    if !GraphicsCaptureApi::is_supported().unwrap_or(false) {
        return Err(Error::unsupported(t("record.error.windows_capture")));
    }
    let timer = TimerResolution::raise();

    // Windows owned by this process (the recorder HUD) can ask to be left out of the capture.
    let excluded: Vec<HWND> = spec
        .exclude_windows
        .iter()
        .map(|&id| HWND(id as usize as *mut _))
        // SAFETY: affinity can only be changed for windows of this process; failures are skipped.
        .filter(|&h| unsafe { SetWindowDisplayAffinity(h, WDA_EXCLUDEFROMCAPTURE) }.is_ok())
        .collect();

    let (first_tx, first_rx) = crossbeam_channel::bounded::<()>(1);
    let size = OnceLock::new();
    let mut passthrough = true;
    let mut plan: Vec<(Monitor, Piece)> = Vec::new();
    let mut window: Option<Window> = None;
    let origin: OriginFn;

    match spec.target {
        RecordTarget::Monitor(idx) => {
            let mons = monitors()?;
            let m = mons.get(*idx).ok_or_else(|| Error::NotFound(tf("record.error.monitor_not_found", &[("index", &idx.to_string())])))?;
            let o = m.rect.origin();
            origin = Arc::new(move || o);
            plan.push((m.monitor, Piece { crop: None, dst: (0, 0) }));
        }
        RecordTarget::Region(r) => {
            if r.is_empty() {
                return Err(Error::invalid(t("record.error.region_empty")));
            }
            let mons = monitors()?;
            let hits: Vec<(&MonitorGeom, Rect)> = mons.iter().filter_map(|m| m.rect.intersect(*r).map(|i| (m, i))).collect();
            if hits.is_empty() {
                return Err(Error::invalid(t("record.error.region_outside")));
            }
            let (w, h) = (even(r.w), even(r.h));
            let _ = size.set((w, h));
            passthrough = hits.len() == 1 && hits[0].1.w >= w && hits[0].1.h >= h;
            for (m, inter) in hits {
                let local = inter.relative_to(m.rect.origin());
                let crop = (local.x as u32, local.y as u32, local.x as u32 + local.w.min(w), local.y as u32 + local.h.min(h));
                plan.push((m.monitor, Piece { crop: Some(crop), dst: (inter.x - r.x, inter.y - r.y) }));
            }
            let o = r.origin();
            origin = Arc::new(move || o);
        }
        RecordTarget::Window(id) => {
            let hwnd = HWND(*id as usize as *mut _);
            // SAFETY: IsWindow accepts any handle value.
            if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
                return Err(Error::NotFound(t("record.error.window_not_found").into_owned()));
            }
            window = Some(Window::from_raw_hwnd(hwnd.0));
            let raw = hwnd.0 as usize;
            origin = Arc::new(move || window_origin(HWND(raw as *mut _)));
        }
    }

    let pieces = plan.len().max(1);
    let composer = Arc::new(Composer {
        slot,
        size,
        passthrough,
        latest: Mutex::new((0..pieces).map(|_| None).collect()),
        dst: if plan.is_empty() { vec![(0, 0)] } else { plan.iter().map(|(_, p)| p.dst).collect() },
        first: Mutex::new(Some(first_tx)),
    });

    let mut controls: Vec<Control> = Vec::new();
    let started: Result<()> = (|| {
        if let Some(win) = window {
            controls.push(start_capture(win, spec.show_cursor, &composer, 0, None)?);
        }
        for (i, (monitor, piece)) in plan.into_iter().enumerate() {
            controls.push(start_capture(monitor, spec.show_cursor, &composer, i, piece.crop)?);
        }
        first_rx.recv_timeout(Duration::from_secs(5)).map_err(|_| Error::Other(anyhow!(t("record.error.no_frames").into_owned())))
    })();
    if let Err(e) = started {
        for c in controls {
            let _ = c.stop();
        }
        return Err(e);
    }

    let size = composer.size.get().copied().ok_or_else(|| Error::Other(anyhow!("capture size unknown")))?;
    Ok(Box::new(WgcSource { controls, size, origin, excluded, _timer: timer }))
}

fn window_origin(hwnd: HWND) -> Point {
    let mut r = RECT::default();
    // SAFETY: out-parameter of the exact size DWM expects.
    let ok =
        unsafe { DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, (&mut r as *mut RECT).cast(), size_of::<RECT>() as u32) }.is_ok();
    if ok { Point::new(r.left, r.top) } else { Point::default() }
}
