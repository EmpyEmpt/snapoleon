//! Recording glue: options from settings, history entries for finished recordings.

use std::path::{Path, PathBuf};

use snap_core::{
    CaptureContext, CaptureId, CaptureKind, Result, Settings,
    naming::{self, NameContext},
    now_ms, paths,
    settings::VideoFormat,
};
use snap_history::Entry;

use crate::{Engine, RecordOptions, RecordTarget, Recording};

/// Longest side of the poster frame handed to the thumbnail writer.
const POSTER_MAX_WIDTH: u32 = 640;
/// The poster is grabbed a little way in: the very first frames are often black or a transition.
const POSTER_OFFSET_MS: u64 = 400;

/// Push settings that live in other crates' globals (the ffmpeg location) into them.
pub(crate) fn apply_runtime_settings(s: &Settings) {
    let path = s.recording.ffmpeg_path.trim();
    snap_record::set_ffmpeg_path((!path.is_empty()).then(|| PathBuf::from(path)));
}

pub(crate) fn record_options(engine: &Engine, target: RecordTarget, format: VideoFormat) -> RecordOptions {
    let s = engine.settings();
    let r = &s.recording;
    let animated = format != VideoFormat::Mp4;
    RecordOptions {
        target,
        format,
        fps: if animated { r.gif_fps } else { r.fps },
        bitrate: r.bitrate,
        system_audio: r.system_audio && !animated,
        microphone: r.microphone && !animated,
        mic_device: Some(r.mic_device.clone()).filter(|d| !d.trim().is_empty()),
        show_cursor: r.show_cursor,
        highlight_clicks: r.highlight_clicks,
        show_keystrokes: r.show_keystrokes,
        encoder: r.encoder,
        max_gif_width: r.max_gif_width,
        output: output_path(&s, format),
        ffmpeg: Some(PathBuf::from(r.ffmpeg_path.trim())).filter(|p| !p.as_os_str().is_empty()),
        exclude_windows: Vec::new(),
    }
}

/// A fresh file name in the video folder, from the capture naming template.
fn output_path(s: &Settings, format: VideoFormat) -> PathBuf {
    let ext = match format {
        VideoFormat::Mp4 => "mp4",
        VideoFormat::Gif => "gif",
        VideoFormat::Webp => "webp",
    };
    let dir = paths::default_video_dir();
    let cx = NameContext { kind: Some(CaptureKind::Recording), ..NameContext::now() };
    let stem = naming::expand(&s.capture.filename_template, &cx);
    let stem = stem.trim_end_matches(['.', ' ']);
    let stem = if stem.is_empty() { snap_core::APP_NAME } else { stem };
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..).map(|n| dir.join(format!("{stem} ({n}).{ext}"))).find(|p| !p.exists()).expect("an unused name exists")
}

pub(crate) fn add_recording(engine: &Engine, rec: &Recording, context: CaptureContext) -> Result<CaptureId> {
    let id = CaptureId::new();
    let s = engine.settings();
    let Some(history) = engine.history().filter(|_| s.history.enabled) else { return Ok(id) };
    let entry = recording_entry(&id, &rec.path, rec.duration_ms, (rec.width, rec.height), rec.bytes, context, &s);
    history.add(&entry)?;
    engine.history_changed();
    Ok(id)
}

/// History row for a video file, with a poster frame as thumbnail.
pub(crate) fn recording_entry(
    id: &CaptureId,
    path: &Path,
    duration_ms: u64,
    size: (u32, u32),
    bytes: u64,
    context: CaptureContext,
    s: &Settings,
) -> Entry {
    let poster = snap_record::edit::frame_at(path, POSTER_OFFSET_MS.min(duration_ms / 2), POSTER_MAX_WIDTH)
        .map_err(|e| tracing::warn!("no poster frame for {}: {e}", path.display()))
        .ok();
    let thumb = poster
        .and_then(|img| snap_history::write_thumbnail(&img, id).map_err(|e| tracing::warn!("cannot write poster thumbnail: {e}")).ok());
    Entry {
        id: id.clone(),
        kind: CaptureKind::Recording,
        file: Some(path.to_path_buf()),
        project: None,
        thumb,
        width: size.0,
        height: size.1,
        bytes,
        created_ms: now_ms(),
        context: if s.privacy.record_context { context } else { CaptureContext::default() },
        text: None,
        links: Vec::new(),
        tags: Vec::new(),
        favorite: false,
        duration_ms: Some(duration_ms),
    }
}
