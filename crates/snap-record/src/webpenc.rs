//! Animated WebP encoding on libwebp's `WebPAnimEncoder`.
//!
//! Uses `libwebp-sys` directly (the same libwebp instance the `webp` image crate
//! links) instead of a second wrapper crate that would bundle its own copy.

use std::mem::MaybeUninit;

use libwebp_sys as w;
use snap_core::{Error, Result};

use crate::clock::{HNS_PER_SEC, Hns};

pub(crate) struct WebpEncoder {
    enc: *mut w::WebPAnimEncoder,
    pic: w::WebPPicture,
    config: w::WebPConfig,
    width: u32,
    height: u32,
    last_ms: i32,
    frames: u32,
    prev: Vec<u8>,
}

fn err(what: &str) -> Error {
    Error::Other(anyhow::anyhow!("webp: {what}"))
}

impl WebpEncoder {
    /// `quality` 0..=100 (lossy).
    pub fn new(width: u32, height: u32, quality: f32) -> Result<Self> {
        let mut opts = MaybeUninit::<w::WebPAnimEncoderOptions>::zeroed();
        // SAFETY: the options struct is plain C data initialised by libwebp.
        let enc = unsafe {
            if w::WebPAnimEncoderOptionsInitInternal(opts.as_mut_ptr(), w::WEBP_MUX_ABI_VERSION as _) == 0 {
                return Err(err("cannot initialise animation options"));
            }
            let mut opts = opts.assume_init();
            opts.anim_params.loop_count = 0;
            w::WebPAnimEncoderNewInternal(width as _, height as _, &opts, w::WEBP_MUX_ABI_VERSION as _)
        };
        if enc.is_null() {
            return Err(err("cannot create the animation encoder"));
        }
        let mut pic = w::WebPPicture::new().map_err(|()| err("cannot initialise a picture"))?;
        pic.use_argb = 1;
        pic.width = width as _;
        pic.height = height as _;
        let mut config =
            w::WebPConfig::new_with_preset(w::WebPPreset::WEBP_PRESET_DRAWING, quality).map_err(|()| err("cannot initialise config"))?;
        config.method = 2;
        config.thread_level = 1;
        // SAFETY: config was initialised above.
        if unsafe { w::WebPValidateConfig(&config) } == 0 {
            return Err(err("invalid encoder configuration"));
        }
        Ok(Self { enc, pic, config, width, height, last_ms: -1, frames: 0, prev: Vec::new() })
    }

    /// Add a BGRA frame (`width*height*4`) shown from `pts`. Identical consecutive frames are merged.
    pub fn push(&mut self, bgra: &[u8], pts: Hns) -> Result<()> {
        if self.prev == bgra {
            return Ok(());
        }
        let ms = ((pts * 1000 / HNS_PER_SEC) as i32).max(self.last_ms + 1);
        // SAFETY: `bgra` holds width*height BGRA pixels; the picture imports (copies) it.
        unsafe {
            if w::WebPPictureImportBGRA(&mut self.pic, bgra.as_ptr(), self.width as i32 * 4) == 0 {
                return Err(err("cannot import frame"));
            }
            if w::WebPAnimEncoderAdd(self.enc, &mut self.pic, ms, &self.config) == 0 {
                return Err(err("cannot add frame"));
            }
        }
        self.prev.clear();
        self.prev.extend_from_slice(bgra);
        self.last_ms = ms;
        self.frames += 1;
        Ok(())
    }

    /// Assemble the animation; the last frame ends at `end`.
    pub fn finish(self, end: Hns) -> Result<Vec<u8>> {
        if self.frames == 0 {
            return Err(err("no frames were recorded"));
        }
        let end_ms = ((end * 1000 / HNS_PER_SEC) as i32).max(self.last_ms + 50);
        let mut data = w::WebPData::default();
        // SAFETY: flushing with a null frame then assembling is the documented sequence.
        unsafe {
            if w::WebPAnimEncoderAdd(self.enc, std::ptr::null_mut(), end_ms, std::ptr::null()) == 0 {
                return Err(err("cannot finalise animation"));
            }
            if w::WebPAnimEncoderAssemble(self.enc, &mut data) == 0 {
                return Err(err("cannot assemble animation"));
            }
            let out = std::slice::from_raw_parts(data.bytes, data.size).to_vec();
            w::WebPDataClear(&mut data);
            Ok(out)
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

impl Drop for WebpEncoder {
    fn drop(&mut self) {
        // SAFETY: both were created by libwebp and are released exactly once.
        unsafe {
            w::WebPPictureFree(&mut self.pic);
            w::WebPAnimEncoderDelete(self.enc);
        }
    }
}
