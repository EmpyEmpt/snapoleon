//! AVAssetWriter MP4 sink: H.264 through VideoToolbox (hardware when available) and AAC.

use std::{
    ffi::c_void,
    path::Path,
    ptr::{self, NonNull},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::anyhow;
use block2::RcBlock;
use crossbeam_channel::RecvTimeoutError;
use objc2::{AllocAnyThread, rc::Retained, runtime::AnyObject};
use objc2_av_foundation::{
    AVAssetWriter, AVAssetWriterInput, AVAssetWriterInputPixelBufferAdaptor, AVAssetWriterStatus, AVFileTypeMPEG4, AVMediaTypeAudio,
    AVMediaTypeVideo, AVVideoAllowFrameReorderingKey, AVVideoAverageBitRateKey, AVVideoCodecKey, AVVideoCodecTypeH264,
    AVVideoColorPrimaries_ITU_R_709_2, AVVideoColorPrimariesKey, AVVideoColorPropertiesKey, AVVideoCompressionPropertiesKey,
    AVVideoEncoderSpecificationKey, AVVideoExpectedSourceFrameRateKey, AVVideoHeightKey, AVVideoMaxKeyFrameIntervalKey,
    AVVideoProfileLevelH264HighAutoLevel, AVVideoProfileLevelKey, AVVideoTransferFunction_ITU_R_709_2, AVVideoTransferFunctionKey,
    AVVideoWidthKey, AVVideoYCbCrMatrix_ITU_R_709_2, AVVideoYCbCrMatrixKey,
};
use objc2_avf_audio::{AVEncoderBitRateKey, AVFormatIDKey, AVNumberOfChannelsKey, AVSampleRateKey};
use objc2_core_audio_types::{AudioStreamBasicDescription, kAudioFormatMPEG4AAC};
use objc2_core_foundation::CFRetained;
use objc2_core_media::{
    CMAudioFormatDescription, CMAudioFormatDescriptionCreate, CMAudioSampleBufferCreateReadyWithPacketDescriptions, CMBlockBuffer,
    CMSampleBuffer, CMTime,
};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferPool, CVPixelBufferUnlockBaseAddress, kCVPixelFormatType_32BGRA,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};
use snap_core::{Error, Result, settings::EncoderPref};

use crate::{
    EncoderInfo,
    audio::AudioChunk,
    clock::{HNS_PER_SEC, Hns},
    frame::Frame,
    sink::{Sink, SinkSpec},
};

/// `'lpcm'`
const AUDIO_FORMAT_LINEAR_PCM: u32 = 0x6C70_636D;
/// `kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked`
const PCM_FLOAT_PACKED: u32 = 1 | 8;

fn av(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Other(anyhow!("{what}: {e}"))
}

fn time(hns: Hns) -> CMTime {
    // SAFETY: plain value constructor.
    unsafe { CMTime::new(hns, HNS_PER_SEC as i32) }
}

/// A `&T` Objective-C object viewed as `&AnyObject` for dictionary values.
fn any<T: objc2::Message>(o: &T) -> &AnyObject {
    // SAFETY: every Objective-C object is an AnyObject.
    unsafe { &*(o as *const T as *const AnyObject) }
}

fn number_u32(v: u32) -> Retained<NSNumber> {
    NSNumber::new_u32(v)
}

fn dict(keys: &[&NSString], values: &[&AnyObject]) -> Retained<NSDictionary<NSString, AnyObject>> {
    NSDictionary::from_slices(keys, values)
}

pub(crate) fn native_encoders() -> Vec<EncoderInfo> {
    vec![
        EncoderInfo { name: "Apple VideoToolbox H.264 (hardware)".into(), hardware: true },
        EncoderInfo { name: "Apple VideoToolbox H.264 (software)".into(), hardware: false },
    ]
}

struct AvSink {
    writer: Retained<AVAssetWriter>,
    video: Retained<AVAssetWriterInput>,
    adaptor: Retained<AVAssetWriterInputPixelBufferAdaptor>,
    audio: Option<Retained<AVAssetWriterInput>>,
    size: (u32, u32),
}

pub(crate) fn native_mp4_sink(spec: &SinkSpec) -> Result<Box<dyn Sink>> {
    // SAFETY: Objective-C object construction following AVAssetWriter's documented sequence.
    unsafe {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&spec.output.to_string_lossy()));
        remove_existing(&spec.output);
        let file_type = AVFileTypeMPEG4.ok_or_else(|| av("AVFoundation", "AVFileTypeMPEG4 missing"))?;
        let writer =
            AVAssetWriter::assetWriterWithURL_fileType_error(&url, file_type).map_err(|e| av("create writer", e.localizedDescription()))?;

        // Video input: H.264 High profile.
        let (k_codec, k_w, k_h, k_props) = (
            AVVideoCodecKey.ok_or_else(missing)?,
            AVVideoWidthKey.ok_or_else(missing)?,
            AVVideoHeightKey.ok_or_else(missing)?,
            AVVideoCompressionPropertiesKey.ok_or_else(missing)?,
        );
        let fps = spec.fps.max(1);
        let bitrate = number_u32(spec.video_bitrate());
        let gop = number_u32(fps * 2);
        let rate = number_u32(fps);
        let no_reorder = NSNumber::new_bool(false);
        let compression = dict(
            &[
                AVVideoAverageBitRateKey.ok_or_else(missing)?,
                AVVideoMaxKeyFrameIntervalKey.ok_or_else(missing)?,
                AVVideoExpectedSourceFrameRateKey.ok_or_else(missing)?,
                AVVideoAllowFrameReorderingKey.ok_or_else(missing)?,
                AVVideoProfileLevelKey.ok_or_else(missing)?,
            ],
            &[any(&*bitrate), any(&*gop), any(&*rate), any(&*no_reorder), any(AVVideoProfileLevelH264HighAutoLevel.ok_or_else(missing)?)],
        );
        let width = number_u32(spec.w);
        let height = number_u32(spec.h);
        let codec = AVVideoCodecTypeH264.ok_or_else(missing)?;
        // Tag the stream BT.709 (the sRGB primaries): the capture is converted to sRGB (see capture.rs) and edits
        // re-encode decoded RGB. Untagged H.264 is guessed as BT.601 or 709 depending on the player, shifting colours.
        let colour = dict(
            &[
                AVVideoColorPrimariesKey.ok_or_else(missing)?,
                AVVideoTransferFunctionKey.ok_or_else(missing)?,
                AVVideoYCbCrMatrixKey.ok_or_else(missing)?,
            ],
            &[
                any(AVVideoColorPrimaries_ITU_R_709_2.ok_or_else(missing)?),
                any(AVVideoTransferFunction_ITU_R_709_2.ok_or_else(missing)?),
                any(AVVideoYCbCrMatrix_ITU_R_709_2.ok_or_else(missing)?),
            ],
        );
        let mut keys = vec![k_codec, k_w, k_h, k_props, AVVideoColorPropertiesKey.ok_or_else(missing)?];
        let mut values: Vec<&AnyObject> = vec![any(codec), any(&*width), any(&*height), any(&*compression), any(&*colour)];
        // VideoToolbox picks hardware by default; ask for software explicitly when requested.
        let software_spec;
        let enable_hw;
        if spec.encoder == EncoderPref::Software {
            enable_hw = NSNumber::new_bool(false);
            software_spec = dict(&[&*NSString::from_str("EnableHardwareAcceleratedVideoEncoder")], &[any(&*enable_hw)]);
            keys.push(AVVideoEncoderSpecificationKey.ok_or_else(missing)?);
            values.push(any(&*software_spec));
        }
        let settings = dict(&keys, &values);
        let video = AVAssetWriterInput::initWithMediaType_outputSettings(
            AVAssetWriterInput::alloc(),
            AVMediaTypeVideo.ok_or_else(missing)?,
            Some(&settings),
        );
        video.setExpectsMediaDataInRealTime(true);
        if !writer.canAddInput(&video) {
            return Err(av("configure writer", "the video input was rejected"));
        }
        writer.addInput(&video);

        // With the size included, the adaptor's pool hands out frame-sized buffers (see `pixel_buffer`).
        let attrs = dict(
            &[&*NSString::from_str("PixelFormatType"), &*NSString::from_str("Width"), &*NSString::from_str("Height")],
            &[any(&*NSNumber::new_u32(kCVPixelFormatType_32BGRA)), any(&*width), any(&*height)],
        );
        let adaptor =
            AVAssetWriterInputPixelBufferAdaptor::assetWriterInputPixelBufferAdaptorWithAssetWriterInput_sourcePixelBufferAttributes(
                &video,
                Some(&attrs),
            );

        let audio = if spec.audio {
            let format = NSNumber::new_u32(kAudioFormatMPEG4AAC);
            let channels = NSNumber::new_u32(2);
            let sample_rate = NSNumber::new_u32(48_000);
            let bit_rate = NSNumber::new_u32(192_000);
            let settings = dict(
                &[
                    AVFormatIDKey.ok_or_else(missing)?,
                    AVNumberOfChannelsKey.ok_or_else(missing)?,
                    AVSampleRateKey.ok_or_else(missing)?,
                    AVEncoderBitRateKey.ok_or_else(missing)?,
                ],
                &[any(&*format), any(&*channels), any(&*sample_rate), any(&*bit_rate)],
            );
            let input = AVAssetWriterInput::initWithMediaType_outputSettings(
                AVAssetWriterInput::alloc(),
                AVMediaTypeAudio.ok_or_else(missing)?,
                Some(&settings),
            );
            input.setExpectsMediaDataInRealTime(true);
            if !writer.canAddInput(&input) {
                return Err(av("configure writer", "the audio input was rejected"));
            }
            writer.addInput(&input);
            Some(input)
        } else {
            None
        };

        // moov ahead of mdat ("fast start"): browsers and chat apps can play the file while it downloads.
        writer.setShouldOptimizeForNetworkUse(true);
        if !writer.startWriting() {
            return Err(av("start writing", writer.error().map(|e| e.localizedDescription().to_string()).unwrap_or_default()));
        }
        writer.startSessionAtSourceTime(time(0));
        Ok(Box::new(AvSink { writer, video, adaptor, audio, size: (spec.w, spec.h) }))
    }
}

fn missing() -> Error {
    av("AVFoundation", "a required settings key is unavailable on this macOS version")
}

fn remove_existing(path: &Path) {
    let _ = std::fs::remove_file(path);
}

impl AvSink {
    fn failed(&self) -> Option<Error> {
        // SAFETY: reading writer state.
        unsafe {
            (self.writer.status() == AVAssetWriterStatus::Failed)
                .then(|| av("AVAssetWriter", self.writer.error().map(|e| e.localizedDescription().to_string()).unwrap_or_default()))
        }
    }

    /// Wait (briefly) until `input` accepts more data; `false` if it never does.
    fn wait_ready(&self, input: &AVAssetWriterInput, patience: Duration) -> Result<bool> {
        let start = Instant::now();
        // SAFETY: reading input state.
        while !unsafe { input.isReadyForMoreMediaData() } {
            if let Some(e) = self.failed() {
                return Err(e);
            }
            if start.elapsed() > patience {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(true)
    }

    fn pixel_buffer(&self, frame: &Frame) -> Result<CFRetained<CVPixelBuffer>> {
        // SAFETY: the buffer is created, locked, filled row by row within its stride and unlocked.
        unsafe {
            let mut raw: *mut CVPixelBuffer = ptr::null_mut();
            // The adaptor's pool recycles buffers the encoder is done with, rather than allocating a full frame
            // (24 MB at 3024x1964) for every picture. It exists once writing has started.
            let status = match self.adaptor.pixelBufferPool() {
                Some(pool) => CVPixelBufferPool::create_pixel_buffer(None, &pool, NonNull::from(&mut raw)),
                None => CVPixelBufferCreate(
                    None,
                    self.size.0 as usize,
                    self.size.1 as usize,
                    kCVPixelFormatType_32BGRA,
                    None,
                    NonNull::from(&mut raw),
                ),
            };
            let raw = NonNull::new(raw).filter(|_| status == 0).ok_or_else(|| av("CVPixelBuffer", status))?;
            let buffer = CFRetained::from_raw(raw);
            CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags(0));
            let base = CVPixelBufferGetBaseAddress(&buffer) as *mut u8;
            let stride = CVPixelBufferGetBytesPerRow(&buffer);
            if !base.is_null() {
                let row = frame.stride().min(stride);
                for y in 0..self.size.1 as usize {
                    ptr::copy_nonoverlapping(frame.data.as_ptr().add(y * frame.stride()), base.add(y * stride), row);
                }
            }
            CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags(0));
            Ok(buffer)
        }
    }

    fn audio_sample_buffer(samples: &[f32], pts: Hns) -> Result<CFRetained<CMSampleBuffer>> {
        let frames = samples.len() / 2;
        let bytes = samples.len() * 4;
        // SAFETY: Core Media object creation with correctly sized memory; objects are released by CFRetained.
        unsafe {
            let mut asbd = AudioStreamBasicDescription {
                mSampleRate: 48_000.0,
                mFormatID: AUDIO_FORMAT_LINEAR_PCM,
                mFormatFlags: PCM_FLOAT_PACKED,
                mBytesPerPacket: 8,
                mFramesPerPacket: 1,
                mBytesPerFrame: 8,
                mChannelsPerFrame: 2,
                mBitsPerChannel: 32,
                mReserved: 0,
            };
            let mut desc: *const CMAudioFormatDescription = ptr::null();
            let status = CMAudioFormatDescriptionCreate(
                None,
                NonNull::from(&mut asbd),
                0,
                ptr::null(),
                0,
                ptr::null(),
                None,
                NonNull::from(&mut desc),
            );
            let desc = NonNull::new(desc as *mut CMAudioFormatDescription)
                .filter(|_| status == 0)
                .ok_or_else(|| av("CMAudioFormatDescriptionCreate", status))?;
            let desc = CFRetained::from_raw(desc);

            let mut block: *mut CMBlockBuffer = ptr::null_mut();
            let status = CMBlockBuffer::create_with_memory_block(
                None,
                ptr::null_mut(),
                bytes,
                None,
                ptr::null(),
                0,
                bytes,
                objc2_core_media::kCMBlockBufferAssureMemoryNowFlag,
                NonNull::from(&mut block),
            );
            let block = NonNull::new(block).filter(|_| status == 0).ok_or_else(|| av("CMBlockBufferCreate", status))?;
            let block = CFRetained::from_raw(block);
            let status = CMBlockBuffer::replace_data_bytes(NonNull::new_unchecked(samples.as_ptr() as *mut c_void), &block, 0, bytes);
            if status != 0 {
                return Err(av("CMBlockBufferReplaceDataBytes", status));
            }

            let mut sample: *mut CMSampleBuffer = ptr::null_mut();
            let status = CMAudioSampleBufferCreateReadyWithPacketDescriptions(
                None,
                &block,
                &desc,
                frames as isize,
                time(pts),
                ptr::null(),
                NonNull::from(&mut sample),
            );
            let sample = NonNull::new(sample).filter(|_| status == 0).ok_or_else(|| av("CMAudioSampleBufferCreate", status))?;
            Ok(CFRetained::from_raw(sample))
        }
    }
}

impl Sink for AvSink {
    fn video(&mut self, frame: &Arc<Frame>, pts: Hns) -> Result<()> {
        if !self.wait_ready(&self.video, Duration::from_millis(500))? {
            return Ok(()); // encoder saturated: drop this picture, timestamps stay real-time
        }
        let buffer = self.pixel_buffer(frame)?;
        // SAFETY: the input reported readiness and the pixel buffer is valid.
        let ok = unsafe { self.adaptor.appendPixelBuffer_withPresentationTime(&buffer, time(pts)) };
        match (ok, self.failed()) {
            (_, Some(e)) => Err(e),
            _ => Ok(()),
        }
    }

    fn audio(&mut self, chunk: &AudioChunk) -> Result<()> {
        let Some(input) = &self.audio else { return Ok(()) };
        if !self.wait_ready(input, Duration::from_millis(200))? {
            return Ok(());
        }
        let sample = Self::audio_sample_buffer(&chunk.samples, chunk.pts)?;
        // SAFETY: the input reported readiness and the sample buffer is valid.
        let ok = unsafe { input.appendSampleBuffer(&sample) };
        match (ok, self.failed()) {
            (_, Some(e)) => Err(e),
            _ => Ok(()),
        }
    }

    fn finish(self: Box<Self>, end: Hns) -> Result<()> {
        // SAFETY: documented finishing sequence; the completion block only signals a channel.
        unsafe {
            self.video.markAsFinished();
            if let Some(a) = &self.audio {
                a.markAsFinished();
            }
            self.writer.endSessionAtSourceTime(time(end));
            let (tx, rx) = crossbeam_channel::bounded::<()>(1);
            let done = RcBlock::new(move || {
                let _ = tx.send(());
            });
            self.writer.finishWritingWithCompletionHandler(&done);
            // Fast start copies the whole movie behind a new moov here, so finishing takes time in proportion to the
            // file (seconds per GB, far more on slow volumes): wait as long as the writer is still at it.
            loop {
                match rx.recv_timeout(Duration::from_secs(1)) {
                    Ok(()) => break,
                    Err(RecvTimeoutError::Timeout) if self.writer.status() == AVAssetWriterStatus::Writing => {}
                    // The writer left Writing: its handler is about to run, or it failed (reported below).
                    Err(RecvTimeoutError::Timeout) => {
                        let _ = rx.recv_timeout(Duration::from_secs(5));
                        break;
                    }
                    Err(RecvTimeoutError::Disconnected) => return Err(av("finish writing", "completion handler dropped")),
                }
            }
        }
        match self.failed() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn abort(self: Box<Self>) {
        // SAFETY: cancelling an active writer.
        unsafe { self.writer.cancelWriting() };
    }
}
