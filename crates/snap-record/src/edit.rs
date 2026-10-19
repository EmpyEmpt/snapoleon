//! Video toolbox: probe, frame grab, trim and export (including export-to-size).
//!
//! ffmpeg is used when it can be found; otherwise the native decoder/encoder
//! pair of the platform (Media Foundation on Windows) does the work.

use std::{
    ffi::OsString,
    path::Path,
    process::Stdio,
    sync::Arc,
};

use image::{RgbaImage, imageops::FilterType};
use snap_core::{
    Error, Result,
    settings::{EncoderPref, VideoFormat},
    t, tf,
};

use crate::{
    audio::AudioChunk,
    clock::{HNS_PER_SEC, Hns},
    decode::{self, Decoded, Decoder},
    ffdec::FfmpegDecoder,
    ffmpeg,
    frame::{Frame, even, fit_width, scale_box},
    sink::{self, SinkSpec},
};

#[derive(Debug, Clone, PartialEq)]
pub struct VideoInfo {
    pub duration_ms: u64,
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    pub has_audio: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    pub format: VideoFormat,
    /// Upper bound for the output size; bitrate / resolution are chosen to fit.
    pub max_bytes: Option<u64>,
    /// Output width (never upscaled); height follows the aspect ratio.
    pub width: Option<u32>,
    pub fps: Option<u32>,
    /// Keep only `[start_ms, end_ms)`.
    pub range: Option<(u64, u64)>,
    pub mute: bool,
}

pub fn probe(p: &Path) -> Result<VideoInfo> {
    if let Some(ff) = ffmpeg::ffmpeg_path()
        && let Ok(text) = ffmpeg::describe(&ff, p)
        && let Some(info) = parse_info(&text)
    {
        return Ok(info);
    }
    Ok(decode::open(p, false)?.info())
}

/// Decode the picture at `ms`, scaled down to at most `max_w` pixels wide (0 = native size).
pub fn frame_at(p: &Path, ms: u64, max_w: u32) -> Result<RgbaImage> {
    if let Some(ff) = ffmpeg::ffmpeg_path()
        && let Ok(img) = frame_with_ffmpeg(&ff, p, ms, max_w)
    {
        return Ok(img);
    }
    let mut dec = decode::open(p, false)?;
    if ms > 0 {
        dec.seek(ms)?;
    }
    let target = ms as Hns * (HNS_PER_SEC / 1000);
    let mut best: Option<Frame> = None;
    while let Some(item) = dec.next()? {
        if let Decoded::Video(frame, pts) = item {
            let reached = pts >= target;
            best = Some(frame);
            if reached {
                break;
            }
        }
    }
    let frame = best.ok_or_else(|| Error::NotFound(format!("no picture at {ms} ms in {}", p.display())))?;
    let mut img = frame.to_rgba_image();
    if max_w > 0 && img.width() > max_w {
        let h = (img.height() as u64 * max_w as u64 / img.width() as u64).max(1) as u32;
        img = image::imageops::resize(&img, max_w, h, FilterType::Triangle);
    }
    Ok(img)
}

/// Cut `[start_ms, end_ms)` into `output` (container chosen by its extension), re-encoding for exact cuts.
pub fn trim(input: &Path, output: &Path, start_ms: u64, end_ms: u64) -> Result<()> {
    let format = format_of(output)?;
    export(input, output, &ExportOptions { format, range: Some((start_ms, end_ms)), ..ExportOptions::default() }).map(|_| ())
}

/// Re-encode `input` into `output` according to `o`; returns the output size in bytes.
pub fn export(input: &Path, output: &Path, o: &ExportOptions) -> Result<u64> {
    let info = probe(input)?;
    let (start, end) = match o.range {
        Some((s, e)) => (s.min(info.duration_ms), e.min(info.duration_ms)),
        None => (0, info.duration_ms),
    };
    if end <= start {
        return Err(Error::invalid(t("record.error.empty_range")));
    }
    if let Some(dir) = output.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let plan = Plan { info, start, end, opts: o.clone() };
    match o.format {
        VideoFormat::Mp4 => plan.export_mp4(input, output),
        VideoFormat::Gif | VideoFormat::Webp => plan.export_animation(input, output),
    }
}

fn format_of(path: &Path) -> Result<VideoFormat> {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("mp4" | "m4v" | "mov") => Ok(VideoFormat::Mp4),
        Some("gif") => Ok(VideoFormat::Gif),
        Some("webp") => Ok(VideoFormat::Webp),
        _ => Err(Error::invalid(t("record.error.bad_extension"))),
    }
}

struct Plan {
    info: VideoInfo,
    start: u64,
    end: u64,
    opts: ExportOptions,
}

impl Plan {
    fn duration_s(&self) -> f64 {
        (self.end - self.start) as f64 / 1000.0
    }

    fn has_audio(&self) -> bool {
        self.info.has_audio && !self.opts.mute
    }

    /// Output dimensions for a width limit (never upscales; even sizes).
    fn dims(&self, width: u32) -> (u32, u32) {
        fit_width(self.info.width, self.info.height, width.min(self.info.width))
    }

    fn export_mp4(&self, input: &Path, output: &Path) -> Result<u64> {
        let mut width = self.opts.width.unwrap_or(self.info.width);
        let audio_bps = if self.has_audio() { 128_000.0 } else { 0.0 };
        let mut scale = 1.0f64;
        for attempt in 0..3 {
            let bitrate = self.opts.max_bytes.map(|max| {
                let total = max as f64 * 8.0 * 0.95 * scale;
                ((total / self.duration_s() - audio_bps).max(80_000.0)) as u32
            });
            self.encode_mp4(input, output, width, bitrate)?;
            let size = std::fs::metadata(output)?.len();
            let Some(max) = self.opts.max_bytes.filter(|&m| size > m) else { return Ok(size) };
            if attempt == 2 {
                let _ = std::fs::remove_file(output);
                return Err(Error::invalid(tf("record.error.size_unreachable", &[("smallest", &size.to_string()), ("limit", &max.to_string())])));
            }
            let ratio = max as f64 * 0.92 / size as f64;
            width = even(((self.dims(width).0 as f64) * ratio.sqrt()) as u32).max(64);
            scale *= 0.92;
        }
        unreachable!("the loop returns on its last attempt")
    }

    fn encode_mp4(&self, input: &Path, output: &Path, width: u32, bitrate: Option<u32>) -> Result<()> {
        let (w, h) = self.dims(width);
        if let Some(ff) = ffmpeg::ffmpeg_path()
            && let Some(enc) = ffmpeg::pick_encoder(&ff, EncoderPref::Auto)
        {
            let fps = self.opts.fps;
            let mut vf = Vec::new();
            if let Some(f) = fps {
                vf.push(format!("fps={f}"));
            }
            if (w, h) != (even(self.info.width), even(self.info.height)) {
                vf.push(format!("scale={w}:{h}:flags=lanczos"));
            }
            let mut args: Vec<OsString> = vec!["-y".into(), "-ss".into(), format!("{:.3}", self.start as f64 / 1000.0).into(), "-t".into(), format!("{:.3}", self.duration_s()).into(), "-i".into(), input.into()];
            if !vf.is_empty() {
                args.extend(["-vf".into(), vf.join(",").into()]);
            }
            match bitrate {
                Some(b) => args.extend(ffmpeg::encoder_args(&enc, b).into_iter().map(OsString::from)),
                None if enc.name == "libx264" => args.extend(["-c:v", "libx264", "-crf", "18", "-preset", "veryfast", "-pix_fmt", "yuv420p"].map(OsString::from)),
                None => args.extend(ffmpeg::encoder_args(&enc, auto_bitrate(w, h, fps.map_or(self.info.fps, |f| f as f32))).into_iter().map(OsString::from)),
            }
            if self.has_audio() {
                args.extend(["-c:a", "aac", "-b:a", "128k"].map(OsString::from));
            } else {
                args.push("-an".into());
            }
            args.extend(["-movflags".into(), "+faststart".into(), output.into()]);
            let refs: Vec<&std::ffi::OsStr> = args.iter().map(|a| a.as_os_str()).collect();
            return ffmpeg::run(&ff, &refs);
        }

        let fps = self.opts.fps.unwrap_or_else(|| self.info.fps.round().max(1.0) as u32).min(120);
        let mut dec = decode::open(input, self.has_audio())?;
        run_transcode(
            &mut *dec,
            output,
            &Transcode {
                format: VideoFormat::Mp4,
                start: self.start,
                end: self.end,
                size: (w, h),
                fps,
                bitrate: bitrate.unwrap_or(0),
                audio: self.has_audio(),
                dither: false,
            },
        )
    }

    fn export_animation(&self, input: &Path, output: &Path) -> Result<u64> {
        let format = self.opts.format;
        let mut width = self.opts.width.unwrap_or(960).min(self.info.width);
        let mut fps = self.opts.fps.unwrap_or(15).clamp(1, 50);
        for attempt in 0..4 {
            let (w, h) = self.dims(width);
            let ff = ffmpeg::ffmpeg_path();
            let mut dec: Box<dyn Decoder> = match &ff {
                Some(ff) => Box::new(FfmpegDecoder::open(ff, input, self.info.clone(), (w, h), fps as f64)),
                None => decode::open(input, false)?,
            };
            run_transcode(
                &mut *dec,
                output,
                &Transcode { format, start: self.start, end: self.end, size: (w, h), fps, bitrate: 0, audio: false, dither: format == VideoFormat::Gif },
            )?;
            let size = std::fs::metadata(output)?.len();
            let Some(max) = self.opts.max_bytes.filter(|&m| size > m) else { return Ok(size) };
            if attempt == 3 {
                let _ = std::fs::remove_file(output);
                return Err(Error::invalid(tf("record.error.size_unreachable", &[("smallest", &size.to_string()), ("limit", &max.to_string())])));
            }
            let ratio = max as f64 * 0.9 / size as f64;
            width = even(((w as f64) * ratio.sqrt().min(0.9)) as u32).max(64);
            if attempt >= 1 {
                fps = ((fps as f64 * 0.75) as u32).max(5);
            }
        }
        unreachable!("the loop returns on its last attempt")
    }
}

fn auto_bitrate(w: u32, h: u32, fps: f32) -> u32 {
    ((w as f64 * h as f64 * fps.max(1.0) as f64 * 0.12) as u32).clamp(1_500_000, 50_000_000)
}

struct Transcode {
    format: VideoFormat,
    start: u64,
    end: u64,
    size: (u32, u32),
    fps: u32,
    bitrate: u32,
    audio: bool,
    dither: bool,
}

/// Decode `[start, end)` and push it through the same sinks the recorder uses.
fn run_transcode(dec: &mut dyn Decoder, output: &Path, t: &Transcode) -> Result<()> {
    let spec = SinkSpec {
        format: t.format,
        w: t.size.0,
        h: t.size.1,
        fps: t.fps,
        bitrate: t.bitrate,
        audio: t.audio && t.format == VideoFormat::Mp4,
        encoder: EncoderPref::Auto,
        output: output.to_path_buf(),
        ffmpeg: None,
        max_gif_width: t.size.0,
        dither: t.dither,
    };
    let mut sink = sink::build(&spec)?;
    let result = pump(dec, &mut *sink, t, spec.audio);
    match result {
        Ok(()) => sink.finish((t.end - t.start) as Hns * (HNS_PER_SEC / 1000)),
        Err(e) => {
            sink.abort();
            let _ = std::fs::remove_file(output);
            Err(e)
        }
    }
}

fn pump(dec: &mut dyn Decoder, sink: &mut dyn sink::Sink, t: &Transcode, audio: bool) -> Result<()> {
    let ms = HNS_PER_SEC / 1000;
    let (start, end) = (t.start as Hns * ms, t.end as Hns * ms);
    let period = HNS_PER_SEC / t.fps.max(1) as i64;
    if t.start > 0 {
        dec.seek(t.start)?;
    }
    let mut next_emit = start;
    while let Some(item) = dec.next()? {
        match item {
            Decoded::Video(frame, pts) => {
                if pts < start {
                    continue;
                }
                if pts >= end {
                    break;
                }
                if pts < next_emit {
                    continue; // frame-rate reduction
                }
                next_emit = (next_emit + period).max(pts - period / 2);
                let frame = if (frame.w, frame.h) == t.size {
                    frame
                } else {
                    Frame { w: t.size.0, h: t.size.1, data: scale_box(&frame.data, frame.w, frame.h, t.size.0, t.size.1) }
                };
                sink.video(&Arc::new(frame), pts - start)?;
            }
            Decoded::Audio(samples, pts) if audio => {
                let frames = samples.len() / 2;
                let dur = frames as Hns * HNS_PER_SEC / 48_000;
                if pts + dur <= start || pts >= end {
                    continue;
                }
                let skip = if pts < start { ((start - pts) * 48_000 / HNS_PER_SEC) as usize } else { 0 };
                let keep_until = if pts + dur > end { ((end - pts) * 48_000 / HNS_PER_SEC) as usize } else { frames };
                if keep_until <= skip {
                    continue;
                }
                let begin = pts.max(start) - start;
                sink.audio(&AudioChunk { pts: begin, samples: samples[skip * 2..keep_until * 2].to_vec() })?;
            }
            Decoded::Audio(..) => {}
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- ffmpeg helpers

fn frame_with_ffmpeg(ff: &Path, p: &Path, ms: u64, max_w: u32) -> Result<RgbaImage> {
    let mut cmd = ffmpeg::command(ff);
    cmd.arg("-ss").arg(format!("{:.3}", ms as f64 / 1000.0)).arg("-i").arg(p).args(["-frames:v", "1"]);
    if max_w > 0 {
        cmd.arg("-vf").arg(format!("scale='min({max_w},iw)':-2:flags=lanczos"));
    }
    cmd.args(["-f", "image2pipe", "-vcodec", "png", "-"]).stdin(Stdio::null());
    let out = cmd.output()?;
    if !out.status.success() || out.stdout.is_empty() {
        return Err(Error::Other(anyhow::anyhow!("ffmpeg frame grab failed: {}", ffmpeg::tail(&out.stderr))));
    }
    Ok(image::load_from_memory_with_format(&out.stdout, image::ImageFormat::Png)?.to_rgba8())
}

/// Parse the stderr banner of `ffmpeg -i`.
fn parse_info(text: &str) -> Option<VideoInfo> {
    let duration_ms = text.lines().find_map(|l| {
        let rest = l.trim().strip_prefix("Duration:")?;
        let t = rest.trim().split(',').next()?.trim();
        let mut parts = t.split(':');
        let (h, m, s): (f64, f64, f64) = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
        Some(((h * 3600.0 + m * 60.0 + s) * 1000.0).round() as u64)
    })?;
    let video = text.lines().find(|l| l.contains("Video:") && l.contains("Stream #"))?;
    let (width, height) = video.split([' ', ',']).find_map(|tok| {
        let (w, h) = tok.split_once('x')?;
        let (w, h): (u32, u32) = (w.parse().ok()?, h.parse().ok()?);
        (w >= 16 && h >= 16 && !tok.starts_with("0x")).then_some((w, h))
    })?;
    let fps = video
        .split(", ")
        .find_map(|part| part.trim().strip_suffix(" fps").or_else(|| part.trim().strip_suffix(" tbr")).and_then(|v| v.parse::<f32>().ok()))
        .unwrap_or(30.0);
    let has_audio = text.lines().any(|l| l.contains("Audio:") && l.contains("Stream #"));
    Some(VideoInfo { duration_ms, width, height, fps, has_audio })
}
