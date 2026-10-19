//! Encoder sinks: everything that turns timed frames + audio into a file.

use std::{
    fs::File,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Stdio},
    sync::Arc,
    thread::JoinHandle,
};

use parking_lot::Mutex;
use snap_core::{
    Error, Result,
    settings::{EncoderPref, VideoFormat},
    t,
};

use crate::{
    audio::AudioChunk,
    clock::{HNS_PER_SEC, Hns},
    ffmpeg,
    frame::{Frame, scale_box},
    gifenc::GifEncoder,
    platform,
    webpenc::WebpEncoder,
};

/// Consumer of a recording. Lives on one thread; frame sizes always equal [`SinkSpec`]'s.
pub(crate) trait Sink {
    fn video(&mut self, frame: &Arc<Frame>, pts: Hns) -> Result<()>;
    fn audio(&mut self, chunk: &AudioChunk) -> Result<()>;
    /// Finish the file; `end` is the final clock position.
    fn finish(self: Box<Self>, end: Hns) -> Result<()>;
    /// Discard the output (recording cancelled or failed).
    fn abort(self: Box<Self>) {}
}

#[derive(Clone)]
pub(crate) struct SinkSpec {
    pub format: VideoFormat,
    pub w: u32,
    pub h: u32,
    pub fps: u32,
    /// Bits per second (0 = automatic).
    pub bitrate: u32,
    pub audio: bool,
    pub encoder: EncoderPref,
    pub output: PathBuf,
    pub ffmpeg: Option<PathBuf>,
    pub max_gif_width: u32,
    /// Floyd-Steinberg dithering for GIF output (photographic sources).
    pub dither: bool,
}

impl SinkSpec {
    pub fn video_bitrate(&self) -> u32 {
        if self.bitrate > 0 {
            return self.bitrate;
        }
        let bpp = 0.12f64;
        ((self.w as f64 * self.h as f64 * self.fps as f64 * bpp) as u32).clamp(1_500_000, 50_000_000)
    }
}

pub(crate) fn build(spec: &SinkSpec) -> Result<Box<dyn Sink>> {
    match spec.format {
        VideoFormat::Gif => Ok(Box::new(GifSink::new(spec)?)),
        VideoFormat::Webp => Ok(Box::new(WebpSink::new(spec)?)),
        VideoFormat::Mp4 if std::env::var("SNAP_RECORD_BACKEND").is_ok_and(|v| v.eq_ignore_ascii_case("ffmpeg")) => Ok(Box::new(FfmpegSink::new(spec)?)),
        VideoFormat::Mp4 => match platform::native_mp4_sink(spec) {
            Ok(sink) => Ok(sink),
            Err(native) => {
                tracing::warn!("native MP4 encoder unavailable ({native}); trying ffmpeg");
                // Report whichever failure is the more informative one.
                FfmpegSink::new(spec).map(|s| Box::new(s) as Box<dyn Sink>).map_err(|e| if matches!(native, Error::Unsupported(_)) { e } else { native })
            }
        },
    }
}

/// Target size for GIF/WebP, from the recording size and `max_gif_width`.
pub(crate) fn scaled_size(spec: &SinkSpec) -> (u32, u32) {
    let max = if spec.max_gif_width == 0 { spec.w } else { spec.max_gif_width };
    let (w, h) = crate::frame::fit_width(spec.w, spec.h, max);
    (w.min(spec.w & !1).max(2), h.min(spec.h & !1).max(2))
}

fn scaled_bgra(frame: &Frame, w: u32, h: u32) -> Vec<u8> {
    scale_box(&frame.data, frame.w, frame.h, w, h)
}

// ---------------------------------------------------------------- GIF

struct GifSink {
    enc: GifEncoder<BufWriter<File>>,
    size: (u32, u32),
}

impl GifSink {
    fn new(spec: &SinkSpec) -> Result<Self> {
        let size = scaled_size(spec);
        let file = BufWriter::new(File::create(&spec.output)?);
        Ok(Self { enc: GifEncoder::new(file, size.0, size.1, spec.dither)?, size })
    }
}

impl Sink for GifSink {
    fn video(&mut self, frame: &Arc<Frame>, pts: Hns) -> Result<()> {
        self.enc.push(&scaled_bgra(frame, self.size.0, self.size.1), pts)
    }
    fn audio(&mut self, _: &AudioChunk) -> Result<()> {
        Ok(())
    }
    fn finish(self: Box<Self>, end: Hns) -> Result<()> {
        let mut w = self.enc.finish(end)?;
        w.flush()?;
        Ok(())
    }
}

// ---------------------------------------------------------------- WebP

struct WebpSink {
    enc: WebpEncoder,
    output: PathBuf,
}

impl WebpSink {
    fn new(spec: &SinkSpec) -> Result<Self> {
        let (w, h) = scaled_size(spec);
        Ok(Self { enc: WebpEncoder::new(w, h, 75.0)?, output: spec.output.clone() })
    }
}

impl Sink for WebpSink {
    fn video(&mut self, frame: &Arc<Frame>, pts: Hns) -> Result<()> {
        let (w, h) = self.enc.size();
        self.enc.push(&scaled_bgra(frame, w, h), pts)
    }
    fn audio(&mut self, _: &AudioChunk) -> Result<()> {
        Ok(())
    }
    fn finish(self: Box<Self>, end: Hns) -> Result<()> {
        let bytes = self.enc.finish(end)?;
        std::fs::write(&self.output, bytes)?;
        Ok(())
    }
}

// ---------------------------------------------------------------- ffmpeg

/// H.264 MP4 through an ffmpeg child process fed raw BGRA on stdin; audio is
/// spooled to a temporary raw file and muxed in at the end.
pub(crate) struct FfmpegSink {
    ffmpeg: PathBuf,
    child: Child,
    stdin: Option<ChildStdin>,
    stderr: Arc<Mutex<Vec<u8>>>,
    stderr_thread: Option<JoinHandle<()>>,
    fps: f64,
    written: i64,
    last: Option<Arc<Frame>>,
    audio: Option<(PathBuf, BufWriter<File>)>,
    silent_video: PathBuf,
    output: PathBuf,
}

impl FfmpegSink {
    pub fn new(spec: &SinkSpec) -> Result<Self> {
        let ffmpeg = ffmpeg::require(spec.ffmpeg.as_deref())?;
        let enc = ffmpeg::pick_encoder(&ffmpeg, spec.encoder).ok_or_else(|| Error::unsupported(t("record.error.no_h264")))?;
        tracing::info!("ffmpeg sink using {}", enc.name);

        let tmp_dir = spec.output.parent().map(Path::to_path_buf).unwrap_or_default();
        let stem = spec.output.file_stem().and_then(|s| s.to_str()).unwrap_or("recording");
        let audio = if spec.audio {
            let p = tmp_dir.join(format!(".{stem}.audio.f32"));
            Some((p.clone(), BufWriter::new(File::create(&p)?)))
        } else {
            None
        };
        let silent_video = if spec.audio { tmp_dir.join(format!(".{stem}.video.mp4")) } else { spec.output.clone() };

        let mut cmd = ffmpeg::command(&ffmpeg);
        cmd.args(["-y", "-f", "rawvideo", "-pix_fmt", "bgra", "-video_size"])
            .arg(format!("{}x{}", spec.w, spec.h))
            .arg("-framerate")
            .arg(spec.fps.to_string())
            .args(["-i", "-"])
            .args(ffmpeg::encoder_args(&enc, spec.video_bitrate()))
            .arg("-r")
            .arg(spec.fps.to_string())
            .args(["-movflags", "+faststart"])
            .arg(&silent_video)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take();
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let mut pipe = child.stderr.take().expect("stderr piped");
        let sink_buf = stderr.clone();
        let stderr_thread = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = std::io::Read::read_to_end(&mut pipe, &mut buf);
            *sink_buf.lock() = buf;
        });

        Ok(Self {
            ffmpeg,
            child,
            stdin,
            stderr,
            stderr_thread: Some(stderr_thread),
            fps: spec.fps as f64,
            written: 0,
            last: None,
            audio,
            silent_video,
            output: spec.output.clone(),
        })
    }

    fn write_frame(&mut self, frame: &Frame) -> Result<()> {
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(Error::Other(anyhow::anyhow!("ffmpeg already closed")));
        };
        if let Err(e) = stdin.write_all(&frame.data) {
            let _ = self.child.wait();
            if let Some(t) = self.stderr_thread.take() {
                let _ = t.join();
            }
            return Err(Error::Other(anyhow::anyhow!("ffmpeg stopped: {} ({e})", ffmpeg::tail(&self.stderr.lock()))));
        }
        self.written += 1;
        Ok(())
    }
}

impl Sink for FfmpegSink {
    fn video(&mut self, frame: &Arc<Frame>, pts: Hns) -> Result<()> {
        let idx = (pts as f64 / HNS_PER_SEC as f64 * self.fps).round() as i64;
        if idx < self.written {
            return Ok(()); // faster than the constant frame rate: drop
        }
        // Repeat the previous picture across skipped slots to keep a constant rate.
        let filler = self.last.clone().unwrap_or_else(|| frame.clone());
        while self.written < idx {
            self.write_frame(&filler)?;
        }
        self.write_frame(frame)?;
        self.last = Some(frame.clone());
        Ok(())
    }

    fn audio(&mut self, chunk: &AudioChunk) -> Result<()> {
        if let Some((_, w)) = self.audio.as_mut() {
            for s in &chunk.samples {
                w.write_all(&s.to_le_bytes())?;
            }
        }
        Ok(())
    }

    fn abort(mut self: Box<Self>) {
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some((path, _)) = self.audio.take() {
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_file(&self.silent_video);
    }

    fn finish(mut self: Box<Self>, end: Hns) -> Result<()> {
        if let Some(last) = self.last.clone() {
            let end_idx = (end as f64 / HNS_PER_SEC as f64 * self.fps).round() as i64;
            while self.written < end_idx {
                self.write_frame(&last)?;
            }
        }
        drop(self.stdin.take());
        let status = self.child.wait()?;
        if let Some(t) = self.stderr_thread.take() {
            let _ = t.join();
        }
        if !status.success() {
            return Err(Error::Other(anyhow::anyhow!("ffmpeg failed: {}", ffmpeg::tail(&self.stderr.lock()))));
        }
        if let Some((path, mut w)) = self.audio.take() {
            w.flush()?;
            drop(w);
            let result = mux_audio(&self.ffmpeg, &self.silent_video, &path, &self.output);
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_file(&self.silent_video);
            result?;
        }
        Ok(())
    }
}

fn mux_audio(ffmpeg: &Path, video: &Path, audio_f32: &Path, output: &Path) -> Result<()> {
    use std::ffi::OsStr;
    let args: Vec<&OsStr> = vec![
        "-y".as_ref(),
        "-i".as_ref(),
        video.as_os_str(),
        "-f".as_ref(),
        "f32le".as_ref(),
        "-ar".as_ref(),
        "48000".as_ref(),
        "-ac".as_ref(),
        "2".as_ref(),
        "-i".as_ref(),
        audio_f32.as_os_str(),
        "-map".as_ref(),
        "0:v:0".as_ref(),
        "-map".as_ref(),
        "1:a:0".as_ref(),
        "-c:v".as_ref(),
        "copy".as_ref(),
        "-c:a".as_ref(),
        "aac".as_ref(),
        "-b:a".as_ref(),
        "192k".as_ref(),
        "-shortest".as_ref(),
        "-movflags".as_ref(),
        "+faststart".as_ref(),
        output.as_os_str(),
    ];
    ffmpeg::run(ffmpeg, &args)
}
