//! The recording engine: source -> pacer (+overlays) -> sink thread, with the
//! audio mixer feeding the same sink.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use crossbeam_channel::{Receiver, Select, Sender, TrySendError};
use parking_lot::Mutex;
use snap_core::{Error, Result, settings::VideoFormat};

use crate::{
    RecordOptions, Recording,
    audio::{self, AudioChunk, AudioHandle, AudioSpec, Levels},
    clock::{Hns, PauseClock, hns_from},
    frame::Frame,
    overlay::{Overlay, OverlayConfig},
    platform::{self, FrameSlot, FrameSource, SourceSpec},
    sink::{self, Sink, SinkSpec},
};

/// Video frames waiting for the encoder; beyond this the pacer drops frames (timestamps stay real-time).
const VIDEO_QUEUE: usize = 6;

/// A running recording. Dropping it without calling [`Recorder::stop`] cancels and deletes the file.
pub struct Recorder {
    clock: Arc<PauseClock>,
    levels: Arc<Levels>,
    mic: bool,
    failure: Arc<Mutex<Option<String>>>,
    stop: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    end: Arc<AtomicI64>,
    parts: Option<Parts>,
    output: PathBuf,
    format: VideoFormat,
    size: (u32, u32),
}

struct Parts {
    source: Box<dyn FrameSource>,
    pacer: Option<JoinHandle<()>>,
    sink: JoinHandle<Result<()>>,
    audio: Option<AudioHandle>,
    /// Senders kept here so the encoder thread sees its inputs close only when everything is joined.
    video_tx: Sender<(Arc<Frame>, Hns)>,
    audio_tx: Option<Sender<AudioChunk>>,
}

impl Recorder {
    pub fn pause(&self) {
        self.clock.pause();
    }

    pub fn resume(&self) {
        self.clock.resume();
    }

    pub fn is_paused(&self) -> bool {
        self.clock.is_paused()
    }

    /// Recorded (un-paused) time so far.
    pub fn elapsed_ms(&self) -> u64 {
        self.clock.elapsed().as_millis() as u64
    }

    /// Input level 0..1: the microphone when it is being recorded, otherwise system audio.
    pub fn audio_level(&self) -> f32 {
        self.levels.get(self.mic)
    }

    /// Set once the encoder or capture fails; the recording stops producing output and
    /// [`Recorder::stop`] returns the same error.
    pub fn error(&self) -> Option<String> {
        self.failure.lock().clone()
    }

    /// Finish the file and return its description.
    pub fn stop(mut self) -> Result<Recording> {
        let end = hns_from(self.clock.elapsed());
        self.end.store(end, Ordering::SeqCst);
        let result = self.join_all();
        result?;
        let bytes = std::fs::metadata(&self.output).map(|m| m.len()).unwrap_or(0);
        Ok(Recording {
            path: self.output.clone(),
            duration_ms: (end / 10_000) as u64,
            width: self.size.0,
            height: self.size.1,
            bytes,
            format: self.format,
        })
    }

    /// Stop and delete the partial output.
    pub fn cancel(mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.join_all();
    }

    fn join_all(&mut self) -> Result<()> {
        let Some(parts) = self.parts.take() else {
            return Ok(());
        };
        self.stop.store(true, Ordering::SeqCst);
        if let Some(p) = parts.pacer {
            let _ = p.join();
        }
        if let Some(a) = parts.audio {
            a.stop();
        }
        parts.source.stop();
        drop((parts.video_tx, parts.audio_tx));
        let sink_result = parts.sink.join().unwrap_or_else(|_| Err(Error::Other(anyhow::anyhow!("encoder thread panicked"))));
        if self.cancel.load(Ordering::SeqCst) {
            let _ = std::fs::remove_file(&self.output);
            return Ok(());
        }
        if let Err(e) = &sink_result {
            tracing::error!("recording failed: {e}");
            let _ = std::fs::remove_file(&self.output);
        }
        sink_result
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if self.parts.is_some() {
            self.cancel.store(true, Ordering::SeqCst);
            let _ = self.join_all();
        }
    }
}

pub(crate) fn start(o: RecordOptions) -> Result<Recorder> {
    let fps = match o.format {
        VideoFormat::Mp4 => o.fps.clamp(1, 120),
        VideoFormat::Gif | VideoFormat::Webp => o.fps.clamp(1, 50),
    };
    if let Some(dir) = o.output.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }

    let levels = Arc::new(Levels::default());
    let failure = Arc::new(Mutex::new(None));
    let stop = Arc::new(AtomicBool::new(false));
    let cancel = Arc::new(AtomicBool::new(false));
    let end = Arc::new(AtomicI64::new(0));

    let want_audio = o.format == VideoFormat::Mp4 && (o.system_audio || o.microphone);
    let slot = Arc::new(FrameSlot::default());
    let source = platform::open_source(
        &SourceSpec {
            target: &o.target,
            show_cursor: o.show_cursor,
            exclude_windows: &o.exclude_windows,
            system_audio: want_audio && o.system_audio,
            fps,
            ffmpeg: o.ffmpeg.as_deref(),
        },
        slot.clone(),
    )?;
    let (w, h) = source.size();

    let sink_spec = SinkSpec {
        format: o.format,
        w,
        h,
        fps,
        bitrate: o.bitrate,
        audio: want_audio,
        encoder: o.encoder,
        output: o.output.clone(),
        ffmpeg: o.ffmpeg.clone(),
        max_gif_width: o.max_gif_width,
        // Screen content stays crisp without dithering.
        dither: false,
    };
    let out_size = match o.format {
        VideoFormat::Mp4 => (w, h),
        VideoFormat::Gif | VideoFormat::Webp => sink::scaled_size(&sink_spec),
    };

    // The encoder is created on its own thread (COM / non-Send handles); wait for its verdict.
    let (video_tx, video_rx) = crossbeam_channel::bounded::<(Arc<Frame>, Hns)>(VIDEO_QUEUE);
    let (audio_tx, audio_rx) = if want_audio {
        let (t, r) = crossbeam_channel::unbounded::<AudioChunk>();
        (Some(t), Some(r))
    } else {
        (None, None)
    };
    let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<()>>(1);
    let sink_thread = {
        let (end, cancel, failure) = (end.clone(), cancel.clone(), failure.clone());
        std::thread::Builder::new().name("snap-record-encode".into()).spawn(move || match sink::build(&sink_spec) {
            Ok(s) => {
                let _ = ready_tx.send(Ok(()));
                run_sink(s, video_rx, audio_rx, end, cancel, failure)
            }
            Err(e) => {
                let msg = e.to_string();
                let _ = ready_tx.send(Err(e));
                Err(Error::Other(anyhow::anyhow!(msg)))
            }
        })?
    };
    match ready_rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            source.stop();
            let _ = sink_thread.join();
            let _ = std::fs::remove_file(&o.output);
            return Err(e);
        }
        Err(_) => {
            source.stop();
            return Err(Error::Other(anyhow::anyhow!("encoder thread exited during start")));
        }
    }

    // From here on, returning early drops the recorder, which cancels and cleans up.
    let clock = Arc::new(PauseClock::new());
    let mut rec = Recorder {
        clock: clock.clone(),
        levels: levels.clone(),
        mic: o.microphone,
        failure: failure.clone(),
        stop: stop.clone(),
        cancel,
        end: end.clone(),
        parts: Some(Parts { source, pacer: None, sink: sink_thread, audio: None, video_tx: video_tx.clone(), audio_tx: audio_tx.clone() }),
        output: o.output.clone(),
        format: o.format,
        size: out_size,
    };
    let source = &mut rec.parts.as_mut().expect("just created").source;

    let open_audio = if want_audio {
        let spec = AudioSpec { system: o.system_audio, mic: o.microphone, mic_device: o.mic_device.clone().filter(|s| !s.is_empty()) };
        Some(audio::open(&spec, &mut **source)?)
    } else {
        None
    };
    let overlay = if o.show_cursor && o.highlight_clicks || o.show_keystrokes {
        let cfg = OverlayConfig {
            highlight: o.show_cursor && o.highlight_clicks,
            keystrokes: o.show_keystrokes,
            input_scale: source.input_scale(),
        };
        match Overlay::new(cfg, platform::cursor_position()) {
            Ok(ov) => Some(ov),
            Err(e) => {
                tracing::warn!("input overlays disabled: {e}");
                None
            }
        }
    } else {
        None
    };
    let origin = source.origin_fn();

    // Everything is open: time zero is now, for video and audio alike.
    clock.restart();
    let audio = match (open_audio, audio_tx) {
        (Some(sources), Some(tx)) => Some(sources.run(clock.clone(), tx, levels)?),
        _ => None,
    };
    let ctx = PacerCtx { slot, clock, stop, failure, end, fps, overlay, tx: video_tx };
    let pacer = std::thread::Builder::new().name("snap-record-pacer".into()).spawn(move || run_pacer(ctx, origin))?;
    let parts = rec.parts.as_mut().expect("just created");
    parts.pacer = Some(pacer);
    parts.audio = audio;
    Ok(rec)
}

struct PacerCtx {
    slot: Arc<FrameSlot>,
    clock: Arc<PauseClock>,
    stop: Arc<AtomicBool>,
    failure: Arc<Mutex<Option<String>>>,
    end: Arc<AtomicI64>,
    fps: u32,
    overlay: Option<Overlay>,
    tx: Sender<(Arc<Frame>, Hns)>,
}

/// Emit one frame per tick: the newest captured frame (repeated while the screen is static)
/// with overlays applied, stamped with the pause-adjusted clock.
fn run_pacer(mut c: PacerCtx, origin: platform::OriginFn) {
    let period = Duration::from_secs_f64(1.0 / c.fps as f64);
    let mut next = Instant::now();
    let mut last_sent: Option<(Arc<Frame>, Hns)> = None;
    loop {
        platform::sleep_until(next);
        if c.stop.load(Ordering::Relaxed) || c.failure.lock().is_some() {
            break;
        }
        let now = Instant::now();
        next += period;
        if next < now {
            next = now; // fell behind: skip ticks instead of bursting
        }
        if c.clock.is_paused() {
            continue;
        }
        let pts = hns_from(c.clock.elapsed());
        if last_sent.as_ref().is_some_and(|(_, t)| *t >= pts) {
            continue;
        }
        let Some(base) = c.slot.get() else { continue };
        let frame = match c.overlay.as_mut() {
            Some(ov) => {
                let mut f = (*base).clone();
                ov.draw(&mut f, origin(), platform::cursor_position(), now);
                Arc::new(f)
            }
            None => base,
        };
        match c.tx.try_send((frame.clone(), pts)) {
            Ok(()) => last_sent = Some((frame, pts)),
            Err(TrySendError::Full(_)) => {} // encoder is behind: drop this tick
            Err(TrySendError::Disconnected(_)) => break,
        }
    }
    // Close the stream at the exact stop time so the video is as long as the audio.
    if let Some((frame, _)) = last_sent {
        let _ = c.tx.send_timeout((frame, c.end.load(Ordering::SeqCst)), Duration::from_millis(500));
    }
}

fn run_sink(
    mut sink: Box<dyn Sink>,
    video: Receiver<(Arc<Frame>, Hns)>,
    audio: Option<Receiver<AudioChunk>>,
    end: Arc<AtomicI64>,
    cancel: Arc<AtomicBool>,
    failure: Arc<Mutex<Option<String>>>,
) -> Result<()> {
    let mut first_error: Option<Error> = None;
    let mut video_open = true;
    let mut audio_open = audio.is_some();
    while video_open || audio_open {
        let mut sel = Select::new();
        let vi = video_open.then(|| sel.recv(&video));
        let ai = audio.as_ref().filter(|_| audio_open).map(|a| sel.recv(a));
        let op = sel.select();
        let idx = op.index();
        if Some(idx) == vi {
            match op.recv(&video) {
                Ok((frame, pts)) => {
                    if first_error.is_none()
                        && let Err(e) = sink.video(&frame, pts)
                    {
                        fail(&failure, &e);
                        first_error = Some(e);
                    }
                }
                Err(_) => video_open = false,
            }
        } else if let (Some(a), true) = (audio.as_ref(), Some(idx) == ai) {
            match op.recv(a) {
                Ok(chunk) => {
                    if first_error.is_none()
                        && let Err(e) = sink.audio(&chunk)
                    {
                        fail(&failure, &e);
                        first_error = Some(e);
                    }
                }
                Err(_) => audio_open = false,
            }
        }
    }
    if cancel.load(Ordering::SeqCst) || first_error.is_some() {
        sink.abort();
        return first_error.map_or(Ok(()), Err);
    }
    sink.finish(end.load(Ordering::SeqCst))
}

/// Record the first encoder failure; the pacer polls it and stops feeding frames.
fn fail(failure: &Mutex<Option<String>>, e: &Error) {
    failure.lock().get_or_insert_with(|| e.to_string());
}
