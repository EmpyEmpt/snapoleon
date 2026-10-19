//! The capture "shutter" click, synthesised in memory (no asset to ship).
//!
//! Playback never blocks the caller and fails silently: a missing audio device
//! must not break a screenshot.

use std::sync::OnceLock;

const SAMPLE_RATE: u32 = 44_100;

/// Play a short, soft two-part shutter click.
pub fn shutter() {
    imp::play(wav());
}

/// 16-bit mono WAV of the click.
fn wav() -> &'static [u8] {
    static WAV: OnceLock<Vec<u8>> = OnceLock::new();
    WAV.get_or_init(|| encode_wav(&synthesize()))
}

/// Two quick clicks (shutter opening and closing), each a lowpassed noise burst
/// plus two decaying sine "body" tones.
fn synthesize() -> Vec<i16> {
    let rate = SAMPLE_RATE as f32;
    let total = (0.085 * rate) as usize;
    let mut out = vec![0f32; total];
    let mut noise_state: u32 = 0x1234_5678;
    let mut smooth = 0f32;

    for (start_ms, gain) in [(0.0f32, 1.0f32), (42.0, 0.55)] {
        let start = (start_ms / 1000.0 * rate) as usize;
        for (i, sample) in out.iter_mut().enumerate().skip(start) {
            let t = (i - start) as f32 / rate;
            if t > 0.04 {
                break;
            }
            noise_state = noise_state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let white = (noise_state >> 8) as f32 / (1u32 << 23) as f32 - 1.0;
            smooth += 0.35 * (white - smooth);
            let burst = smooth * (-t / 0.0015).exp() * 0.55;
            let high = (std::f32::consts::TAU * 2400.0 * t).sin() * (-t / 0.004).exp() * 0.35;
            let low = (std::f32::consts::TAU * 900.0 * t).sin() * (-t / 0.008).exp() * 0.25;
            *sample += gain * (burst + high + low);
        }
    }

    let peak = out.iter().fold(0f32, |m, v| m.max(v.abs())).max(f32::EPSILON);
    let fade = (0.004 * rate) as usize;
    let len = out.len();
    out.iter()
        .enumerate()
        .map(|(i, v)| {
            let tail = if i + fade > len { (len - i) as f32 / fade as f32 } else { 1.0 };
            ((v / peak) * 0.5 * tail * f32::from(i16::MAX)) as i16
        })
        .collect()
}

fn encode_wav(samples: &[i16]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        wav.extend_from_slice(&s.to_le_bytes());
    }
    wav
}

/// Write the WAV once into the cache dir for players that need a file.
#[cfg(not(windows))]
fn wav_file() -> Option<std::path::PathBuf> {
    static FILE: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    FILE.get_or_init(|| {
        let dir = snap_core::paths::cache_dir();
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join("shutter.wav");
        std::fs::write(&path, wav()).ok()?;
        Some(path)
    })
    .clone()
}

#[cfg(windows)]
mod imp {
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
    use windows::core::PCWSTR;

    /// `wav` must stay valid while it plays asynchronously; it is a process-lifetime static.
    pub fn play(wav: &'static [u8]) {
        // SAFETY: with SND_MEMORY the "name" argument points at the WAV image.
        unsafe {
            let _ = PlaySoundW(PCWSTR(wav.as_ptr().cast()), None, SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::process::Command;

    pub fn play(_wav: &'static [u8]) {
        if let Some(file) = super::wav_file() {
            let _ = crate::util::spawn_detached(Command::new("afplay").arg(file));
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::process::{Command, Stdio};

    pub fn play(_wav: &'static [u8]) {
        let Some(file) = super::wav_file() else { return };
        // Try the usual players in turn on a helper thread so the caller never waits.
        let _ = std::thread::Builder::new().name("snap-sound".into()).spawn(move || {
            for (player, args) in [("pw-play", &[][..]), ("paplay", &[][..]), ("aplay", &["-q"][..])] {
                let child = Command::new(player).args(args).arg(&file).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
                if let Ok(mut child) = child
                    && child.wait().is_ok_and(|status| status.success())
                {
                    return;
                }
            }
        });
    }
}
