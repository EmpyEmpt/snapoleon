//! Media Foundation: H.264/AAC MP4 encoding (hardware MFT when available) and decoding.

use std::{ffi::c_void, path::Path, ptr, sync::Arc};

use anyhow::anyhow;
use snap_core::{Error, Result, settings::EncoderPref};
use windows::{
    Win32::{
        Media::MediaFoundation::*,
        System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize},
    },
    core::{GUID, HSTRING, PWSTR},
};

use crate::{
    EncoderInfo,
    audio::AudioChunk,
    clock::{HNS_PER_SEC, Hns},
    decode::{Decoded, Decoder},
    edit::VideoInfo,
    frame::Frame,
    sink::{Sink, SinkSpec},
};

fn mf(e: windows::core::Error) -> Error {
    Error::Other(anyhow!("Media Foundation: {e}"))
}

fn hstr(path: &Path) -> HSTRING {
    HSTRING::from(path.to_string_lossy().as_ref())
}

/// Per-thread COM + Media Foundation lifetime.
struct MfInit {
    com: bool,
}

impl MfInit {
    fn new() -> Result<Self> {
        // SAFETY: balanced with CoUninitialize / MFShutdown in Drop.
        unsafe {
            // S_FALSE (already initialised) is fine; a different apartment model is tolerated too.
            let com = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
            if let Err(e) = MFStartup(MF_VERSION, MFSTARTUP_FULL) {
                if com {
                    CoUninitialize();
                }
                return Err(mf(e));
            }
            Ok(Self { com })
        }
    }
}

impl Drop for MfInit {
    fn drop(&mut self) {
        // SAFETY: matches `new`.
        unsafe {
            let _ = MFShutdown();
            if self.com {
                CoUninitialize();
            }
        }
    }
}

fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

/// H.264 + AAC MP4 writer on an `IMFSinkWriter`.
pub(crate) struct Mp4Writer {
    writer: IMFSinkWriter,
    video: u32,
    audio: Option<u32>,
    frame_dur: Hns,
    _init: MfInit,
}

impl Mp4Writer {
    pub fn create(path: &Path, w: u32, h: u32, fps: u32, bitrate: u32, hardware: bool, with_audio: bool) -> Result<Self> {
        let init = MfInit::new()?;
        // SAFETY: plain COM calls with valid arguments; results are checked.
        unsafe {
            let mut attrs: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attrs, 4).map_err(mf)?;
            let attrs = attrs.ok_or_else(|| Error::Other(anyhow!("MFCreateAttributes returned nothing")))?;
            attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, hardware as u32).map_err(mf)?;
            attrs.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1).map_err(mf)?;
            let writer = MFCreateSinkWriterFromURL(&hstr(path), None::<&IMFByteStream>, &attrs).map_err(mf)?;

            let out = MFCreateMediaType().map_err(mf)?;
            out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(mf)?;
            out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264).map_err(mf)?;
            out.SetUINT32(&MF_MT_AVG_BITRATE, bitrate).map_err(mf)?;
            out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).map_err(mf)?;
            out.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h)).map_err(mf)?;
            out.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1)).map_err(mf)?;
            out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1)).map_err(mf)?;
            out.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32).map_err(mf)?;
            out.SetUINT32(&MF_MT_MAX_KEYFRAME_SPACING, fps * 2).map_err(mf)?;
            let video = writer.AddStream(&out).map_err(mf)?;

            let input = MFCreateMediaType().map_err(mf)?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(mf)?;
            input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).map_err(mf)?;
            input.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).map_err(mf)?;
            input.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h)).map_err(mf)?;
            input.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1)).map_err(mf)?;
            input.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1)).map_err(mf)?;
            // A positive stride declares top-down rows.
            input.SetUINT32(&MF_MT_DEFAULT_STRIDE, w * 4).map_err(mf)?;
            writer.SetInputMediaType(video, &input, None::<&IMFAttributes>).map_err(mf)?;

            let audio = if with_audio {
                let out = MFCreateMediaType().map_err(mf)?;
                out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).map_err(mf)?;
                out.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC).map_err(mf)?;
                out.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).map_err(mf)?;
                out.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 48_000).map_err(mf)?;
                out.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2).map_err(mf)?;
                out.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 24_000).map_err(mf)?; // 192 kbit/s
                out.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0).map_err(mf)?;
                out.SetUINT32(&MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, 0x29).map_err(mf)?;
                let idx = writer.AddStream(&out).map_err(mf)?;

                let input = MFCreateMediaType().map_err(mf)?;
                input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).map_err(mf)?;
                input.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM).map_err(mf)?;
                input.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).map_err(mf)?;
                input.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 48_000).map_err(mf)?;
                input.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2).map_err(mf)?;
                input.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4).map_err(mf)?;
                input.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 192_000).map_err(mf)?;
                writer.SetInputMediaType(idx, &input, None::<&IMFAttributes>).map_err(mf)?;
                Some(idx)
            } else {
                None
            };

            writer.BeginWriting().map_err(mf)?;
            Ok(Self { writer, video, audio, frame_dur: HNS_PER_SEC / fps as i64, _init: init })
        }
    }

    fn sample(data: &[u8], pts: Hns, dur: Hns) -> Result<IMFSample> {
        // SAFETY: the buffer is locked for exactly `data.len()` bytes while copying.
        unsafe {
            let buf = MFCreateMemoryBuffer(data.len() as u32).map_err(mf)?;
            let mut dst: *mut u8 = ptr::null_mut();
            buf.Lock(&mut dst, None, None).map_err(mf)?;
            ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
            buf.Unlock().map_err(mf)?;
            buf.SetCurrentLength(data.len() as u32).map_err(mf)?;
            let sample = MFCreateSample().map_err(mf)?;
            sample.AddBuffer(&buf).map_err(mf)?;
            sample.SetSampleTime(pts).map_err(mf)?;
            sample.SetSampleDuration(dur).map_err(mf)?;
            Ok(sample)
        }
    }

    /// `dur` of `None` means one frame period.
    pub fn write_video(&self, bgra: &[u8], pts: Hns, dur: Option<Hns>) -> Result<()> {
        let sample = Self::sample(bgra, pts, dur.unwrap_or(self.frame_dur).max(1))?;
        // SAFETY: valid stream index and sample.
        unsafe { self.writer.WriteSample(self.video, &sample) }.map_err(mf)
    }

    pub fn write_audio(&self, stereo_f32: &[f32], pts: Hns) -> Result<()> {
        let Some(idx) = self.audio else { return Ok(()) };
        let pcm: Vec<u8> = stereo_f32.iter().flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes()).collect();
        let dur = (stereo_f32.len() as i64 / 2) * HNS_PER_SEC / 48_000;
        let sample = Self::sample(&pcm, pts, dur.max(1))?;
        // SAFETY: valid stream index and sample.
        unsafe { self.writer.WriteSample(idx, &sample) }.map_err(mf)
    }

    pub fn finalize(self) -> Result<()> {
        // SAFETY: the writer was started with BeginWriting.
        unsafe { self.writer.Finalize() }.map_err(mf)
    }
}

/// Whether a hardware H.264 MFT really encodes at this size (checked once per size class by a tiny test encode).
fn hardware_encoder_works(w: u32, h: u32, fps: u32, bitrate: u32) -> bool {
    static CACHE: parking_lot::Mutex<Vec<((u32, u32), bool)>> = parking_lot::Mutex::new(Vec::new());
    if let Some((_, ok)) = CACHE.lock().iter().find(|(k, _)| *k == (w, h)) {
        return *ok;
    }
    let path = std::env::temp_dir().join(format!("snap-record-probe-{}-{w}x{h}.mp4", std::process::id()));
    let result = (|| -> Result<()> {
        let writer = Mp4Writer::create(&path, w, h, fps, bitrate, true, false)?;
        let frame = Frame::black(w, h);
        for i in 0..6 {
            writer.write_video(&frame.data, i * HNS_PER_SEC / fps as i64, None)?;
        }
        writer.finalize()
    })();
    let _ = std::fs::remove_file(&path);
    let ok = result.is_ok();
    if let Err(e) = result {
        tracing::info!("hardware H.264 encoder rejected {w}x{h}: {e}");
    }
    CACHE.lock().push(((w, h), ok));
    ok
}

struct MfSink {
    writer: Mp4Writer,
}

impl Sink for MfSink {
    fn video(&mut self, frame: &Arc<Frame>, pts: crate::clock::Hns) -> Result<()> {
        self.writer.write_video(&frame.data, pts, None)
    }

    fn audio(&mut self, chunk: &AudioChunk) -> Result<()> {
        self.writer.write_audio(&chunk.samples, chunk.pts)
    }

    fn finish(self: Box<Self>, _end: crate::clock::Hns) -> Result<()> {
        self.writer.finalize()
    }
}

pub(crate) fn native_mp4_sink(spec: &SinkSpec) -> Result<Box<dyn Sink>> {
    let bitrate = spec.video_bitrate();
    let hardware = spec.encoder != EncoderPref::Software && hardware_encoder_works(spec.w, spec.h, spec.fps, bitrate);
    if spec.encoder == EncoderPref::Hardware && !hardware {
        tracing::warn!("no working hardware H.264 encoder; using the software encoder");
    }
    tracing::info!(hardware, bitrate, "native MP4 encoder: {}x{} @ {} fps", spec.w, spec.h, spec.fps);
    let writer = Mp4Writer::create(&spec.output, spec.w, spec.h, spec.fps, bitrate, hardware, spec.audio)?;
    Ok(Box::new(MfSink { writer }))
}

/// H.264 encoder MFTs known to Media Foundation.
pub(crate) fn native_encoders() -> Vec<EncoderInfo> {
    let Ok(_init) = MfInit::new() else { return Vec::new() };
    let mut out = Vec::new();
    let output = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_H264 };
    for (flags, hardware) in
        [(MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER, true), (MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER, false)]
    {
        let mut list: *mut Option<IMFActivate> = ptr::null_mut();
        let mut count = 0u32;
        // SAFETY: MFTEnumEx allocates `list`; each activate is released and the array freed.
        unsafe {
            if MFTEnumEx(MFT_CATEGORY_VIDEO_ENCODER, flags, None, Some(&output), &mut list, &mut count).is_err() || list.is_null() {
                continue;
            }
            for i in 0..count as usize {
                let Some(activate) = (*list.add(i)).take() else { continue };
                let mut name = PWSTR::null();
                let mut len = 0u32;
                if activate.GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut name, &mut len).is_ok() {
                    let s = name.to_string().unwrap_or_default();
                    CoTaskMemFree(Some(name.0 as *const c_void));
                    if !s.is_empty() {
                        out.push(EncoderInfo { name: s, hardware });
                    }
                }
            }
            CoTaskMemFree(Some(list as *const c_void));
        }
    }
    out
}

// ------------------------------------------------------------------ decoding

const FIRST_VIDEO: u32 = 0xFFFF_FFFC;
const FIRST_AUDIO: u32 = 0xFFFF_FFFD;
const ANY_STREAM: u32 = 0xFFFF_FFFE;
const MEDIA_SOURCE: u32 = 0xFFFF_FFFF;

pub(crate) struct MfDecoder {
    reader: IMFSourceReader,
    info: VideoInfo,
    w: u32,
    h: u32,
    video_stream: Option<u32>,
    audio: bool,
    video_eos: bool,
    audio_eos: bool,
    _init: MfInit,
}

impl MfDecoder {
    pub fn open(path: &Path, with_audio: bool) -> Result<Self> {
        let init = MfInit::new()?;
        // SAFETY: standard source-reader setup; every call is checked.
        unsafe {
            let mut attrs: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attrs, 2).map_err(mf)?;
            let attrs = attrs.ok_or_else(|| Error::Other(anyhow!("MFCreateAttributes returned nothing")))?;
            attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1).map_err(mf)?;
            let reader =
                MFCreateSourceReaderFromURL(&hstr(path), &attrs).map_err(|e| Error::NotFound(format!("{}: {e}", path.display())))?;

            let native =
                reader.GetNativeMediaType(FIRST_VIDEO, 0).map_err(|_| Error::invalid(format!("{} has no video track", path.display())))?;
            let fps = native.GetUINT64(&MF_MT_FRAME_RATE).map(|r| (r >> 32) as f32 / (r as u32).max(1) as f32).unwrap_or(30.0);

            let want = MFCreateMediaType().map_err(mf)?;
            want.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(mf)?;
            want.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).map_err(mf)?;
            reader.SetCurrentMediaType(FIRST_VIDEO, None, &want).map_err(mf)?;
            let current = reader.GetCurrentMediaType(FIRST_VIDEO).map_err(mf)?;
            let size = current.GetUINT64(&MF_MT_FRAME_SIZE).map_err(mf)?;

            let has_audio = reader.GetNativeMediaType(FIRST_AUDIO, 0).is_ok();
            let audio = has_audio && with_audio;
            if audio {
                let want = MFCreateMediaType().map_err(mf)?;
                want.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).map_err(mf)?;
                want.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float).map_err(mf)?;
                want.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2).map_err(mf)?;
                want.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 48_000).map_err(mf)?;
                want.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 32).map_err(mf)?;
                want.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 8).map_err(mf)?;
                want.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 384_000).map_err(mf)?;
                reader.SetCurrentMediaType(FIRST_AUDIO, None, &want).map_err(mf)?;
            }
            reader.SetStreamSelection(ANY_STREAM, false).map_err(mf)?;
            reader.SetStreamSelection(FIRST_VIDEO, true).map_err(mf)?;
            if audio {
                reader.SetStreamSelection(FIRST_AUDIO, true).map_err(mf)?;
            }

            let duration =
                reader.GetPresentationAttribute(MEDIA_SOURCE, &MF_PD_DURATION).ok().and_then(|v| u64::try_from(&v).ok()).unwrap_or(0);
            let (w, h) = ((size >> 32) as u32, size as u32);
            Ok(Self {
                video_stream: find_stream(&reader, &MFMediaType_Video),
                reader,
                info: VideoInfo { duration_ms: duration / 10_000, width: w, height: h, fps, has_audio },
                w,
                h,
                audio,
                video_eos: false,
                audio_eos: !audio,
                _init: init,
            })
        }
    }

    fn video_frame(&self, sample: &IMFSample) -> Result<Frame> {
        // SAFETY: the contiguous buffer is locked while its bytes are copied out.
        unsafe {
            let buf = sample.ConvertToContiguousBuffer().map_err(mf)?;
            // The decoder hands out padded pictures (1008x576 for a 1000x562 clip) and says so only in the
            // stream's current media type, which changes after the first sample: read the stride from there.
            let current = self.reader.GetCurrentMediaType(FIRST_VIDEO).map_err(mf)?;
            let signed = current.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| s as i32).unwrap_or(self.w as i32 * 4);
            let (stride, bottom_up) = (signed.unsigned_abs() as usize, signed < 0);
            let row = self.w as usize * 4;
            let (mut p, mut len) = (ptr::null_mut::<u8>(), 0u32);
            buf.Lock(&mut p, None, Some(&mut len)).map_err(mf)?;
            let src = std::slice::from_raw_parts(p, len as usize);
            let memory_rows = len as usize / stride.max(1);
            let mut data = vec![0u8; row * self.h as usize];
            for (y, out) in data.chunks_exact_mut(row).enumerate() {
                let from = if bottom_up { memory_rows.saturating_sub(1 + y) } else { y };
                if let Some(line) = src.get(from * stride..from * stride + row) {
                    out.copy_from_slice(line);
                }
            }
            buf.Unlock().map_err(mf)?;
            data.iter_mut().skip(3).step_by(4).for_each(|a| *a = 255);
            Ok(Frame { w: self.w, h: self.h, data })
        }
    }

    fn audio_samples(sample: &IMFSample) -> Result<Vec<f32>> {
        // SAFETY: the contiguous buffer is locked while its bytes are copied out.
        unsafe {
            let buf = sample.ConvertToContiguousBuffer().map_err(mf)?;
            let (mut p, mut len) = (ptr::null_mut::<u8>(), 0u32);
            buf.Lock(&mut p, None, Some(&mut len)).map_err(mf)?;
            let bytes = std::slice::from_raw_parts(p, len as usize);
            let out = bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
            buf.Unlock().map_err(mf)?;
            Ok(out)
        }
    }
}

impl Decoder for MfDecoder {
    fn info(&self) -> VideoInfo {
        self.info.clone()
    }

    fn seek(&mut self, ms: u64) -> Result<()> {
        let pos = windows::Win32::System::Com::StructuredStorage::PROPVARIANT::from(ms as i64 * 10_000);
        // SAFETY: GUID_NULL selects 100 ns units.
        unsafe { self.reader.SetCurrentPosition(&GUID::zeroed(), &pos) }.map_err(mf)?;
        self.video_eos = false;
        self.audio_eos = !self.audio;
        Ok(())
    }

    fn next(&mut self) -> Result<Option<Decoded>> {
        while !(self.video_eos && self.audio_eos) {
            let (mut stream, mut flags, mut ts) = (0u32, 0u32, 0i64);
            let mut sample: Option<IMFSample> = None;
            // SAFETY: all out-parameters are valid locals.
            unsafe { self.reader.ReadSample(ANY_STREAM, 0, Some(&mut stream), Some(&mut flags), Some(&mut ts), Some(&mut sample)) }
                .map_err(mf)?;
            let video = Some(stream) == self.video_stream;
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                if video {
                    self.video_eos = true;
                } else {
                    self.audio_eos = true;
                }
            }
            let Some(sample) = sample else { continue };
            return Ok(Some(if video {
                Decoded::Video(self.video_frame(&sample)?, ts)
            } else {
                Decoded::Audio(Self::audio_samples(&sample)?, ts)
            }));
        }
        Ok(None)
    }
}

/// Index of the first stream with the given major type.
fn find_stream(reader: &IMFSourceReader, major: &GUID) -> Option<u32> {
    // SAFETY: probing stream types; an invalid index ends the scan.
    (0..32)
        .take_while(|&i| unsafe { reader.GetNativeMediaType(i, 0) }.is_ok())
        .find(|&i| unsafe { reader.GetNativeMediaType(i, 0).and_then(|t| t.GetGUID(&MF_MT_MAJOR_TYPE)) }.is_ok_and(|g| g == *major))
}

pub(crate) fn open_decoder(path: &Path, with_audio: bool) -> Result<Box<dyn Decoder>> {
    Ok(Box::new(MfDecoder::open(path, with_audio)?))
}
