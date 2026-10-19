//! Step-capture video: stills held for their durations with short cross-fades.

use std::{path::Path, sync::Arc};

use image::{RgbaImage, imageops::FilterType};
use snap_core::{Error, Result, settings::{EncoderPref, VideoFormat}};

use crate::{
    clock::{HNS_PER_SEC, Hns},
    frame::{Frame, even},
    sink::{self, SinkSpec},
};

const CROSSFADE_MS: u64 = 300;
const BACKGROUND: [u8; 3] = [16, 16, 20];
const MAX_SIDE: u32 = 3840;

/// Render `slides` (picture, hold time in ms) into an H.264 MP4. All pictures are
/// letterboxed onto one even-sized canvas; each slide fades into the next during
/// the last 300 ms of its hold time.
pub fn render(slides: &[(RgbaImage, u64)], output: &Path, fps: u32) -> Result<()> {
    if slides.is_empty() {
        return Err(Error::invalid(snap_core::t("record.error.slideshow_empty")));
    }
    let fps = fps.clamp(1, 60);
    let (w, h) = canvas_size(slides);
    if let Some(dir) = output.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }

    let frames: Vec<Arc<Frame>> = slides.iter().map(|(img, _)| Arc::new(letterbox(img, w, h))).collect();
    let spec = SinkSpec {
        format: VideoFormat::Mp4,
        w,
        h,
        fps,
        bitrate: 0,
        audio: false,
        encoder: EncoderPref::Auto,
        output: output.to_path_buf(),
        ffmpeg: None,
        max_gif_width: w,
        dither: false,
    };
    let mut sink = sink::build(&spec)?;
    let result = (|| {
        let period = HNS_PER_SEC / fps as i64;
        let mut t_ms = 0u64;
        let mut index = 0u64;
        for (i, (_, hold)) in slides.iter().enumerate() {
            let hold = (*hold).max(CROSSFADE_MS);
            let fade_from = if i + 1 < slides.len() { hold - CROSSFADE_MS } else { hold };
            let frame_count = hold * fps as u64 / 1000;
            for k in 0..frame_count.max(1) {
                let local_ms = k * 1000 / fps as u64;
                let pts: Hns = index as i64 * period;
                let picture = if local_ms >= fade_from && i + 1 < slides.len() {
                    let mix = ((local_ms - fade_from) as f32 / CROSSFADE_MS as f32).clamp(0.0, 1.0);
                    Arc::new(crossfade(&frames[i], &frames[i + 1], mix))
                } else {
                    frames[i].clone()
                };
                sink.video(&picture, pts)?;
                index += 1;
            }
            t_ms += hold;
        }
        Ok(t_ms as Hns * (HNS_PER_SEC / 1000))
    })();
    match result {
        Ok(end) => sink.finish(end),
        Err(e) => {
            sink.abort();
            let _ = std::fs::remove_file(output);
            Err(e)
        }
    }
}

fn canvas_size(slides: &[(RgbaImage, u64)]) -> (u32, u32) {
    let w = slides.iter().map(|(i, _)| i.width()).max().unwrap_or(2).min(MAX_SIDE);
    let h = slides.iter().map(|(i, _)| i.height()).max().unwrap_or(2).min(MAX_SIDE);
    (even(w), even(h))
}

/// Fit `img` inside `w`x`h`, centred on the background colour.
fn letterbox(img: &RgbaImage, w: u32, h: u32) -> Frame {
    let scale = (w as f64 / img.width() as f64).min(h as f64 / img.height() as f64).min(1.0);
    let (iw, ih) = (((img.width() as f64 * scale).round() as u32).max(1), ((img.height() as f64 * scale).round() as u32).max(1));
    let fitted = if (iw, ih) == img.dimensions() { img.clone() } else { image::imageops::resize(img, iw, ih, FilterType::Lanczos3) };
    let mut canvas = Frame::black(w, h);
    for px in canvas.data.as_chunks_mut::<4>().0 {
        px[0] = BACKGROUND[2];
        px[1] = BACKGROUND[1];
        px[2] = BACKGROUND[0];
    }
    let (ox, oy) = ((w - iw) as i32 / 2, (h - ih) as i32 / 2);
    // Composite over the background so transparent screenshots stay readable; the result is opaque.
    let stride = canvas.stride();
    for y in 0..ih as usize {
        for x in 0..iw as usize {
            let p = fitted.get_pixel(x as u32, y as u32).0;
            let a = p[3] as u32;
            let o = (oy as usize + y) * stride + (ox as usize + x) * 4;
            let dst = &mut canvas.data[o..o + 4];
            dst[0] = ((p[2] as u32 * a + dst[0] as u32 * (255 - a)) / 255) as u8;
            dst[1] = ((p[1] as u32 * a + dst[1] as u32 * (255 - a)) / 255) as u8;
            dst[2] = ((p[0] as u32 * a + dst[2] as u32 * (255 - a)) / 255) as u8;
        }
    }
    canvas
}

fn crossfade(a: &Frame, b: &Frame, mix: f32) -> Frame {
    let m = (mix * 256.0) as u32;
    let data = a.data.iter().zip(&b.data).map(|(&x, &y)| ((x as u32 * (256 - m) + y as u32 * m) >> 8) as u8).collect();
    Frame { w: a.w, h: a.h, data }
}
