//! ScreenCaptureKit source: video frames and system audio.

use std::{sync::Arc, time::Duration};

use anyhow::anyhow;
use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send, rc::Retained, runtime::ProtocolObject};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayBounds, CGDisplayCopyDisplayMode, CGDisplayMode, CGMainDisplayID, CGPreflightScreenCaptureAccess,
    CGRequestScreenCaptureAccess,
};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::{
    CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress, kCVPixelFormatType_32BGRA,
};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamOutput, SCStreamOutputType, SCWindow,
};
use parking_lot::Mutex;
use snap_core::{Error, Point, Rect, Result, t, tf};

use crate::{
    RecordTarget,
    audio::{Kind, Resampler, Ring, Source},
    frame::{Frame, even},
    platform::{FrameSlot, FrameSource, OriginFn, SourceSpec},
};

/// `SCStreamErrorUserDeclined`: the user has not granted Screen Recording permission.
const USER_DECLINED: isize = -3801;

struct AudioFeed {
    ring: Ring,
    resampler: Mutex<Option<Resampler>>,
}

struct OutputState {
    slot: Arc<FrameSlot>,
    size: (u32, u32),
    audio: Option<AudioFeed>,
    first: Mutex<Option<crossbeam_channel::Sender<()>>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and this class does not implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "SnapRecordStreamOutput"]
    #[ivars = OutputState]
    struct StreamOutput;

    // SAFETY: NSObjectProtocol has no safety requirements.
    unsafe impl NSObjectProtocol for StreamOutput {}

    // SAFETY: the method signature matches the SCStreamOutput protocol.
    unsafe impl SCStreamOutput for StreamOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn did_output(&self, _stream: &SCStream, buffer: &CMSampleBuffer, kind: SCStreamOutputType) {
            if kind == SCStreamOutputType::Screen {
                self.video(buffer);
            } else if kind == SCStreamOutputType::Audio {
                self.audio(buffer);
            }
        }
    }
);

impl StreamOutput {
    fn new(state: OutputState) -> Retained<Self> {
        let this = Self::alloc().set_ivars(state);
        // SAFETY: NSObject's init has the expected signature.
        unsafe { msg_send![super(this), init] }
    }

    fn video(&self, buffer: &CMSampleBuffer) {
        let st = self.ivars();
        // SAFETY: the sample buffer is valid for the duration of the callback; the pixel buffer is
        // locked while its rows are copied.
        unsafe {
            // Idle/blank frames carry no picture.
            let Some(pixels) = buffer.image_buffer() else { return };
            if CVPixelBufferLockBaseAddress(&pixels, CVPixelBufferLockFlags::ReadOnly) != 0 {
                return;
            }
            let (w, h, stride) = (CVPixelBufferGetWidth(&pixels), CVPixelBufferGetHeight(&pixels), CVPixelBufferGetBytesPerRow(&pixels));
            let base = CVPixelBufferGetBaseAddress(&pixels) as *const u8;
            if !base.is_null() {
                let src = std::slice::from_raw_parts(base, stride * h);
                let mut frame = Frame::black(st.size.0, st.size.1);
                frame.blit(src, stride, w as u32, h as u32, 0, 0);
                st.slot.set(frame);
                if let Some(tx) = st.first.lock().take() {
                    let _ = tx.send(());
                }
            }
            CVPixelBufferUnlockBaseAddress(&pixels, CVPixelBufferLockFlags::ReadOnly);
        }
    }

    fn audio(&self, buffer: &CMSampleBuffer) {
        let Some(feed) = &self.ivars().audio else { return };
        let Some((samples, rate, channels)) = super::pcm_f32(buffer) else { return };
        feed.resampler.lock().get_or_insert_with(|| Resampler::new(rate, channels)).process(&samples, &feed.ring);
    }
}

struct MacSource {
    stream: Retained<SCStream>,
    _output: Retained<StreamOutput>,
    _queue: DispatchRetained<DispatchQueue>,
    size: (u32, u32),
    origin: Point,
    scale: f32,
    audio: Option<Ring>,
}

// SAFETY: the retained Objective-C objects are only used through thread-safe SCStream calls
// (start/stop) and are never touched from the capture queue after being handed over.
unsafe impl Send for MacSource {}

impl FrameSource for MacSource {
    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn origin_fn(&self) -> OriginFn {
        let o = self.origin;
        Arc::new(move || o)
    }

    fn input_scale(&self) -> f32 {
        self.scale
    }

    fn audio_source(&mut self, kind: Kind, _device: Option<&str>, _stop: Arc<std::sync::atomic::AtomicBool>) -> Option<Result<Source>> {
        match kind {
            Kind::System => self.audio.take().map(|ring| Ok(Source { kind, ring, thread: None })),
            Kind::Mic => None,
        }
    }

    fn stop(self: Box<Self>) {
        let (tx, rx) = crossbeam_channel::bounded::<()>(1);
        let done = RcBlock::new(move |_err: *mut NSError| {
            let _ = tx.send(());
        });
        // SAFETY: valid stream; the completion block signals the channel.
        unsafe { self.stream.stopCaptureWithCompletionHandler(Some(&done)) };
        let _ = rx.recv_timeout(Duration::from_secs(3));
    }
}

fn ns_error(e: *mut NSError) -> Option<Error> {
    // SAFETY: SCK passes either null or a valid NSError.
    let e = unsafe { e.as_ref() }?;
    if e.code() == USER_DECLINED {
        return Some(Error::permission(t("record.error.screen_permission")));
    }
    Some(Error::Other(anyhow!("ScreenCaptureKit: {}", e.localizedDescription())))
}

fn shareable_content() -> Result<Retained<SCShareableContent>> {
    if !CGPreflightScreenCaptureAccess() {
        CGRequestScreenCaptureAccess();
    }
    let (tx, rx) = crossbeam_channel::bounded::<Result<Retained<SCShareableContent>>>(1);
    let block = RcBlock::new(move |content: *mut SCShareableContent, err: *mut NSError| {
        let result = match ns_error(err) {
            Some(e) => Err(e),
            // SAFETY: on success SCK passes a valid SCShareableContent.
            None => unsafe { Retained::retain(content) }.ok_or_else(|| Error::Other(anyhow!("ScreenCaptureKit returned no content"))),
        };
        let _ = tx.send(result);
    });
    // SAFETY: the block outlives the call and only uses thread-safe channel operations.
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&block) };
    rx.recv_timeout(Duration::from_secs(10)).map_err(|_| Error::Other(anyhow!("ScreenCaptureKit did not answer")))?
}

/// Pixels per point of a display.
fn display_scale(id: CGDirectDisplayID) -> f64 {
    let bounds = CGDisplayBounds(id);
    let mode = CGDisplayCopyDisplayMode(id);
    let px = CGDisplayMode::pixel_width(mode.as_deref());
    if bounds.size.width > 0.0 && px > 0 { px as f64 / bounds.size.width } else { 1.0 }
}

pub(crate) fn open_source(spec: &SourceSpec, slot: Arc<FrameSlot>) -> Result<Box<dyn FrameSource>> {
    let content = shareable_content()?;
    // SAFETY: reading immutable properties of SCK objects.
    let (displays, windows) = unsafe { (content.displays(), content.windows()) };

    // Main display first, then SCK order: a stable index for `RecordTarget::Monitor`.
    let main = CGMainDisplayID();
    let mut ordered: Vec<Retained<SCDisplay>> = displays.iter().collect();
    // SAFETY: reading the display id.
    ordered.sort_by_key(|d| unsafe { d.displayID() } != main);

    let excluded: Vec<Retained<SCWindow>> =
        windows.iter().filter(|w| spec.exclude_windows.contains(&(unsafe { w.windowID() } as u64))).collect();
    let excluded_refs: Vec<&SCWindow> = excluded.iter().map(|w| &**w).collect();
    let exclude = NSArray::from_slice(&excluded_refs);

    // (filter, source rect in points relative to the content, pixel size, origin in px, scale)
    let (filter, source_rect, size_px, origin_px, scale);
    match spec.target {
        RecordTarget::Monitor(i) => {
            let display =
                ordered.get(*i).ok_or_else(|| Error::NotFound(tf("record.error.monitor_not_found", &[("index", &i.to_string())])))?;
            // SAFETY: display properties and filter construction.
            unsafe {
                let id = display.displayID();
                scale = display_scale(id);
                let f = display.frame();
                filter = SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), display, &exclude);
                source_rect = None;
                size_px = (even((f.size.width * scale) as u32), even((f.size.height * scale) as u32));
                origin_px = Point::new((f.origin.x * scale) as i32, (f.origin.y * scale) as i32);
            }
        }
        RecordTarget::Region(r) => {
            if r.is_empty() {
                return Err(Error::invalid(t("record.error.region_empty")));
            }
            // Rectangles arrive in pixels; find the display under the region's centre.
            let centre = (r.x as f64 + r.w as f64 / 2.0, r.y as f64 + r.h as f64 / 2.0);
            let hit = ordered.iter().find_map(|d| {
                // SAFETY: display properties.
                let (id, f) = unsafe { (d.displayID(), d.frame()) };
                let s = display_scale(id);
                let (px, py) = (f.origin.x * s, f.origin.y * s);
                (centre.0 >= px && centre.0 < px + f.size.width * s && centre.1 >= py && centre.1 < py + f.size.height * s)
                    .then(|| (d.clone(), s, f))
            });
            let (display, s, f) = hit.ok_or_else(|| Error::invalid(t("record.error.region_outside")))?;
            scale = s;
            let rect_pts = Rect::new(r.x, r.y, r.w, r.h);
            let local = CGRect::new(
                CGPoint::new(rect_pts.x as f64 / s - f.origin.x, rect_pts.y as f64 / s - f.origin.y),
                CGSize::new(rect_pts.w as f64 / s, rect_pts.h as f64 / s),
            );
            // SAFETY: filter construction.
            filter = unsafe { SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), &display, &exclude) };
            source_rect = Some(local);
            size_px = (even(r.w), even(r.h));
            origin_px = r.origin();
        }
        RecordTarget::Window(id) => {
            let window = windows
                .iter()
                // SAFETY: reading the window id.
                .find(|w| unsafe { w.windowID() } as u64 == *id)
                .ok_or_else(|| Error::NotFound(t("record.error.window_not_found").into_owned()))?;
            // SAFETY: window properties and filter construction.
            unsafe {
                let f = window.frame();
                // The display containing the window's centre supplies the scale.
                let centre = (f.origin.x + f.size.width / 2.0, f.origin.y + f.size.height / 2.0);
                scale = ordered
                    .iter()
                    .find_map(|d| {
                        let df = d.frame();
                        (centre.0 >= df.origin.x
                            && centre.0 < df.origin.x + df.size.width
                            && centre.1 >= df.origin.y
                            && centre.1 < df.origin.y + df.size.height)
                            .then(|| display_scale(d.displayID()))
                    })
                    .unwrap_or(1.0);
                filter = SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &window);
                source_rect = None;
                size_px = (even((f.size.width * scale) as u32), even((f.size.height * scale) as u32));
                origin_px = Point::new((f.origin.x * scale) as i32, (f.origin.y * scale) as i32);
            }
        }
    }

    // SAFETY: configuration setters on a freshly created object.
    let config = unsafe {
        let c = SCStreamConfiguration::new();
        c.setWidth(size_px.0 as usize);
        c.setHeight(size_px.1 as usize);
        c.setPixelFormat(kCVPixelFormatType_32BGRA);
        c.setMinimumFrameInterval(CMTime::new(1, spec.fps as i32));
        c.setShowsCursor(spec.show_cursor);
        c.setQueueDepth(6);
        if let Some(r) = source_rect {
            c.setSourceRect(r);
        }
        if spec.system_audio {
            c.setCapturesAudio(true);
            c.setSampleRate(48_000);
            c.setChannelCount(2);
            c.setExcludesCurrentProcessAudio(true);
        }
        c
    };

    let audio_ring = spec.system_audio.then(Ring::default);
    let (first_tx, first_rx) = crossbeam_channel::bounded::<()>(1);
    let output = StreamOutput::new(OutputState {
        slot,
        size: size_px,
        audio: audio_ring.clone().map(|ring| AudioFeed { ring, resampler: Mutex::new(None) }),
        first: Mutex::new(Some(first_tx)),
    });
    let queue = DispatchQueue::new("app.snapoleon.record.capture", None);

    // SAFETY: stream construction, output registration and start follow the documented sequence.
    let stream = unsafe {
        let stream = SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &config, None);
        let proto = ProtocolObject::from_ref(&*output);
        stream
            .addStreamOutput_type_sampleHandlerQueue_error(proto, SCStreamOutputType::Screen, Some(&queue))
            .map_err(|e| Error::Other(anyhow!("add video output: {}", e.localizedDescription())))?;
        if spec.system_audio {
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(proto, SCStreamOutputType::Audio, Some(&queue))
                .map_err(|e| Error::Other(anyhow!("add audio output: {}", e.localizedDescription())))?;
        }
        stream
    };
    let (start_tx, start_rx) = crossbeam_channel::bounded::<Option<Error>>(1);
    let started = RcBlock::new(move |err: *mut NSError| {
        let _ = start_tx.send(ns_error(err));
    });
    // SAFETY: valid stream; the block only signals a channel.
    unsafe { stream.startCaptureWithCompletionHandler(Some(&started)) };
    match start_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(None) => {}
        Ok(Some(e)) => return Err(e),
        Err(_) => return Err(Error::Other(anyhow!("ScreenCaptureKit did not start"))),
    }
    if first_rx.recv_timeout(Duration::from_secs(5)).is_err() {
        // SAFETY: stop the stream we just started.
        unsafe { stream.stopCaptureWithCompletionHandler(None) };
        return Err(Error::Other(anyhow!(t("record.error.no_frames").into_owned())));
    }

    Ok(Box::new(MacSource {
        stream,
        _output: output,
        _queue: queue,
        size: size_px,
        origin: origin_px,
        scale: scale as f32,
        audio: audio_ring,
    }))
}
