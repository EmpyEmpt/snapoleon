//! macOS backend: ScreenCaptureKit capture, AVAssetWriter (VideoToolbox) encoding and
//! AVAssetReader decoding.

use std::{
    ptr::{self, NonNull},
    time::{Duration, Instant},
};

use objc2_app_kit::NSEvent;
use objc2_core_audio_types::{AudioBufferList, AudioStreamBasicDescription};
use objc2_core_foundation::CFRetained;
use objc2_core_media::{CMAudioFormatDescriptionGetStreamBasicDescription, CMBlockBuffer, CMSampleBuffer};
use snap_core::Point;

mod capture;
mod input;
mod reader;
mod writer;

pub(crate) use capture::open_source;
pub(crate) use input::{InputTaps, listen_input};
pub(crate) use reader::open_decoder;
pub(crate) use writer::{native_encoders, native_mp4_sink};

pub(crate) fn mic_devices() -> Option<Vec<String>> {
    None
}

/// Pointer position in the global top-left-origin point space that pointer events use.
pub(crate) fn cursor_position() -> Option<Point> {
    let p = NSEvent::mouseLocation();
    // Cocoa's origin is bottom-left of the main display; events use top-left.
    let main_height = objc2_core_graphics::CGDisplayBounds(objc2_core_graphics::CGMainDisplayID()).size.height;
    Some(Point::new(p.x.round() as i32, (main_height - p.y).round() as i32))
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

/// Interleaved f32 samples of an audio sample buffer, with its sample rate and channel count.
/// Handles both planar (one buffer per channel) and interleaved layouts.
pub(super) fn pcm_f32(buffer: &CMSampleBuffer) -> Option<(Vec<f32>, u32, usize)> {
    // SAFETY: the buffer list is read into properly sized, aligned storage and the retained block
    // buffer is released afterwards; sample data is only read within the reported sizes.
    unsafe {
        let desc = buffer.format_description()?;
        let asbd: *const AudioStreamBasicDescription = CMAudioFormatDescriptionGetStreamBasicDescription(&desc);
        let asbd = asbd.as_ref()?;

        // Room for up to 8 buffers (planar layouts carry one per channel). Core Media wants the exact
        // size of the list, not just enough room: AVAssetReader buffers reject a larger one with
        // kCMSampleBufferError_ArrayTooSmall, so ask for the size first.
        let mut storage = [0u64; 64];
        let list = storage.as_mut_ptr().cast::<AudioBufferList>();
        let mut needed = 0usize;
        let status = buffer.audio_buffer_list_with_retained_block_buffer(&mut needed, ptr::null_mut(), 0, None, None, 0, ptr::null_mut());
        if status != 0 || needed == 0 || needed > size_of_val(&storage) {
            return None;
        }
        let mut block: *mut CMBlockBuffer = ptr::null_mut();
        let status = buffer.audio_buffer_list_with_retained_block_buffer(ptr::null_mut(), list, needed, None, None, 0, &mut block);
        let _block = NonNull::new(block).map(|b| CFRetained::from_raw(b));
        if status != 0 {
            return None;
        }
        let count = (*list).mNumberBuffers as usize;
        let buffers = std::slice::from_raw_parts((*list).mBuffers.as_ptr(), count.min(8));
        let planar = buffers.len() > 1;
        let channels = if planar { buffers.len() } else { asbd.mChannelsPerFrame as usize }.max(1);
        let first = buffers.first()?;
        let frames = first.mDataByteSize as usize / 4 / if planar { 1 } else { channels };
        let mut out = Vec::with_capacity(frames * channels);
        if planar {
            let planes: Vec<&[f32]> = buffers.iter().map(|b| std::slice::from_raw_parts(b.mData as *const f32, frames)).collect();
            for i in 0..frames {
                out.extend(planes.iter().map(|p| p[i]));
            }
        } else {
            out.extend_from_slice(std::slice::from_raw_parts(first.mData as *const f32, frames * channels));
        }
        Some((out, asbd.mSampleRate as u32, channels))
    }
}
