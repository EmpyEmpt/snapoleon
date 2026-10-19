//! Audio capture and mixing.
//!
//! Every source (microphone, system loopback, platform specific feeds) is
//! converted to 48 kHz stereo f32 into its own ring. A wall-clock driven mixer
//! pulls from the rings, so silent or stalled devices (WASAPI loopback delivers
//! nothing while nothing plays) still yield a gap-free, correctly timed stream.

use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

use cpal::{
    Data, SampleFormat,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use crossbeam_channel::Sender;
use parking_lot::Mutex;
use snap_core::{Error, Result, t, tf};

use crate::{
    clock::{HNS_PER_SEC, Hns, PauseClock},
    platform::FrameSource,
};

pub(crate) const RATE: u32 = 48_000;
/// The mixer consumes slightly behind the wall clock so device callbacks that
/// arrive in bursts do not turn into zero-padded gaps.
const LAG_FRAMES: u64 = RATE as u64 / 33;
const CHUNK_FRAMES: u64 = RATE as u64 / 50;
const MAX_BACKLOG_FRAMES: usize = RATE as usize / 4;
const RING_MAX_FRAMES: usize = RATE as usize * 2;

pub(crate) struct AudioChunk {
    #[cfg_attr(target_os = "linux", expect(dead_code, reason = "only the native media-foundation and AVFoundation sinks timestamp audio"))]
    pub pts: Hns,
    /// Interleaved stereo f32.
    pub samples: Vec<f32>,
}

#[derive(Default)]
pub(crate) struct Levels {
    mic: AtomicU32,
    system: AtomicU32,
}

impl Levels {
    fn store(slot: &AtomicU32, peak: f32) {
        let prev = f32::from_bits(slot.load(Ordering::Relaxed));
        let next = if peak > prev { peak } else { prev * 0.88 };
        slot.store(next.to_bits(), Ordering::Relaxed);
    }

    pub fn get(&self, mic: bool) -> f32 {
        let v = if mic { &self.mic } else { &self.system };
        f32::from_bits(v.load(Ordering::Relaxed)).clamp(0.0, 1.0)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Mic,
    System,
}

/// Stereo f32 FIFO shared between a device callback and the mixer.
#[derive(Clone, Default)]
pub(crate) struct Ring(Arc<Mutex<VecDeque<f32>>>);

impl Ring {
    pub fn push(&self, stereo: &[f32]) {
        let mut q = self.0.lock();
        q.extend(stereo.iter().copied());
        let max = RING_MAX_FRAMES * 2;
        if q.len() > max {
            let drop = q.len() - max;
            q.drain(..drop);
        }
    }

    fn clear(&self) {
        self.0.lock().clear();
    }

    /// Add up to `frames` frames into `mix`, returning the peak of what was added.
    fn mix_into(&self, mix: &mut [f32], frames: usize) -> f32 {
        let mut q = self.0.lock();
        // Keep latency bounded when a device clock runs fast.
        if q.len() / 2 > frames + MAX_BACKLOG_FRAMES {
            let drop = q.len() - (frames + MAX_BACKLOG_FRAMES) * 2;
            q.drain(..drop);
        }
        let n = (q.len() / 2).min(frames) * 2;
        let mut peak = 0.0f32;
        for (m, s) in mix.iter_mut().zip(q.drain(..n)) {
            *m += s;
            peak = peak.max(s.abs());
        }
        peak
    }
}

/// Streaming converter from an arbitrary interleaved format to 48 kHz stereo (linear interpolation).
pub(crate) struct Resampler {
    in_rate: u32,
    channels: usize,
    pos: f64,
    prev: [f32; 2],
    out: Vec<f32>,
}

impl Resampler {
    pub fn new(in_rate: u32, channels: usize) -> Self {
        Self { in_rate: in_rate.max(1), channels: channels.max(1), pos: 0.0, prev: [0.0; 2], out: Vec::new() }
    }

    fn frame_to_stereo(&self, f: &[f32]) -> [f32; 2] {
        match f.len() {
            1 => [f[0], f[0]],
            2 => [f[0], f[1]],
            _ => {
                // Fold surround layouts (FL FR C LFE SL SR ...) down to stereo.
                let c = f.get(2).copied().unwrap_or(0.0) * 0.707;
                let l = f[0] + c + f.get(4).copied().unwrap_or(0.0) * 0.707;
                let r = f[1] + c + f.get(5).copied().unwrap_or(0.0) * 0.707;
                [l.clamp(-1.0, 1.0), r.clamp(-1.0, 1.0)]
            }
        }
    }

    pub fn process(&mut self, interleaved: &[f32], ring: &Ring) {
        self.out.clear();
        let step = self.in_rate as f64 / RATE as f64;
        for frame in interleaved.chunks_exact(self.channels) {
            let cur = self.frame_to_stereo(frame);
            if self.in_rate == RATE {
                self.out.extend_from_slice(&cur);
                continue;
            }
            // `pos` is the output sample position in [0, 1) between prev and cur.
            while self.pos < 1.0 {
                let t = self.pos as f32;
                self.out.push(self.prev[0] + (cur[0] - self.prev[0]) * t);
                self.out.push(self.prev[1] + (cur[1] - self.prev[1]) * t);
                self.pos += step;
            }
            self.pos -= 1.0;
            self.prev = cur;
        }
        ring.push(&self.out);
    }
}

/// Running capture threads; dropping stops them.
pub(crate) struct AudioHandle {
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl AudioHandle {
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

impl Drop for AudioHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// A started source: its ring plus the thread keeping the device stream alive.
pub(crate) struct Source {
    pub kind: Kind,
    pub ring: Ring,
    pub thread: Option<JoinHandle<()>>,
}

pub(crate) struct AudioSpec {
    pub system: bool,
    pub mic: bool,
    pub mic_device: Option<String>,
}

/// Opened capture devices, not yet mixed. Opening can take a while, so recording starts its
/// clock only after this succeeded and then calls [`OpenSources::run`].
pub(crate) struct OpenSources {
    stop: Arc<AtomicBool>,
    sources: Vec<Source>,
    /// Cleared once the mixer owns shutdown; until then dropping stops the devices.
    armed: bool,
}

/// Open the requested sources, using platform specific feeds where the backend has them.
pub(crate) fn open(spec: &AudioSpec, platform: &mut dyn FrameSource) -> Result<OpenSources> {
    let stop = Arc::new(AtomicBool::new(false));
    let mut sources: Vec<Source> = Vec::new();

    if spec.mic {
        let opened = platform
            .audio_source(Kind::Mic, spec.mic_device.as_deref(), stop.clone())
            .unwrap_or_else(|| spawn_cpal(Kind::Mic, spec.mic_device.clone(), stop.clone()));
        match opened {
            Ok(s) => sources.push(s),
            Err(e) => {
                stop.store(true, Ordering::Relaxed);
                return Err(e);
            }
        }
    }
    if spec.system {
        match platform.audio_source(Kind::System, None, stop.clone()).unwrap_or_else(|| spawn_cpal(Kind::System, None, stop.clone())) {
            Ok(s) => sources.push(s),
            // No output device (servers, RDP sessions): record without system audio rather than fail.
            Err(e) => tracing::warn!("system audio unavailable: {e}"),
        }
    }
    Ok(OpenSources { stop, sources, armed: true })
}

impl OpenSources {
    /// Start mixing against `clock`. The mixer runs until the handle is stopped, then flushes
    /// up to the final clock position.
    pub fn run(mut self, clock: Arc<PauseClock>, out: Sender<AudioChunk>, levels: Arc<Levels>) -> Result<AudioHandle> {
        let mut threads: Vec<JoinHandle<()>> = self.sources.iter_mut().filter_map(|s| s.thread.take()).collect();
        let rings: Vec<(Kind, Ring)> = self.sources.iter().map(|s| (s.kind, s.ring.clone())).collect();
        // Whatever the devices captured while opening predates the clock.
        rings.iter().for_each(|(_, r)| r.clear());
        let stop = self.stop.clone();
        let mixer = std::thread::Builder::new()
            .name("snap-record-mixer".into())
            .spawn(move || run_mixer(rings, clock, out, levels, stop))
            .map_err(Error::Io)?;
        threads.push(mixer);
        self.armed = false;
        Ok(AudioHandle { stop: self.stop.clone(), threads })
    }
}

impl Drop for OpenSources {
    fn drop(&mut self) {
        if self.armed {
            self.stop.store(true, Ordering::Relaxed);
        }
    }
}

fn run_mixer(rings: Vec<(Kind, Ring)>, clock: Arc<PauseClock>, out: Sender<AudioChunk>, levels: Arc<Levels>, stop: Arc<AtomicBool>) {
    let mut emitted: u64 = 0;
    loop {
        let stopping = stop.load(Ordering::Relaxed);
        if clock.is_paused() {
            for (_, r) in &rings {
                r.clear();
            }
            if stopping {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        let now_frames = (clock.elapsed().as_secs_f64() * RATE as f64) as u64;
        let target = if stopping { now_frames } else { now_frames.saturating_sub(LAG_FRAMES) };
        while target.saturating_sub(emitted) >= CHUNK_FRAMES || (stopping && target > emitted) {
            let frames = (target - emitted).min(CHUNK_FRAMES.max(1)) as usize;
            let mut mix = vec![0.0f32; frames * 2];
            for (kind, ring) in &rings {
                let peak = ring.mix_into(&mut mix, frames);
                Levels::store(if *kind == Kind::Mic { &levels.mic } else { &levels.system }, peak);
            }
            for s in &mut mix {
                *s = s.clamp(-1.0, 1.0);
            }
            let pts = (emitted as i128 * HNS_PER_SEC as i128 / RATE as i128) as Hns;
            if out.send(AudioChunk { pts, samples: mix }).is_err() {
                return;
            }
            emitted += frames as u64;
        }
        if stopping {
            break;
        }
        std::thread::sleep(Duration::from_millis(8));
    }
}

/// Open a cpal input stream (microphone, or loopback of the default output device on Windows).
fn spawn_cpal(kind: Kind, device_name: Option<String>, stop: Arc<AtomicBool>) -> Result<Source> {
    let ring = Ring::default();
    let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<()>>(1);
    let thread_ring = ring.clone();
    let thread = std::thread::Builder::new()
        .name("snap-record-audio".into())
        .spawn(move || match open_stream(kind, device_name.as_deref(), thread_ring) {
            Ok(stream) => {
                let _ = ready_tx.send(Ok(()));
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(20));
                }
                drop(stream);
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e));
            }
        })
        .map_err(Error::Io)?;
    match ready_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => Ok(Source { kind, ring, thread: Some(thread) }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => Err(Error::Other(anyhow::anyhow!("audio device did not start in time"))),
    }
}

fn map_cpal(e: cpal::Error, what: &'static str) -> Error {
    match e.kind() {
        cpal::ErrorKind::PermissionDenied => {
            Error::permission(t(if what == "microphone" { "record.error.microphone_permission" } else { "record.error.input_permission" }))
        }
        cpal::ErrorKind::DeviceNotAvailable => Error::NotFound(format!("{what}: {e}")),
        _ => Error::Other(anyhow::anyhow!("{what}: {e}")),
    }
}

fn device_name(d: &cpal::Device) -> Option<String> {
    d.description().ok().map(|d| d.name().to_string())
}

fn open_stream(kind: Kind, wanted: Option<&str>, ring: Ring) -> Result<cpal::Stream> {
    let host = cpal::default_host();
    let (device, what) = match kind {
        Kind::Mic => {
            let device = match wanted.filter(|n| !n.is_empty()) {
                Some(name) => host
                    .input_devices()
                    .map_err(|e| map_cpal(e, "microphone"))?
                    .find(|d| device_name(d).as_deref() == Some(name))
                    .ok_or_else(|| Error::NotFound(tf("record.error.microphone_not_found", &[("name", name)])))?,
                None => host.default_input_device().ok_or_else(|| Error::NotFound(t("record.error.no_microphone").into_owned()))?,
            };
            (device, "microphone")
        }
        Kind::System => {
            (host.default_output_device().ok_or_else(|| Error::NotFound(t("record.error.no_output_device").into_owned()))?, "system audio")
        }
    };

    // On WASAPI an output device opened as input is a loopback capture.
    let config = match kind {
        Kind::Mic => device.default_input_config(),
        Kind::System => device.default_output_config(),
    }
    .map_err(|e| map_cpal(e, what))?;
    let format = config.sample_format();
    let channels = config.channels() as usize;
    let mut resampler = Resampler::new(config.sample_rate(), channels);
    let mut scratch: Vec<f32> = Vec::new();

    let stream = device
        .build_input_stream_raw(
            config.config(),
            format,
            move |data: &Data, _| {
                scratch.clear();
                if to_f32(data, &mut scratch) {
                    resampler.process(&scratch, &ring);
                }
            },
            move |e| match e.kind() {
                cpal::ErrorKind::Xrun => tracing::debug!("{what} glitch: {e}"),
                _ => tracing::warn!("{what} stream error: {e}"),
            },
            None,
        )
        .map_err(|e| map_cpal(e, what))?;
    stream.play().map_err(|e| map_cpal(e, what))?;
    Ok(stream)
}

/// Convert a cpal buffer of any common sample format to f32; false for unsupported formats.
fn to_f32(data: &Data, out: &mut Vec<f32>) -> bool {
    use cpal::Sample;
    macro_rules! conv {
        ($t:ty) => {
            match data.as_slice::<$t>() {
                Some(s) => {
                    out.extend(s.iter().map(|v| v.to_sample::<f32>()));
                    true
                }
                None => false,
            }
        };
    }
    match data.sample_format() {
        SampleFormat::F32 => conv!(f32),
        SampleFormat::F64 => conv!(f64),
        SampleFormat::I16 => conv!(i16),
        SampleFormat::I32 => conv!(i32),
        SampleFormat::U16 => conv!(u16),
        SampleFormat::I8 => conv!(i8),
        SampleFormat::U8 => conv!(u8),
        _ => false,
    }
}

/// Names of available input devices (default first).
pub(crate) fn input_device_names() -> Vec<String> {
    let host = cpal::default_host();
    let default = host.default_input_device().and_then(|d| device_name(&d));
    let mut names: Vec<String> = host.input_devices().map(|it| it.filter_map(|d| device_name(&d)).collect()).unwrap_or_default();
    names.dedup();
    if let Some(d) = default {
        names.retain(|n| *n != d);
        names.insert(0, d);
    }
    names
}
