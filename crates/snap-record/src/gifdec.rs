//! Animated GIF input for `edit` (probe, seek, frame iteration) via the `image` crate.

use std::{fs::File, io::BufReader, path::Path};

use image::{AnimationDecoder, Frames, codecs::gif::GifDecoder as ImageGif};
use snap_core::{Error, Result};

use crate::{
    clock::{HNS_PER_SEC, Hns},
    decode::{Decoded, Decoder},
    edit::VideoInfo,
    frame::Frame,
};

pub(crate) struct GifDecoder {
    path: std::path::PathBuf,
    info: VideoInfo,
    frames: Frames<'static>,
    clock: Hns,
    carry: Option<(Frame, Hns)>,
}

fn frames_of(path: &Path) -> Result<(Frames<'static>, u32, u32)> {
    let dec = ImageGif::new(BufReader::new(File::open(path)?))?;
    let (w, h) = image::ImageDecoder::dimensions(&dec);
    Ok((dec.into_frames(), w, h))
}

impl GifDecoder {
    pub fn open(path: &Path) -> Result<Self> {
        let (frames, width, height) = frames_of(path)?;
        let (mut total_ms, mut count) = (0f64, 0u32);
        for f in frames {
            let f = f.map_err(Error::Image)?;
            let (n, d) = f.delay().numer_denom_ms();
            total_ms += n as f64 / d.max(1) as f64;
            count += 1;
        }
        if count == 0 {
            return Err(Error::invalid(format!("{} has no frames", path.display())));
        }
        let fps = count as f32 / (total_ms / 1000.0).max(0.001) as f32;
        let (frames, ..) = frames_of(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            info: VideoInfo { duration_ms: total_ms.round() as u64, width, height, fps, has_audio: false },
            frames,
            clock: 0,
            carry: None,
        })
    }
}

impl GifDecoder {
    /// Next picture with its start time and duration.
    fn read_one(&mut self) -> Result<Option<(Frame, Hns, Hns)>> {
        let Some(f) = self.frames.next() else { return Ok(None) };
        let f = f.map_err(Error::Image)?;
        let (n, d) = f.delay().numer_denom_ms();
        let dur = (n as i64 * (HNS_PER_SEC / 1000)) / d.max(1) as i64;
        let pts = self.clock;
        self.clock += dur;
        Ok(Some((Frame::from_rgba_image(f.buffer()), pts, dur)))
    }
}

impl Decoder for GifDecoder {
    fn info(&self) -> VideoInfo {
        self.info.clone()
    }

    /// GIF has no index: restart and skip pictures that end before the target.
    fn seek(&mut self, ms: u64) -> Result<()> {
        let target = ms as Hns * (HNS_PER_SEC / 1000);
        self.frames = frames_of(&self.path)?.0;
        self.clock = 0;
        self.carry = None;
        while let Some((frame, pts, dur)) = self.read_one()? {
            if pts + dur > target {
                self.carry = Some((frame, pts));
                break;
            }
        }
        Ok(())
    }

    fn next(&mut self) -> Result<Option<Decoded>> {
        if let Some((frame, pts)) = self.carry.take() {
            return Ok(Some(Decoded::Video(frame, pts)));
        }
        Ok(self.read_one()?.map(|(frame, pts, _)| Decoded::Video(frame, pts)))
    }
}
