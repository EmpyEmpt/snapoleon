//! Linux backend. Video comes from an ffmpeg child (`x11grab`, or `pipewiregrab` on
//! Wayland when this ffmpeg has it) producing raw BGRA; PulseAudio / PipeWire audio
//! is read through ffmpeg as well. There is no native MP4 encoder: the generic
//! ffmpeg sink does the encoding (VAAPI/NVENC/x264/openh264, see `ffmpeg.rs`).

use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use snap_core::{Error, Point, Rect, Result, t, tf};

use super::{FrameSlot, FrameSource, OriginFn, SourceSpec};
use crate::{
    EncoderInfo, RecordTarget,
    audio::{Kind, Ring, Source},
    decode::{self, Decoder},
    ffmpeg,
    frame::{Frame, even},
    sink::{Sink, SinkSpec},
};

pub(crate) fn native_mp4_sink(_: &SinkSpec) -> Result<Box<dyn Sink>> {
    Err(Error::unsupported(t("record.error.no_native_encoder")))
}

pub(crate) fn native_encoders() -> Vec<EncoderInfo> {
    Vec::new()
}

pub(crate) fn open_decoder(_: &Path, _: bool) -> Result<Box<dyn Decoder>> {
    Err(decode::unsupported_decoder())
}

/// The cursor position is learned from the first pointer event instead.
pub(crate) fn cursor_position() -> Option<Point> {
    None
}

pub(crate) fn sleep_until(deadline: Instant) {
    loop {
        let Some(left) = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) else { return };
        if left > Duration::from_millis(2) {
            std::thread::sleep(left - Duration::from_millis(1));
        } else {
            std::thread::yield_now();
        }
    }
}

/// PulseAudio / PipeWire source names (microphones only), via `pactl`.
pub(crate) fn mic_devices() -> Option<Vec<String>> {
    let out = command_output("pactl", &["list", "short", "sources"])?;
    let mut names: Vec<String> =
        out.lines().filter_map(|l| l.split('\t').nth(1)).filter(|n| !n.ends_with(".monitor")).map(str::to_string).collect();
    if let Some(default) = command_output("pactl", &["get-default-source"]).map(|s| s.trim().to_string())
        && let Some(i) = names.iter().position(|n| *n == default)
    {
        names.swap(0, i);
    }
    (!names.is_empty()).then_some(names)
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

// ------------------------------------------------------------------ geometry

#[derive(Debug, Clone)]
struct Monitor {
    rect: Rect,
}

/// Monitors from `xrandr --listmonitors`; one full-screen monitor from `xdpyinfo` as a fallback.
fn monitors() -> Vec<Monitor> {
    if let Some(text) = command_output("xrandr", &["--listmonitors"]) {
        let list: Vec<Monitor> = text
            .lines()
            .skip(1)
            .filter_map(|l| {
                // " 0: +*DP-1 2560/597x1440/336+0+0  DP-1"
                let geom = l.split_whitespace().nth(2)?;
                let (size, pos) = geom.split_once('+')?;
                let (w, h) = size.split_once('x')?;
                let w: u32 = w.split('/').next()?.parse().ok()?;
                let h: u32 = h.split('/').next()?.parse().ok()?;
                let (x, y) = pos.split_once('+')?;
                Some(Monitor { rect: Rect::new(x.parse().ok()?, y.parse().ok()?, w, h) })
            })
            .collect();
        if !list.is_empty() {
            return list;
        }
    }
    command_output("xdpyinfo", &[])
        .and_then(|t| t.lines().find(|l| l.contains("dimensions:"))?.split_whitespace().nth(1).map(str::to_string))
        .and_then(|d| {
            let (w, h) = d.split_once('x')?;
            Some(vec![Monitor { rect: Rect::new(0, 0, w.parse().ok()?, h.parse().ok()?) }])
        })
        .unwrap_or_default()
}

/// Window rectangle via `xwininfo -id`.
fn window_rect(id: u64) -> Option<Rect> {
    let text = command_output("xwininfo", &["-id", &id.to_string()])?;
    let field = |name: &str| -> Option<i64> { text.lines().find_map(|l| l.trim().strip_prefix(name)?.trim().parse().ok()) };
    Some(Rect::new(
        field("Absolute upper-left X:")? as i32,
        field("Absolute upper-left Y:")? as i32,
        field("Width:")? as u32,
        field("Height:")? as u32,
    ))
}

fn is_wayland() -> bool {
    std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v.eq_ignore_ascii_case("wayland"))
        || (std::env::var_os("WAYLAND_DISPLAY").is_some() && std::env::var_os("DISPLAY").is_none())
}

fn has_pipewire_grab(ffmpeg: &Path) -> bool {
    Command::new(ffmpeg)
        .args(["-hide_banner", "-devices"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("pipewiregrab"))
        .unwrap_or(false)
}

// ------------------------------------------------------------------ video

struct GrabSource {
    child: Child,
    reader: Option<std::thread::JoinHandle<()>>,
    size: (u32, u32),
    origin: Point,
    ffmpeg: PathBuf,
}

impl FrameSource for GrabSource {
    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn origin_fn(&self) -> OriginFn {
        let o = self.origin;
        Arc::new(move || o)
    }

    fn audio_source(&mut self, kind: Kind, device: Option<&str>, stop: Arc<AtomicBool>) -> Option<Result<Source>> {
        pulse_source(&self.ffmpeg, kind, device, stop)
    }

    fn stop(mut self: Box<Self>) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(r) = self.reader.take() {
            let _ = r.join();
        }
    }
}

pub(crate) fn open_source(spec: &SourceSpec, slot: Arc<FrameSlot>) -> Result<Box<dyn FrameSource>> {
    let ffmpeg = ffmpeg::locate(spec.ffmpeg).ok_or_else(|| Error::unsupported(t("record.error.ffmpeg_missing")))?;
    let wayland = is_wayland();
    if wayland && !has_pipewire_grab(&ffmpeg) {
        return Err(Error::unsupported(t("record.error.wayland_unsupported")));
    }

    let mons = monitors();
    let (area, window_id) = match spec.target {
        RecordTarget::Monitor(i) => {
            (mons.get(*i).ok_or_else(|| Error::NotFound(tf("record.error.monitor_not_found", &[("index", &i.to_string())])))?.rect, None)
        }
        RecordTarget::Region(r) if r.is_empty() => return Err(Error::invalid(t("record.error.region_empty"))),
        RecordTarget::Region(r) => (*r, None),
        RecordTarget::Window(id) => {
            (window_rect(*id).ok_or_else(|| Error::NotFound(t("record.error.window_not_found").into_owned()))?, Some(*id))
        }
    };
    let (w, h) = (even(area.w), even(area.h));
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into());
    let display = if display.contains('.') { display } else { format!("{display}.0") };

    let mut cmd = ffmpeg::command(&ffmpeg);
    cmd.arg("-f");
    if wayland {
        // Best effort: the portal asks the user which output to share.
        cmd.arg("pipewiregrab").args(["-framerate", &spec.fps.to_string(), "-i", "0"]);
        cmd.arg("-vf").arg(format!("hwdownload,format=bgra,crop={w}:{h}:{}:{}", area.x.max(0), area.y.max(0)));
    } else {
        cmd.arg("x11grab")
            .args(["-framerate", &spec.fps.to_string(), "-draw_mouse", if spec.show_cursor { "1" } else { "0" }])
            .args(["-video_size", &format!("{w}x{h}")]);
        match window_id {
            Some(id) => {
                cmd.args(["-window_id", &format!("0x{id:x}"), "-i", &display]);
            }
            None => {
                cmd.args(["-i", &format!("{display}+{},{}", area.x.max(0), area.y.max(0))]);
            }
        }
    }
    cmd.args(["-f", "rawvideo", "-pix_fmt", "bgra", "-"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let mut stdout = child.stdout.take().expect("stdout piped");
    let mut stderr = child.stderr.take().expect("stderr piped");

    let (first_tx, first_rx) = crossbeam_channel::bounded::<()>(1);
    let reader = std::thread::Builder::new().name("snap-record-grab".into()).spawn(move || {
        let mut first = Some(first_tx);
        loop {
            let mut data = vec![0u8; w as usize * h as usize * 4];
            if stdout.read_exact(&mut data).is_err() {
                break;
            }
            data.iter_mut().skip(3).step_by(4).for_each(|a| *a = 255);
            slot.set(Frame { w, h, data });
            if let Some(tx) = first.take() {
                let _ = tx.send(());
            }
        }
    })?;

    if first_rx.recv_timeout(Duration::from_secs(if wayland { 60 } else { 8 })).is_err() {
        let _ = child.kill();
        let _ = child.wait();
        let _ = reader.join();
        let mut log = Vec::new();
        let _ = stderr.read_to_end(&mut log);
        return Err(Error::Other(anyhow::anyhow!("screen capture did not start: {}", ffmpeg::tail(&log))));
    }
    Ok(Box::new(GrabSource { child, reader: Some(reader), size: (w, h), origin: area.origin(), ffmpeg }))
}

// ------------------------------------------------------------------ audio

/// Capture a PulseAudio/PipeWire source through ffmpeg as 48 kHz stereo f32. `None` = use cpal instead.
fn pulse_source(ffmpeg: &Path, kind: Kind, device: Option<&str>, stop: Arc<AtomicBool>) -> Option<Result<Source>> {
    let input = match kind {
        Kind::Mic => device.filter(|d| !d.is_empty()).unwrap_or("default").to_string(),
        Kind::System => {
            let sink = command_output("pactl", &["get-default-sink"]).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            sink.map(|s| format!("{s}.monitor")).unwrap_or_else(|| "@DEFAULT_MONITOR@".to_string())
        }
    };
    let mut child = ffmpeg::command(ffmpeg)
        .args(["-f", "pulse", "-i", &input, "-f", "f32le", "-ar", "48000", "-ac", "2", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    std::thread::sleep(Duration::from_millis(400));
    if child.try_wait().ok().flatten().is_some() {
        return None; // no PulseAudio server or unknown device: let cpal try
    }
    let mut out = child.stdout.take()?;
    let ring = Ring::default();
    let feed = ring.clone();
    let thread = std::thread::Builder::new()
        .name("snap-record-pulse".into())
        .spawn(move || {
            let mut buf = vec![0u8; 3840];
            let mut carry: Vec<u8> = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                let Ok(n) = out.read(&mut buf) else { break };
                if n == 0 {
                    break;
                }
                carry.extend_from_slice(&buf[..n]);
                let whole = carry.len() / 4 * 4;
                let samples: Vec<f32> = carry[..whole].as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
                feed.push(&samples);
                carry.drain(..whole);
            }
            let _ = child.kill();
            let _ = child.wait();
        })
        .ok()?;
    Some(Ok(Source { kind, ring, thread: Some(thread) }))
}
