//! AVAssetReader decoder: BGRA pictures and 48 kHz stereo float audio, merged in time order.

use std::path::Path;

use anyhow::anyhow;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_av_foundation::{AVAssetReader, AVAssetReaderTrackOutput, AVAssetTrack, AVMediaTypeAudio, AVMediaTypeVideo, AVURLAsset};
use objc2_avf_audio::{
    AVFormatIDKey, AVLinearPCMBitDepthKey, AVLinearPCMIsBigEndianKey, AVLinearPCMIsFloatKey, AVLinearPCMIsNonInterleaved,
    AVNumberOfChannelsKey, AVSampleRateKey,
};
use objc2_core_media::{CMSampleBuffer, CMTime, CMTimeRange};
use objc2_core_video::{
    CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress, kCVPixelFormatType_32BGRA,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};
use snap_core::{Error, Result};

use crate::{
    clock::{HNS_PER_SEC, Hns},
    decode::{Decoded, Decoder},
    edit::VideoInfo,
    frame::Frame,
};

fn av(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Other(anyhow!("{what}: {e}"))
}

fn any<T: objc2::Message>(o: &T) -> &AnyObject {
    // SAFETY: every Objective-C object is an AnyObject.
    unsafe { &*(o as *const T as *const AnyObject) }
}

fn missing() -> Error {
    av("AVFoundation", "a required settings key is unavailable on this macOS version")
}

struct Session {
    reader: Retained<AVAssetReader>,
    video: Retained<AVAssetReaderTrackOutput>,
    audio: Option<Retained<AVAssetReaderTrackOutput>>,
    next_video: Option<Retained<CMSampleBuffer>>,
    next_audio: Option<Retained<CMSampleBuffer>>,
    video_done: bool,
    audio_done: bool,
}

pub(crate) struct AvDecoder {
    asset: Retained<AVURLAsset>,
    video_track: Retained<AVAssetTrack>,
    audio_track: Option<Retained<AVAssetTrack>>,
    info: VideoInfo,
    start_ms: u64,
    session: Option<Session>,
}

pub(crate) fn open_decoder(path: &Path, with_audio: bool) -> Result<Box<dyn Decoder>> {
    if !path.exists() {
        return Err(Error::NotFound(path.display().to_string()));
    }
    // SAFETY: Objective-C calls on freshly created objects; the deprecated synchronous track
    // accessors are fine for local files.
    #[allow(deprecated)]
    unsafe {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let asset = AVURLAsset::URLAssetWithURL_options(&url, None);
        let video_track = asset
            .tracksWithMediaType(AVMediaTypeVideo.ok_or_else(missing)?)
            .firstObject()
            .ok_or_else(|| Error::invalid(format!("{} has no video track", path.display())))?;
        let audio_track = asset.tracksWithMediaType(AVMediaTypeAudio.ok_or_else(missing)?).firstObject();
        let size = video_track.naturalSize();
        let duration = asset.duration().seconds();
        let info = VideoInfo {
            duration_ms: if duration.is_finite() { (duration * 1000.0).round() as u64 } else { 0 },
            width: size.width.abs() as u32,
            height: size.height.abs() as u32,
            fps: video_track.nominalFrameRate(),
            has_audio: audio_track.is_some(),
        };
        Ok(Box::new(AvDecoder { asset, video_track, audio_track: audio_track.filter(|_| with_audio), info, start_ms: 0, session: None }))
    }
}

impl AvDecoder {
    fn start_session(&mut self) -> Result<()> {
        // SAFETY: reader construction and configuration per AVAssetReader's documented sequence.
        unsafe {
            let reader = AVAssetReader::assetReaderWithAsset_error(&self.asset).map_err(|e| av("open reader", e.localizedDescription()))?;
            let remaining = (self.info.duration_ms.saturating_sub(self.start_ms)).max(1);
            reader.setTimeRange(CMTimeRange {
                start: CMTime::new(self.start_ms as i64, 1000),
                duration: CMTime::new(remaining as i64, 1000),
            });

            let pixel_key = NSString::from_str("PixelFormatType");
            let video_settings =
                NSDictionary::<NSString, AnyObject>::from_slices(&[&*pixel_key], &[any(&*NSNumber::new_u32(kCVPixelFormatType_32BGRA))]);
            let video = AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(&self.video_track, Some(&video_settings));
            video.setAlwaysCopiesSampleData(false);
            reader.addOutput(&video);

            let audio = match &self.audio_track {
                Some(track) => {
                    let format = NSNumber::new_u32(0x6C70_636D); // 'lpcm'
                    let (bits, channels, rate) = (NSNumber::new_u32(32), NSNumber::new_u32(2), NSNumber::new_u32(48_000));
                    let (yes, no) = (NSNumber::new_bool(true), NSNumber::new_bool(false));
                    let settings = NSDictionary::<NSString, AnyObject>::from_slices(
                        &[
                            AVFormatIDKey.ok_or_else(missing)?,
                            AVLinearPCMBitDepthKey.ok_or_else(missing)?,
                            AVNumberOfChannelsKey.ok_or_else(missing)?,
                            AVSampleRateKey.ok_or_else(missing)?,
                            AVLinearPCMIsFloatKey.ok_or_else(missing)?,
                            AVLinearPCMIsNonInterleaved.ok_or_else(missing)?,
                            AVLinearPCMIsBigEndianKey.ok_or_else(missing)?,
                        ],
                        &[any(&*format), any(&*bits), any(&*channels), any(&*rate), any(&*yes), any(&*no), any(&*no)],
                    );
                    let out = AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(track, Some(&settings));
                    reader.addOutput(&out);
                    Some(out)
                }
                None => None,
            };
            if !reader.startReading() {
                return Err(av("start reading", reader.error().map(|e| e.localizedDescription().to_string()).unwrap_or_default()));
            }
            self.session =
                Some(Session { reader, video, audio_done: audio.is_none(), audio, next_video: None, next_audio: None, video_done: false });
        }
        Ok(())
    }
}

fn seconds_to_hns(t: CMTime) -> Hns {
    // SAFETY: plain value conversion.
    (unsafe { t.seconds() } * HNS_PER_SEC as f64) as Hns
}

fn picture(buffer: &CMSampleBuffer) -> Option<Frame> {
    // SAFETY: the pixel buffer is locked while its rows are copied out.
    unsafe {
        let pixels = buffer.image_buffer()?;
        if CVPixelBufferLockBaseAddress(&pixels, CVPixelBufferLockFlags::ReadOnly) != 0 {
            return None;
        }
        let (w, h, stride) = (CVPixelBufferGetWidth(&pixels), CVPixelBufferGetHeight(&pixels), CVPixelBufferGetBytesPerRow(&pixels));
        let base = CVPixelBufferGetBaseAddress(&pixels) as *const u8;
        let frame = (!base.is_null()).then(|| {
            let src = std::slice::from_raw_parts(base, stride * h);
            let mut f = Frame::black(w as u32, h as u32);
            f.blit(src, stride, w as u32, h as u32, 0, 0);
            f
        });
        CVPixelBufferUnlockBaseAddress(&pixels, CVPixelBufferLockFlags::ReadOnly);
        frame
    }
}

impl Decoder for AvDecoder {
    fn info(&self) -> VideoInfo {
        self.info.clone()
    }

    fn seek(&mut self, ms: u64) -> Result<()> {
        if let Some(s) = self.session.take() {
            // SAFETY: cancelling the previous reader.
            unsafe { s.reader.cancelReading() };
        }
        self.start_ms = ms;
        self.start_session()
    }

    fn next(&mut self) -> Result<Option<Decoded>> {
        if self.session.is_none() {
            self.start_session()?;
        }
        let s = self.session.as_mut().expect("session started above");
        // SAFETY: pulling samples from started reader outputs.
        unsafe {
            if s.next_video.is_none() && !s.video_done {
                s.next_video = s.video.copyNextSampleBuffer();
                s.video_done = s.next_video.is_none();
            }
            if s.next_audio.is_none() && !s.audio_done {
                s.next_audio = s.audio.as_ref().and_then(|a| a.copyNextSampleBuffer());
                s.audio_done = s.next_audio.is_none();
            }
            let v_pts = s.next_video.as_ref().map(|b| seconds_to_hns(b.presentation_time_stamp()));
            let a_pts = s.next_audio.as_ref().map(|b| seconds_to_hns(b.presentation_time_stamp()));
            match (v_pts, a_pts) {
                (None, None) => {
                    if let Some(e) = s.reader.error() {
                        return Err(av("decode", e.localizedDescription()));
                    }
                    Ok(None)
                }
                (Some(v), a) if a.is_none_or(|a| v <= a) => {
                    let buffer = s.next_video.take().expect("peeked");
                    Ok(Some(match picture(&buffer) {
                        Some(frame) => Decoded::Video(frame, v),
                        None => return self.next(),
                    }))
                }
                (_, Some(a)) => {
                    let buffer = s.next_audio.take().expect("peeked");
                    match super::pcm_f32(&buffer) {
                        Some((samples, _, _)) => Ok(Some(Decoded::Audio(samples, a))),
                        None => self.next(),
                    }
                }
                (Some(_), None) => unreachable!("covered by the first arm"),
            }
        }
    }
}
