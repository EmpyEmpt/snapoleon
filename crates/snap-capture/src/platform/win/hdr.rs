//! HDR detection and scRGB (FP16, linear, 1.0 = 80 nits) to SDR sRGB tone mapping.

use std::{collections::HashSet, sync::OnceLock};

use snap_core::RgbaImage;
use windows::{
    Win32::Graphics::Dxgi::{Common::DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020, CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput6},
    core::Interface,
};

/// HMONITOR handles whose output currently uses the HDR10 (ST 2084) colour space.
pub fn hdr_monitors() -> HashSet<isize> {
    let mut out = HashSet::new();
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else { return out };
    for a in 0.. {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(a) }) else { break };
        for o in 0.. {
            let Ok(output) = (unsafe { adapter.EnumOutputs(o) }) else { break };
            let Ok(output6) = output.cast::<IDXGIOutput6>() else { continue };
            if let Ok(desc) = unsafe { output6.GetDesc1() }
                && desc.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020
            {
                out.insert(desc.Monitor.0 as isize);
            }
        }
    }
    out
}

/// Linear values up to here pass through unchanged so ordinary SDR content stays bit-exact-ish.
const KNEE: f32 = 0.8;
const SRGB_STEPS: usize = 4096;

/// Smooth shoulder: identity (slope 1) below `KNEE`, asymptotically approaching 1.0 above it.
fn shoulder(m: f32) -> f32 {
    if m <= KNEE {
        return m;
    }
    let range = 1.0 - KNEE;
    KNEE + range * (1.0 - (-(m - KNEE) / range).exp())
}

fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((h >> 10) & 0x1f) as i32;
    let frac = (h & 0x3ff) as f32;
    match exp {
        0 => sign * frac * 2f32.powi(-24),
        31 => {
            if frac == 0.0 {
                sign * f32::INFINITY
            } else {
                f32::NAN
            }
        }
        _ => sign * (1.0 + frac / 1024.0) * 2f32.powi(exp - 15),
    }
}

struct Luts {
    half: Vec<f32>,
    srgb: Vec<u8>,
}

fn luts() -> &'static Luts {
    static LUTS: OnceLock<Luts> = OnceLock::new();
    LUTS.get_or_init(|| {
        let half = (0..=u16::MAX).map(half_to_f32).collect();
        let srgb = (0..=SRGB_STEPS)
            .map(|i| {
                let l = i as f32 / SRGB_STEPS as f32;
                let v = if l <= 0.003_130_8 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 };
                (v * 255.0 + 0.5) as u8
            })
            .collect();
        Luts { half, srgb }
    })
}

/// Convert tightly packed premultiplied RGBA16F pixels (`width * height * 8` bytes) to a straight-alpha
/// SDR image (`opaque` forces alpha to 255). `sdr_white_nits` is the display's SDR content brightness:
/// that level maps to white.
pub fn tone_map(fp16: &[u8], width: u32, height: u32, sdr_white_nits: f32, opaque: bool) -> RgbaImage {
    let luts = luts();
    let scale = 80.0 / sdr_white_nits.max(1.0);
    let (w, h) = (width as usize, height as usize);
    let mut out = vec![0u8; w * h * 4];

    let workers = rayon::current_num_threads().min(8);
    let rows_per = h.div_ceil(workers).max(1);
    rayon::scope(|s| {
        for (chunk_i, dst) in out.chunks_mut(rows_per * w * 4).enumerate() {
            let src = &fp16[chunk_i * rows_per * w * 8..];
            s.spawn(move |_| {
                for (px, dst) in src.as_chunks::<8>().0.iter().zip(dst.as_chunks_mut::<4>().0) {
                    let half = |i: usize| luts.half[u16::from_le_bytes([px[2 * i], px[2 * i + 1]]) as usize];
                    let alpha = if opaque { 1.0 } else { half(3).clamp(0.0, 1.0) };
                    let unpremultiply = if alpha > 0.0 && alpha < 1.0 { scale / alpha } else { scale };
                    let c = [half(0) * unpremultiply, half(1) * unpremultiply, half(2) * unpremultiply];
                    let m = c[0].max(c[1]).max(c[2]);
                    let k = if m > KNEE { shoulder(m) / m } else { 1.0 };
                    for i in 0..3 {
                        let lin = (c[i] * k).clamp(0.0, 1.0);
                        dst[i] = luts.srgb[(lin * SRGB_STEPS as f32 + 0.5) as usize];
                    }
                    dst[3] = (alpha * 255.0 + 0.5) as u8;
                }
            });
        }
    });
    RgbaImage::from_raw(width, height, out).expect("buffer size matches dimensions")
}
