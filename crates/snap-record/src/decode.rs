//! Native media decoding used by `edit` when ffmpeg is unavailable.

use std::path::Path;

use snap_core::Result;

use crate::{clock::Hns, edit::VideoInfo, frame::Frame};

pub(crate) enum Decoded {
    /// Opaque BGRA picture.
    Video(Frame, Hns),
    /// Interleaved 48 kHz stereo f32.
    #[cfg_attr(target_os = "linux", expect(dead_code, reason = "Linux has no native decoder, only the GIF one"))]
    Audio(Vec<f32>, Hns),
}

pub(crate) trait Decoder {
    fn info(&self) -> VideoInfo;
    /// Position near `ms` (decoders may land on the previous keyframe).
    fn seek(&mut self, ms: u64) -> Result<()>;
    /// Next decoded item in file order, `None` at the end. Audio is only produced when `with_audio` was requested.
    fn next(&mut self) -> Result<Option<Decoded>>;
}

/// Open a media file with the best native decoder for it.
pub(crate) fn open(path: &Path, with_audio: bool) -> Result<Box<dyn Decoder>> {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("gif")) {
        return Ok(Box::new(crate::gifdec::GifDecoder::open(path)?));
    }
    crate::platform::open_decoder(path, with_audio)
}

#[cfg(target_os = "linux")]
pub(crate) fn unsupported_decoder() -> snap_core::Error {
    snap_core::Error::unsupported(snap_core::t("record.error.no_decoder"))
}
