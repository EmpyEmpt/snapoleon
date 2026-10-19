//! Locating and driving an external `ffmpeg` binary.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::OnceLock,
};

use parking_lot::Mutex;
use snap_core::{Error, Result};

struct Config {
    path: Option<PathBuf>,
    enabled: bool,
}

static CONFIG: Mutex<Config> = Mutex::new(Config { path: None, enabled: true });

/// Set the ffmpeg binary configured by the user (`recording.ffmpeg_path`).
/// `None` or an empty path falls back to "next to the executable, then PATH".
pub fn set_ffmpeg_path(path: Option<PathBuf>) {
    CONFIG.lock().path = path.filter(|p| !p.as_os_str().is_empty());
}

/// Globally allow or forbid ffmpeg. With `false` only native backends are used.
pub fn set_ffmpeg_enabled(enabled: bool) {
    CONFIG.lock().enabled = enabled;
}

/// Resolve an ffmpeg binary: `explicit` (per recording), the configured path,
/// next to the running executable, then PATH. `None` when unavailable or disabled.
pub fn ffmpeg_path() -> Option<PathBuf> {
    locate(None)
}

pub(crate) fn locate(explicit: Option<&Path>) -> Option<PathBuf> {
    let cfg = CONFIG.lock();
    if !cfg.enabled {
        return None;
    }
    let configured = cfg.path.clone();
    drop(cfg);

    let name = exe_name("ffmpeg");
    let candidates = explicit
        .map(Path::to_path_buf)
        .into_iter()
        .chain(configured)
        .chain(std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join(&name))))
        .chain(std::env::var_os("PATH").into_iter().flat_map(|p| std::env::split_paths(&p).map(|d| d.join(&name)).collect::<Vec<_>>()));
    candidates.into_iter().find(|p| p.is_file())
}

fn exe_name(base: &str) -> String {
    if cfg!(windows) { format!("{base}.exe") } else { base.to_string() }
}

pub(crate) fn require(explicit: Option<&Path>) -> Result<PathBuf> {
    locate(explicit).ok_or_else(|| Error::unsupported(snap_core::t("record.error.ffmpeg_missing")))
}

pub(crate) fn command(ffmpeg: &Path) -> Command {
    let mut c = Command::new(ffmpeg);
    c.args(["-hide_banner", "-nostdin", "-loglevel", "error"]);
    no_window(&mut c);
    c
}

#[cfg(windows)]
pub(crate) fn no_window(c: &mut Command) {
    use std::os::windows::process::CommandExt;
    c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
}

#[cfg(not(windows))]
pub(crate) fn no_window(_: &mut Command) {}

/// Run ffmpeg to completion with captured output; error text is the tail of stderr.
pub(crate) fn run(ffmpeg: &Path, args: &[&std::ffi::OsStr]) -> Result<()> {
    let out = command(ffmpeg).args(args).stdin(Stdio::null()).output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(Error::Other(anyhow::anyhow!("ffmpeg failed: {}", tail(&out.stderr))))
    }
}

pub(crate) fn tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(4)..].join(" | ")
}

/// Output of `ffmpeg -hide_banner -i <file>` (stderr), used for probing without ffprobe.
pub(crate) fn describe(ffmpeg: &Path, file: &Path) -> Result<String> {
    let mut c = Command::new(ffmpeg);
    c.args(["-hide_banner", "-nostdin", "-i"]).arg(file).stdin(Stdio::null());
    no_window(&mut c);
    let out = c.output()?;
    let text = String::from_utf8_lossy(&out.stderr).into_owned();
    if text.contains("Duration:") || text.contains("Stream #") {
        Ok(text)
    } else {
        Err(Error::NotFound(format!("{}: {}", file.display(), tail(&out.stderr))))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct H264Encoder {
    pub name: &'static str,
    pub hardware: bool,
}

/// H.264 encoders that actually work in this ffmpeg build, hardware first.
/// Each candidate is validated with a tiny test encode and the answer is cached.
pub(crate) fn working_encoders(ffmpeg: &Path) -> &'static [H264Encoder] {
    static CACHE: OnceLock<Vec<H264Encoder>> = OnceLock::new();
    CACHE.get_or_init(|| {
        const CANDIDATES: &[(&str, bool)] = &[
            ("h264_nvenc", true),
            ("h264_qsv", true),
            ("h264_amf", true),
            ("h264_videotoolbox", true),
            ("libx264", false),
            ("libopenh264", false),
            ("h264_mf", true),
        ];
        let listed = Command::new(ffmpeg)
            .args(["-hide_banner", "-encoders"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        CANDIDATES
            .iter()
            .filter(|(name, _)| listed.split_whitespace().any(|w| w == *name))
            .filter(|(name, _)| test_encoder(ffmpeg, name))
            .map(|&(name, hardware)| H264Encoder { name, hardware })
            .collect()
    })
}

fn test_encoder(ffmpeg: &Path, encoder: &str) -> bool {
    let mut c = Command::new(ffmpeg);
    c.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=black:s=256x144:r=10:d=0.3", "-c:v", encoder, "-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    no_window(&mut c);
    c.status().map(|s| s.success()).unwrap_or(false)
}

/// Pick the best encoder honouring the preference; `None` if ffmpeg has no H.264 encoder.
pub(crate) fn pick_encoder(ffmpeg: &Path, pref: snap_core::settings::EncoderPref) -> Option<H264Encoder> {
    use snap_core::settings::EncoderPref;
    let all = working_encoders(ffmpeg);
    let hw = all.iter().find(|e| e.hardware && e.name != "h264_mf");
    let sw = all.iter().find(|e| !e.hardware);
    let mf = all.iter().find(|e| e.name == "h264_mf");
    match pref {
        EncoderPref::Software => sw.or(hw).or(mf),
        EncoderPref::Auto | EncoderPref::Hardware => hw.or(sw).or(mf),
    }
    .cloned()
}

/// Encoder specific arguments for a target bitrate (bits per second).
pub(crate) fn encoder_args(enc: &H264Encoder, bitrate: u32) -> Vec<String> {
    let mut a: Vec<String> = ["-c:v", enc.name, "-b:v"].map(String::from).into();
    a.push(bitrate.to_string());
    match enc.name {
        "libx264" => {
            a.extend(["-preset", "veryfast", "-maxrate"].map(String::from));
            a.push((bitrate as u64 * 3 / 2).to_string());
            a.push("-bufsize".into());
            a.push((bitrate as u64 * 2).to_string());
        }
        "h264_nvenc" => a.extend(["-preset", "p4", "-rc", "vbr"].map(String::from)),
        "h264_qsv" => a.extend(["-preset", "veryfast"].map(String::from)),
        _ => {}
    }
    a.extend(["-pix_fmt", "yuv420p"].map(String::from));
    a
}
